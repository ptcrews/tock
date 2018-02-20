/* This file contains the structs, traits, and methods associated with the UDP
   layer in the Tock Networking stack. This networking stack is explained more
   in depth in the Thread_Stack_Design.txt document. */

use net::ip_utils::{IPAddr};
use net::ip::{IPPayload, TransportHeader};
use ieee802154::mac::Frame;
use net::stream::{decode_u16, decode_u8, decode_bytes};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;
use kernel::ReturnCode;

// TODO: These values should be in network-byte order; if we want
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
        self.dst_port = port.to_be();
    }
    pub fn set_src_port(&mut self, port: u16) {
        self.src_port = port.to_be();
    }

    pub fn set_len(&mut self, len: u16) {
        self.len = len.to_be();
    }

    pub fn set_cksum(&mut self, cksum: u16) {
        //self.cksum = cksum.to_be();
        self.cksum = cksum;
    }

    pub fn get_src_port(&self) -> u16 {
        u16::from_be(self.src_port)
    }

    pub fn get_dst_port(&self) -> u16 {
        u16::from_be(self.dst_port)
    }

    pub fn get_len(&self) -> u16 {
        u16::from_be(self.len)
    }

    pub fn get_cksum(&self) -> u16 {
        //u16::from_be(self.cksum)
        self.cksum
    }

    // TODO: This function is not ideal; here, we are breaking layering in
    // order to set the payload. This is an artifact of the networking stack
    // design, and I cannot find an easy way to fix this.
    pub fn set_payload<'a>(&self, buffer: &'a [u8], ip_payload: &'a mut IPPayload<'a>) -> Result<(), ()> {
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
