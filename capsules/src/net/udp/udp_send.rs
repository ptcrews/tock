use net::ipv6::ip_utils::{IPAddr, ip6_nh};
use net::ipv6::ipv6::{IPPayload, IP6Header, TransportHeader, IP6Packet};
use net::ipv6::ipv6_send::{IP6Sender, IP6Client};
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

/*pub trait UDPSocket:UDPSend {
    fn bind(&self, src_ip: IPAddr, src_port: u16) -> ReturnCode;
    fn send<'a>(&self, dest: IPAddr, udp_header: UDPHeader, buf: &'a [u8]) -> ReturnCode;
    // TODO: Isn't this supposed to be a callback?
    fn send_done(&self, udp_header: UDPHeader, result: ReturnCode);
}*/


pub struct UDPSendStruct<'a, T: IP6Sender<'a> + 'a> {
    ip_send_struct: &'a T,
    client: Cell<Option<&'a UDPSendClient>>,
}

//Below is a proposed UDP trait. I tried using it with app_layer_lowpan_frag and 
//gave up after an hour of trying to get it to compile. I am also still not sure this is
//quite what we want.
pub trait UDPSender<'a> {
    fn set_client(&self, client: &'a UDPSendClient);

    fn send_to(&self, dest: IPAddr, dst_port: u16, src_port: u16, buf: &'a [u8]) -> ReturnCode;

    fn send(&self, dest: IPAddr, mut udp_header: UDPHeader, buf: &'a [u8]) -> ReturnCode;
}


impl<'a, T: IP6Sender<'a>> UDPSender<'a> for UDPSendStruct<'a, T> {

    fn set_client(&self, client: &'a UDPSendClient) {
        self.client.set(Some(client));
    }

    fn send_to(&self, dest: IPAddr, dst_port: u16, src_port: u16, buf: &'a [u8]) -> ReturnCode {
        let mut udp_header = UDPHeader::new();
        udp_header.set_dst_port(dst_port);
        udp_header.set_src_port(src_port);
        self.send(dest, udp_header, buf)
    }

    fn send(&self, dest: IPAddr, mut udp_header: UDPHeader, buf: &'a [u8]) -> ReturnCode {
        let total_length = buf.len() + udp_header.get_hdr_size();
        udp_header.set_len(total_length as u16);
        let transport_header = TransportHeader::UDP(udp_header);
        self.ip_send_struct.send_to(dest, transport_header, buf)
    }
}

impl<'a, T: IP6Sender<'a>> UDPSendStruct<'a, T> {
    pub fn new(ip_send_struct: &'a T) -> UDPSendStruct<'a, T> {
        UDPSendStruct {
            ip_send_struct: ip_send_struct,
            client: Cell::new(None),
        }
    }
}

/*
impl<'a, T: IP6Sender<'a>> UDPSendStruct<'a, T> {
    pub fn new(ip_send_struct: &'a T) -> UDPSendStruct<'a, T> {
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
*/

impl<'a, T: IP6Sender<'a>> IP6Client for UDPSendStruct<'a, T> {
    fn send_done(&self, result: ReturnCode) {
        self.client.get().map(|client| client.send_done(result));
    }
}
