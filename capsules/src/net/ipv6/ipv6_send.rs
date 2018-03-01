use net::ip_utils::{IPAddr, IP6Header, compute_udp_checksum, ip6_nh};
use ieee802154::mac::{Frame, Mac};
use net::ieee802154::MacAddress;
use net::udp::{UDPHeader};
use net::tcp::{TCPHeader};
use net::sixlowpan::{TxState, SixlowpanTxClient};
use net::ip::{IP6Packet, TransportHeader};
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
    fn send_done(&self, ip6_packet: &'static mut IP6Packet, result: ReturnCode);
}

pub struct IP6SendStruct<'a> {
    ip6_packet: TakeCell<'static, IP6Packet<'static>>,
    src_addr: Cell<IPAddr>,
    tx_buf: TakeCell<'static, [u8]>,
    sixlowpan: TxState<'a>,
    radio: &'a Mac<'a>,
    client: Cell<Option<&'a IP6Client>>,
}

impl<'a> IP6SendStruct<'a> {
    pub fn new(tx_buf: &'static mut [u8],
               sixlowpan: TxState<'a>,
               radio: &'a Mac<'a>,
               client: &'a IP6Client) -> IP6SendStruct<'a> {
        IP6SendStruct {
            ip6_packet: TakeCell::empty(),
            src_addr: Cell::new(IPAddr::new()),
            tx_buf: TakeCell::new(tx_buf),
            sixlowpan: sixlowpan,
            radio: radio,
            client: Cell::new(Some(client)),
        }
    }

    pub fn set_addr(&self, src_addr: IPAddr) {
        self.src_addr.set(src_addr);
    }

    pub fn set_next_header(&self) {
    }

    pub fn initialize_packet(&self) {
        self.ip6_packet.map(|ip6_packet| {
            ip6_packet.header = IP6Header::default();
            ip6_packet.header.src_addr = self.src_addr.get();
        });
    }

    pub fn set_header(&self, ip6_header: IP6Header) {
        self.ip6_packet.map(|ip6_packet| ip6_packet.header = ip6_header);
    }

    pub fn send_to(&self, dest: IPAddr, ip6_packet: &'static mut IP6Packet<'static>)
            -> Result<(), (ReturnCode, &'static mut IP6Packet<'static>)> {
        self.sixlowpan.init(SRC_MAC_ADDR, DST_MAC_ADDR, None);
        if self.ip6_packet.is_some() {
            return Err((ReturnCode::EBUSY, ip6_packet));
        }
        // This synchronously returns any errors in the first fragment
        let (result, completed) = self.send_next_fragment();
        if result != ReturnCode::SUCCESS {
            Err((result, self.ip6_packet.take().unwrap()))
        } else {
            if completed {
                self.send_completed(result);
            }
            Ok(())
        }
    }

    /*
    pub fn send_to(&self, dest: IPAddr, transport_header: TransportHeader) {
    }
    */

    fn send_next_fragment(&self) -> (ReturnCode, bool) {
        // TODO: Fix unwrap
        let tx_buf = self.tx_buf.take().unwrap();
        let ip6_packet = self.ip6_packet.take().unwrap();
        let next_frame = self.sixlowpan.next_fragment(&ip6_packet, tx_buf, self.radio);
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
        let ip6_packet = self.ip6_packet.take().unwrap();
        self.client.get().map(move |client| client.send_done(ip6_packet, result));
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
