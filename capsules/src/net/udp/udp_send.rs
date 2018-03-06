use net::ipv6::ip_utils::{IPAddr, IP6Header, ip6_nh};
use net::ipv6::ipv6::{IPPayload, TransportHeader, IP6Packet};
use net::ipv6::ipv6_send::{IP6SendStruct, IP6Client};
use net::udp::udp::UDPHeader;
use ieee802154::mac::Frame;
use net::stream::{decode_u16, decode_u8, decode_bytes};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;
use kernel::ReturnCode;
use kernel::common::take_cell::TakeCell;
use core::cell::Cell;


pub trait UDPSendClient {
    fn send_done(&self, result: ReturnCode);
}

pub struct UDPSocketExample { /* Example UDP socket implementation */
    pub src_ip: IPAddr,
    pub src_port: u16,
}

pub trait UDPSocket:UDPSend {
    fn bind(&self, src_ip: IPAddr, src_port: u16) -> ReturnCode;
    fn send<'a>(&self, dest: IPAddr, udp_header: UDPHeader, buf: &'a [u8]) -> ReturnCode;
    // TODO: Isn't this supposed to be a callback?
    fn send_done(&self, udp_header: UDPHeader, result: ReturnCode);
}

pub trait UDPSend {
    fn send<'a>(&self, dest: IPAddr, udp_header: UDPHeader, buf: &'a [u8]) -> ReturnCode;
    // TODO: Isn't this supposed to be a callback?
    fn send_done(&self, udp_header: UDPHeader, result: ReturnCode);
}

pub struct UDPSendStruct<'a> {
    ip_send_struct: &'a IP6SendStruct<'a>,
    client: Cell<Option<&'a UDPSendClient>>,
}

impl<'a> UDPSendStruct<'a> {
    pub fn new(ip_send_struct: &'a IP6SendStruct<'a>) -> UDPSendStruct<'a> {
        UDPSendStruct {
            ip_send_struct: ip_send_struct,
            client: Cell::new(None),
        }
    }

    pub fn set_client(&self, client: &'a UDPSendClient) {
        self.client.set(Some(client));
    }

    pub fn send_to(&self, dest: IPAddr, dst_port: u16, src_port: u16, buf: &'a [u8]) -> ReturnCode {
        let mut udp_header = UDPHeader::new();
        udp_header.set_dst_port(dst_port);
        udp_header.set_src_port(src_port);
        self.send(dest, udp_header, buf)
    }

    pub fn send(&self, dest: IPAddr, mut udp_header: UDPHeader, buf: &'a [u8]) -> ReturnCode {
        let total_length = buf.len() + udp_header.get_hdr_size();
        udp_header.set_len(total_length as u16);
        let transport_header = TransportHeader::UDP(udp_header);
        self.ip_send_struct.send_to(dest, transport_header, buf)
    }
}

impl<'a> IP6Client for UDPSendStruct<'a> {
    fn send_done(&self, result: ReturnCode) {
        self.client.get().map(|client| client.send_done(result));
    }
}
