//! This implements a simple testing framework for 6LoWPAN fragmentation and
//! compression. Two Imix boards run this code, one for receiving and one for
//! transmitting. The transmitting board must call the `start` function in
//! the `main.rs` file. The transmitting Imix then sends a variety of packets
//! to the receiving Imix, relying on the 6LoWPAN fragmentation and reassembly
//! layer. Note that this layer also performs 6LoWPAN compression (invisible
//! to the upper layers), so this test suite is also dependent on the
//! correctness of the compression/decompression implementation; for this
//! reason, tests solely for compression/decompression have been left in a
//! different file.
//!
//! This test suite will print out whether a receive packet is different than
//! the expected packet. For this test to work correctly, and for both sides
//! to remain in sync, they must both be started at the same time. Any dropped
//! frames will prevent the test from completing successfully.
//!
//! To use this test suite, allocate space for a new LowpanTest structure, and
//! set it as the client for the FragState struct and for the respective TxState
//! struct. For the transmit side, call the LowpanTest::start method. A simple
//! example is given below:
//!
//! let lowpan_frag_test = static_init!(
//!     lowpan_frag_dummy::LowpanTest<'static,
//!         VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
//!         lowpan_frag_dummy::LowpanTest::new(radio_mac as &'static Mac,
//!                                            frag_state,
//!                                            tx_state,
//!                                            frag_dummy_alarm)
//! );
//!
//! frag_state.set_receive_client(lowpan_frag_test);
//! tx_state.set_transmit_client(lowpan_frag_test);
//! frag_dummy_alarm.set_client(lowpan_frag_test);
//!
//! ...
//!
//! lowpan_frag_test.start();

use capsules::mac;
use capsules::net::ieee802154::MacAddress;
use capsules::net::ip::{IP6Header, IPAddr, ip6_nh};
use capsules::net::lowpan;
use capsules::net::lowpan::{ContextStore, Context};
use capsules::net::lowpan_fragment::{FragState, TxState, TransmitClient, ReceiveClient};
use capsules::net::util;
use core::cell::Cell;

use core::mem;
use kernel::ReturnCode;

use kernel::hil::radio;
use kernel::hil::time;
use kernel::hil::time::Frequency;

trait OnesComplement {
    fn ones_complement_add(self, other: Self) -> Self;
}

/// Implements one's complement addition for use in calculating the UDP checksum
impl OnesComplement for u16 {
    fn ones_complement_add(self, other: u16) -> u16 {
        let (sum, overflow) = self.overflowing_add(other);
        if overflow { sum + 1 } else { sum }
    }
}

fn compute_icmp_checksum(src_addr: &IPAddr,
                         dst_addr: &IPAddr,
                         icmp_packet: &[u8],
                         icmp_length: u16)
                         -> u16 {
    // The ICMP checksum is computed on the IPv6 pseudo-header concatenated
    // with the ICMP header and payload, but with the ICMP checksum field
    // zeroed out. Hence, this function assumes that `icmp_header` has already
    // been filled with the ICMP header, except for the ignored checksum.
    let mut checksum: u16 = 0;

    //               ICMPv6 pseudo-header
    //
    // +-- 8 bits -+-- 8 bits -+-- 8 bits -+-- 8 bits -+
    // |                                               |
    // +              Source IPv6 Address              +
    // |                                               |
    // +-----------+-----------+-----------+-----------+
    // |                                               |
    // +           Destination IPv6 Address            +
    // |                                               |
    // +-----------+-----------+-----------+-----------+
    // |                  ICMP Length                  |
    // +-----------+-----------+-----------+-----------+
    // |               Zeros               |  NH type  |
    // +-----------+-----------+-----------+-----------+

    // Source and destination addresses (assumed to already be in big endian)
    for two_bytes in src_addr.0.chunks(2) {
        checksum = checksum.ones_complement_add(util::slice_to_u16(two_bytes));
    }
    for two_bytes in dst_addr.0.chunks(2) {
        checksum = checksum.ones_complement_add(util::slice_to_u16(two_bytes));
    }

    // ICMP length and ICMP next header type. Note that we can avoid adding zeros,
    // but the pseudo header must be in network byte-order.
    checksum = checksum.ones_complement_add(icmp_length);
    checksum = checksum.ones_complement_add(58 as u16);

    // ICMP payload
    for bytes in icmp_packet.chunks(2) {
        checksum = checksum.ones_complement_add(if bytes.len() == 2 {
            util::slice_to_u16(bytes)
        } else {
            (bytes[0] as u16).to_be()
        });
    }

    // Return the complement of the checksum, unless it is 0, in which case we
    // the checksum is one's complement -0 for a non-zero binary representation
    if !checksum != 0 { !checksum } else { checksum }
}

