//! This is an implementation of the Trickle algorithm given in RFC 6206.
//!
//! TODO: Need to set TrickleData as client for alarm object
//!
//! TODO: Confirm that correct behavior is, for multiple queries to get_random_data
//! only one callback is returned

use core::cell::Cell;
use core::cmp::min;
use kernel::hil::{time, rng};
use kernel::hil::time::Frequency;

// TODO: Replace default constants
const I_MIN: usize = 10000; // In ms, minimum interval size
const I_MAX: usize = 7;     // Doublings of interval size
const K: usize = 4;         // Redundancy constant

// We expect the TrickleClient to maintain the received data packet, as we
// never need to look at it. Also, we expect the TrickleClient to parse
// out packets/frames not for us. This keeps the implementation of Trickle
// general for all radio/Mac layers.
pub trait TrickleClient {
    fn transmit(&self);
}

pub trait Trickle {
    fn set_default_parameters(&self, i_max: usize, i_min: usize, k: usize);
    fn initialize(&self);
    fn received_transmission(&self, bool);

    // TODO: Functions to change default parameters
}

pub struct TrickleData<'a, A: time::Alarm + 'a> {

    // Trickle parameters
    i_max: Cell<usize>,     // Maximum interval size (in doublings of i_min)
    i_max_val: Cell<usize>, // Maximum interval size (in ms) - computed from i_max, i_min
    i_min: Cell<usize>,     // Minimum interval size (in ms)
    k: Cell<usize>,         // Redundancy constant

    // Trickle variables
    i_cur: Cell<usize>,     // Current interval size
    t: Cell<usize>,         // Time to transmit in current interval
    c: Cell<usize>,         // Counter for how many transmissions have been received
    t_fired: Cell<bool>,    // Whether timer t has already fired for the interval

    client: &'a TrickleClient,
    clock: &'a A,
}

impl<'a, A: time::Alarm + 'a> TrickleData<'a, A> {
    pub fn new(client: &'a TrickleClient, clock: &'a A) -> TrickleData<'a, A> {
        let mut i_max_val = I_MIN;
        for _ in 0..I_MAX {
            i_max_val *= 2;
        }
        TrickleData{

            i_max: Cell::new(I_MAX),
            i_max_val: Cell::new(i_max_val),
            i_min: Cell::new(I_MIN),
            k: Cell::new(K),

            i_cur: Cell::new(0),
            t: Cell::new(0),
            c: Cell::new(0),
            t_fired: Cell::new(false),

            client: client,
            clock: clock
        }
    }

    // TODO: Some things to consider: First, getting random bytes is
    // asynchronous. Therefore, we exit control flow here. We must
    // guarantee that (even if other interrupts come in) we restart
    // the state machine correctly.
    fn start_next_interval(&self) {
        // Reset the counter
        self.c.set(0);

        // TODO: Get random byte(s)
        let random_bytes = 0x15;
        // This should select a random time in the second half of the interval
        let interval_offset = (random_bytes % (self.i_cur.get()/2)) + self.i_cur.get()/2;

        self.t.set(interval_offset);
        self.t_fired.set(false);

        // Set the transmit timer
        self.set_timer(interval_offset);
    }

    fn transmission_timer_fired(&self) {
        self.t_fired.set(true);
        // Approximately i_cur - t time is left in the interval
        // after the t timer fires. We need to set a timer for
        // the i interval.
        let time_left = self.i_cur.get() - self.t.get();
        self.set_timer(time_left);

        if self.c.get() < self.k.get() {
            self.client.transmit();
        }
    }

    fn interval_timer_fired(&self) {
        // Double interval size
        if self.i_cur.get() < self.i_max_val.get() {
            self.i_cur.set(min(self.i_cur.get()*2, self.i_max_val.get()));
        }
        self.start_next_interval();
    }

    // Time is in ms
    fn set_timer(&self, time: usize) {
        // TODO: Cancel pending alarms
        // TODO: Consider issue with overflow w/u32
        let tics = self.clock.now().wrapping_add((time as u32) * A::Frequency::frequency());
        self.clock.set_alarm(tics);
    }
}

impl<'a, A: time::Alarm + 'a> Trickle for TrickleData<'a, A> {

    fn set_default_parameters(&self, i_max: usize, i_min: usize, k: usize) {
        self.i_max.set(i_max);
        self.i_min.set(i_min);

        let mut i_max_val = i_min;
        for _ in 0..self.i_max.get() {
            i_max_val *= 2;
        }
        self.i_max_val.set(i_max_val);
        self.k.set(k);
    }

    fn initialize(&self) {
        self.i_cur.set(self.i_min.get());
        self.start_next_interval();
    }

    fn received_transmission(&self, is_consistent: bool) {
        if is_consistent {
            // Increment the counter c
            self.c.set(self.c.get() + 1);
        } else {
            // Reset interval only if i_cur > i_min; otherwise, ignore
            if self.i_cur.get() > self.i_min.get() {
                self.i_cur.set(self.i_min.get());
                self.start_next_interval();
            }
        }
    }
}

impl<'a, A: time::Alarm + 'a> time::Client for TrickleData<'a, A> {
    fn fired(&self) {
        // This happens after the timer expires
        if self.t_fired.get() {
            self.interval_timer_fired();
        } else {
            self.transmission_timer_fired();
        }
    }
}

impl<'a, A: time::Alarm + 'a> rng::Client for TrickleData<'a, A> {
    fn randomness_available(&self, randomness: &mut Iterator<Item = u32>) -> rng::Continue {
        rng::Continue::Done // or rng::Continue::More
    }
}
