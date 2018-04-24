//! Implementation of a single hardware timer.
//!
//! - Author: Amit Levy <levya@cs.stanford.edu>
//! - Author: Philip Levis <pal@cs.stanford.edu>
//! - Date: July 16, 2015

use core::cell::Cell;
use kernel::common::regs::{ReadOnly, ReadWrite, WriteOnly};
use kernel::hil::Controller;
use kernel::hil::time::{self, Alarm, Freq16KHz, Time};
use pm::{self, PBDClock};

/// Minimum number of clock tics to make sure ALARM0 register is synchronized
///
/// The datasheet has the following ominous language (Section 19.5.3.2):
///
/// > Because of synchronization, the transfer of the alarm value will not
/// > happen immediately. When changing/setting the alarm value, the user must
/// > make sure that the counter will not count the selected alarm value before
/// > the value is transferred to the register. In that case, the first alarm
/// > interrupt after the change will not be triggered.
///
/// In practice, we've observed that when the alarm is set for a counter value
/// less than or equal to four tics ahead of the current counter value, the
/// alarm interrupt doesn't fire. Thus, we simply round up to at least eight
/// tics. Seems safe enough and in practice has seemed to work.
const ALARM0_SYNC_TICS: u32 = 8;

#[repr(C)]
struct AstRegisters {
    cr: ReadWrite<u32, Control::Register>,
    cv: ReadWrite<u32, Value::Register>,
    sr: ReadOnly<u32, Status::Register>,
    scr: WriteOnly<u32, Interrupt::Register>,
    ier: WriteOnly<u32, Interrupt::Register>,
    idr: WriteOnly<u32, Interrupt::Register>,
    imr: ReadOnly<u32, Interrupt::Register>,
    wer: ReadWrite<u32, Event::Register>,
    // 0x20
    ar0: ReadWrite<u32, Value::Register>,
    ar1: ReadWrite<u32, Value::Register>,
    _reserved0: [u32; 2],
    pir0: ReadWrite<u32, PeriodicInterval::Register>,
    pir1: ReadWrite<u32, PeriodicInterval::Register>,
    _reserved1: [u32; 2],
    // 0x40
    clock: ReadWrite<u32, ClockControl::Register>,
    dtr: ReadWrite<u32, DigitalTuner::Register>,
    eve: WriteOnly<u32, Event::Register>,
    evd: WriteOnly<u32, Event::Register>,
    evm: ReadOnly<u32, Event::Register>,
    calv: ReadWrite<u32, Calendar::Register>, // we leave out parameter and version
}

register_bitfields![u32,
    Control [
        /// Prescalar Select
        PSEL OFFSET(16) NUMBITS(5) [],
        /// Clear on Alarm 1
        CA1  OFFSET(9) NUMBITS(1) [
            NoClearCounter = 0,
            ClearCounter = 1
        ],
        /// Clear on Alarm 0
        CA0  OFFSET(8) NUMBITS(1) [
            NoClearCounter = 0,
            ClearCounter = 1
        ],
        /// Calendar Mode
        CAL  OFFSET(2) NUMBITS(1) [
            CounterMode = 0,
            CalendarMode = 1
        ],
        /// Prescalar Clear
        PCLR OFFSET(1) NUMBITS(1) [],
        /// Enable
        EN   OFFSET(0) NUMBITS(1) [
            Disable = 0,
            Enable = 1
        ]
    ],

    Value [
        VALUE OFFSET(0) NUMBITS(32) []
    ],

    Status [
        /// Clock Ready
        CLKRDY 29,
        /// Clock Busy
        CLKBUSY 28,
        /// AST Ready
        READY 25,
        /// AST Busy
        BUSY 24,
        /// Periodic 0
        PER0 16,
        /// Alarm 0
        ALARM0 8,
        /// Overflow
        OVF 0
    ],

    Interrupt [
        /// Clock Ready
        CLKRDY 29,
        /// AST Ready
        READY 25,
        /// Periodic 0
        PER0 16,
        /// Alarm 0
        ALARM0 8,
        /// Overflow
        OVF 0
    ],

    Event [
        /// Periodic 0
        PER0 16,
        /// Alarm 0
        ALARM0 8,
        /// Overflow
        OVF 0
    ],

    PeriodicInterval [
        /// Interval Select
        INSEL OFFSET(0) NUMBITS(5) []
    ],

    ClockControl [
        /// Clock Source Selection
        CSSEL OFFSET(8) NUMBITS(3) [
            RCSYS = 0,
            OSC32 = 1,
            APBClock = 2,
            GCLK = 3,
            Clk1k = 4
        ],
        /// Clock Enable
        CEN   OFFSET(0) NUMBITS(1) [
            Disable = 0,
            Enable = 1
        ]
    ],

    DigitalTuner [
        VALUE OFFSET(8) NUMBITS(8) [],
        ADD   OFFSET(5) NUMBITS(1) [],
        EXP   OFFSET(0) NUMBITS(5) []
    ],

    Calendar [
        YEAR  OFFSET(26) NUMBITS(6) [],
        MONTH OFFSET(22) NUMBITS(4) [],
        DAY   OFFSET(17) NUMBITS(5) [],
        HOUR  OFFSET(12) NUMBITS(5) [],
        MIN   OFFSET( 6) NUMBITS(6) [],
        SEC   OFFSET( 0) NUMBITS(6) []
    ]
];

const AST_BASE: usize = 0x400F0800;

pub struct Ast<'a> {
    regs: *const AstRegisters,
    callback: Cell<Option<&'a time::Client>>,
}

pub static mut AST: Ast<'static> = Ast {
    regs: AST_BASE as *const AstRegisters,
    callback: Cell::new(None),
};

