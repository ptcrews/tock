//! Implementation of the SAM4L TRNG.

use core::cell::Cell;
use kernel::common::regs::{ReadOnly, WriteOnly};
use kernel::hil::rng::{self, Continue};
use pm;

#[repr(C)]
struct Registers {
    cr: WriteOnly<u32, Control::Register>,
    _reserved0: [u32; 3],
    ier: WriteOnly<u32, Interrupt::Register>,
    idr: WriteOnly<u32, Interrupt::Register>,
    imr: ReadOnly<u32, Interrupt::Register>,
    isr: ReadOnly<u32, Interrupt::Register>,
    _reserved1: [u32; 12],
    odata: ReadOnly<u32, OutputData::Register>,
}

register_bitfields![u32,
    Control [
        /// Security Key
        KEY OFFSET(8) NUMBITS(24) [],
        /// Enables the TRNG to provide random values
        ENABLE OFFSET(0) NUMBITS(1) [
            Disable = 0,
            Enable = 1
        ]
    ],

    Interrupt [
        /// Data Ready
        DATRDY 0
    ],

    OutputData [
        /// Output Data
        ODATA OFFSET(0) NUMBITS(32) []
    ]
];

const BASE_ADDRESS: *const Registers = 0x40068000 as *const Registers;

pub struct Trng<'a> {
    regs: *const Registers,
    client: Cell<Option<&'a rng::Client>>,
}

pub static mut TRNG: Trng<'static> = Trng::new();
const KEY: u32 = 0x524e47;

impl<'a> Trng<'a> {
    const fn new() -> Trng<'a> {
        Trng {
            regs: BASE_ADDRESS,
            client: Cell::new(None),
        }
    }

    pub fn handle_interrupt(&self) {
        let regs = unsafe { &*self.regs };

        if !regs.imr.is_set(Interrupt::DATRDY) {
            return;
        }
        regs.idr.write(Interrupt::DATRDY::SET);

        self.client.get().map(|client| {
            let result = client.randomness_available(&mut TrngIter(self));
            if let Continue::Done = result {
                // disable controller
                regs.cr
                    .write(Control::KEY.val(KEY) + Control::ENABLE::Disable);
                pm::disable_clock(pm::Clock::PBA(pm::PBAClock::TRNG));
            } else {
                regs.ier.write(Interrupt::DATRDY::SET);
            }
        });
    }

    pub fn set_client(&self, client: &'a rng::Client) {
        self.client.set(Some(client));
    }
}

struct TrngIter<'a, 'b: 'a>(&'a Trng<'b>);

impl<'a, 'b> Iterator for TrngIter<'a, 'b> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let regs = unsafe { &*self.0.regs };
        if regs.isr.is_set(Interrupt::DATRDY) {
            Some(regs.odata.read(OutputData::ODATA))
        } else {
            None
        }
    }
}

impl<'a> rng::RNG for Trng<'a> {
    fn get(&self) {
        let regs = unsafe { &*self.regs };
        pm::enable_clock(pm::Clock::PBA(pm::PBAClock::TRNG));

        regs.cr
            .write(Control::KEY.val(KEY) + Control::ENABLE::Enable);
        regs.ier.write(Interrupt::DATRDY::SET);
    }
}
