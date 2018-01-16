/* This file contains the structs, traits, and methods associated with the UDP
   layer in the Tock Networking stack. This networking stack is explained more
   in depth in the Thread_Stack_Design.txt document. */

use net::ip_utils::{IPAddr};
use kernel::ReturnCode;


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
    pub head: UDPHeader,
    pub payload: &'a mut [u8], 
    pub len: u16, // length of payload
}

impl<'a> UDPPacket<'a> {
    pub fn reset(&mut self){} //Sets fields to appropriate defaults    
    pub fn get_offset(&self) -> usize{8} //Always returns 8

    pub fn set_dest_port(&mut self, port: u16){
        self.head.dst_port = port.to_be();
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

}

pub trait UDPSend {
    fn send(dest: IPAddr, udp_packet: &'static mut UDPPacket); // dest rqrd
    fn send_done(buf: &'static mut UDPPacket, result: ReturnCode);
}