/// Maps values of a IPv6 next header field to a corresponding LoWPAN
/// NHC-encoding extension ID, if that next header type is NHC-compressible

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


pub const MLP: [u8; 8] = [0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7];
pub const SRC_ADDR: IPAddr = IPAddr([0xfe, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x24, 0xa0, 0xff, 0xf6, 0xad, 0x71, 0x8e]);
pub const SRC_MAC_ADDR: MacAddress = MacAddress::Long([0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16,
                                                       0x17]);

// RASPBERRY PI AS DEST
pub const DST_ADDR: IPAddr = IPAddr([0xfe, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x24, 0xa0, 0xff, 0xf6, 0xad, 0x71, 0x8f]);
pub const DST_MAC_ADDR: MacAddress = MacAddress::Long([0x06, 0x24, 0xa0, 0xff, 0xf6, 0xad, 0x71, 0x8f]);

/*
// PAUL'S MACHINE AS DEST
pub const DST_ADDR: IPAddr = IPAddr([0xfe, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xd0, 0x75, 0xc1, 0x2f, 0x47, 0x8f, 0xb3, 0x92]);
pub const DST_MAC_ADDR: MacAddress = MacAddress::Long([0xd2, 0x75, 0xc1, 0x2f, 0x47, 0x8f, 0xb3, 0x92]);
*/


pub const IP6_HDR_SIZE: usize = 40;
pub const PAYLOAD_LEN: usize = 10;
pub static mut RF233_BUF: [u8; radio::MAX_BUF_SIZE] = [0 as u8; radio::MAX_BUF_SIZE];

#[derive(Copy,Clone,Debug,PartialEq)]
enum TF {
    Inline = 0b00,
    Traffic = 0b01,
    Flow = 0b10,
    TrafficFlow = 0b11,
}

#[derive(Copy,Clone,Debug)]
enum SAC {
    Inline,
    LLP64,
    LLP16,
    LLPIID,
    Unspecified,
    Ctx64,
    Ctx16,
    CtxIID,
}
#[derive(Copy,Clone,Debug)]
enum DAC {
    Inline,
    LLP64,
    LLP16,
    LLPIID,
    Ctx64,
    Ctx16,
    CtxIID,
    McastInline,
    Mcast48,
    Mcast32,
    Mcast8,
    McastCtx,
}

pub const TEST_DELAY_MS: u32 = 10000;
pub const TEST_LOOP: bool = false;

pub struct LowpanTest<'a, A: time::Alarm + 'a> {
    radio: &'a mac::Mac,
    alarm: &'a A,
    frag_state: &'a FragState<'a, A>,
    tx_state: &'a TxState<'a>,
    test_counter: Cell<usize>,
}

