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

pub struct UDPPacket<'a> { /* Example UDP Packet struct */
    pub header: UDPHeader,
    pub payload: &'a mut [u8], 
    pub len: u16, // length of payload
}

impl<'a> UDPPacket<'a> {
    pub fn reset(&self){} //Sets fields to appropriate defaults    
    pub fn get_offset(&self) -> usize{8} //Always returns 8 TODO: Why??

    pub fn set_dest_port(&self, port: u16){} 
    pub fn set_src_port(&self, port: u16){}
    pub fn set_len(&self, len: u16){}
    pub fn set_cksum(&self, cksum: u16){}
    pub fn get_dest_port(&self) -> u16{0}
    pub fn get_src_port(&self) -> u16{0}
    pub fn get_len(&self) -> u16{0}
    pub fn get_cksum(&self) -> u16{0}

    pub fn set_payload(&self, payload: &'a [u8]){}

}

pub trait UDPSend {
    fn send(dest: IPAddr, udp_packet: &'static mut UDPPacket); // dest rqrd
    fn send_done(buf: &'static mut UDPPacket, result: ReturnCode);
}
