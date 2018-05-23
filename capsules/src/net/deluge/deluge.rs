//! This is an implementation of the Deluge wireless binary updating protocol.
//!
//! Author: Paul Crews (ptcrews@cs.stanford.edu)
//! Date: 2018-02-01

use core::cell::Cell;
use core::mem;
use kernel::hil::time;
use kernel::hil::time::Frequency;
use kernel::ReturnCode;
use net::stream::{decode_u16, decode_u8};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;

use net::deluge::trickle::{Trickle, TrickleClient};
use net::deluge::transmit_layer::{DelugeTransmit, DelugeRxClient, DelugeTxClient};
use net::deluge::program_state::{DelugeProgramState, DelugeProgramStateClient};
use net::deluge::program_state;

#[derive(Copy, Clone)]
enum DelugePacketType {
    MaintainSummary {
        version: u16,
        page_num: u16,
    },
    MaintainObjectProfile {
        version: u16,
        age_vector_size: u16,
    },
    RequestForData {
        version: u16,
        page_num: u16,
        packet_num: u16,
    },
    DataPacket {
        version: u16,
        page_num: u16,
        packet_num: u16,
    },
}

/*
 * PACKET_HDR:  u8
 * OBJ_ID:      u16
 * PACKET_TYPE: u8
 * type fields: u16
 *              u16
 *              u16
 * BUFFER
 */

const MAX_HEADER_SIZE: usize = 12; // Max header size in bytes
const DELUGE_PACKET_HDR: u8 = 0xd0;

const MAINTAIN_SUMMARY: u8 = 0x01;
const MAINTAIN_PROFILE: u8 = 0x02;
const REQUEST_FOR_DATA: u8 = 0x03;
const DATA_PACKET: u8 = 0x04;

struct DelugePacket<'a> {
    object_id: u16,
    payload_type: DelugePacketType,
    buffer: &'a [u8],
}

