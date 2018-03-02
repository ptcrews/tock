/* This file contains the structs, traits, and methods associated with the UDP
   layer in the Tock Networking stack. This networking stack is explained more
   in depth in the Thread_Stack_Design.txt document. */

use net::ip_utils::{IPAddr, IP6Header, ip6_nh};
use net::ip::{IPPayload, TransportHeader, IP6Packet};
use net::ipv6::ipv6_send::{IP6SendStruct, IP6Client};
use ieee802154::mac::Frame;
use net::stream::{decode_u16, decode_u8, decode_bytes};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;
use kernel::ReturnCode;
use kernel::common::take_cell::TakeCell;

#[derive(Copy, Clone)]
pub struct UDPHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub len: u16,
    pub cksum: u16,
}

impl Default for UDPHeader {
    fn default() -> UDPHeader {
        UDPHeader {
            src_port: 0,
            dst_port: 0,
            len: 8,
            cksum: 0,
        }
    }
}

impl UDPHeader {
    pub fn new() -> UDPHeader {
        UDPHeader::default()
    }
    pub fn get_offset(&self) -> usize{8} //Always returns size of UDP Header

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

    // TODO: change this to encode/decode stream functions?
    pub fn get_hdr_size(&self) -> usize {
        // TODO
        8
    }

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

    pub fn decode(buf: &[u8]) -> SResult<UDPHeader> { //TODO: Test me
        stream_len_cond!(buf, 8);
        let mut udp_header = Self::new();
        let off = 0;
        let (off, src_port) = dec_try!(buf, off; decode_u16);
        udp_header.src_port = u16::from_be(src_port);
        let (off, dst_port) = dec_try!(buf, off; decode_u16);
        udp_header.dst_port = u16::from_be(dst_port);
        let (off, len) = dec_try!(buf, off; decode_u16);
        udp_header.len = u16::from_be(len);
        let (off, cksum) = dec_try!(buf, off; decode_u16);
        udp_header.cksum = u16::from_be(cksum);
        stream_done!(off, udp_header);
    }
}
