/* This file contains structs, traits, and methods associated with the IP layer
   of the networking stack. For a full description of the networking stack on
   tock, see the Thread_Stack_Design.txt document */

use net::ip_utils::{IPAddr, IP6Header, compute_udp_checksum, ip6_nh};
use ieee802154::mac::{Frame, Mac};
use net::ieee802154::MacAddress;
use net::udp::udp::{UDPHeader};
use net::tcp::{TCPHeader};
use net::icmp::ICMPHeader;
use net::sixlowpan::{TxState, SixlowpanTxClient};
use kernel::ReturnCode;
use kernel::common::take_cell::TakeCell;
use core::cell::Cell;

use net::stream::{decode_u16, decode_u8, decode_bytes};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;

// TODO: Note that this design decision means that we cannot have recursive
// IP6 packets directly - we must have/use RawIPPackets instead. This makes
// it difficult to recursively compress IP6 packets as required by 6lowpan
pub enum TransportHeader {
    UDP(UDPHeader),
    TCP(TCPHeader),
    ICMP(ICMPHeader),
    
    // NOTE: TCP,ICMP,RawIP traits not yet implemented
    // , but follow logically from UDPPacket. 
    
    // TODO: Need a length in RawIPPacket for the buffer in TransportHeader
    /* Raw(RawIPPacket<'a>), */
}

pub struct IPPayload<'a> {
    pub header: TransportHeader,
    pub payload: &'a mut [u8],
}

impl<'a> IPPayload<'a> {
    pub fn new(header: TransportHeader, payload: &'a mut [u8]) -> IPPayload<'a> {
        IPPayload {
            header: header,
            payload: payload,
        }
    }

    pub fn set_payload(&mut self, transport_header: TransportHeader, payload: &[u8])
            -> (u8, u16) {
        if self.payload.len() < payload.len() {
            // TODO: Error
        }
        self.payload.copy_from_slice(&payload);
        match transport_header {
            TransportHeader::UDP(mut udp_header) => {
                debug!("I am a UDP Packet");
                let length = (payload.len() + udp_header.get_hdr_size()) as u16;
                udp_header.set_len(length);
                (ip6_nh::UDP, length)
            },
            _ => {
                debug!("I am a failure!");
                (ip6_nh::NO_NEXT, payload.len() as u16)
            },
        }
    }

    pub fn encode(&self, buf: &mut [u8], offset: usize) -> SResult<usize> {
        let (offset, _) = match self.header {
            TransportHeader::UDP(udp_header) => {
                udp_header.encode(buf, offset).done().unwrap()
            },
            _ => {
                unimplemented!();
                stream_done!(offset, offset);
            },
        };
        let payload_length = self.get_payload_length();
        let offset = enc_consume!(buf, offset; encode_bytes, &self.payload[..payload_length]);
        stream_done!(offset, offset);
    }

    fn get_payload_length(&self) -> usize {
        match self.header {
            TransportHeader::UDP(udp_header) => {
                udp_header.get_len() as usize - udp_header.get_hdr_size()
            },
            _ => {
                unimplemented!();
            },
        }
    }
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

    pub fn new(pyld: IPPayload<'a>) -> IP6Packet<'a>{
        IP6Packet {
            header: IP6Header::default(),
            payload: pyld,
        }
    }

    pub fn reset(&mut self) {
        self.header = IP6Header::default();
    }

    pub fn get_total_len(&self) -> u16 {
        40 + self.header.get_payload_len()
    }

    pub fn get_payload(&self) -> &[u8] {
        self.payload.payload
    }

    pub fn get_total_hdr_size(&self) -> usize {
        let transport_hdr_size = match self.payload.header {
            TransportHeader::UDP(udp_hdr) => udp_hdr.get_hdr_size(),
            _ => 0, 
        };
        40 + transport_hdr_size
    }

    pub fn set_transport_checksum(&mut self){ //Looks at internal buffer assuming
    // it contains a valid IP packet, checks the payload type. If the payload
    // type requires a cksum calculation, this function calculates the 
    // psuedoheader cksum and calls the appropriate transport packet function
    // using this pseudoheader cksum to set the transport packet cksum

        match self.payload.header {
            TransportHeader::UDP(ref mut udp_header) => {

                let cksum = compute_udp_checksum(&self.header, &udp_header, udp_header.get_len(),
                self.payload.payload);

                udp_header.set_cksum(cksum);

            },
            _ => {
                unimplemented!();
                debug!("Transport cksum setting not supported for this transport payload");
            }
        }
    }

    pub fn set_payload(&mut self, transport_header: TransportHeader, payload: &[u8]) {
        let (next_header, payload_len) = self.payload.set_payload(transport_header, payload);
        self.header.set_next_header(next_header);
        self.header.set_payload_len(payload_len);
    }

    // TODO: Implement
    pub fn decode(buf: &[u8], ip6_packet: &mut IP6Packet) -> Result<usize, ()> {
        let (offset, header) = IP6Header::decode(buf).done().ok_or(())?;
        ip6_packet.header = header;
        // TODO: When deserializing, its not clear to me how to construct
        // the inner packet. Easiset would be to probably assume the 
        // TODO: Not sure how to convert an IP6Packet with a UDP payload to 
        // an IP6Packet with a TCP payload.
        unimplemented!();
        Ok(offset)
    }

    pub fn encode(&self, buf: &mut [u8]) -> SResult<usize> {
        let ip6_header = self.header;

        // TODO: Confirm this works (that stream_done! doesn't break stuff)
        // Also, handle unwrap safely
        let (off, _) = ip6_header.encode(buf).done().unwrap();
        self.payload.encode(buf, off)
    }
}
