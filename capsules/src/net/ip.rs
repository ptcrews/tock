/* This file contains structs, traits, and methods associated with the IP layer
   of the networking stack. For a full description of the networking stack on
   tock, see the Thread_Stack_Design.txt document */

use net::ip_utils::{IPAddr, IP6Header, compute_udp_checksum};
use ieee802154::mac::{Frame, Mac};
use net::ieee802154::MacAddress;
use net::udp::{UDPHeader};
use net::tcp::{TCPHeader};
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
    TCP(TCPHeader), // NOTE: TCP,ICMP,RawIP traits not yet implemented
                        // , but follow logically from UDPPacket. 
/*
    ICMP(ICMPPacket<'a>),
    // TODO: Need a length in RawIPPacket for the buffer in TransportHeader
    Raw(RawIPPacket<'a>), */
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

    pub fn change_type(&mut self, new_type: TransportHeader) {
        self.header = new_type;
    }

    pub fn change_payload(&mut self, new_payload: &[u8]) -> ReturnCode {
        match self.header {
            TransportHeader::UDP(udp_header) => {
                udp_header.set_payload(new_payload, self);
            },
            _ => {
                unimplemented!();
            },
        }
        ReturnCode::SUCCESS
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
    pub fn reset(&mut self) {
        self.header = IP6Header::default();
    }

    pub fn get_total_len(&self) -> u16 {
        40 + self.header.get_payload_len()
    }

    pub fn get_payload(&self) -> &[u8] {
        self.payload.payload
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
        
        let transport_hdr_size = match self.payload.header {
            TransportHeader::UDP(udp_hdr) => udp_hdr.get_hdr_size(),
            _ => 0, 
        };
        
        40 + transport_hdr_size
        
    }

    pub fn set_transpo_cksum(&mut self){ //Looks at internal buffer assuming
    // it contains a valid IP packet, checks the payload type. If the payload
    // type requires a cksum calculation, this function calculates the 
    // psuedoheader cksum and calls the appropriate transport packet function
    // using this pseudoheader cksum to set the transport packet cksum
        
        match self.payload.header {
            TransportHeader::UDP(ref mut udp_header) => {

            let cksum = compute_udp_checksum(&self.header, &udp_header, udp_header.get_len(), self.payload.payload);

            udp_header.set_cksum(cksum);


            },
            _ => {
                debug!("Transport cksum setting not supported for this transport payload");
            }
        }
        
    }

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

pub trait IP6Send {
    fn send_to(&self, dest: IPAddr, ip6_packet: IP6Packet); //Convenience fn, sets dest addr, sends
    fn send(&self, ip6_packet: IP6Packet); //Length can be determined from IP6Packet
    fn send_done(&self, ip6_packet: IP6Packet, result: ReturnCode);
}

pub trait IP6Client {
    fn send_done(&self, ip6_packet: &'static mut IP6Packet, result: ReturnCode);
}

// TODO: Make not constants
const SRC_MAC_ADDR: MacAddress = MacAddress::Short(0xf00f);
const DST_MAC_ADDR: MacAddress = MacAddress::Short(0xf00f);

pub struct IP6SendStruct<'a> {
    ip6_packet: TakeCell<'static, IP6Packet<'static>>,
    src_addr: Cell<IPAddr>,
    tx_buf: TakeCell<'static, [u8]>,
    sixlowpan: TxState<'a>,
    radio: &'a Mac<'a>,
    client: Cell<Option<&'a IP6Client>>,
}

impl<'a> IP6SendStruct<'a> {
    pub fn new(tx_buf: &'static mut [u8],
               sixlowpan: TxState<'a>,
               radio: &'a Mac<'a>,
               client: &'a IP6Client) -> IP6SendStruct<'a> {
        IP6SendStruct {
            ip6_packet: TakeCell::empty(),
            src_addr: Cell::new(IPAddr::new()),
            tx_buf: TakeCell::new(tx_buf),
            sixlowpan: sixlowpan,
            radio: radio,
            client: Cell::new(Some(client)),
        }
    }

    pub fn set_addr(&self, src_addr: IPAddr) {
        self.src_addr.set(src_addr);
    }

    pub fn set_next_header(&self) {
    }

    pub fn initialize_packet(&self) {
        self.ip6_packet.map(|ip6_packet| {
            ip6_packet.header = IP6Header::default();
            ip6_packet.header.src_addr = self.src_addr.get();
        });
    }

    pub fn set_header(&self, ip6_header: IP6Header) {
        self.ip6_packet.map(|ip6_packet| ip6_packet.header = ip6_header);
    }

    pub fn send_to(&self, dest: IPAddr, ip6_packet: &'static mut IP6Packet<'static>)
            -> Result<(), (ReturnCode, &'static mut IP6Packet<'static>)> {
        self.sixlowpan.init(SRC_MAC_ADDR, DST_MAC_ADDR, None);
        if self.ip6_packet.is_some() {
            return Err((ReturnCode::EBUSY, ip6_packet));
        }
        // This synchronously returns any errors in the first fragment
        let (result, completed) = self.send_next_fragment();
        if result != ReturnCode::SUCCESS {
            Err((result, self.ip6_packet.take().unwrap()))
        } else {
            if completed {
                self.send_completed(result);
            }
            Ok(())
        }
    }

    fn send_next_fragment(&self) -> (ReturnCode, bool) {
        // TODO: Fix unwrap
        let tx_buf = self.tx_buf.take().unwrap();
        let ip6_packet = self.ip6_packet.take().unwrap();
        let next_frame = self.sixlowpan.next_fragment(&ip6_packet, tx_buf, self.radio);
        self.ip6_packet.replace(ip6_packet);

        let result = match next_frame {
            Ok((is_done, frame)) => {
                if is_done {
                    self.tx_buf.replace(frame.into_buf());
                    self.send_completed(ReturnCode::SUCCESS);
                } else {
                    self.radio.transmit(frame);
                }
                (ReturnCode::SUCCESS, is_done)
            },
            Err((retcode, buf)) => {
                self.tx_buf.replace(buf);
                self.send_completed(ReturnCode::FAIL);
                (ReturnCode::FAIL, false)
            },
        };
        result
    }

    fn send_completed(&self, result: ReturnCode) {
        // TODO: Fix unwrap
        let ip6_packet = self.ip6_packet.take().unwrap();
        self.client.get().map(move |client| client.send_done(ip6_packet, result));
    }
}

impl<'a> SixlowpanTxClient for IP6SendStruct<'a> {
    fn send_done(&self, buf: &'static mut [u8], _acked: bool, result: ReturnCode) {
        self.tx_buf.replace(buf);
        if result != ReturnCode::SUCCESS {
            self.send_completed(result);
        }

        let (result, completed) = self.send_next_fragment();
        if completed || result != ReturnCode::SUCCESS {
            self.send_completed(result);
        }
    }
}
