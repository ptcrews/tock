//! ICMP layer of the Tock networking stack.
//!
//! - Author: Conor McAvity <cmcavity@stanford.edu>

use net::ip_utils::IPAddr;
use kernel::ReturnCode;
use net::stream::{decode_u16, decode_u8, decode_bytes};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;

#[derive(Copy, Clone)]
pub struct ICMPHeader {
    pub code: u8,
    pub cksum: u16,
    pub options: ICMPHeaderOptions,
}

pub enum ICMPHeaderOptions {
    Type0 { id: u16, seqno: u16 },
    Type3 { _unused: u16, next_mtu: u16 },
}

impl ICMPHeader {
    pub fn new(hdr_type: u8) -> ICMPHeader {
        let options = match hdr_type {
            0 => Type0 { 0, 0 },
            3 => Type3 { 0, 0 },
        };
        
        ICMPHeader {
            code: 0,
            cksum: 0,
            options: options,
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
        match options {
            ICMPHeaderOptions::Type0 { id, seqno } => 0,
            ICMPHeaderOptions::Type3 { _unused, next_mtu } => 3,
        };
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
        off = enc_consume!(buf, off; encode_u8, self.type_num);
        off = enc_consume!(buf, off; encode_u8, self.code);
        off = enc_consume!(buf, off; encode_u16, self.cksum);
        off = enc_consume!(buf, off; encode_u32, self.options);
        stream_done!(off, off);
    }

    pub fn decode(buf: &[u8]) -> SResult<ICMPHeader> {
        // TODO: finish
        let mut icmp_header = Self::new();
        let off = 0;
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
}

impl<'a> IP6Client for ICMPSendStruct<'a> {
    fn send_done(&self, result: ReturnCode) {
        self.client.get().map(|client| client.send_done(result));
    }
}
