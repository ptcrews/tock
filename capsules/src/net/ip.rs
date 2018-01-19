/* This file contains structs, traits, and methods associated with the IP layer
   of the networking stack. For a full description of the networking stack on
   tock, see the Thread_Stack_Design.txt document */

use net::ip_utils::{IPAddr, IP6Header};
use ieee802154::mac::Frame;
use net::udp::{UDPPacket};
use net::tcp::{TCPPacket};
use kernel::ReturnCode;


// TODO: Note that this design decision means that we cannot have recursive
// IP6 packets directly - we must have/use RawIPPackets instead. This makes
// it difficult to recursively compress IP6 packets as required by 6lowpan
pub enum TransportPacket<'a> {
    UDP(UDPPacket<'a>),
    TCP(TCPPacket<'a>), // NOTE: TCP,ICMP,RawIP traits not yet implemented
                        // , but follow logically from UDPPacket. 
/*
    ICMP(ICMPPacket<'a>),
    Raw(RawIPPacket<'a>), */
}

pub struct IP6Packet<'a> {
    pub header: IP6Header,
    pub payload: TransportPacket<'a>,
}

impl<'a> IP6Packet<'a> {
    pub fn reset(&self){} //Sets fields to appropriate defaults
    pub fn get_offset(&self) -> usize{40} //Always returns 40 until we add options support

    // Remaining functions are just getters and setters for the header fields
    pub fn set_traffic_class(&mut self, new_tc: u8){
        self.header.set_traffic_class(new_tc);
    }

    pub fn set_dscp(&mut self, new_dscp: u8) {
        self.header.set_dscp(new_dscp);
    }

    pub fn set_ecn(&mut self, new_ecn: u8) {
        self.header.set_ecn(new_ecn);
    }

    pub fn set_flow_label(&mut self, flow_label: u32){
        self.header.set_flow_label(flow_label);
    }

    pub fn set_payload_len(&mut self, len: u16){
        self.header.set_payload_len(len);
    }

    pub fn set_next_header(&mut self, new_nh: u8){
        self.header.set_next_header(new_nh);
    }

    pub fn set_hop_limit(&mut self, new_hl: u8) {
        self.header.set_hop_limit(new_hl);
    }

    pub fn set_dest_addr(&mut self, dest: IPAddr){
        self.header.dst_addr = dest;
    }
    pub fn set_src_addr(&mut self, src: IPAddr){
        self.header.src_addr = src;
    }

    pub fn get_traffic_class(&self) -> u8{
        self.header.get_traffic_class()
    }

    pub fn get_dscp(&self) -> u8{
        self.header.get_dscp()
    }

    pub fn get_ecn(&self) -> u8{
        self.header.get_ecn()
    }

    pub fn get_flow_label(&self)-> u32{
        self.header.get_flow_label()
    }

    pub fn get_payload_len(&self) -> u16{
        self.header.get_payload_len()
    }

    pub fn get_total_len(&self) -> u16 {
        self.header.get_total_len()
    }

    pub fn get_next_header(&self) -> u8{
        self.header.get_next_header()
    }

    pub fn get_dest_addr(&self) -> IPAddr{
        self.header.dst_addr
    }

    pub fn get_src_addr(&self) -> IPAddr{
        self.header.src_addr
    }

    pub fn get_payload(&self) -> &[u8] {
        match self.payload {
            TransportPacket::UDP(ref udp_packet) => {
                return udp_packet.payload
            },
        }
    }

    pub fn write_to_frame(&self, mut frame: Frame) {
        match self.payload {
            TransportPacket::UDP(ref udp_packet) => {
                udp_packet.write_to_frame(frame);
            },
        }
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
