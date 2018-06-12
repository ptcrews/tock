use core::cell::Cell;
use gpio;
use kernel;
use kernel::common::cells::TakeCell;
use kernel::common::cells::VolatileCell;
use kernel::hil;
use sysctl;

#[allow(dead_code)]
#[repr(C)]
struct UARTRegisters {
    dr: VolatileCell<u32>,
    rsr: VolatileCell<u32>,
    _reserved0: [u32; 4],
    fr: VolatileCell<u32>,
    _reserved1: [u32; 1],
    ilpr: VolatileCell<u32>,
    ibrd: VolatileCell<u32>,
    fbrd: VolatileCell<u32>,
    lcrh: VolatileCell<u32>,
    ctl: VolatileCell<u32>,
    ifls: VolatileCell<u32>,
    im: VolatileCell<u32>,
    ris: VolatileCell<u32>,
    mis: VolatileCell<u32>,
    icr: VolatileCell<u32>,
    dmactl: VolatileCell<u32>,
    _reserved2: [u32; 22],
    _9bitaddr: VolatileCell<u32>,
    _9bitamask: VolatileCell<u32>,
    _reserved3: [u32; 965],
    pp: VolatileCell<u32>,
    _reserved4: [u32; 1],
    cc: VolatileCell<u32>,
}

const UART_BASE_ADDRS: [*mut UARTRegisters; 8] = [
    0x4000C000 as *mut UARTRegisters,
    0x4000D000 as *mut UARTRegisters,
    0x4000E000 as *mut UARTRegisters,
    0x4000F000 as *mut UARTRegisters,
    0x40010000 as *mut UARTRegisters,
    0x40011000 as *mut UARTRegisters,
    0x40012000 as *mut UARTRegisters,
    0x40013000 as *mut UARTRegisters,
];

pub struct UART {
    registers: *mut UARTRegisters,
    clock: sysctl::Clock,
    rx: Cell<Option<&'static gpio::GPIOPin>>,
    tx: Cell<Option<&'static gpio::GPIOPin>>,
    client: Cell<Option<&'static kernel::hil::uart::Client>>,
    buffer: TakeCell<'static, [u8]>,
    remaining: Cell<usize>,
    offset: Cell<usize>,
}

pub static mut UART0: UART = UART::new(
    UART_BASE_ADDRS[0],
    sysctl::Clock::UART(sysctl::RCGCUART::UART0),
);

impl UART {
    const fn new(base_addr: *mut UARTRegisters, clock: sysctl::Clock) -> UART {
        UART {
            registers: base_addr,
            clock: clock,
            rx: Cell::new(None),
            tx: Cell::new(None),
            client: Cell::new(None),
            buffer: TakeCell::empty(),
            remaining: Cell::new(0),
            offset: Cell::new(0),
        }
    }

    fn set_baud_rate(&self, baud_rate: u32) {
        let regs: &UARTRegisters = unsafe { &*self.registers };

        regs.cc.set(0x5);
        let brd = /*uartclk*/16000000 * /*width(brdf)*/64 / (/*clkdiv*/16 * /*baud*/baud_rate);
        let brdh = brd >> 6;
        let brdf = brd % 64;
        regs.ibrd.set(brdh);
        regs.fbrd.set(brdf);

        regs.lcrh.set(0x60);
        regs.ctl.set(regs.ctl.get() | 1); // UE
    }

    pub fn specify_pins(&self, rx: &'static gpio::GPIOPin, tx: &'static gpio::GPIOPin) {
        self.rx.set(Some(rx));
        self.tx.set(Some(tx));
    }

    fn enable(&self) {
        unsafe {
            sysctl::enable_clock(self.clock);
        }
    }

    fn enable_tx_interrupts(&self) {
        let regs: &UARTRegisters = unsafe { &*self.registers };
        regs.im.set(regs.im.get() | (1 << 5)); // TCIE
    }

    fn disable_tx_interrupts(&self) {
        let regs: &UARTRegisters = unsafe { &*self.registers };
        regs.im.set(regs.im.get() & !(1 << 5)); // TCIE
    }

    pub fn enable_tx(&self) {
        let regs: &UARTRegisters = unsafe { &*self.registers };
        self.tx
            .get()
            .map(|pin| pin.configure(gpio::Mode::InputOutput(gpio::InputOutputMode::DigitalAfsel)));
        self.enable();
        regs.ctl.set(regs.ctl.get() | (1 << 8)); // TE
    }

    pub fn enable_rx(&self) {
        let regs: &UARTRegisters = unsafe { &*self.registers };
        self.rx
            .get()
            .map(|pin| pin.configure(gpio::Mode::InputOutput(gpio::InputOutputMode::DigitalAfsel)));
        self.enable();
        regs.ctl.set(regs.ctl.get() | (1 << 9)); // RE
    }

    pub fn send_byte(&self, byte: u8) {
        let regs: &UARTRegisters = unsafe { &*self.registers };
        while regs.fr.get() & (1 << 3) != 0 {} // TXE
        regs.dr.set(byte as u32);
    }

    pub fn tx_ready(&self) -> bool {
        let regs: &UARTRegisters = unsafe { &*self.registers };
        regs.fr.get() & (1 << 3) == 0 // TC
    }

    fn send_next(&self) {
        self.buffer.map(|buffer| {
            self.send_byte(buffer[self.offset.get()]);
        });
    }

    pub fn handle_interrupt(&self) {
        let regs: &UARTRegisters = unsafe { &*self.registers };
        // check if caused by TC
        if regs.mis.get() & (1 << 5) != 0 {
            self.remaining.set(self.remaining.get() - 1);
            self.offset.set(self.offset.get() + 1);
            if self.remaining.get() > 0 {
                self.send_next();
            } else {
                self.disable_tx_interrupts();
                self.client.get().map(|client| {
                    self.buffer.take().map(|buffer| {
                        client.transmit_complete(buffer, kernel::hil::uart::Error::CommandComplete);
                    });
                });
            }
        }
    }
}

impl hil::uart::UART for UART {
    fn set_client(&self, client: &'static hil::uart::Client) {
        self.client.set(Some(client));
    }

    fn init(&self, params: hil::uart::UARTParams) {
        self.enable();
        self.set_baud_rate(params.baud_rate)
    }

    fn transmit(&self, tx_data: &'static mut [u8], tx_len: usize) {
        self.buffer.replace(tx_data);
        self.offset.set(0);
        self.remaining.set(tx_len);
        self.enable_tx();
        self.enable_tx_interrupts();
        self.send_next();
    }

    fn receive(&self, _rx_buffer: &'static mut [u8], _rx_len: usize) {
        unimplemented!()
    }

    fn abort_receive(&self) {
        unimplemented!()
    }
}
