//! `ip_dummy.rs`: IPv6 Test Suite
//!
//! This implements a simple testing framework for sending and receiving
//! IPv6 packets. This framework follows the same conceptual design as the
//! other networking test suites. Two Imix boards run this code, one for
//! receiving and one for transmitting. The transmitting Imix then sends a
//! variety of packets to the receiving Imix, relying on the IPv6 encoding
//! and decoding layer. Note that this layer relies on the lower 6LoWPAN layer.
//! For 6LoWPAN-specific tests, consult the `lowpan_frag_dummy.rs` and
//! `sixlowpan_dummy.rs` files.

use capsules;
extern crate sam4l;
use capsules::net::sixlowpan_compression::{ContextStore, Context};
use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use capsules::ieee802154::mac::Mac;
use capsules::net::ip::{IPAddr};
use capsules::net::ip_state::{IPClient, IPLayer, IPState};
use capsules::net::util;
use core::cell::Cell;

use kernel::hil::radio;
use kernel::hil::time;
use kernel::hil::time::Frequency;
use kernel::returncode::ReturnCode;

pub struct DummyStore {
    context0: Context,
}

impl DummyStore {
    pub fn new(context0: Context) -> DummyStore {
        DummyStore { context0: context0 }
    }
}

impl ContextStore for DummyStore {
    fn get_context_from_addr(&self, ip_addr: IPAddr) -> Option<Context> {
        if util::matches_prefix(&ip_addr.0, &self.context0.prefix, self.context0.prefix_len) {
            Some(self.context0)
        } else {
            None
        }
    }

    fn get_context_from_id(&self, ctx_id: u8) -> Option<Context> {
        if ctx_id == 0 {
            Some(self.context0)
        } else {
            None
        }
    }

    fn get_context_from_prefix(&self, prefix: &[u8], prefix_len: u8) -> Option<Context> {
        if prefix_len == self.context0.prefix_len &&
            util::matches_prefix(prefix, &self.context0.prefix, prefix_len) {
            Some(self.context0)
        } else {
            None
        }
    }
}

pub const SRC_ADDR: IPAddr = IPAddr([0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
                                     0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f]);
pub const DST_ADDR: IPAddr = IPAddr([0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29,
                                     0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f]);

pub const IP6_HDR_SIZE: usize = 40;
pub const PAYLOAD_LEN: usize = 200;
pub static mut IPLAYER_BUF: [u8; 1280] = [0 as u8; 1280]; 

static mut IP6_DGRAM: [u8; IP6_HDR_SIZE + PAYLOAD_LEN] = [0; IP6_HDR_SIZE + PAYLOAD_LEN];


/* 6LoWPAN Constants */
const DEFAULT_CTX_PREFIX_LEN: u8 = 8;
static DEFAULT_CTX_PREFIX: [u8; 16] = [0x0 as u8; 16];
static mut RX_STATE_BUF: [u8; 1280] = [0x0; 1280];
static mut RADIO_BUF_TMP: [u8; radio::MAX_BUF_SIZE] = [0x0; radio::MAX_BUF_SIZE];

pub const TEST_DELAY_MS: u32 = 10000;
pub const TEST_LOOP: bool = false;

pub struct IPTest<'a, A: time::Alarm + 'a, T: time::Alarm + 'a> {
    alarm: A,
    ip_layer: IPLayer<'a, T, DummyStore>,
    ip_state: &'a IPState<'a>,
    test_counter: Cell<usize>,
}

