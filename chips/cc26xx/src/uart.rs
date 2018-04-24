//! UART driver, cc26xx family
use core::cell::Cell;
use gpio;
use ioc;
use kernel;
use kernel::common::regs::{ReadOnly, ReadWrite, WriteOnly};
use kernel::hil::gpio::Pin;
use kernel::hil::uart;
use prcm;

const UART_BASE: usize = 0x4000_1000;
const MCU_CLOCK: u32 = 48_000_000;

#[repr(C)]
struct Registers {
    dr: ReadWrite<u32>,
    rsr_ecr: ReadWrite<u32>,
    _reserved0: [u32; 0x4],
    fr: ReadOnly<u32, Flags::Register>,
    _reserved1: [u32; 0x2],
    ibrd: ReadWrite<u32, IntDivisor::Register>,
    fbrd: ReadWrite<u32, FracDivisor::Register>,
    lcrh: ReadWrite<u32, LineControl::Register>,
    ctl: ReadWrite<u32, Control::Register>,
    ifls: ReadWrite<u32>,
    imsc: ReadWrite<u32, Interrupts::Register>,
    ris: ReadOnly<u32, Interrupts::Register>,
    mis: ReadOnly<u32, Interrupts::Register>,
    icr: WriteOnly<u32, Interrupts::Register>,
    dmactl: ReadWrite<u32>,
}

pub static mut UART0: UART = UART::new();

register_bitfields![
    u32,
    Control [
        UART_ENABLE OFFSET(0) NUMBITS(1) [],
        TX_ENABLE OFFSET(8) NUMBITS(1) [],
        RX_ENABLE OFFSET(9) NUMBITS(1) []
    ],
    LineControl [
        FIFO_ENABLE OFFSET(4) NUMBITS(1) [],
        WORD_LENGTH OFFSET(5) NUMBITS(2) [
            Len5 = 0x0,
            Len6 = 0x1,
            Len7 = 0x2,
            Len8 = 0x3
        ]
    ],
    IntDivisor [
        DIVISOR OFFSET(0) NUMBITS(16) []
    ],
    FracDivisor [
        DIVISOR OFFSET(0) NUMBITS(6) []
    ],
    Flags [
        TX_FIFO_FULL OFFSET(5) NUMBITS(1) []
    ],
    Interrupts [
        ALL_INTERRUPTS OFFSET(0) NUMBITS(12) []
    ]
];

pub struct UART {
    regs: *const Registers,
    client: Cell<Option<&'static uart::Client>>,
    tx_pin: Cell<Option<u8>>,
    rx_pin: Cell<Option<u8>>,
}

impl UART {
    const fn new() -> UART {
        UART {
            regs: UART_BASE as *const Registers,
            client: Cell::new(None),
            tx_pin: Cell::new(None),
            rx_pin: Cell::new(None),
        }
    }

    /// Sets pin number for transmit and receive line.
    ///
    /// This function needs to be run before the UART module is initialized.
    /// Initializing the module without setting the pins will make the kernel panic.
    pub fn set_pins(&self, tx_pin: u8, rx_pin: u8) {
        self.tx_pin.set(Some(tx_pin));
        self.rx_pin.set(Some(rx_pin));
    }

    fn configure(&self, params: kernel::hil::uart::UARTParams) {
        let tx_pin = match self.tx_pin.get() {
            Some(pin) => pin,
            None => panic!("Tx pin not configured for UART"),
        };

        let rx_pin = match self.rx_pin.get() {
            Some(pin) => pin,
            None => panic!("Rx pin not configured for UART"),
        };

        unsafe {
            // Make sure the TX pin is output/high before assigning it to UART control
            // to avoid falling edge glitches
            gpio::PORT[tx_pin as usize].make_output();
            gpio::PORT[tx_pin as usize].set();

            // Map UART signals to IO pin
            ioc::IOCFG[tx_pin as usize].enable_uart_tx();
            ioc::IOCFG[rx_pin as usize].enable_uart_rx();
        }

        // Disable the UART before configuring
        self.disable();

        self.set_baud_rate(params.baud_rate);

        // Set word length
        let regs = unsafe { &*self.regs };
        regs.lcrh.write(LineControl::WORD_LENGTH::Len8);

        self.fifo_enable();

        // Enable UART, RX and TX
        regs.ctl
            .write(Control::UART_ENABLE::SET + Control::RX_ENABLE::SET + Control::TX_ENABLE::SET);
    }

    fn power_and_clock(&self) {
        prcm::Power::enable_domain(prcm::PowerDomain::Serial);
        while !prcm::Power::is_enabled(prcm::PowerDomain::Serial) {}
        prcm::Clock::enable_uart();
    }

    fn set_baud_rate(&self, baud_rate: u32) {
        // Fractional baud rate divider
        let div = (((MCU_CLOCK * 8) / baud_rate) + 1) / 2;
        // Set the baud rate
        let regs = unsafe { &*self.regs };
        regs.ibrd.write(IntDivisor::DIVISOR.val(div / 64));
        regs.fbrd.write(FracDivisor::DIVISOR.val(div % 64));
    }

    fn fifo_enable(&self) {
        let regs = unsafe { &*self.regs };
        regs.lcrh.modify(LineControl::FIFO_ENABLE::SET);
    }

    fn fifo_disable(&self) {
        let regs = unsafe { &*self.regs };
        regs.lcrh.modify(LineControl::FIFO_ENABLE::CLEAR);
    }

    fn disable(&self) {
        self.fifo_disable();
        let regs = unsafe { &*self.regs };
        regs.ctl.modify(
            Control::UART_ENABLE::CLEAR + Control::TX_ENABLE::CLEAR + Control::RX_ENABLE::CLEAR,
        );
    }

    fn disable_interrupts(&self) {
        // Disable all UART interrupts
        let regs = unsafe { &*self.regs };
        regs.imsc.modify(Interrupts::ALL_INTERRUPTS::CLEAR);
        // Clear all UART interrupts
        regs.icr.write(Interrupts::ALL_INTERRUPTS::SET);
    }

    /// Clears all interrupts related to UART.
    pub fn handle_interrupt(&self) {
        let regs = unsafe { &*self.regs };
        // Clear interrupts
        regs.icr.write(Interrupts::ALL_INTERRUPTS::SET);
    }

    /// Transmits a single byte if the hardware is ready.
    pub fn send_byte(&self, c: u8) {
        // Wait for space in FIFO
        while !self.tx_ready() {}
        // Put byte in data register
        let regs = unsafe { &*self.regs };
        regs.dr.set(c as u32);
    }

    /// Checks if there is space in the transmit fifo queue.
    pub fn tx_ready(&self) -> bool {
        let regs = unsafe { &*self.regs };
        !regs.fr.is_set(Flags::TX_FIFO_FULL)
    }
}

impl kernel::hil::uart::UART for UART {
    fn set_client(&self, client: &'static kernel::hil::uart::Client) {
        self.client.set(Some(client));
    }

    fn init(&self, params: kernel::hil::uart::UARTParams) {
        self.power_and_clock();
        self.disable_interrupts();
        self.configure(params);
    }

    fn transmit(&self, tx_data: &'static mut [u8], tx_len: usize) {
        if tx_len == 0 {
            return;
        }

        for i in 0..tx_len {
            self.send_byte(tx_data[i]);
        }

        self.client.get().map(move |client| {
            client.transmit_complete(tx_data, kernel::hil::uart::Error::CommandComplete);
        });
    }

    #[allow(unused)]
    fn receive(&self, rx_buffer: &'static mut [u8], rx_len: usize) {}
}
