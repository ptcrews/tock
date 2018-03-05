//! ICMP layer of the Tock networking stack.
//!
//! - Author: Conor McAvity <cmcavity@stanford.edu>

use core::cell::Cell;
use net::ip_utils::IPAddr;
use net::ip::TransportHeader;
use net::ipv6::ipv6_send::{IP6SendStruct, IP6Client};
use net::stream::{decode_u16, decode_u8};
use net::stream::{encode_u16, encode_u8};
use net::stream::SResult;
use kernel::ReturnCode;

#[derive(Copy, Clone)]
pub struct ICMPHeader {
    pub code: u8,
    pub cksum: u16,
    pub options: ICMPHeaderOptions,
}

#[derive(Copy, Clone)]
pub enum ICMPHeaderOptions {
    Type0 { id: u16, seqno: u16 },
    Type3 { unused: u16, next_mtu: u16 },
}

impl ICMPHeader {
    pub fn new(hdr_type: u8) -> ICMPHeader {
        let options = match hdr_type {
            0 => ICMPHeaderOptions::Type0 { id: 0, seqno: 0 },
            3 => ICMPHeaderOptions::Type3 { unused: 0, next_mtu: 0 },
            _ => ICMPHeaderOptions::Type0 { id: 0, seqno: 0 },
        };
        
        ICMPHeader {
            code: 0,
            cksum: 0,
            options: options,
        }
    }

    pub fn set_type(&mut self, hdr_type: u8) {
        match hdr_type {
            0 => self.set_options(ICMPHeaderOptions::Type0 { id: 0, seqno: 0 }),
            3 => self.set_options(ICMPHeaderOptions::Type3 { unused: 0, next_mtu: 0 }),
            _ => (),
        }
    }

    pub fn set_code(&mut self, code: u8) {
        self.code = code;
    }

    pub fn set_cksum(&mut self, cksum: u16) {
        self.cksum = cksum;
    }

    pub fn set_options(&mut self, options: ICMPHeaderOptions) {
        self.options = options;
    }

    pub fn get_type(&self) -> u8 {
        match self.options {
            ICMPHeaderOptions::Type0 { id, seqno } => 0,
            ICMPHeaderOptions::Type3 { unused, next_mtu } => 3,
        }
    }

    pub fn get_code(&self) -> u8 {
        self.code
    }

    pub fn get_cksum(&self) -> u16 {
        self.cksum
    }

    pub fn get_options(&self) -> ICMPHeaderOptions {
        self.options
    }

    pub fn encode(&self, buf: &mut [u8], offset: usize) -> SResult<usize> {
        let mut off = offset;  

        off = enc_consume!(buf, off; encode_u8, self.get_type());
        off = enc_consume!(buf, off; encode_u8, self.code);
        off = enc_consume!(buf, off; encode_u16, self.cksum);

        match self.options {
             ICMPHeaderOptions::Type0 { id, seqno } => {
                off = enc_consume!(buf, off; encode_u16, id);
                off = enc_consume!(buf, off; encode_u16, seqno);
             },
             ICMPHeaderOptions::Type3 { unused, next_mtu } => {
                off = enc_consume!(buf, off; encode_u16, unused);
                off = enc_consume!(buf, off; encode_u16, next_mtu);
             }
        }
        
        stream_done!(off, off);
    }

    pub fn decode(buf: &[u8]) -> SResult<ICMPHeader> {
        let off = 0;
        
        let (off, hdr_type) = dec_try!(buf, off; decode_u8);
        
        let mut icmp_header = Self::new(hdr_type);
        
        let (off, code) = dec_try!(buf, off; decode_u8);
        icmp_header.code = code; 
        
        let (off, cksum) = dec_try!(buf, off; decode_u16);
        icmp_header.cksum = u16::from_be(cksum);
       
        match hdr_type {
            0 => {
                let (off, id) = dec_try!(buf, off; decode_u16);
                let id = u16::from_be(id);
                    
                let (off, seqno) = dec_try!(buf, off; decode_u16);
                let seqno = u16::from_be(seqno);

                icmp_header.set_options(ICMPHeaderOptions::Type0 { id, seqno });
            },
            3 => {
                let (off, unused) = dec_try!(buf, off; decode_u16);
                let unused = u16::from_be(unused);
                    
                let (off, next_mtu) = dec_try!(buf, off; decode_u16);
                let next_mtu = u16::from_be(next_mtu);

                icmp_header.set_options(ICMPHeaderOptions::Type3 { unused: unused, next_mtu: next_mtu });
            },
            _ => (),
        }

        stream_done!(off, icmp_header);
    }
}

pub trait ICMPSendClient {
    fn send_done(&self, result: ReturnCode);
}

pub struct ICMPSendStruct<'a> {
    ip_send_struct: &'a IP6SendStruct<'a>,
    client: Cell<Option<&'a ICMPSendClient>>,
}

impl<'a> ICMPSendStruct<'a> {
    pub fn new(ip_send_struct: &'a IP6SendStruct<'a>) -> ICMPSendStruct<'a> {
        ICMPSendStruct {
            ip_send_struct: ip_send_struct,
            client: Cell::new(None),
        }
    }
    
    pub fn set_client(&self, client: &'a ICMPSendClient) {
        self.client.set(Some(client));
    }

    pub fn send(&self, dest: IPAddr, icmp_header: ICMPHeader, buf: &'a [u8]) -> ReturnCode {
        let transport_header = TransportHeader::ICMP(icmp_header);
        self.ip_send_struct.send_to(dest, transport_header, buf)
    }
}

impl<'a> IP6Client for ICMPSendStruct<'a> {
    fn send_done(&self, result: ReturnCode) {
        self.client.get().map(|client| client.send_done(result));
    }
}
