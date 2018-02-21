/* This file contains the structs, traits, and methods associated with the UDP
   layer in the Tock Networking stack. This networking stack is explained more
   in depth in the Thread_Stack_Design.txt document. */

use net::ip_utils::{IPAddr, IP6Header};
use net::ip::{IPPayload, TransportHeader, IP6SendStruct, IP6Packet, IP6Client};
use ieee802154::mac::Frame;
use net::stream::{decode_u16, decode_u8, decode_bytes};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;
use kernel::ReturnCode;
use kernel::common::take_cell::TakeCell;

// TODO: These values should be in host-byte order; if we want
// host-byte order, use the getters/setters in UDPPacket
#[derive(Copy, Clone)]
pub struct UDPHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub len: u16,
    pub cksum: u16,
}


//TODO: Remove functions reduntantly implemented on Header and Packet
impl UDPHeader {
    pub fn get_offset(&self) -> usize{8} //Always returns 8 TODO: B/c size of UDPHeader

    pub fn set_dst_port(&mut self, port: u16) {
        self.dst_port = port;
    }
    pub fn set_src_port(&mut self, port: u16) {
        self.src_port = port;
    }

    pub fn set_len(&mut self, len: u16) {
        self.len = len;
    }

    pub fn set_cksum(&mut self, cksum: u16) {
        self.cksum = cksum;
    }

    pub fn get_src_port(&self) -> u16 {
        self.src_port
    }

    pub fn get_dst_port(&self) -> u16 {
        self.dst_port
    }

    pub fn get_len(&self) -> u16 {
        self.len
    }

    pub fn get_cksum(&self) -> u16 {
        self.cksum
    }

    // TODO: This function is not ideal; here, we are breaking layering in
    // order to set the payload. This is an artifact of the networking stack
    // design, and I cannot find an easy way to fix this.
    pub fn set_payload<'a>(&self, buffer: &'a [u8], ip_payload: &mut IPPayload) -> Result<(), ()> {
        if ip_payload.payload.len() < buffer.len() {
            return Err(());
        }
        ip_payload.header = TransportHeader::UDP(*self);
        ip_payload.payload.copy_from_slice(&buffer);
        Ok(())
    }

    // TODO: change this to encode/decode stream functions?
    pub fn get_hdr_size(&self) -> usize {
        // TODO
        8
    }

    // Note that we encode all values in network-byte order
    pub fn encode(&self, buf: &mut [u8], offset: usize) -> SResult<usize> {
        // TODO
        stream_len_cond!(buf, 8 + offset);

        let mut off = offset; 
        off = enc_consume!(buf, off; encode_u16, self.src_port);
        off = enc_consume!(buf, off; encode_u16, self.dst_port);
        off = enc_consume!(buf, off; encode_u16, self.len);
        off = enc_consume!(buf, off; encode_u16, self.cksum);
        stream_done!(off, off);
    }
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
    ip6_packet: TakeCell<'static, IP6Packet<'static>>,
    ip_send_struct: &'a IP6SendStruct<'a>,
}

impl<'a> UDPSendStruct<'a> {
    pub fn new(ip6_packet: &'static mut IP6Packet<'a>,
               ip_send_struct: &'a IP6SendStruct<'a>) -> UDPSendStruct<'a> {
        UDPSendStruct {
            ip6_packet: TakeCell::new(ip6_packet),
            ip_send_struct: ip_send_struct,
        }
    }

    pub fn initialize(&self) {
        self.ip6_packet.map(|ip6_packet| ip6_packet.header = IP6Header::default());
    }
}

impl<'a> IP6Client for UDPSendStruct<'a> {
    fn send_done(&self, ip6_packet: &'static mut IP6Packet, result: ReturnCode) {
        self.ip6_packet.replace(ip6_packet);
    }
}
