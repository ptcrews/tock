/* This file contains structs, traits, and methods associated with the IP layer
   of the networking stack. For a full description of the networking stack on
   tock, see the Thread_Stack_Design.txt document */

use net::ip_utils::{IPAddr, IP6Header};
use ieee802154::mac::Frame;
use net::udp::{UDPPacket};
use net::tcp::{TCPPacket};
use kernel::ReturnCode;

use net::stream::{decode_u16, decode_u8, decode_bytes};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;

// TODO: Note that this design decision means that we cannot have recursive
// IP6 packets directly - we must have/use RawIPPackets instead. This makes
// it difficult to recursively compress IP6 packets as required by 6lowpan
pub enum TransportHeader {
    UDP(UDPPacket),
    TCP(TCPPacket), // NOTE: TCP,ICMP,RawIP traits not yet implemented
                        // , but follow logically from UDPPacket. 
/*
    ICMP(ICMPPacket<'a>),
    Raw(RawIPPacket<'a>), */
}

pub struct IPPayload<'a> {
    pub payload_header: TransportPacket,
    pub payload: &'a mut [u8],
}

pub struct IP6Packet<'a> {
    pub header: IP6Header,
    pub payload: IPPayload<'a>,
}

// Note: We want to have the IP6Header struct implement these methods,
// as there are cases where we want to allocate/modify the IP6Header without
// allocating/modifying the entire IP6Packet
impl<'a> IP6Packet<'a> {
    // Sets fields to appropriate defaults
    pub fn reset(&mut self) {
        self.header = IP6Header::default();
    }

    pub fn get_total_len(&self) -> u16 {
        40 + self.header.get_payload_len()
    }

    pub fn get_payload(&self) -> &[u8] {
        unimplemented!();
        /*
        match self.payload {
            TransportPacket::UDP(ref udp_packet) => {
                return udp_packet.payload
            },
            TransportPacket::TCP(ref tcp_packet) => {
                return tcp_packet.payload
            },
        }
        */
    }

    pub fn get_total_hdr_size(&self) -> usize {
        /*
        let transport_hdr_size = match self.payload {
            TransportPacket::UDP(ref udp_packet) => udp_packet.get_hdr_size(),
            TransportPacket::TCP(ref tcp_packet) => 0, //tcp_packet.get_hdr_size(),
        };
        */
        //40 + transport_hdr_size
        40
    }

    pub fn set_transpo_cksum(&self){} //Looks at internal buffer assuming
    // it contains a valid IP packet, checks the payload type. If the payload
    // type requires a cksum calculation, this function calculates the 
    // psuedoheader cksum and calls the appropriate transport packet function
    // using this pseudoheader cksum to set the transport packet cksum

    pub fn change_transport_type(&mut self) {
    }

    // TODO: Implement
    pub fn decode(buf: &[u8], ip6_packet: &mut IP6Packet) -> Result<usize, ()> {
        let (offset, header) = IP6Header::decode(buf).done().ok_or(())?;
        ip6_packet.header = header;
        // TODO: When deserializing, its not clear to me how to construct
        // the inner packet. Easiset would be to probably assume the 
        // TODO: Not sure how to convert an IP6Packet with a UDP payload to 
        // an IP6Packet with a TCP payload.
        Ok(offset)
    }

    pub fn encode(&self, buf: &mut [u8]) -> SResult<usize> {
        let ip6_header = self.header;

        // TODO: Confirm this works (that stream_done! doesn't break stuff)
        // Also, handle unwrap safely
        let (off, _) = ip6_header.encode(buf).done().unwrap();
        unimplemented!();

        /*
        match self.payload {
            TransportPacket::UDP(ref udp_packet) => {
                udp_packet.encode(buf, off)
            },
            // TODO
            _ => {
                stream_done!(off, off);
            },
        }
        */
    }

}

pub trait IP6Send {
    fn send_to(&self, dest: IPAddr, ip6_packet: IP6Packet); //Convenience fn, sets dest addr, sends
    fn send(&self, ip6_packet: IP6Packet); //Length can be determined from IP6Packet
    fn send_done(&self, ip6_packet: IP6Packet, result: ReturnCode);
}
