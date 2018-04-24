//! Sample capsule for Tock course at SenSys. Prints 'Hello World'

#![feature(const_fn,const_cell_new)]
#![no_std]

#[allow(unused_imports)]
#[macro_use(debug)]
extern crate kernel;

use kernel::hil::time::{self, Alarm, Frequency};
use kernel::hil::sensors::{AmbientLight, AmbientLightClient};

pub struct Sensys<'a, A: Alarm + 'a>  {
    alarm: &'a A,
    light: &'a AmbientLight,
}

impl<'a, A: Alarm> Sensys<'a, A> {
    pub fn new(alarm: &'a A, light: &'a AmbientLight) -> Sensys<'a, A> {
        Sensys {
           alarm: alarm,
           light: light,
        }
    }

    pub fn start(&self) {
        debug!("Hello World");
    }
}

impl<'a, A: Alarm> time::Client for Sensys<'a, A> {
    fn fired(&self) {
    }
}

impl<'a, A: Alarm> AmbientLightClient for Sensys<'a, A> {
    fn callback(&self, lux: usize) {
    }
}
