//! ICMP layer of the Tock networking stack.
//!
//! - Author: Conor McAvity <cmcavity@stanford.edu>

use net::ip_utils::IPAddr;
use kernel::ReturnCode;
use net::stream::{decode_u16, decode_u8, decode_bytes};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;

pub struct ICMPHeader {
    pub code: u8,
    pub cksum: u16,
    pub options: ICMPHeaderOptions,
}

pub enum ICMPHeaderOptions {
    Type0 { id: u16, seqno: u16 },
    Type3 { _unused: u16, next_mtu: u16 },
}

impl Default for ICMPHeader {
    fn default() -> ICMPHeader {
        ICMPHeader {
            code: 0,
            cksum: 0,
            options: Type0 { 0, 0 },
        }
    }
}

impl ICMPHeader {
    pub fn new() -> ICMPHeader {
        ICMPHeader::default()
    }

    pub fn set_type(&mut self, hdr_type: u8) {
        match hdr_type {
            0 => self.options = Type0 { 0, 0 },
            3 => self.options = Type3 { 0, 0 }, 
        };
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

pub struct ICMPSendStruct<'a> {
    ip_send_struct: &'a IP6SendStruct<'a>,
    client: Cell<Option<&'a ICMPSendClient>>,
}
