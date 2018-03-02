use net::ip_utils::{IPAddr, IP6Header, compute_udp_checksum, ip6_nh};
use ieee802154::mac::{Frame, Mac};
use net::ieee802154::MacAddress;
use net::udp::udp::{UDPHeader};
use net::tcp::{TCPHeader};
use net::sixlowpan::{TxState, SixlowpanTxClient};
use net::ip::{IP6Packet, TransportHeader};
use net::ip_utils;
use kernel::ReturnCode;
use kernel::common::take_cell::TakeCell;
use core::cell::Cell;

use net::stream::{decode_u16, decode_u8, decode_bytes};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;

// TODO: Make not constants
const SRC_MAC_ADDR: MacAddress = MacAddress::Short(0xf00f);
const DST_MAC_ADDR: MacAddress = MacAddress::Short(0xf00f);

pub trait IP6Send {
    fn send_to(&self, dest: IPAddr, ip6_packet: IP6Packet); //Convenience fn, sets dest addr, sends
    fn send(&self, ip6_packet: IP6Packet); //Length can be determined from IP6Packet
    fn send_done(&self, ip6_packet: IP6Packet, result: ReturnCode);
}

pub trait IP6Client {
    fn send_done(&self, result: ReturnCode);
}

pub struct IP6SendStruct<'a> {
    ip6_packet: TakeCell<'static, IP6Packet<'static>>,   // We want this to be a TakeCell,
                                                    // so that it is easy to mutate
    src_addr: Cell<IPAddr>,
    gateway: Cell<MacAddress>,
    tx_buf: TakeCell<'static, [u8]>,
    sixlowpan: TxState<'a>,
    radio: &'a Mac<'a>,
    client: Cell<Option<&'a IP6Client>>,
}

impl<'a> IP6SendStruct<'a> {
    pub fn new(ip6_packet: &'static mut IP6Packet<'static>,
               tx_buf: &'static mut [u8],
               sixlowpan: TxState<'a>,
               radio: &'a Mac<'a>,
               client: &'a IP6Client) -> IP6SendStruct<'a> {
        IP6SendStruct {
            ip6_packet: TakeCell::new(ip6_packet),
            src_addr: Cell::new(IPAddr::new()),
            gateway: Cell::new(DST_MAC_ADDR),
            tx_buf: TakeCell::new(tx_buf),
            sixlowpan: sixlowpan,
            radio: radio,
            client: Cell::new(Some(client)),
        }
    }

    pub fn init(&self) {
    }

    pub fn set_addr(&self, src_addr: IPAddr) {
        self.src_addr.set(src_addr);
    }

    pub fn set_gateway(&self, gateway: MacAddress) {
        self.gateway.set(gateway);
    }

    fn init_packet(&self,
                   dst_addr: IPAddr,
                   transport_header: TransportHeader,
                   payload: &[u8]) {
        self.ip6_packet.map(|mut ip6_packet| {
            ip6_packet.header = IP6Header::default();
            ip6_packet.header.src_addr = self.src_addr.get();
            ip6_packet.header.dst_addr = dst_addr;
            ip6_packet.payload.set_payload(transport_header, payload);
            ip6_packet.set_transport_checksum();

        });
    }

    pub fn set_header(&mut self, ip6_header: IP6Header) {
        self.ip6_packet.map(|mut ip6_packet| ip6_packet.header = ip6_header);
    }

    pub fn send_to(&self, dst: IPAddr, transport_header: TransportHeader, payload: &[u8])
        -> ReturnCode {
        // TODO: Check return code
        self.sixlowpan.init(SRC_MAC_ADDR, DST_MAC_ADDR, None);
        self.init_packet(dst, transport_header, payload);
        let (result, completed) = self.send_next_fragment();
        if result != ReturnCode::SUCCESS {
            result
        } else {
            if completed {
                self.send_completed(result);
            }
            ReturnCode::SUCCESS
        }
    }

    fn send_next_fragment(&self) -> (ReturnCode, bool) {
        // TODO: Fix unwrap
        let tx_buf = self.tx_buf.take().unwrap();
        let ip6_packet = self.ip6_packet.take().unwrap();
        let next_frame = self.sixlowpan.next_fragment(ip6_packet, tx_buf, self.radio);
        self.ip6_packet.replace(ip6_packet);

        let result = match next_frame {
            Ok((is_done, frame)) => {
                if is_done {
                    self.tx_buf.replace(frame.into_buf());
                    self.send_completed(ReturnCode::SUCCESS);
                } else {
                    self.radio.transmit(frame);
                }
                (ReturnCode::SUCCESS, is_done)
            },
            Err((retcode, buf)) => {
                self.tx_buf.replace(buf);
                self.send_completed(ReturnCode::FAIL);
                (ReturnCode::FAIL, false)
            },
        };
        result
    }

    fn send_completed(&self, result: ReturnCode) {
        // TODO: Fix unwrap
        self.client.get().map(move |client| client.send_done(result));
    }
}

impl<'a> SixlowpanTxClient for IP6SendStruct<'a> {
    fn send_done(&self, buf: &'static mut [u8], _acked: bool, result: ReturnCode) {
        self.tx_buf.replace(buf);
        if result != ReturnCode::SUCCESS {
            self.send_completed(result);
        }

        let (result, completed) = self.send_next_fragment();
        if completed || result != ReturnCode::SUCCESS {
            self.send_completed(result);
        }
    }
}