impl<'a, A: time::Alarm + 'a> LowpanTest<'a, A> {
    pub fn new(radio: &'a mac::Mac,
               frag_state: &'a FragState<'a, A>,
               tx_state: &'a TxState<'a>,
               alarm: &'a A)
               -> LowpanTest<'a, A> {
        LowpanTest {
            radio: radio,
            alarm: alarm,
            frag_state: frag_state,
            tx_state: tx_state,
            test_counter: Cell::new(0),
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
        28
    }

    fn run_test(&self, test_id: usize) {
        debug!("Running test {}:", test_id);
        match test_id {
            // Change TF compression
            0 => self.ipv6_send_packet_test(TF::Inline, 255, SAC::Inline, DAC::Inline),
            1 => self.ipv6_send_packet_test(TF::Traffic, 255, SAC::Inline, DAC::Inline),
            2 => self.ipv6_send_packet_test(TF::Flow, 255, SAC::Inline, DAC::Inline),
            3 => self.ipv6_send_packet_test(TF::TrafficFlow, 255, SAC::Inline, DAC::Inline),

            // Change HL compression
            4 => self.ipv6_send_packet_test(TF::TrafficFlow, 255, SAC::Inline, DAC::Inline),
            5 => self.ipv6_send_packet_test(TF::TrafficFlow, 64, SAC::Inline, DAC::Inline),
            6 => self.ipv6_send_packet_test(TF::TrafficFlow, 1, SAC::Inline, DAC::Inline),
            7 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::Inline, DAC::Inline),

            // Change source compression
            8 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::Inline, DAC::Inline),
            9 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::LLP64, DAC::Inline),
            10 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::LLP16, DAC::Inline),
            11 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::LLPIID, DAC::Inline),
            12 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::Unspecified, DAC::Inline),
            13 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::Ctx64, DAC::Inline),
            14 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::Ctx16, DAC::Inline),
            15 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Inline),

            // Change dest compression
            16 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Inline),
            17 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::LLP64),
            18 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::LLP16),
            19 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::LLPIID),
            20 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Ctx64),
            21 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Ctx16),
            22 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::CtxIID),
            23 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::McastInline),
            24 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Mcast48),
            25 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Mcast32),
            26 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Mcast8),
            27 => self.ipv6_send_packet_test(TF::TrafficFlow, 42, SAC::CtxIID, DAC::McastCtx),

            _ => {}
        }
    }

    fn run_check_test(&self, test_id: usize, buf: &[u8], len: u16) {
        debug!("Running test {}:", test_id);
        match test_id {
            // Change TF compression
            0 => ipv6_check_receive_packet(TF::Inline, 255, SAC::Inline, DAC::Inline, buf, len),
            1 => ipv6_check_receive_packet(TF::Traffic, 255, SAC::Inline, DAC::Inline, buf, len),
            2 => ipv6_check_receive_packet(TF::Flow, 255, SAC::Inline, DAC::Inline, buf, len),
            3 => {
                ipv6_check_receive_packet(TF::TrafficFlow, 255, SAC::Inline, DAC::Inline, buf, len)
            }

            // Change HL compression
            4 => {
                ipv6_check_receive_packet(TF::TrafficFlow, 255, SAC::Inline, DAC::Inline, buf, len)
            }
            5 => ipv6_check_receive_packet(TF::TrafficFlow, 64, SAC::Inline, DAC::Inline, buf, len),
            6 => ipv6_check_receive_packet(TF::TrafficFlow, 1, SAC::Inline, DAC::Inline, buf, len),
            7 => ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::Inline, DAC::Inline, buf, len),

            // Change source compression
            8 => ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::Inline, DAC::Inline, buf, len),
            9 => ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::LLP64, DAC::Inline, buf, len),
            10 => ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::LLP16, DAC::Inline, buf, len),
            11 => {
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::LLPIID, DAC::Inline, buf, len)
            }
            12 => {
                ipv6_check_receive_packet(TF::TrafficFlow,
                                          42,
                                          SAC::Unspecified,
                                          DAC::Inline,
                                          buf,
                                          len)
            }
            13 => ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::Ctx64, DAC::Inline, buf, len),
            14 => ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::Ctx16, DAC::Inline, buf, len),
            15 => {
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Inline, buf, len)
            }

            // Change dest compression
            16 => {
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Inline, buf, len)
            }
            17 => ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::LLP64, buf, len),
            18 => ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::LLP16, buf, len),
            19 => {
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::LLPIID, buf, len)
            }
            20 => ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Ctx64, buf, len),
            21 => ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Ctx16, buf, len),
            22 => {
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::CtxIID, buf, len)
            }
            23 => {
                ipv6_check_receive_packet(TF::TrafficFlow,
                                          42,
                                          SAC::CtxIID,
                                          DAC::McastInline,
                                          buf,
                                          len)
            }
            24 => {
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Mcast48, buf, len)
            }
            25 => {
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Mcast32, buf, len)
            }
            26 => {
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Mcast8, buf, len)
            }
            27 => {
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::McastCtx, buf, len)
            }

            _ => debug!("Finished tests"),

        }
    }
    fn ipv6_send_packet_test(&self, tf: TF, hop_limit: u8, sac: SAC, dac: DAC) {
        ipv6_prepare_packet(tf, hop_limit, sac, dac);
        unsafe {
            self.send_ipv6_packet(&MLP, SRC_MAC_ADDR, DST_MAC_ADDR);
        }
    }

    unsafe fn send_ipv6_packet(&self,
                               _: &[u8],
                               src_mac_addr: MacAddress,
                               dst_mac_addr: MacAddress) {
        let frag_state = self.frag_state;
        let tx_state = self.tx_state;
        //frag_state.radio.config_set_pan(0xABCD);
        let ret_code = frag_state.transmit_packet(src_mac_addr,
                                                  dst_mac_addr,
                                                  &mut IP6_DGRAM,
                                                  IP6_DGRAM.len(),
                                                  None,
                                                  tx_state,
                                                  true,
                                                  true);
        debug!("Ret code: {:?}", ret_code);
    }
}

