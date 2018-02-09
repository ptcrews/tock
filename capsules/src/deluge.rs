//! This is an implementation of the Deluge wireless binary updating protocol.
//!
//! Author: Paul Crews (ptcrews@cs.stanford.edu)
//! Date: 2018-02-01

use core::cell::Cell;
use core::cmp::min;

use trickle::{Trickle, TrickleClient};

struct AgeVector<'a> {
    size: usize,
    profile: &'a [u8], // TODO: What type of integers encode a version #?
}

enum DelugePayloadType<'a> {
    MaintainSummary((usize, usize)),
    MaintainObjectProfile((usize, AgeVector<'a>)),
    // RequestForData(),
    // ?
}

struct DelugePayload<'a> {
    payload: DelugePayloadType<'a>,
}

const DELUGE_PROFILE_HDR: u8 = 0xd0;
const DELUGE_SUM_HDR: u8 = 0xd1;
const CONST_K: usize = 0x1;

enum DelugeState {
    Maintenance,
    Transmit,
    Receive,
}

pub struct DelugeData<'a> {
    // General application state
    version: Cell<usize>,       // v in paper
    largest_page: Cell<usize>,  // \gamma in paper

    // Deluge network state
    received_old_v: Cell<bool>, // Whether to transmit full object profile or not
    obj_update_count: Cell<usize>,
    last_page_req_time: Cell<usize>,
    data_packet_recv_time: Cell<usize>,

    state: Cell<DelugeState>,

    // Other
    trickle: &'a Trickle,
}

impl<'a> DelugeData<'a> {
    pub fn new(trickle: &'a Trickle) -> DelugeData<'a> {
        DelugeData{
            version: Cell::new(0),
            largest_page: Cell::new(0),

            received_old_v: Cell::new(false),
            obj_update_count: Cell::new(0),
            // TODO: Initialize these to max?
            last_page_req_time: Cell::new(0),
            data_packet_recv_time: Cell::new(0),

            state: Cell::new(DelugeState::Maintenance),

            trickle: trickle,
        }
    }

    fn transition_state(&self, new_state: DelugeState) {
        self.state.set(new_state);
        match new_state {
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
    fn maintain_received_packet<'b>(&self, packet: &'b DelugePayload<'b>) {
        match packet.payload {
            DelugePayloadType::MaintainSummary((version, page_num)) => {
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
            DelugePayloadType::MaintainObjectProfile((version, ref profile)) => {
                if version < self.version.get() {
                    self.received_old_v.set(true);
                    self.trickle.received_transmission(false);
                } else {
                    self.trickle.received_transmission(true);
                }
            },
        }
    }
}

impl<'a> TrickleClient for DelugeData<'a> {
    fn transmit(&self) {
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

/*
 * impl<'a> ReceiveClient for DelugeData<'a> {
 *      fn receive<'b>(&self, _: &'b [u8]) {
 *          match self.state {
 *              DelugeState::Maintenance => self.maintain_received_packet(),
 *              DelugeState::Transmit => self.transmit_received_packet(),
 *              DelugeState::Receive => self.receive_received_packet(),
 *          }
 *      }
 * }