impl<'a> DelugePacket<'a> {
    pub fn new(buffer: &'a [u8]) -> DelugePacket<'a> {

        DelugePacket {
            object_id: 0,
            payload_type: DelugePacketType::MaintainSummary { version: 0, page_num: 0 },
            buffer: buffer,
        }
    }

    pub fn decode(packet: &'a [u8]) -> SResult<DelugePacket<'a>> {
        // TODO: This is probably wrong
        let len = mem::size_of::<DelugePacket>() + 1;
        stream_len_cond!(packet, len);

        let (off, packet_hdr) = dec_try!(packet, 0; decode_u8);

        if packet_hdr != DELUGE_PACKET_HDR {
            stream_err!(());
        }

        let (off, object_id) = dec_try!(packet, off; decode_u16);
        // TODO: Unsafe
        let (off, packet_type) = DelugePacket::decode_payload_type(off, packet).done().unwrap();
        let mut deluge_packet = DelugePacket::new(&packet[off..]);
        deluge_packet.object_id = object_id;
        deluge_packet.payload_type = packet_type;
        stream_done!(off, deluge_packet);
    }

    fn decode_payload_type(off: usize, buf: &[u8]) -> SResult<DelugePacketType> {
        let (off, type_as_int) = dec_try!(buf, off; decode_u8);
        match type_as_int {
            MAINTAIN_SUMMARY => {
                let (off, version) = dec_try!(buf, off; decode_u16);
                let (off, page_num) = dec_try!(buf, off; decode_u16);
                let result = DelugePacketType::MaintainSummary { version: version, page_num: page_num };
                stream_done!(off, result);
            },
            MAINTAIN_PROFILE => {
                let (off, version) = dec_try!(buf, off; decode_u16);
                let (off, age_vec_sz) = dec_try!(buf, off; decode_u16);
                let result = DelugePacketType::MaintainObjectProfile { version: version,
                    age_vector_size: age_vec_sz };
                stream_done!(off, result);
            },
            REQUEST_FOR_DATA => {
                let (off, version) = dec_try!(buf, off; decode_u16);
                let (off, page_num) = dec_try!(buf, off; decode_u16);
                let (off, packet_num) = dec_try!(buf, off; decode_u16);
                let result = DelugePacketType::RequestForData { version: version,
                    page_num: page_num, packet_num: packet_num };
                stream_done!(off, result);
            },
            DATA_PACKET => {
                let (off, version) = dec_try!(buf, off; decode_u16);
                let (off, page_num) = dec_try!(buf, off; decode_u16);
                let (off, packet_num) = dec_try!(buf, off; decode_u16);
                let result = DelugePacketType::DataPacket { version: version,
                    page_num: page_num, packet_num: packet_num };
                stream_done!(off, result);
            },
            _ => {
                stream_err!(());
            }
        }
    }

    fn encode(&self, buffer: &mut [u8]) -> SResult<usize> {
        stream_len_cond!(buffer, MAX_HEADER_SIZE + self.buffer.len());
        let mut off = enc_consume!(buffer, 0; encode_u8, DELUGE_PACKET_HDR);
        off = enc_consume!(buffer, off; encode_u16, self.object_id);

        match self.payload_type {
            DelugePacketType::MaintainSummary { version, page_num } => {
                off = enc_consume!(buffer, off; encode_u8, MAINTAIN_SUMMARY);
                off = enc_consume!(buffer, off; encode_u16, version);
                off = enc_consume!(buffer, off; encode_u16, page_num);
            },
            DelugePacketType::MaintainObjectProfile { version, age_vector_size } => {
                off = enc_consume!(buffer, off; encode_u8, MAINTAIN_PROFILE);
                off = enc_consume!(buffer, off; encode_u16, version);
                off = enc_consume!(buffer, off; encode_u16, age_vector_size);
            },
            DelugePacketType::RequestForData { version, page_num, packet_num } => {
                off = enc_consume!(buffer, off; encode_u8, REQUEST_FOR_DATA);
                off = enc_consume!(buffer, off; encode_u16, version);
                off = enc_consume!(buffer, off; encode_u16, page_num);
                off = enc_consume!(buffer, off; encode_u16, packet_num);
            },
            DelugePacketType::DataPacket { version, page_num, packet_num } => {
                off = enc_consume!(buffer, off; encode_u8, DATA_PACKET);
                off = enc_consume!(buffer, off; encode_u16, version);
                off = enc_consume!(buffer, off; encode_u16, page_num);
                off = enc_consume!(buffer, off; encode_u16, packet_num);
            },
        }
        off = enc_consume!(buffer, off; encode_bytes, self.buffer);
        stream_done!(off, off);
    }
}

const CONST_K: usize = 0x1;

#[derive(Copy, Clone, PartialEq)]
enum DelugeState {
    Maintenance,
    Transmit,
    Receive,
}

pub struct DelugeData<'a, A: time::Alarm + 'a> {
    // Deluge network state
    received_old_v: Cell<bool>, // Whether to transmit full object profile or not
    obj_update_count: Cell<usize>,
    last_page_req_time: Cell<usize>,
    data_packet_recv_time: Cell<usize>,

    program_state: &'a DelugeProgramState<'a>,
    state: Cell<DelugeState>,
    flash_txn_busy: Cell<bool>,

    // Other
    deluge_transmit_layer: &'a DelugeTransmit<'a>,
    trickle: &'a Trickle<'a>,
    alarm: &'a A,
}

impl<'a, A: time::Alarm + 'a> DelugeData<'a, A> {
    pub fn new(program_state: &'a DelugeProgramState<'a>,
               transmit_layer: &'a DelugeTransmit<'a>,
               trickle: &'a Trickle<'a>,
               alarm: &'a A) -> DelugeData<'a, A> {
        DelugeData{
            received_old_v: Cell::new(false),
            obj_update_count: Cell::new(0),
            // TODO: Initialize these to max?
            last_page_req_time: Cell::new(usize::max_value()),
            data_packet_recv_time: Cell::new(usize::max_value()),

            state: Cell::new(DelugeState::Maintenance),
            program_state: program_state,
            flash_txn_busy: Cell::new(false),

            deluge_transmit_layer: transmit_layer,
            trickle: trickle,
            alarm: alarm,
        }
    }

    pub fn init(&self) {
        self.trickle.initialize();
    }

    fn transition_state(&self, new_state: DelugeState) {
        self.state.set(new_state);
        // TODO: Do anything here?
        match self.state.get() {
            DelugeState::Maintenance => {
            },
            DelugeState::Transmit => {
            },
            DelugeState::Receive => {
                self.rx_state_reset_timer();
            },
        }
    }

    // TODO: Handle M.5 - setting last_page_req_time and data_packet_recv_time
    // appropriately
    // TODO: Handle other inconsistent transmission cases: 1) advertisements
    // with inconsistent summaries, 2) any requests, or 3) any data packets
    fn mt_state_received_packet<'b>(&self, packet: &'b DelugePacket) {
        // TODO: Multiplex based on application ID
        match packet.payload_type {
            DelugePacketType::MaintainSummary { version, page_num } => {
                debug!("mt state MaintainSummary received");
                // Inconsistent summary
                if version != self.program_state.current_version_number() as u16 {
                    debug!("cvn diff");
                    if version < self.program_state.current_version_number() as u16 {
                        //self.received_old_v.set(true);
                    }
                    if version > self.program_state.current_version_number() as u16 {
                        self.program_state.received_new_version(version as usize);
                    }
                    self.trickle.received_transmission(false);
                } else if page_num != self.program_state.current_page_number() as u16 {
                    debug!("pn diff");

                    // Get the current interval, then notify Trickle that
                    // we received an inconsitent transmission
                    let cur_interval = self.trickle.get_current_interval();
                    self.trickle.received_transmission(false);

                    if page_num > self.program_state.current_page_number() as u16 {
                        debug!("Transition state to RX!");
                        self.transition_state(DelugeState::Receive);
                    }
                    // TODO: handle this correctly
                    /*
                    if (self.last_page_req_time.get() > cur_interval * 2)
                        && (self.data_packet_recv_time.get() > cur_interval) {
                        debug!("Transition state to RX!");
                        self.transition_state(DelugeState::Receive);
                    }
                    */
                } else {
                    debug!("valid txn");
                    // Transmission only consistent if v=v', y=y'
                    self.trickle.received_transmission(true);
                }
            },
            // Note that we diverge a bit from the explicit behavior described
            // in part M.4 in the Deluge paper. In particular, we don't
            // independently track the # of received object profiles versus
            // # of received summaries
            // TODO: What to do with age_vector_size (and corr. age_vector)?
            DelugePacketType::MaintainObjectProfile{ version, age_vector_size } => {
                debug!("mt state MaintainObjectProfile received");
                if version < self.program_state.current_version_number() as u16 {
                    self.received_old_v.set(true);
                    self.trickle.received_transmission(false);
                } else if version > self.program_state.current_version_number() as u16 {
                    // TODO: Need to transition/tell trickle we found an
                    // inconsistent transmission?
                    self.program_state.received_new_version(version as usize);
                } else {
                    self.trickle.received_transmission(true);
                }
            },
            DelugePacketType::RequestForData { version, page_num, packet_num } => {
                debug!("mt state RequestForData received");
                if version < self.program_state.current_version_number() as u16 {
                    // Received inconsistent transmission
                    self.trickle.received_transmission(false);
                }
                // TODO: Handle edge case where packet_num > current_page num
                // What should we do in that case?
                if page_num <= self.program_state.current_page_number() as u16 {
                    self.transition_state(DelugeState::Transmit);
                    self.tx_state_received_packet(packet);
                }
            },
            DelugePacketType::DataPacket { version, page_num, packet_num } => {
                debug!("mt state DataPacket received");
                if version < self.program_state.current_version_number() as u16 {
                    // Received inconsistent transmission
                    self.trickle.received_transmission(false);
                }
                // Is it an inconsistent transmission if page_num > cur_page_num?
                self.any_state_receive_data_packet(version,
                                                   page_num,
                                                   packet_num,
                                                   packet.buffer);
            },
        }
    }

    // TODO: Handle transition to MT if a' < a over \lambda transmissions
    #[allow(unused_variables)]
    fn rx_state_received_packet<'b>(&self, packet: &'b DelugePacket) {
        match packet.payload_type {
            // TODO: Confirm: Don't do anything for these packets?
            DelugePacketType::MaintainSummary { version, page_num } => {
                // We know we are already out of date, so really shouldn't
                // do anything here
            },
            DelugePacketType::MaintainObjectProfile { version, age_vector_size } => {
                // Again, already know we are outdated, so don't need to do
                // anything
            },
            DelugePacketType::RequestForData { version, page_num, packet_num } => {
                // Reset timer
                // Somebody else also wants data, so delay broadcast
                self.rx_state_reset_timer();
            },
            DelugePacketType::DataPacket { version, page_num, packet_num } => {
                debug!("RxState: DataPacket!!");
                // Reset timer
                self.rx_state_reset_timer();
                self.any_state_receive_data_packet(version, page_num, packet_num, packet.buffer);
            },
        }
    }

