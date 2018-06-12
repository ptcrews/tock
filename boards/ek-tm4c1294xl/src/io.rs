use core::fmt::*;
use core::str;
use kernel::debug;
use kernel::hil::led;
use kernel::hil::uart::{self, UART};
use tm4c129x;

pub struct Writer {
    initialized: bool,
}

pub static mut WRITER: Writer = Writer { initialized: false };

impl Write for Writer {
    fn write_str(&mut self, s: &str) -> ::core::fmt::Result {
        let uart = unsafe { &mut tm4c129x::uart::UART0 };
        if !self.initialized {
            self.initialized = true;
            uart.init(uart::UARTParams {
                baud_rate: 115200,
                stop_bits: uart::StopBits::One,
                parity: uart::Parity::None,
                hw_flow_control: false,
            });
            unsafe {
                uart.specify_pins(&tm4c129x::gpio::PA[0], &tm4c129x::gpio::PA[1]);
            }
            uart.enable_tx();
        }
        for c in s.bytes() {
            uart.send_byte(c);
            while !uart.tx_ready() {}
        }
        Ok(())
    }
}

/// Panic handler.
#[cfg(not(test))]
#[no_mangle]
#[lang = "panic_fmt"]
pub unsafe extern "C" fn panic_fmt(args: Arguments, file: &'static str, line: u32) -> ! {
    let led = &mut led::LedLow::new(&mut tm4c129x::gpio::PF[0]);
    let writer = &mut WRITER;
    debug::panic(led, writer, args, file, line)
}
