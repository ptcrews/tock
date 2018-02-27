//! This is an implementation of the Deluge wireless binary updating protocol.
//!
//! Author: Paul Crews (ptcrews@cs.stanford.edu)
//! Date: 2018-02-01

use core::cell::Cell;
use core::mem;
use core::cmp::min;
use kernel::hil::time;
use kernel::hil::time::Frequency;
use kernel::common::{List, ListLink, ListNode};
use kernel::common::take_cell::TakeCell;
use kernel::ReturnCode;
use net::stream::{decode_u16, decode_u8};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;

use net::deluge::trickle::{Trickle, TrickleClient};
use net::deluge::transmit_layer::{DelugeTransmit, DelugeRxClient, DelugeTxClient};
use net::deluge::program_state::DelugeProgramState;
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

    // TODO: Encode function
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

    program_state: &'a DelugeProgramState,
    state: Cell<DelugeState>,

    // Other
    deluge_transmit_layer: &'a DelugeTransmit,
    trickle: &'a Trickle,
    alarm: &'a A,
}

impl<'a, A: time::Alarm + 'a> DelugeData<'a, A> {
    pub fn new(program_state: &'a DelugeProgramState,
               transmit_layer: &'a DelugeTransmit,
               trickle: &'a Trickle,
               alarm: &'a A) -> DelugeData<'a, A> {
        DelugeData{
            received_old_v: Cell::new(false),
            obj_update_count: Cell::new(0),
            // TODO: Initialize these to max?
            last_page_req_time: Cell::new(0),
            data_packet_recv_time: Cell::new(0),

            state: Cell::new(DelugeState::Maintenance),
            program_state: program_state,

            deluge_transmit_layer: transmit_layer,
            trickle: trickle,
            alarm: alarm,
        }
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
                // Inconsistent summary
                if version != self.program_state.current_version_number() as u16 {
                    self.trickle.received_transmission(false);
                } else if page_num > self.program_state.current_page_number() as u16 {

                    // Get the current interval, then notify Trickle that
                    // we received an inconsitent transmission
                    let cur_interval = self.trickle.get_current_interval();
                    self.trickle.received_transmission(false);

                    if (self.last_page_req_time.get() > cur_interval * 2)
                        && (self.data_packet_recv_time.get() > cur_interval) {
                        //TODO self.transition_to_rx();
                    }
                } else {
                    // Transmission only consistent if v=v', y=y'
                    self.trickle.received_transmission(true);
                }
            },
            // Note that we diverge a bit from the explicit behavior described
            // in part M.4 in the Deluge paper. In particular, we don't
            // independently track the # of received object profiles versus
            // # of received summaries
            DelugePacketType::MaintainObjectProfile{ version, age_vector_size } => {
                if version < self.program_state.current_version_number() as u16 {
                    self.received_old_v.set(true);
                    self.trickle.received_transmission(false);
                } else {
                    self.trickle.received_transmission(true);
                }
            },
            DelugePacketType::RequestForData { version, page_num, packet_num } => {
                // TODO: Do nothing?
            },
            DelugePacketType::DataPacket { version, page_num, packet_num } => {
                if version < self.program_state.current_version_number() as u16 {
                    // Received inconsistent transmission
                    self.trickle.received_transmission(false);
                }
                self.any_state_receive_data_packet(version,
                                                   page_num,
                                                   packet_num,
                                                   packet.buffer);
            },
        }
    }

    // TODO: Handle transition to MT if a' < a over \lambda transmissions
    fn rx_state_received_packet<'b>(&self, packet: &'b DelugePacket) {
        match packet.payload_type {
            // TODO: Confirm: Don't do anything for these packets?
            DelugePacketType::MaintainSummary { version, page_num } => {
            },
            DelugePacketType::MaintainObjectProfile { version, age_vector_size } => {
            },
            DelugePacketType::RequestForData { version, page_num, packet_num } => {
                // Reset timer
                self.rx_state_reset_timer();
            },
            DelugePacketType::DataPacket { version, page_num, packet_num } => {
                // Reset timer
                self.rx_state_reset_timer();
                self.any_state_receive_data_packet(version, page_num, packet_num, packet.buffer);
            },
        }
    }

    fn rx_state_reset_timer(&self) {
        // TODO
        let time = 5;
        let tics = self.alarm.now().wrapping_add((time as u32) * A::Frequency::frequency());
        self.alarm.set_alarm(tics);
    }

    fn tx_state_received_packet<'b>(&self, packet: &'b DelugePacket) {
        match packet.payload_type {
            DelugePacketType::RequestForData { version, page_num, packet_num } => {
                self.tx_state_received_request(version, page_num, packet_num);
                //let bit_vector = packet.
                //self.program_state.
            },
            DelugePacketType::DataPacket { version, page_num, packet_num } => {
                self.any_state_receive_data_packet(version, page_num, packet_num, packet.buffer);
            },
            _ => {
                // TODO?
            },
        }
    }

    fn tx_state_received_request(&self, version: u16, page_num: u16, packet_num: u16) {
        let mut packet_buf: [u8; program_state::PACKET_SIZE] = [0; program_state::PACKET_SIZE];
        if self.program_state.get_requested_packet(version as usize,
                                                   page_num as usize,
                                                   packet_num as usize,
                                                   &mut packet_buf) {
            let data_packet = DelugePacketType::DataPacket { version,
                                                             page_num,
                                                             packet_num };
            let mut deluge_packet = DelugePacket::new(&packet_buf);
            deluge_packet.payload_type = data_packet;
            self.transmit_packet(&deluge_packet);
        }
    }

    fn any_state_receive_data_packet<'b>(&self,
                                         version: u16,
                                         page_num: u16,
                                         packet_num: u16,
                                         payload: &[u8]) {
        // TODO: Check for errors vs completion
        // TODO: Check CRC
        if self.program_state.receive_packet(version as usize,
                                             page_num as usize,
                                             packet_num as usize,
                                             payload) &&
            self.state.get() == DelugeState::Receive {
            // If we completed a page and are in the receive state, transition to maintain
            self.transition_state(DelugeState::Maintenance);
        }
    }

    fn transmit_packet(&self, deluge_packet: &DelugePacket) {
        let mut send_buf: [u8; program_state::PACKET_SIZE + MAX_HEADER_SIZE] 
            = [0; program_state::PACKET_SIZE + MAX_HEADER_SIZE];
        deluge_packet.encode(&mut send_buf);
        //self.
    }
}

