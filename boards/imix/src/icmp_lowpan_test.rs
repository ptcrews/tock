//! `app_layer_icmp_lowpan_frag.rs`: Test application layer sending of
//! 6LoWPAN packets
//!
//! Currently this file only tests sending messages.
//!
//! To use this test suite, allocate space for a new LowpanICMPTest structure,
//! and set it as the client for the Sixlowpan struct and for the respective
//! TxState struct. For the transmit side, call the LowpanICMPTest::start method.
//! The `initialize_all` function performs this initialization; simply call this
//! function in `boards/imix/src/main.rs` as follows:
//!
//! Alternatively, you can call the `initialize_all` function, which performs
//! the initialization routines for the 6LoWPAN, TxState, RxState, and Sixlowpan
//! structs. Insert the code into `boards/imix/src/main.rs` as follows:
//!
//! ...
//! // Radio initialization code
//! ...
//! let app_lowpan_frag_test = app_layer_icmp_lowpan_frag::initialize_all(
//!                                                 radio_mac as &'static Mac,
//!                                                          mux_alarm as &'static
//!                                                             MuxAlarm<'static,
//!                                                                 sam4l::ast::Ast>);
//! radio_mac.set_transmit_client(app_lowpan_frag_test);
//! ...
//! // Imix initialization
//! ...
//! app_lowpan_frag_test.start(); // If flashing the transmitting Imix

use capsules;
extern crate sam4l;
use capsules::ieee802154::device::MacDevice;
use capsules::net::icmpv6::icmpv6::{ICMP6Header, ICMP6Type};
use capsules::net::icmpv6::icmpv6_send::{ICMP6SendStruct, ICMP6Sender};
use capsules::net::ipv6::ip_utils::IPAddr;
use capsules::net::ipv6::ipv6::{IP6Packet, IPPayload, TransportHeader};
use capsules::net::ipv6::ipv6_send::{IP6SendStruct, IP6Sender};
use capsules::net::sixlowpan::sixlowpan_compression;
use capsules::net::sixlowpan::sixlowpan_state::{Sixlowpan, SixlowpanState, TxState};
use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use core::cell::Cell;
use kernel::hil::radio;
use kernel::hil::time;
use kernel::hil::time::Frequency;
use kernel::ReturnCode;

pub const SRC_ADDR: IPAddr = IPAddr([
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
]);
pub const DST_ADDR: IPAddr = IPAddr([
    0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f,
]);

/* 6LoWPAN Constants */
const DEFAULT_CTX_PREFIX_LEN: u8 = 8;
static DEFAULT_CTX_PREFIX: [u8; 16] = [0x0 as u8; 16];
static mut RX_STATE_BUF: [u8; 1280] = [0x0; 1280];

pub const TEST_DELAY_MS: u32 = 10000;
pub const TEST_LOOP: bool = false;

static mut ICMP_PAYLOAD: [u8; 10] = [0; 10];

pub static mut RF233_BUF: [u8; radio::MAX_BUF_SIZE] = [0 as u8; radio::MAX_BUF_SIZE];

//Use a global variable option, initialize as None, then actually initialize in initialize all

pub struct LowpanICMPTest<'a, A: time::Alarm + 'a> {
    alarm: A,
    //sixlowpan_tx: TxState<'a>,
    //radio: &'a Mac<'a>,
    test_counter: Cell<usize>,
    icmp_sender: &'a ICMP6Sender<'a>,
}

pub unsafe fn initialize_all(
    radio_mac: &'static MacDevice,
    mux_alarm: &'static MuxAlarm<'static, sam4l::ast::Ast>,
) -> &'static LowpanICMPTest<
    'static,
    capsules::virtual_alarm::VirtualMuxAlarm<'static, sam4l::ast::Ast<'static>>,
