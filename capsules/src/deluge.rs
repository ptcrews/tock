//! This is an implementation of the Deluge wireless binary updating protocol.
//!
//! Author: Paul Crews (ptcrews@cs.stanford.edu)
//! Date: 2018-02-01

use core::cell::Cell;
use core::cmp::min;

use trickle::{Trickle, TrickleClient};

pub struct ObjectProfile<'a> {
    size: usize,
    profile: &'a [u8], // TODO: What type of integers encode a version #?
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
    k_profile: Cell<usize>,
    v_current: Cell<usize>,
    largest_page: Cell<usize>,
    recv_v_old: Cell<bool>,
    last_page_req_time: Cell<usize>,
    data_packet_recv_time: Cell<usize>,

    state: Cell<DelugeState>,

    trickle: &'a Trickle,
}

impl<'a> DelugeData<'a> {
    pub fn new(trickle: &'a Trickle) -> DelugeData<'a> {
        DelugeData{
            // We let Trickle track the k_summary part
            k_profile: Cell::new(0),
            v_current: Cell::new(0),
            largest_page: Cell::new(0),
            recv_v_old: Cell::new(false),
            last_page_req_time: Cell::new(0),
            data_packet_recv_time: Cell::new(0),

            state: Cell::new(DelugeState::Maintenance),

            trickle: trickle,
        }
    }

    fn receive_packet(&self, packet: &[u8]) {
        if packet[0] == DELUGE_PROFILE_HDR {
            let version = packet[1] as usize;
            let size = packet[2] as usize;
            let profile = &packet[3..size+3];
            let obj_prof = ObjectProfile {
                size: size,
                profile: profile,
            };
            self.receive_object_profile(version, &obj_prof);
        } else if packet[0] == DELUGE_SUM_HDR {
            let version = packet[1] as usize;
            let largest_page = packet[2] as usize;
            self.receive_summary(version, largest_page);
        } // TODO: Receive request for data from page p \leq \gamma from version v
    }

    fn receive_summary(&self, version: usize, largest_page: usize) {
        // Inconsistent summary
        if version != self.v_current.get() {
            self.trickle.received_transmission(false);
        } else {
            if largest_page > self.largest_page.get() {
                // Confirm that request p \leq \gamma was *not* previously
                // received within time t = 2*interval AND that
                // a data packet for page p \leq \gamma +1 was previously received
                // in time t = interval
                // If so, transition to RX state
            }
            self.trickle.received_transmission(true);
        }
    }

    // M4
    fn receive_object_profile(&self, version: usize, profile: &ObjectProfile) {
        if version < self.v_current.get() && self.k_profile.get() < CONST_K {
            // TODO: Transmit object profile
        } else {
            self.k_profile.set(self.k_profile.get() + 1);
            self.trickle.received_transmission(true);
        }
    }
}

impl<'a> TrickleClient for DelugeData<'a> {
    fn transmit(&self) {
        // Transmit summary
        if self.k_profile.get() < CONST_K {
            // Transmit object profile
        }
    }

    fn new_interval(&self) {
        self.k_profile.set(0);
        self.recv_v_old.set(false);
    }
}
