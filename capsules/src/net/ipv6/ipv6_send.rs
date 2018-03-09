use net::ipv6::ip_utils::{IPAddr, compute_udp_checksum, ip6_nh};
use ieee802154::mac::{Frame, Mac, TxClient};
use net::ieee802154::MacAddress;
use net::udp::udp::{UDPHeader};
use net::tcp::{TCPHeader};
use net::sixlowpan::{TxState, SixlowpanTxClient};
use net::ipv6::ipv6::{IP6Packet, IP6Header, TransportHeader};
use net::ipv6::ip_utils;
use kernel::ReturnCode;
use kernel::common::take_cell::TakeCell;
use core::cell::Cell;

use net::stream::{decode_u16, decode_u8, decode_bytes};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;

// TODO: Make not constants
const SRC_MAC_ADDR: MacAddress = MacAddress::Short(0xf00f);
const DST_MAC_ADDR: MacAddress = MacAddress::Short(0xf00e);

pub trait IP6Client {
    fn send_done(&self, result: ReturnCode);
}
//TODO: Should we enforce that anything that implements IP6Sender
// IP6Client as well?
pub trait IP6Sender<'a> {
    fn set_client(&self, client: &'a IP6Client);

    fn set_addr(&self, src_addr: IPAddr);

    fn set_gateway(&self, gateway: MacAddress);

    fn set_header(&mut self, ip6_header: IP6Header);

    fn send_to(&self, dst: IPAddr, transport_header: TransportHeader, payload: &[u8])
        -> ReturnCode;
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

impl<'a> IP6Sender<'a> for IP6SendStruct<'a> { //Public functions for this IP6Sender

    fn set_client(&self, client: &'a IP6Client) {
        self.client.set(Some(client));
    }

    fn set_addr(&self, src_addr: IPAddr) {
        self.src_addr.set(src_addr);
    }

    fn set_gateway(&self, gateway: MacAddress) {
        self.gateway.set(gateway);
    }

    fn set_header(&mut self, ip6_header: IP6Header) {
        self.ip6_packet.map(|mut ip6_packet| ip6_packet.header = ip6_header);
    }

    // NOTE that if there is an error during sending, it will be delivered via
    // the callback
    fn send_to(&self, dst: IPAddr, transport_header: TransportHeader, payload: &[u8])
        -> ReturnCode {
        // TODO: Check return code
        self.sixlowpan.init(SRC_MAC_ADDR, DST_MAC_ADDR, None);
        self.init_packet(dst, transport_header, payload);
        self.send_next_fragment()
    }
}

impl<'a> IP6SendStruct<'a> { //Private Functions for this sender
    pub fn new(ip6_packet: &'static mut IP6Packet<'static>,
               tx_buf: &'static mut [u8],
               sixlowpan: TxState<'a>,
               radio: &'a Mac<'a>) -> IP6SendStruct<'a> {
        IP6SendStruct {
            ip6_packet: TakeCell::new(ip6_packet),
            src_addr: Cell::new(IPAddr::new()),
            gateway: Cell::new(DST_MAC_ADDR),
            tx_buf: TakeCell::new(tx_buf),
            sixlowpan: sixlowpan,
            radio: radio,
            client: Cell::new(None), 
        }
    }

    fn init_packet(&self,
                   dst_addr: IPAddr,
                   transport_header: TransportHeader,
                   payload: &[u8]) {
        self.ip6_packet.map(|mut ip6_packet| {
            ip6_packet.header = IP6Header::default();
            ip6_packet.header.src_addr = self.src_addr.get();
            ip6_packet.header.dst_addr = dst_addr;
            ip6_packet.set_payload(transport_header, payload);
            ip6_packet.set_transport_checksum();
            debug!("within map: {}", ip6_packet.header.get_next_header());
        });
    }

    // Returns EBUSY if the tx_buf is not there
    fn send_next_fragment(&self) -> ReturnCode {
        // TODO: Fix unwrap
        match self.tx_buf.take() {
            Some(tx_buf) => {
                let ip6_packet = self.ip6_packet.take().unwrap();
                debug!("Next Header is: {}", ip6_packet.header.get_next_header());
                let next_frame = self.sixlowpan.next_fragment(ip6_packet, tx_buf, self.radio);
                self.ip6_packet.replace(ip6_packet);

                match next_frame {
                    Ok((is_done, frame)) => {
                        if is_done {
                            self.tx_buf.replace(frame.into_buf());
                            self.send_completed(ReturnCode::SUCCESS);
                        } else {
                            self.radio.transmit(frame);
                        }
                    },
                    Err((retcode, buf)) => {
                        self.tx_buf.replace(buf);
                        self.send_completed(ReturnCode::FAIL);
                    },
                }
                ReturnCode::SUCCESS
            },
            None => {
                ReturnCode::EBUSY
            },
        }
    }

    fn send_completed(&self, result: ReturnCode) {
        self.client.get().map(move |client| client.send_done(result));
    }
}

impl<'a> TxClient for IP6SendStruct<'a> {
    fn send_done(&self, tx_buf: &'static mut [u8], acked: bool, result: ReturnCode) {
        self.tx_buf.replace(tx_buf);
        debug!("sendDone return code is: {:?}", result);
        //The below code introduces a delay between frames to prevent 
        // a race condition on the receiver
        //it is sorta complicated bc I was having some trouble with dead code eliminationa
        //TODO: Remove this one link layer is fixed
        let mut i = 0;
        let mut array: [u8; 100] = [0x0; 100]; //used in introducing delay between frames
        while(i < 1000000) {
            array[i % 100] = (i % 100) as u8;
            i = i + 1;
            if (i % 100000 == 0) {
                debug!("Delay, step {:?}", i / 100000);
            }
        }
        //TODO: Handle sending link layer ACKs        
        //self.send_next(tx_buf);
        // TODO: Check return value
        self.send_next_fragment();
    }
}