pub unsafe fn initialize_all(radio_mac: &'static Mac,
                             mux_alarm: &'static MuxAlarm<'static, sam4l::ast::Ast>)
        -> &'static IPTest<'static,
        capsules::virtual_alarm::VirtualMuxAlarm<'static, sam4l::ast::Ast<'static>>,
        sam4l::ast::Ast<'static>> {

    let default_rx_state = static_init!(
        capsules::net::sixlowpan::RxState<'static>,
        capsules::net::sixlowpan::RxState::new(&mut RX_STATE_BUF)
        );

    let sixlowpan = static_init!(
        capsules::net::sixlowpan::Sixlowpan<'static,
            sam4l::ast::Ast<'static>,
            DummyStore>,
        capsules::net::sixlowpan::Sixlowpan::new(
            radio_mac,
            DummyStore::new(capsules::net::sixlowpan_compression::Context {
                prefix: DEFAULT_CTX_PREFIX,
                prefix_len: DEFAULT_CTX_PREFIX_LEN,
                id: 0,
                compress: false,
            }),
            &mut RADIO_BUF_TMP,
            &sam4l::ast::AST
        )
    );

    // Init sixlowpan state
    sixlowpan.add_rx_state(default_rx_state);
    radio_mac.set_transmit_client(sixlowpan);
    radio_mac.set_receive_client(sixlowpan);

    let ip_layer = capsules::net::ip_state::IPLayer::new(
        &mut IPLAYER_BUF,
        sixlowpan
    );

    let ip_state = static_init!(
        capsules::net::ip_state::IPState<'static>,
        capsules::net::ip_state::IPState::new(SRC_ADDR));
    ip_layer.add_ip_state(ip_state);


    let ip_test = static_init!(
        IPTest<'static,
        VirtualMuxAlarm<'static, sam4l::ast::Ast>,
        sam4l::ast::Ast>,
        IPTest::new(ip_layer, ip_state, VirtualMuxAlarm::new(mux_alarm))
    );

    ip_test.alarm.set_client(ip_test);
    ip_test.init();
    ip_test

}

impl<'a, A: time::Alarm, T: time::Alarm + 'a> IPClient for IPTest<'a, A, T> {
    fn receive<'b>(&self, buf: &'b [u8], len: u16, result: ReturnCode) {
        debug!("Receive completed: {:?}", result);
        let test_num = self.test_counter.get();
        self.test_counter.set((test_num + 1) % self.num_tests());
        self.run_check_test(test_num, buf, len)
    }

    fn send_done(&self, buf: &'static mut [u8], result: ReturnCode) {
        debug!("Send completed");
        self.schedule_next();
    }
}

impl<'a, A: time::Alarm, T: time::Alarm + 'a> time::Client for IPTest<'a, A, T> {
    fn fired(&self) {
        self.run_test_and_increment();
    }
}

impl<'a, A: time::Alarm, T: time::Alarm + 'a> IPTest<'a, A, T> {
    pub fn new(ip_layer: IPLayer<'a, T, DummyStore>,
               ip_state: &'a IPState<'a>,
               alarm: A) -> IPTest<'a, A, T> {
        IPTest {
            alarm: alarm,
            ip_layer: ip_layer,
            ip_state: ip_state,
            test_counter: Cell::new(0),
        }
    }

    pub fn init(&'a self) {
        self.ip_state.set_client(self);
    }

    pub fn start(&self) {
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
            true => self.test_counter.set((test_counter + 1) & self.num_tests()),
            false => self.test_counter.set(test_counter + 1),
        };
    }

    fn num_tests(&self) -> usize {
        1
    }

    fn run_test(&self, test_id: usize) {
        debug!("Running test {}:", test_id);
        match test_id {
            0 => self.ipv6_send_packet_test(),
            _ => {}
        }
    }

    fn run_check_test(&self, test_id: usize, buf: &[u8], len: u16) {
        debug!("Running test {}:", test_id);
        match test_id {
            0 => ipv6_check_receive_packet(buf),
            _ => {}
        }
    }

    fn ipv6_send_packet_test(&self) {
        ipv6_prepare_payload();
        unsafe {
            self.send_ipv6_packet();
        }
    }

    unsafe fn send_ipv6_packet(&self) {
        let ret_code = self.ip_layer.send(self.ip_state, &mut IP6_DGRAM, PAYLOAD_LEN);
        debug!("Ret code: {:?}", ret_code);
    }
}

fn ipv6_check_receive_packet(recv_packet: &[u8]) {
    ipv6_prepare_payload();
    let len = PAYLOAD_LEN;
    unsafe {
        for i in 0..len as usize {
            if recv_packet[i] != IP6_DGRAM[i] {
                debug!("Packets differ at idx: {} where recv = {}, ref = {}",
                       i,
                       recv_packet[i],
                       IP6_DGRAM[i]);
            }
        }
    }
}

fn ipv6_prepare_payload() {
    {
        let payload = unsafe { &mut IP6_DGRAM[IP6_HDR_SIZE..] };
        for i in 0..PAYLOAD_LEN {
            payload[i] = i as u8;
        }
    }
}