> {
    let sixlowpan = static_init!(
        Sixlowpan<'static, sam4l::ast::Ast<'static>, sixlowpan_compression::Context>,
        Sixlowpan::new(
            sixlowpan_compression::Context {
                prefix: DEFAULT_CTX_PREFIX,
                prefix_len: DEFAULT_CTX_PREFIX_LEN,
                id: 0,
                compress: false,
            },
            &sam4l::ast::AST
        )
    );

    let sixlowpan_state = sixlowpan as &SixlowpanState;
    let sixlowpan_tx = TxState::new(sixlowpan_state);

    let icmp_hdr = ICMP6Header::new(ICMP6Type::Type128); // Echo Request

    let ip_pyld: IPPayload = IPPayload {
        header: TransportHeader::ICMP(icmp_hdr),
        payload: &mut ICMP_PAYLOAD,
    };

    let ip6_dg = static_init!(IP6Packet<'static>, IP6Packet::new(ip_pyld));

    let ip6_sender = static_init!(
        IP6SendStruct<'static>,
        IP6SendStruct::new(ip6_dg, &mut RF233_BUF, sixlowpan_tx, radio_mac)
    );
    radio_mac.set_transmit_client(ip6_sender);

    let icmp_send_struct = static_init!(
        ICMP6SendStruct<'static, IP6SendStruct<'static>>,
        ICMP6SendStruct::new(ip6_sender)
    );

    let app_lowpan_frag_test = static_init!(
        LowpanICMPTest<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
        LowpanICMPTest::new(
            //sixlowpan_tx,
            //radio_mac,
            VirtualMuxAlarm::new(mux_alarm),
            icmp_send_struct
        )
    );

    ip6_sender.set_client(icmp_send_struct);
    icmp_send_struct.set_client(app_lowpan_frag_test);
    app_lowpan_frag_test.alarm.set_client(app_lowpan_frag_test);

    app_lowpan_frag_test
}

impl<'a, A: time::Alarm> capsules::net::icmpv6::icmpv6_send::ICMP6SendClient
    for LowpanICMPTest<'a, A>
{
    fn send_done(&self, result: ReturnCode) {
        match result {
            ReturnCode::SUCCESS => {
                debug!("ICMP Echo Request Packet Sent!");
                self.schedule_next();
            }
            _ => debug!("Failed to send ICMP Packet!"),
        }
    }
}

impl<'a, A: time::Alarm + 'a> LowpanICMPTest<'a, A> {
    pub fn new(
        //sixlowpan_tx: TxState<'a>,
        //radio: &'a Mac<'a>,
        alarm: A,
        //ip6_packet: &'static mut IP6Packet<'a>
        icmp_sender: &'a ICMP6Sender<'a>,
    ) -> LowpanICMPTest<'a, A> {
        LowpanICMPTest {
            alarm: alarm,
            //sixlowpan_tx: sixlowpan_tx,
            //radio: radio,
            test_counter: Cell::new(0),
            icmp_sender: icmp_sender,
        }
    }

    pub fn start(&self) {
        //self.run_test_and_increment();
        self.schedule_next();
    }

    fn schedule_next(&self) {
        let delta = (A::Frequency::frequency() * TEST_DELAY_MS) / 1000;
        let next = self.alarm.now().wrapping_add(delta);
        self.alarm.set_alarm(next);
    }

    fn run_test_and_increment(&self) {
        let test_counter = self.test_counter.get();
        self.run_test(test_counter);
        match TEST_LOOP {
            true => self.test_counter.set((test_counter + 1) % self.num_tests()),
            false => self.test_counter.set(test_counter + 1),
        };
    }

    fn num_tests(&self) -> usize {
        2
    }

    fn run_test(&self, test_id: usize) {
        debug!("Running test {}:", test_id);
        match test_id {
            0 => self.ipv6_send_packet_test(),
            1 => self.ipv6_send_packet_test(),
            _ => {}
        }
    }

    fn ipv6_send_packet_test(&self) {
        unsafe {
            self.send_ipv6_packet();
        }
    }

    unsafe fn send_ipv6_packet(&self) {
        self.send_next();
    }

    fn send_next(&self) {
        let icmp_hdr = ICMP6Header::new(ICMP6Type::Type128); // Echo Request
        unsafe { self.icmp_sender.send(DST_ADDR, icmp_hdr, &ICMP_PAYLOAD) };
    }
}

impl<'a, A: time::Alarm> time::Client for LowpanICMPTest<'a, A> {
    fn fired(&self) {
        self.run_test_and_increment();
    }
}
