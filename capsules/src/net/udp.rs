/* This file contains the structs, traits, and methods associated with the UDP
   layer in the Tock Networking stack. This networking stack is explained more
   in depth in the Thread_Stack_Design.txt document. */

use net::ip_utils::{IPAddr};
use ieee802154::mac::Frame;
use kernel::ReturnCode;

// TODO: These values should be in network-byte order; if we want
// host-byte order, use the getters/setters in UDPPacket
pub struct UDPHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub len: u16,
    pub cksum: u16,
}

pub struct UDPSocketExample { /* Example UDP socket implementation */
    pub src_ip: IPAddr,
    pub src_port: u16,
}

pub trait UDPSocket:UDPSend {
    fn bind(&self, src_ip: IPAddr, src_port: u16) -> ReturnCode;
    fn send(&self, dest: IPAddr, udp_packet: &'static mut UDPPacket) -> ReturnCode;
    fn send_done(&self, udp_packet: &'static mut UDPPacket, result: ReturnCode);
}

pub struct UDPPacket<'a> { /* UDP Packet struct */
    pub header: UDPHeader,
    pub payload: &'a mut [u8], 
    pub len: u16, // length of payload
}

impl<'a> UDPPacket<'a> {
    pub fn reset(&self){} //Sets fields to appropriate defaults    
    pub fn get_offset(&self) -> usize{8} //Always returns 8 TODO: B/c size of UDPHeader

    pub fn set_dst_port(&mut self, port: u16) {
        self.header.dst_port = port.to_be();
    }
    pub fn set_src_port(&mut self, port: u16) {
        self.header.src_port = port.to_be();
    }

    pub fn set_len(&mut self, len: u16) {
        self.header.len = len.to_be();
    }

    // TODO: Check endianness
    pub fn set_cksum(&mut self, cksum: u16) {
        self.header.cksum = cksum;
    }

    pub fn get_src_port(&self) -> u16 {
        u16::from_be(self.header.src_port)
    }

    pub fn get_dst_port(&self) -> u16 {
        u16::from_be(self.header.dst_port)
    }

    pub fn get_len(&self) -> u16 {
        u16::from_be(self.header.len)
    }

    pub fn get_cksum(&self) -> u16 {
        self.header.cksum
    }

    pub fn set_src_port(&mut self, port: u16){
        self.head.src_port = port.to_be();
    }

    pub fn set_len(&mut self, len: u16){
        self.head.len = len.to_be();
    }

    pub fn set_cksum(&mut self, cksum: u16){ // Assumes cksum passed in network byte order
        self.head.cksum = cksum;
    }

    pub fn get_dest_port(&self) -> u16{
        u16::from_be(self.head.dst_port)
    }

    pub fn get_src_port(&self) -> u16{
        u16::from_be(self.head.src_port)
    }

    pub fn get_len(&self) -> u16{
        u16::from_be(self.head.len)
    }

    pub fn get_cksum(&self) -> u16{ // Returns cksum in network byte order
        self.head.cksum
    }

    pub fn set_payload(&self, payload: &'a [u8]){} //TODO

    pub fn write_to_frame(&self, mut frame: Frame) {
        // TODO
        let mut udp_header: [u8; 8] = [0; 8];
        udp_header[0] = self.header.src_port as u8;
        udp_header[1] = (self.header.src_port >> 8) as u8;
        udp_header[2] = self.header.dst_port as u8;
        udp_header[3] = (self.header.dst_port >> 8) as u8;
        udp_header[4] = self.header.len as u8;
        udp_header[5] = (self.header.len >> 8) as u8;
        udp_header[6] = self.header.cksum as u8;
        udp_header[7] = (self.header.cksum >> 8) as u8;
        frame.append_payload(&udp_header);
        frame.append_payload(&self.payload);
    }

}

pub trait UDPSend {
    fn send(dest: IPAddr, udp_packet: &'static mut UDPPacket); // dest rqrd
    fn send_done(buf: &'static mut UDPPacket, result: ReturnCode);
}
