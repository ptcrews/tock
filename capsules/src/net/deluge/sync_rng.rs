use core::cell::Cell;
use kernel::hil::rng;
use kernel::hil::rng::RNG;

// If you really need anything larger than a u64,
// you should probably implement your own asynchronous
// RNG - this RNG is designed to be pretty random for
// small, separate queries
pub trait SyncRNG {
    //TODO: Implement
    //fn get_random_bytes();
    fn get_random_u32(&self, optional_randomness: Option<u32>) -> u32;
}

pub struct SyncRNGStruct<'a> {
    random_val: Cell<u32>,
    called_rng: Cell<bool>,
    rng: &'a RNG,
}

impl<'a> SyncRNGStruct<'a> {
    pub fn new(initial_seed: u32, rng: &'a RNG) -> SyncRNGStruct<'a> {
        SyncRNGStruct {
            random_val: Cell::new(initial_seed),
            called_rng: Cell::new(false),
            rng: rng,
        }
    }
}

impl<'a> SyncRNG for SyncRNGStruct<'a> {
    fn get_random_u32(&self, optional_randomness: Option<u32>) -> u32 {
        if !self.called_rng.get() {
            self.rng.get();
            self.called_rng.set(true);
        }
        match optional_randomness {
            Some(randomness) => {
                self.random_val.get() ^ randomness
            },
            None => {
                self.random_val.get()
            }
        }
    }
}

impl<'a> rng::Client for SyncRNGStruct<'a> {
    fn randomness_available(&self, randomness: &mut Iterator<Item = u32>) -> rng::Continue {
        match randomness.next() {
            Some(random) => {
                self.called_rng.set(false);
                self.random_val.set(random);
                rng::Continue::Done
            },
            None => rng::Continue::More
        }
    }
}