    fn rx_state_reset_timer(&self) {
        // TODO
        debug!("RxState: reset timer!");
        let time = 5;
        let tics = self.alarm.now().wrapping_add((time as u32) * A::Frequency::frequency());
        self.alarm.set_alarm(tics);
        // TODO: Send request if in rxstate
    }

    fn tx_state_received_packet<'b>(&self, packet: &'b DelugePacket) {
        debug!("TxState received packet");
        match packet.payload_type {
            DelugePacketType::RequestForData { version, page_num, packet_num } => {
                debug!("TxState: RFD");
                if version == self.program_state.current_version_number() as u16 &&
                        page_num <= self.program_state.current_page_number() as u16 {
                    self.tx_state_received_request(page_num, packet_num);
                }
            },
            DelugePacketType::DataPacket { version, page_num, packet_num } => {
                debug!("TxState: DataPacket");
                self.any_state_receive_data_packet(version, page_num, packet_num, packet.buffer);
            },
            _ => {
                // TODO?
            },
        }
    }

    fn tx_state_received_request(&self, page_num: u16, packet_num: u16) {
        debug!("Tx received request");
        self.flash_txn_busy.set(true);
        // This issues an asynchronous callback
        // TODO: Make all page requests go through the asynch callback
        if !self.program_state.get_requested_packet(page_num as usize,
                                                    packet_num as usize) {
            self.flash_txn_busy.set(false);
        }
    }

    // TODO: remove version number here
    fn any_state_receive_data_packet(&self,
                                     version: u16,
                                     page_num: u16,
                                     packet_num: u16,
                                     payload: &[u8]) {
        // TODO: Check CRC
        debug!("Received data packet");
        self.flash_txn_busy.set(true);
        // NOTE: If we receive an invalid packet here, we just drop it
        // and don't return an error - this should probably be changed
        if !self.program_state.receive_packet(version as usize,
                                             page_num as usize,
                                             packet_num as usize,
                                             payload) {
            // TODO: Make all calls to receive_packet asynchronous
            self.flash_txn_busy.set(false);
        }
    }

    fn transmit_packet(&self, deluge_packet: &DelugePacket) {
        debug!("DelugeData: Transmit packet!");
        let mut send_buf: [u8; program_state::PACKET_SIZE + MAX_HEADER_SIZE]
            = [0; program_state::PACKET_SIZE + MAX_HEADER_SIZE];
        // TODO: Check results
        let _encode_result = deluge_packet.encode(&mut send_buf);
        let _result = self.deluge_transmit_layer.transmit_packet(&send_buf);
    }
}