impl<'a> Controller for Ast<'a> {
    type Config = &'static time::Client;

    fn configure(&self, client: &'a time::Client) {
        self.callback.set(Some(client));

        pm::enable_clock(pm::Clock::PBD(PBDClock::AST));
        self.select_clock(Clock::ClockOsc32);
        self.set_prescalar(0); // 32KHz / (2^(0 + 1)) = 16KHz
        self.enable_alarm_wake();
        self.clear_alarm();
        self.enable();
    }
}

#[repr(usize)]
pub enum Clock {
    ClockRCSys = 0,
    ClockOsc32 = 1,
    ClockAPB = 2,
    ClockGclk2 = 3,
    Clock1K = 4,
}

impl<'a> Ast<'a> {
    pub fn clock_busy(&self) -> bool {
        unsafe { (*self.regs).sr.is_set(Status::CLKBUSY) }
    }

    pub fn set_client(&self, client: &'a time::Client) {
        self.callback.set(Some(client));
    }

    pub fn busy(&self) -> bool {
        unsafe { (*self.regs).sr.is_set(Status::BUSY) }
    }

    // Clears the alarm bit in the status register (indicating the alarm value
    // has been reached).
    pub fn clear_alarm(&self) {
        while self.busy() {}
        unsafe {
            (*self.regs).scr.write(Interrupt::ALARM0::SET);
        }
    }

    // Clears the per0 bit in the status register (indicating the alarm value
    // has been reached).
    pub fn clear_periodic(&mut self) {
        while self.busy() {}
        unsafe {
            (*self.regs).scr.write(Interrupt::PER0::SET);
        }
    }

    pub fn select_clock(&self, clock: Clock) {
        unsafe {
            // Disable clock by setting first bit to zero
            while self.clock_busy() {}
            (*self.regs).clock.modify(ClockControl::CEN::CLEAR);
            while self.clock_busy() {}

            // Select clock
            (*self.regs)
                .clock
                .write(ClockControl::CSSEL.val(clock as u32));
            while self.clock_busy() {}

            // Re-enable clock
            (*self.regs).clock.modify(ClockControl::CEN::SET);
        }
    }

    pub fn enable(&self) {
        while self.busy() {}
        unsafe {
            (*self.regs).cr.modify(Control::EN::SET);
        }
    }

    pub fn is_enabled(&self) -> bool {
        while self.busy() {}
        unsafe { (*self.regs).cr.is_set(Control::EN) }
    }

    pub fn disable(&self) {
        while self.busy() {}
        unsafe {
            (*self.regs).cr.modify(Control::EN::CLEAR);
        }
    }

    pub fn set_prescalar(&self, val: u8) {
        while self.busy() {}
        unsafe {
            (*self.regs).cr.modify(Control::PSEL.val(val as u32));
        }
    }

    pub fn enable_alarm_irq(&self) {
        unsafe {
            (*self.regs).ier.write(Interrupt::ALARM0::SET);
        }
    }

    pub fn disable_alarm_irq(&self) {
        unsafe {
            (*self.regs).idr.write(Interrupt::ALARM0::SET);
        }
    }

    pub fn enable_ovf_irq(&mut self) {
        unsafe {
            (*self.regs).ier.write(Interrupt::OVF::SET);
        }
    }

    pub fn disable_ovf_irq(&mut self) {
        unsafe {
            (*self.regs).idr.write(Interrupt::OVF::SET);
        }
    }

    pub fn enable_periodic_irq(&mut self) {
        unsafe {
            (*self.regs).ier.write(Interrupt::PER0::SET);
        }
    }

    pub fn disable_periodic_irq(&mut self) {
        unsafe {
            (*self.regs).idr.write(Interrupt::PER0::SET);
        }
    }

    pub fn enable_alarm_wake(&self) {
        while self.busy() {}
        unsafe {
            (*self.regs).wer.modify(Event::ALARM0::SET);
        }
    }

    pub fn set_periodic_interval(&mut self, interval: u32) {
        while self.busy() {}
        unsafe {
            (*self.regs)
                .pir0
                .write(PeriodicInterval::INSEL.val(interval));
        }
    }

    pub fn get_counter(&self) -> u32 {
        while self.busy() {}
        unsafe { (*self.regs).cv.read(Value::VALUE) }
    }

    pub fn set_counter(&self, value: u32) {
        while self.busy() {}
        unsafe {
            (*self.regs).cv.write(Value::VALUE.val(value));
        }
    }

    pub fn handle_interrupt(&mut self) {
        self.clear_alarm();
        self.callback.get().map(|cb| {
            cb.fired();
        });
    }
}

impl<'a> Time for Ast<'a> {
    type Frequency = Freq16KHz;

    fn disable(&self) {
        self.disable_alarm_irq();
    }

    fn is_armed(&self) -> bool {
        self.is_enabled()
    }
}

impl<'a> Alarm for Ast<'a> {
    fn now(&self) -> u32 {
        while self.busy() {}
        unsafe { (*self.regs).cv.read(Value::VALUE) }
    }

    fn set_alarm(&self, mut tics: u32) {
        while self.busy() {}
        unsafe {
            let now = (*self.regs).cv.read(Value::VALUE);
            if tics.wrapping_sub(now) <= ALARM0_SYNC_TICS {
                tics = now.wrapping_add(ALARM0_SYNC_TICS);
            }
            (*self.regs).ar0.write(Value::VALUE.val(tics));
        }
        self.clear_alarm();
        self.enable_alarm_irq();
    }

    fn get_alarm(&self) -> u32 {
        while self.busy() {}
        unsafe { (*self.regs).ar0.read(Value::VALUE) }
    }
}
