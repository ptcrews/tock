/* This file contains structs, traits, and methods associated with the IP layer
   of the networking stack. For a full description of the networking stack on
   tock, see the Thread_Stack_Design.txt document */

use net::ip_utils::{IPAddr};
use net::udp::{UDPPacket};
use kernel::ReturnCode;


pub struct IP6Header {
    pub version_class_flow: [u8; 4],
    pub payload_len: u16,
    pub next_header: u8,
    pub hop_limit: u8,
    pub src_addr: IPAddr,
    pub dst_addr: IPAddr,
}

pub enum TransportPacket<'a> { 
    UDP(UDPPacket<'a>),
    /* TCP(TCPPacket), // NOTE: TCP,ICMP,RawIP traits not yet implemented
                     // , but follow logically from UDPPacket. 
    ICMP(ICMPPacket),
    Raw(RawIPPacket), */
}

pub struct IP6Packet<'a> {
    pub header: IP6Header,
    pub payload: TransportPacket<'a>,
} 


impl<'a> IP6Packet<'a> {
    pub fn reset(&self){} //Sets fields to appropriate defaults
    pub fn get_offset(&self) -> usize{40} //Always returns 40 until we add options support
    
    // Remaining functions are just getters and setters for the header fields
    pub fn set_tf(&self, tf: u8){}
    pub fn set_flow_label(&self, flow_label: u8){}
    pub fn set_len(&self, len: u16){}
    pub fn set_protocol(&self, proto: u8){}
    pub fn set_dest_addr(&self, dest: IPAddr){}
    pub fn set_src_addr(&self, src: IPAddr){}
    pub fn get_tf(&self) -> u8{0}
    pub fn get_flow_label(&self)-> u8{0}
    pub fn get_len(&self) -> u16{0}
    pub fn get_protocol(&self) -> u8{0}
    pub fn get_dest_addr(&self) -> IPAddr{
        self.header.dst_addr
    }
    pub fn get_src_addr(&self) -> IPAddr{
        self.header.src_addr
    }

    pub fn set_transpo_cksum(&self){} //Looks at internal buffer assuming
    // it contains a valid IP packet, checks the payload type. If the payload
    // type requires a cksum calculation, this function calculates the 
    // psuedoheader cksum and calls the appropriate transport packet function
    // using this pseudoheader cksum to set the transport packet cksum

}

pub trait IP6Send {
    fn send_to(&self, dest: IPAddr, ip6_packet: IP6Packet); //Convenience fn, sets dest addr, sends
    fn send(&self, ip6_packet: IP6Packet); //Length can be determined from IP6Packet
    fn send_done(&self, ip6_packet: IP6Packet, result: ReturnCode);
}