impl<'a, A: time::Alarm + 'a> DelugeProgramStateClient for DelugeData<'a, A> {
    // Read a page for transmit
    // Need page number, packet number
    fn read_complete(&self, page_num: usize, packet_num: usize, buffer: &[u8]) {
        debug!("Read complete for page: {}, packet num: {}", page_num, packet_num);
        self.flash_txn_busy.set(false);
        let mut packet_buf: [u8; program_state::PACKET_SIZE] = [0; program_state::PACKET_SIZE];
        let payload_type =
            DelugePacketType::DataPacket { version: self.program_state.current_version_number() as u16,
                                           page_num: page_num as u16,
                                           packet_num: packet_num as u16};
        let mut deluge_packet = DelugePacket::new(&packet_buf);
        deluge_packet.payload_type = payload_type;
        self.transmit_packet(&deluge_packet);
    }

    // Must have received a packet
    fn write_complete(&self, page_completed: bool) {
        self.flash_txn_busy.set(false);
        if page_completed && self.state.get() == DelugeState::Receive {
            // If we completed a page and are in the receive state, transition to mt
            self.transition_state(DelugeState::Maintenance);
        }
    }
}

impl<'a, A: time::Alarm + 'a> time::Client for DelugeData<'a, A> {
    fn fired(&self) {
        debug!("DelugeData: Timer fired");
        // Do nothing if not in the receive state
        if self.state.get() == DelugeState::Receive {
            debug!("Rx transmit");
            self.rx_state_reset_timer();
            let payload_type = DelugePacketType::RequestForData {
                version: self.program_state.current_version_number() as u16,
                // TODO: This will cause problems if we want the *next* page
                page_num: self.program_state.next_page_number() as u16,
                packet_num: self.program_state.next_packet_number() as u16,
            };
            let mut deluge_packet = DelugePacket::new(&[]);
            deluge_packet.payload_type = payload_type;
            self.transmit_packet(&deluge_packet);
        }
    }
}