impl<'a, A: time::Alarm + 'a> TrickleClient for DelugeData<'a, A> {
    fn transmit(&self) {
        // If we are not in the Maintenance state, we don't want to transmit
        // via Trickle
        if self.state.get() != DelugeState::Maintenance {
            return;
        }
        if self.received_old_v.get() {
            // Transmit object profile
        } else {
            // Transmit object summary
        }
    }

    fn new_interval(&self) {
        self.received_old_v.set(false);
        self.obj_update_count.set(0);
    }
}

impl<'a, A: time::Alarm + 'a> DelugeRxClient for DelugeData<'a, A> {
    fn receive(&self, buf: &[u8]) {
        // TODO: Remove unwrap
        // TODO: Fix the way buffers are handled - simply doesn't make sense
        let (_, packet) = DelugePacket::decode(buf).done().unwrap();
        match self.state.get() {
            DelugeState::Maintenance => {
                self.mt_state_received_packet(&packet);
            },
            DelugeState::Receive => {
                self.rx_state_received_packet(&packet);
            },
            DelugeState::Transmit => {
                self.tx_state_received_packet(&packet);
            },
        }
    }
}

impl<'a, A: time::Alarm + 'a> DelugeTxClient for DelugeData<'a, A> {
    fn transmit_done(&self, buf: &'static mut [u8], result: ReturnCode) {
        // Only care about the callback if we need to keep broadcasting. This
        // only occurs if we are in the Transmit state
        if self.state.get() == DelugeState::Transmit {
            // TODO: Note that since we are only transmitting a single packet
            // at a time, we transition to maintain here
            self.transition_state(DelugeState::Maintenance);
        }
    }
}
