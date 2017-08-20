use net::stream::{decode_u8, decode_u16, decode_bytes};
use net::stream::{encode_u8, encode_u16, encode_bytes};
use net::stream::SResult;

/// A small subset of the valid IPv6 Next Header field values.
#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum NextHeaderType {
    HopOpts = 0,
    TCP = 6,
    UDP = 17,
    IP = 41,
    Routing = 43,
    Fragment = 44,
    ICMPv6 = 58,
    NoNext = 59,
    DestOpts = 60,
    Mobility = 135,
}

impl NextHeaderType {
    pub fn from_nh(nh: u8) -> Option<NextHeaderType> {
        match nh {
            0 => Some(NextHeaderType::HopOpts),
            6 => Some(NextHeaderType::TCP),
            17 => Some(NextHeaderType::UDP),
            41 => Some(NextHeaderType::IP),
            43 => Some(NextHeaderType::Routing),
            44 => Some(NextHeaderType::Fragment),
            58 => Some(NextHeaderType::ICMPv6),
            59 => Some(NextHeaderType::NoNext),
            60 => Some(NextHeaderType::DestOpts),
            135 => Some(NextHeaderType::Mobility),
            _ => None,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Address(pub [u8; 16]);

impl Default for Address {
    /// The default IPv6 addresss is the unspecified address
    fn default() -> Address {
        Address([0; 16])
    }
}

impl Address {
    /// Tests if the IP address is the unspecified address (all zeroes)
    pub fn is_unspecified(&self) -> bool {
        self.0.iter().all(|&b| b == 0)
    }

    /// Tests if the IP address belongs to the unicast link-local prefix
    /// fe80:0000:0000:0000
    pub fn is_unicast_link_local(&self) -> bool {
        self.0[0] == 0xfe && (self.0[1] & 0xc0) == 0x80 && (self.0[1] & 0x3f) == 0 &&
        self.0[2..8].iter().all(|&b| b == 0)
    }

    /// Sets the first 32 bits of the IP address to the unicast link-local
    /// prefix fe80:0000:0000:0000
    pub fn set_unicast_link_local(&mut self) {
        self.0[0] = 0xfe;
        self.0[1] = 0x80;
        for i in 2..8 {
            self.0[i] = 0;
        }
    }

    /// Replaces the first `prefix_len` bits in the address with those from
    /// `prefix`. Returns `false` if the provided prefix is not valid
    /// (`prefix_len` is too long for an address or for `prefix` itself), or
    /// `true` if the operation succeeded.
    pub fn set_prefix(&mut self, prefix: &[u8], prefix_len: u8) -> bool {
        let full_bytes = (prefix_len / 8) as usize;
        let remaining = (prefix_len & 0x7) as usize;
        let bytes = full_bytes + (if remaining != 0 { 1 } else { 0 });
        if !(bytes == prefix.len() && bytes <= 16) {
            return false;
        }

        self.0[0..full_bytes].copy_from_slice(&prefix[0..full_bytes]);
        if remaining != 0 {
            let mask = (0xff as u8) << (8 - remaining);
            self.0[full_bytes] &= !mask;
            self.0[full_bytes] |= mask & prefix[full_bytes];
        }
        true
    }

    /// Tests if the IP address is within the multicast prefix ffXX::
    pub fn is_multicast(&self) -> bool {
        self.0[0] == 0xff
    }

    pub fn encode(&self, buf: &mut [u8]) -> SResult {
        let off = enc_consume!(buf; encode_bytes, &self.0);
        stream_done!(off);
    }

    pub fn decode(buf: &[u8]) -> SResult<Address> {
        let mut addr: Address = Default::default();
        let off = dec_consume!(buf; decode_bytes, &mut addr.0);
        stream_done!(off, addr);
    }
}

/// Convenient representation of an IPv6 header.
#[derive(Copy, Clone)]
pub struct Header {
    pub traffic_class: u8,
    /// Only the least significant 20 bits of the flow label are used.
    pub flow_label: u32,
    pub payload_len: u16,
    pub next_header: NextHeaderType,
    pub hop_limit: u8,
    pub src_addr: Address,
    pub dst_addr: Address,
}

impl Default for Header {
    fn default() -> Header {
        Header {
            traffic_class: 0,
            flow_label: 0,
            payload_len: 0,
            next_header: NextHeaderType::NoNext,
            hop_limit: 255,
            src_addr: Default::default(),
            dst_addr: Default::default(),
        }
    }
}

pub const HEADER_SIZE: usize = 40;
const IP_VERSION: u8 = 6;

impl Header {
    /// Gets the DSCP subfield from the traffic class field. Returns the DSCP as
    /// the lower 6 bits in a byte.
    pub fn get_dscp(&self) -> u8 {
        (self.traffic_class & 0b11111100) >> 2
    }

    /// Sets the DSCP subfield of the traffic class field. Uses only the lower 6
    /// bits of `new_dscp`.
    pub fn set_dscp(&mut self, new_dscp: u8) {
        self.traffic_class &= 0b11;
        self.traffic_class |= (new_dscp << 2) & 0b11111100;
    }

    /// Gets the ECN subfield of the traffic class field. Returns the ECN as the
    /// lower 2 bits in a byte.
    pub fn get_ecn(&self) -> u8 {
        self.traffic_class & 0b11
    }

    /// Sets the ECN subfield of the traffic class field. Uses only the lower 2
    /// bits of `new_ecn`.
    pub fn set_ecn(&mut self, new_ecn: u8) {
        self.traffic_class &= 0b11111100;
        self.traffic_class |= new_ecn & 0b11;
    }

    pub fn encode(&self, buf: &mut [u8]) -> SResult {
        // First 4 bytes contain the version, traffic class, and flow label.
        stream_len_cond!(buf, 4);
        buf[0] = ((IP_VERSION << 4) & 0xf0) | ((self.traffic_class >> 4) & 0x0f);
        buf[1] = ((self.traffic_class << 4) & 0xf0) | (((self.flow_label >> 16) & 0x0f) as u8);
        buf[2] = (self.flow_label >> 8) as u8;
        buf[3] = self.flow_label as u8;

        // Next 4 bytes contain the payload length, next header and hop limit.
        let off = enc_consume!(buf, 4; encode_u16, self.payload_len.to_be());
        let off = enc_consume!(buf, off; encode_u8, self.next_header as u8);
        let off = enc_consume!(buf, off; encode_u8, self.hop_limit);

        // Lastly, the two addresses.
        let off = enc_consume!(buf, off; self.src_addr; encode);
        let off = enc_consume!(buf, off; self.dst_addr; encode);
        stream_done!(off);
    }

    pub fn decode(buf: &[u8]) -> SResult<Header> {
        // First 4 bytes contain the version, traffic class, and flow label.
        stream_len_cond!(buf, 4);
        let version = (buf[0] & 0xf0) >> 4;
        stream_cond!(version == IP_VERSION);
        let traffic_class = ((buf[0] & 0x0f) << 4) | ((buf[1] & 0xf0) >> 4);
        let flow_label = (((buf[1] & 0x0f) as u32) << 16) | ((buf[2] as u32) << 8) |
                         (buf[3] as u32);

        // Next 4 bytes contain the payload length, next header and hop limit.
        let (off, payload_len_be) = dec_try!(buf, 4; decode_u16);
        let payload_len = u16::from_be(payload_len_be);
        let (off, nh) = dec_try!(buf, off; decode_u8);
        let next_header = stream_from_option!(NextHeaderType::from_nh(nh));
        let (off, hop_limit) = dec_try!(buf, off; decode_u8);

        // Lastly, the two addresses.
        let (off, src_addr) = dec_try!(buf, off; Address::decode);
        let (off, dst_addr) = dec_try!(buf, off; Address::decode);
        stream_done!(off,
                     Header {
                         traffic_class: traffic_class,
                         flow_label: flow_label,
                         payload_len: payload_len,
                         next_header: next_header,
                         hop_limit: hop_limit,
                         src_addr: src_addr,
                         dst_addr: dst_addr,
                     });
    }
}
