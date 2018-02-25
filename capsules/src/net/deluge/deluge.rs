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
use net::sixlowpan::SixlowpanClient;
use kernel::ReturnCode;
use net::stream::{decode_u16, decode_u8};
use net::stream::SResult;

use net::deluge::trickle::{Trickle, TrickleClient};
use net::deluge::program_state::DelugeProgramClient;

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
        bit_vector_size: u16,
    },
    DataPacket {
        version: u16,
        page_num: u16,
    },
}

const DELUGE_PACKET_HDR: u8 = 0xd0;

const MAINTAIN_SUMMARY: u8 = 0x01;
const MAINTAIN_PROFILE: u8 = 0x02;
const REQUEST_FOR_DATA: u8 = 0x03;
const DATA_PACKET: u8 = 0x04;

struct DelugePacket {
    object_id: u16,
    payload_type: DelugePacketType,
    offset: usize,
    // TODO: Perhaps put a bound on this?
    buffer: TakeCell<'static, [u8]>,
}

impl DelugePacket {
    pub fn new() -> DelugePacket {

        DelugePacket {
            object_id: 0,
            payload_type: DelugePacketType::MaintainSummary { version: 0, page_num: 0 },
            offset: 0,
            buffer: TakeCell::empty(),
        }
    }

    pub fn decode(packet: &[u8]) -> SResult<DelugePacket> {
        // TODO: This is probably wrong
        let len = mem::size_of::<DelugePacket>() + 1;
        stream_len_cond!(packet, len);

        let mut deluge_packet = DelugePacket::new();
        let (off, packet_hdr) = dec_try!(packet, 0; decode_u8);

        if packet_hdr != DELUGE_PACKET_HDR {
            stream_err!(());
        }

        let (off, object_id) = dec_try!(packet, off; decode_u16);
        // TODO: Unsafe
        let (off, packet_type) = DelugePacket::decode_payload_type(off, packet).done().unwrap();
        deluge_packet.object_id = object_id;
        deluge_packet.payload_type = packet_type;
        deluge_packet.offset = off;
        // TODO: Non-optimal, as it requires we allocate an additional buffer
        // for the Deluge packet
        deluge_packet.buffer.map(|mut buf| buf.copy_from_slice(&packet));
        //deluge_packet.buffer.replace(packet);
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
    // General application state
    version: Cell<u16>,       // v in paper
    largest_page: Cell<u16>,  // \gamma in paper

    // Deluge network state
    received_old_v: Cell<bool>, // Whether to transmit full object profile or not
    obj_update_count: Cell<usize>,
    last_page_req_time: Cell<usize>,
    data_packet_recv_time: Cell<usize>,

    client: &'a DelugeProgramClient,
    state: Cell<DelugeState>,

    // Other
    trickle: &'a Trickle,
    alarm: &'a A,
}

impl<'a, A: time::Alarm + 'a> DelugeData<'a, A> {
    pub fn new(client: &'a DelugeProgramClient,
               trickle: &'a Trickle,
               alarm: &'a A) -> DelugeData<'a, A> {
        DelugeData{
            version: Cell::new(0),
            largest_page: Cell::new(0),

            received_old_v: Cell::new(false),
            obj_update_count: Cell::new(0),
            // TODO: Initialize these to max?
            last_page_req_time: Cell::new(0),
            data_packet_recv_time: Cell::new(0),

            state: Cell::new(DelugeState::Maintenance),
            client: client,

            trickle: trickle,
            alarm: alarm,
        }
    }

    fn transition_state(&self, new_state: DelugeState) {
        self.state.set(new_state);
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
        match packet.payload_type {
            DelugePacketType::MaintainSummary { version, page_num } => {
                // Inconsistent summary
                if version != self.version.get() {
                    self.trickle.received_transmission(false);
                } else if page_num > self.largest_page.get() {

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
                if version < self.version.get() {
                    self.received_old_v.set(true);
                    self.trickle.received_transmission(false);
                } else {
                    self.trickle.received_transmission(true);
                }
            },
            DelugePacketType::RequestForData { version, bit_vector_size } => {
                // TODO: Do nothing?
            },
            DelugePacketType::DataPacket { version, page_num } => {
                // Update our version if necessary
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
            DelugePacketType::RequestForData { version, bit_vector_size } => {
                // Reset timer
                self.rx_state_reset_timer();
            },
            DelugePacketType::DataPacket { version, page_num } => {
                // Reset timer
                self.rx_state_reset_timer();
            },
            // TODO: Process packet
        }
    }

    fn rx_state_process_packet<'b>(&self, packet: &'b DelugePacket) {
    }

    fn rx_state_completed_page(&self) {
        // TODO: Check CRC
        self.largest_page.set(self.largest_page.get() + 1);
        self.transition_state(DelugeState::Maintenance);
    }

    fn rx_state_reset_timer(&self) {
        // TODO
        let time = 1;
        let tics = self.alarm.now().wrapping_add((time as u32) * A::Frequency::frequency());
        self.alarm.set_alarm(tics);
    }

    fn tx_state_received_packet<'b>(&self, packet: &'b DelugePacket) {
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

impl<'a, A: time::Alarm + 'a> SixlowpanClient for DelugeData<'a, A> {
    fn receive<'b>(&self, buf: &'b [u8], len: u16, result: ReturnCode) {
        // TODO: Remove unwrap
        // TODO: Fix the way buffers are handled - simply doesn't make sense
        let (_, packet) = DelugePacket::decode(&buf[..len as usize]).done().unwrap();
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

    fn send_done(&self, buf: &'static mut [u8], acked: bool, result: ReturnCode) {
    }
}