impl<'a, A: time::Alarm + 'a> TrickleClient for DelugeData<'a, A> {
    fn transmit(&self) {
        // If we are not in the Maintenance state, we don't want to transmit
        // via Trickle
        debug!("DelugeData: Transmit callback from trickle");
        if self.state.get() != DelugeState::Maintenance {
            return;
        }
        let payload_type = if self.received_old_v.get() {
            // Transmit object profile
            // TODO: Fix the age vector to be correct
            debug!("Sending object profile");
            DelugePacketType::MaintainObjectProfile {
                version: self.program_state.current_version_number() as u16,
                age_vector_size: 0 as u16
            }
        } else {
            // Transmit object summary
            debug!("Sending summary: {}", self.program_state.current_page_number());
            DelugePacketType::MaintainSummary {
                version: self.program_state.current_version_number() as u16,
                page_num: self.program_state.current_page_number() as u16,
            }
        };
        let mut deluge_packet = DelugePacket::new(&[]);
        deluge_packet.payload_type = payload_type;
        self.transmit_packet(&deluge_packet);
    }

    fn new_interval(&self) {
        self.received_old_v.set(false);
        self.obj_update_count.set(0);
    }
}

impl<'a, A: time::Alarm + 'a> DelugeRxClient for DelugeData<'a, A> {
    fn receive(&self, buf: &[u8]) {
        // If we are currently busy, do nothing
        if self.flash_txn_busy.get() {
            return;
        }
        // TODO: Remove unwrap
        let (_, packet) = DelugePacket::decode(buf).done().unwrap();
        match self.state.get() {
            DelugeState::Maintenance => {
                debug!("Received in mt state");
                self.mt_state_received_packet(&packet);
            },
            DelugeState::Receive => {
                debug!("Received in rx state");
                self.rx_state_received_packet(&packet);
            },
            DelugeState::Transmit => {
                debug!("Received in tx state");
                self.tx_state_received_packet(&packet);
            },
        }
    }
}

impl<'a, A: time::Alarm + 'a> DelugeTxClient for DelugeData<'a, A> {
    fn transmit_done(&self, result: ReturnCode) {
        // Only care about the callback if we need to keep broadcasting. This
        // only occurs if we are in the Transmit state
        if self.state.get() == DelugeState::Transmit {
            // TODO: Note that since we are only transmitting a single packet
            // at a time, we transition to maintain here
            self.transition_state(DelugeState::Maintenance);
        }
    }
}