impl<'a, A: time::Alarm + 'a> time::Client for LowpanTest<'a, A> {
    fn fired(&self) {
        self.run_test_and_increment();
    }
}

impl<'a, A: time::Alarm + 'a> TransmitClient for LowpanTest<'a, A> {
    fn send_done(&self, _: &'static mut [u8], _: &TxState, _: bool, _: ReturnCode) {
        debug!("Send completed");
        self.schedule_next();
    }
}

impl<'a, A: time::Alarm + 'a> ReceiveClient for LowpanTest<'a, A> {
    fn receive<'b>(&self, buf: &'b [u8], len: u16, retcode: ReturnCode) {
        debug!("Receive completed: {:?}", retcode);
        let test_num = self.test_counter.get();
        self.test_counter.set((test_num + 1) % self.num_tests());
        self.run_check_test(test_num, buf, len)
    }
}

static mut IP6_DGRAM: [u8; IP6_HDR_SIZE + PAYLOAD_LEN] = [0; IP6_HDR_SIZE + PAYLOAD_LEN];

fn ipv6_check_receive_packet(tf: TF,
                             hop_limit: u8,
                             sac: SAC,
                             dac: DAC,
                             recv_packet: &[u8],
                             len: u16) {
    ipv6_prepare_packet(tf, hop_limit, sac, dac);
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


fn ipv6_prepare_packet(tf: TF, hop_limit: u8, sac: SAC, dac: DAC) {
    {
        let mut payload = unsafe { &mut IP6_DGRAM[IP6_HDR_SIZE..] };
        payload[0] = 128;
        payload[1] = 0;
        payload[2] = 0;
        payload[3] = 0;
        payload[4] = 0;
        payload[5] = 0;
        payload[6] = 0;
        payload[7] = 0;
        let checksum = compute_icmp_checksum(&SRC_ADDR, &DST_ADDR, &payload, 10);
        util::u16_to_slice(checksum, &mut payload[2..4]);
    }
    {
        let mut ip6_header: &mut IP6Header = unsafe { mem::transmute(IP6_DGRAM.as_mut_ptr()) };
        *ip6_header = IP6Header::new();
        ip6_header.set_payload_len(PAYLOAD_LEN as u16);

        if tf != TF::TrafficFlow {
            ip6_header.set_ecn(0b01);
        }
        if (tf as u8) & (TF::Traffic as u8) != 0 {
            ip6_header.set_dscp(0b000000);
        } else {
            ip6_header.set_dscp(0b101010);
        }

        if (tf as u8) & (TF::Flow as u8) != 0 {
            ip6_header.set_flow_label(0);
        } else {
            ip6_header.set_flow_label(0xABCDE);
        }

        ip6_header.set_next_header(58 as u8); // ICMPv6

        ip6_header.set_hop_limit(hop_limit);

        match sac {
            SAC::Inline => {
                ip6_header.src_addr = SRC_ADDR;
            }
            SAC::LLP64 => {
                // LLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.src_addr.set_unicast_link_local();
                ip6_header.src_addr.0[8..16].copy_from_slice(&SRC_ADDR.0[8..16]);
            }
            SAC::LLP16 => {
                // LLP::ff:fe00:xxxx
                ip6_header.src_addr.set_unicast_link_local();
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.src_addr.0[11] = 0xff;
                ip6_header.src_addr.0[12] = 0xfe;
                ip6_header.src_addr.0[14..16].copy_from_slice(&SRC_ADDR.0[14..16]);
            }
            SAC::LLPIID => {
                // LLP::IID
                ip6_header.src_addr.set_unicast_link_local();
                ip6_header.src_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&SRC_MAC_ADDR));
            }
            SAC::Unspecified => {}
            SAC::Ctx64 => {
                // MLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.src_addr.set_prefix(&MLP, 64);
                ip6_header.src_addr.0[8..16].copy_from_slice(&SRC_ADDR.0[8..16]);
            }
            SAC::Ctx16 => {
                // MLP::ff:fe00:xxxx
                ip6_header.src_addr.set_prefix(&MLP, 64);
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.src_addr.0[11] = 0xff;
                ip6_header.src_addr.0[12] = 0xfe;
                ip6_header.src_addr.0[14..16].copy_from_slice(&SRC_ADDR.0[14..16]);
            }
            SAC::CtxIID => {
                // MLP::IID
                ip6_header.src_addr.set_prefix(&MLP, 64);
                ip6_header.src_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&SRC_MAC_ADDR));
            }
        }

        match dac {
            DAC::Inline => {
                ip6_header.dst_addr = DST_ADDR;
            }
            DAC::LLP64 => {
                // LLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.dst_addr.set_unicast_link_local();
                ip6_header.dst_addr.0[8..16].copy_from_slice(&DST_ADDR.0[8..16]);
            }
            DAC::LLP16 => {
                // LLP::ff:fe00:xxxx
                ip6_header.dst_addr.set_unicast_link_local();
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.dst_addr.0[11] = 0xff;
                ip6_header.dst_addr.0[12] = 0xfe;
                ip6_header.dst_addr.0[14..16].copy_from_slice(&SRC_ADDR.0[14..16]);
            }
            DAC::LLPIID => {
                // LLP::IID
                ip6_header.dst_addr.set_unicast_link_local();
                ip6_header.dst_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&DST_MAC_ADDR));
            }
            DAC::Ctx64 => {
                // MLP::xxxx:xxxx:xxxx:xxxx
                ip6_header.dst_addr.set_prefix(&MLP, 64);
                ip6_header.dst_addr.0[8..16].copy_from_slice(&SRC_ADDR.0[8..16]);
            }
            DAC::Ctx16 => {
                // MLP::ff:fe00:xxxx
                ip6_header.dst_addr.set_prefix(&MLP, 64);
                // Distinct from compute_iid because the U/L bit is not flipped
                ip6_header.dst_addr.0[11] = 0xff;
                ip6_header.dst_addr.0[12] = 0xfe;
                ip6_header.dst_addr.0[14..16].copy_from_slice(&SRC_ADDR.0[14..16]);
            }
            DAC::CtxIID => {
                // MLP::IID
                ip6_header.dst_addr.set_prefix(&MLP, 64);
                ip6_header.dst_addr.0[8..16].copy_from_slice(&lowpan::compute_iid(&DST_MAC_ADDR));
            }
            DAC::McastInline => {
                // first byte is ff, that's all we know
                ip6_header.dst_addr = DST_ADDR;
                ip6_header.dst_addr.0[0] = 0xff;
            }
            DAC::Mcast48 => {
                // ffXX::00XX:XXXX:XXXX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = DST_ADDR.0[1];
                ip6_header.dst_addr.0[11..16].copy_from_slice(&DST_ADDR.0[11..16]);
            }
            DAC::Mcast32 => {
                // ffXX::00XX:XXXX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = DST_ADDR.0[1];
                ip6_header.dst_addr.0[13..16].copy_from_slice(&DST_ADDR.0[13..16]);
            }
            DAC::Mcast8 => {
                // ff02::00XX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = DST_ADDR.0[1];
                ip6_header.dst_addr.0[15] = DST_ADDR.0[15];
            }
            DAC::McastCtx => {
                // ffXX:XX + plen + pfx64 + XXXX:XXXX
                ip6_header.dst_addr.0[0] = 0xff;
                ip6_header.dst_addr.0[1] = DST_ADDR.0[1];
                ip6_header.dst_addr.0[2] = DST_ADDR.0[2];
                ip6_header.dst_addr.0[3] = 64 as u8;
                ip6_header.dst_addr.0[4..12].copy_from_slice(&MLP);
                ip6_header.dst_addr.0[12..16].copy_from_slice(&DST_ADDR.0[12..16]);
            }
        }
    }
    debug!("Packet with tf={:?} hl={} sac={:?} dac={:?}",
           tf,
           hop_limit,
           sac,
           dac);
}
