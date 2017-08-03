//! A dummy sixlowpan/IP sender

use capsules::net::ip::{IP6Header, MacAddr, IPAddr, ip6_nh};
use capsules::net::lowpan;
use capsules::net::lowpan::{ContextStore, Context};
use capsules::net::lowpan_fragment::{FragState, TxState, TransmitClient, ReceiveClient};
use capsules::net::util;

use core::mem;
use core::cell::Cell;

use kernel::hil::radio;
use kernel::hil::time;
use kernel::hil::time::Frequency;
use kernel::ReturnCode;

pub struct DummyStore<'a> {
    context0: Context<'a>,
}

impl<'a> DummyStore<'a> {
    pub fn new(context0: Context<'a>) -> DummyStore<'a> {
        DummyStore { context0: context0 }
    }
}

impl<'a> ContextStore<'a> for DummyStore<'a> {
    fn get_context_from_addr(&self, ip_addr: IPAddr) -> Option<Context<'a>> {
        if util::matches_prefix(&ip_addr.0, self.context0.prefix, self.context0.prefix_len) {
            // TODO: Context does not work correctly
            // Some(self.context0)
            None
        } else {
            None
        }
    }

    fn get_context_from_id(&self, ctx_id: u8) -> Option<Context<'a>> {
        if ctx_id == 0 {
            // TODO: Context does not work correctly
            // Some(self.context0)
            None
        } else {
            None
        }
    }

    fn get_context_from_prefix(&self, prefix: &[u8], prefix_len: u8) -> Option<Context<'a>> {
        if prefix_len == self.context0.prefix_len &&
           util::matches_prefix(prefix, self.context0.prefix, prefix_len) {
            //TODO: Context does not work correctly
            //Some(self.context0)
            None
        } else {
            None
        }
    }
}

pub const MLP: [u8; 8] = [0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7];
pub const SRC_ADDR: IPAddr = IPAddr([0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
                                     0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f]);
pub const DST_ADDR: IPAddr = IPAddr([0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29,
                                     0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f]);
pub const SRC_MAC_ADDR: MacAddr = MacAddr::LongAddr([0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16,
                                                     0x17]);
pub const DST_MAC_ADDR: MacAddr = MacAddr::LongAddr([0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
                                                     0x1f]);

pub const IP6_HDR_SIZE: usize = 40;
pub const PAYLOAD_LEN: usize = 200;
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

pub struct LowpanTest<'a, R: radio::Radio + 'a, C: ContextStore<'a> + 'a, A: time::Alarm + 'a> {
    radio: &'a R,
    alarm: &'a A,
    frag_state: &'a FragState<'a, R, C, A>,
    tx_state: &'a TxState<'a>,
    test_counter: Cell<usize>,
}

impl<'a, R: radio::Radio + 'a, C: ContextStore<'a> + 'a, A: time::Alarm + 'a>
LowpanTest<'a, R, C, A> {
    pub fn new(radio: &'a R, frag_state: &'a FragState<'a, R, C, A>,
               tx_state: &'a TxState<'a>,
               alarm: &'a A) -> LowpanTest<'a, R, C, A> {
        LowpanTest {
            radio: radio,
            alarm: alarm,
            frag_state: frag_state,
            tx_state: tx_state,
            test_counter: Cell::new(0),
        }
    }

    pub fn start(&self) {
        self.run_test_and_increment();
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

    fn run_check_test(&self, test_id: usize, buf: &'static mut [u8], len: u16)
    -> &'static mut [u8] {
        debug!("Running test {}:", test_id);
        match test_id {
            // Change TF compression
            0 => ipv6_check_receive_packet(TF::Inline, 255, SAC::Inline, DAC::Inline, buf, len),
            1 => ipv6_check_receive_packet(TF::Traffic, 255, SAC::Inline, DAC::Inline, buf, len),
            2 => ipv6_check_receive_packet(TF::Flow, 255, SAC::Inline, DAC::Inline, buf, len),
            3 => ipv6_check_receive_packet(TF::TrafficFlow, 255, SAC::Inline, DAC::Inline, buf, len),

            // Change HL compression
            4 => ipv6_check_receive_packet(TF::TrafficFlow,
                                           255, SAC::Inline, DAC::Inline, buf, len),
            5 => ipv6_check_receive_packet(TF::TrafficFlow,
                                           64, SAC::Inline, DAC::Inline, buf, len),
            6 => ipv6_check_receive_packet(TF::TrafficFlow,
                                           1, SAC::Inline, DAC::Inline, buf, len),
            7 => ipv6_check_receive_packet(TF::TrafficFlow,
                                           42, SAC::Inline, DAC::Inline, buf, len),

            // Change source compression
            8 => ipv6_check_receive_packet(TF::TrafficFlow,
                                           42, SAC::Inline, DAC::Inline, buf, len),
            9 => ipv6_check_receive_packet(TF::TrafficFlow,
                                           42, SAC::LLP64, DAC::Inline, buf, len),
            10 => ipv6_check_receive_packet(TF::TrafficFlow,
                                            42, SAC::LLP16, DAC::Inline, buf, len),
            11 => ipv6_check_receive_packet(TF::TrafficFlow,
                                            42, SAC::LLPIID, DAC::Inline, buf, len),
            12 => ipv6_check_receive_packet(TF::TrafficFlow,
                                            42, SAC::Unspecified, DAC::Inline, buf, len),
            13 => ipv6_check_receive_packet(TF::TrafficFlow,
                                            42, SAC::Ctx64, DAC::Inline, buf, len),
            14 => ipv6_check_receive_packet(TF::TrafficFlow,
                                            42, SAC::Ctx16, DAC::Inline, buf, len),
            15 => ipv6_check_receive_packet(TF::TrafficFlow,
                                            42, SAC::CtxIID, DAC::Inline, buf, len),

            // Change dest compression
            16 =>
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Inline, buf, len),
            17 =>
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::LLP64, buf, len),
            18 =>
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::LLP16, buf, len),
            19 =>
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::LLPIID, buf, len),
            20 =>
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Ctx64, buf, len),
            21 =>
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::Ctx16, buf, len),
            22 =>
                ipv6_check_receive_packet(TF::TrafficFlow, 42, SAC::CtxIID, DAC::CtxIID, buf, len),
            23 =>
                ipv6_check_receive_packet(TF::TrafficFlow,
                                          42, SAC::CtxIID, DAC::McastInline, buf, len),
            24 =>
                ipv6_check_receive_packet(TF::TrafficFlow,
                                          42, SAC::CtxIID, DAC::Mcast48, buf, len),
            25 =>
                ipv6_check_receive_packet(TF::TrafficFlow,
                                          42, SAC::CtxIID, DAC::Mcast32, buf, len),
            26 =>
                ipv6_check_receive_packet(TF::TrafficFlow,
                                          42, SAC::CtxIID, DAC::Mcast8, buf, len),
            27 =>
                ipv6_check_receive_packet(TF::TrafficFlow,
                                          42, SAC::CtxIID, DAC::McastCtx, buf, len),

            _ => buf,
        }
    }
    fn ipv6_send_packet_test(&self, tf: TF, hop_limit: u8, sac: SAC, dac: DAC) {
            ipv6_prepare_packet(tf, hop_limit, sac, dac);
            unsafe {
                self.send_ipv6_packet(&MLP,
                                      SRC_MAC_ADDR,
                                      DST_MAC_ADDR);
            }
    }

    unsafe fn send_ipv6_packet(&self,
                               _: &[u8],
                               src_mac_addr: MacAddr,
                               dst_mac_addr: MacAddr) {
        let frag_state = self.frag_state;
        let tx_state = self.tx_state;
            frag_state.radio.config_set_pan(0xABCD);
            match src_mac_addr {
                MacAddr::ShortAddr(addr) => frag_state.radio.config_set_address(addr),
                MacAddr::LongAddr(addr) => frag_state.radio.config_set_address_long(addr),
            };

            let src_long = match src_mac_addr {
                MacAddr::ShortAddr(_) => false,
                MacAddr::LongAddr(_) => true,
            };
            /*
            let dst_long = match dst_mac_addr {
                MacAddr::ShortAddr(_) => false,
                MacAddr::LongAddr(_) => true,
            };
            */
            let ret_code = frag_state.transmit_packet(src_mac_addr, dst_mac_addr, &mut IP6_DGRAM,
                                                      tx_state, src_long, true);
            debug!("Ret code: {:?}", ret_code);

        }

}

impl<'a, R: radio::Radio + 'a, C: ContextStore<'a> + 'a, A: time::Alarm + 'a>
time::Client for LowpanTest<'a, R, C, A> {
    fn fired(&self) {
        self.run_test_and_increment();
        if self.test_counter.get() < self.num_tests() {
            self.schedule_next();
        }
    }
}

impl<'a, R: radio::Radio + 'a, C: ContextStore<'a> + 'a, A: time::Alarm + 'a>
TransmitClient for LowpanTest<'a, R, C, A> {
    fn send_done(&self, _: &'static mut [u8], _: &TxState, _: bool, _: ReturnCode) {
        debug!("Send completed!");
        self.schedule_next();
    }
}

impl<'a, R: radio::Radio + 'a, C: ContextStore<'a> + 'a, A: time::Alarm + 'a>
ReceiveClient for LowpanTest<'a, R, C, A> {
    fn receive(&self, buf: &'static mut [u8], len: u16, _: ReturnCode) -> &'static mut [u8] {
        debug!("Receive completed");
        let test_num = self.test_counter.get();
        self.test_counter.set((test_num + 1) % self.num_tests());
        self.run_check_test(test_num, buf, len)
    }
}

static mut IP6_DGRAM: [u8; IP6_HDR_SIZE + PAYLOAD_LEN] = [0; IP6_HDR_SIZE + PAYLOAD_LEN];

fn ipv6_check_receive_packet(tf: TF, hop_limit: u8, sac: SAC, dac: DAC,
                             recv_packet: &'static mut [u8], len: u16) -> &'static mut [u8]{
    ipv6_prepare_packet(tf, hop_limit, sac, dac);
    debug!("Len: {}", len);
    unsafe {
        for i in 0..240 {
            if recv_packet[i] != IP6_DGRAM[i] {
                debug!("Packets differ at idx: {} where recv = {}, ref = {}",
                       i, recv_packet[i], IP6_DGRAM[i]);
            }
        }
    }
    recv_packet
}


fn ipv6_prepare_packet(tf: TF, hop_limit: u8, sac: SAC, dac: DAC) {
    {
        let mut payload = unsafe { &mut IP6_DGRAM[IP6_HDR_SIZE..] };
        for i in 0..PAYLOAD_LEN {
            payload[i] = i as u8;
        }
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

        ip6_header.set_next_header(ip6_nh::NO_NEXT);

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
           tf, hop_limit, sac, dac);
}

