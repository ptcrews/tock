use core::cell::Cell;
use kernel::common::take_cell::TakeCell;
use net::util;

#[derive(Copy,Clone)]
pub enum MacAddr {
    ShortAddr(u16),
    LongAddr([u8; 8]),
}

#[allow(unused_variables)]
pub mod ip6_nh {
    pub const HOP_OPTS: u8 = 0;
    pub const TCP: u8      = 6;
    pub const UDP: u8      = 17;
    pub const IP6: u8      = 41;
    pub const ROUTING: u8  = 43;
    pub const FRAGMENT: u8 = 44;
    pub const ICMP: u8     = 58;
    pub const NO_NEXT: u8  = 59;
    pub const DST_OPTS: u8 = 60;
    pub const MOBILITY: u8 = 135;
}

#[derive(Copy, Clone, Debug)]
pub struct IPAddr(pub [u8; 16]);

impl IPAddr {
    pub fn new() -> IPAddr {
        // Defaults to the unspecified address
        IPAddr([0; 16])
    }

    pub fn is_unspecified(&self) -> bool {
        util::is_zero(&self.0)
    }

    pub fn is_unicast_link_local(&self) -> bool {
        self.0[0] == 0xfe
        && (self.0[1] & 0xc0) == 0x80
        && (self.0[1] & 0x3f) == 0
        && util::is_zero(&self.0[2..8])
    }

    pub fn set_unicast_link_local(&mut self) {
        self.0[0] = 0xfe;
        self.0[1] = 0x80;
        for i in 2..8 {
            self.0[i] = 0;
        }
    }

    // Panics if prefix slice does not contain enough bits
    pub fn set_prefix(&mut self, prefix: &[u8], prefix_len: u8) {
        let full_bytes = (prefix_len / 8) as usize;
        let remaining = (prefix_len & 0x7) as usize;
        let bytes = full_bytes + (if remaining != 0 { 1 } else { 0 });
        assert!(bytes <= prefix.len() && bytes <= 16);

        self.0[0..full_bytes].copy_from_slice(&prefix[0..full_bytes]);
        if remaining != 0 {
            let mask = (0xff as u8) << (8 - remaining);
            self.0[full_bytes] &= !mask;
            self.0[full_bytes] |= mask & prefix[full_bytes];
        }
    }

    pub fn is_multicast(&self) -> bool {
        self.0[0] == 0xff
    }
}

#[allow(unused_variables,dead_code)]
pub fn reverse_u16_bytes(short: u16) -> u16 {
    ((short & 0x00ff) << 8) | (short >> 8)
}

#[allow(unused_variables,dead_code)]
pub fn reverse_u32_bytes(long: u32) -> u32 {
    ((long & 0x000000ff) << 24) |
    ((long & 0x0000ff00) << 8) |
    ((long & 0x00ff0000) >> 8) |
    (long >> 24)
}

pub fn slice_to_u16(buf: &[u8]) -> u16 {
    ((buf[0] as u16) << 8) | (buf[1] as u16)
}

pub fn u16_to_slice(short: u16, slice: &mut [u8]) {
    slice[0] = (short >> 8) as u8;
    slice[1] = (short & 0xff) as u8;
}

#[allow(unused_variables,dead_code)]
pub fn ntohs(net_short: u16) -> u16 {
    return reverse_u16_bytes(net_short);
}

#[allow(unused_variables,dead_code)]
pub fn ntohl(net_long: u32) -> u32 {
    return reverse_u32_bytes(net_long);
}

#[allow(unused_variables,dead_code)]
pub fn htons(host_short: u16) -> u16 {
    return reverse_u16_bytes(host_short);
}

#[allow(unused_variables,dead_code)]
pub fn htonl(host_long: u32) -> u32 {
    return reverse_u32_bytes(host_long);
}

#[repr(C, packed)]
#[allow(unused_variables)]
#[derive(Copy, Clone)]
pub struct IP6Header {
    pub version_class_flow: [u8; 4],
    pub payload_len: u16,
    pub next_header: u8,
    pub hop_limit: u8,
    pub src_addr: IPAddr,
    pub dst_addr: IPAddr,
}

impl Default for IP6Header {
    fn default() -> IP6Header {
        let version = 0x60;
        let hop_limit = 255;
        IP6Header {
            version_class_flow: [version, 0, 0, 0],
            payload_len: 0,
            next_header: ip6_nh::NO_NEXT,
            hop_limit: hop_limit,
            src_addr: IPAddr::new(),
            dst_addr: IPAddr::new()
        }
    }
}

impl IP6Header {
    pub fn new() -> IP6Header {
        IP6Header::default()
    }
    // Version should always be 6
    pub fn get_version(&self) -> u8 {
        (self.version_class_flow[0] & 0xf0) >> 4
    }

    // TODO: Confirm order
    pub fn get_traffic_class(&self) -> u8 {
        (self.version_class_flow[0] & 0x0f) << 4 |
        (self.version_class_flow[1] & 0xf0) >> 4
    }

    pub fn set_traffic_class(&mut self, new_tc: u8) {
        self.version_class_flow[0] &= 0xf0;
        self.version_class_flow[0] |= (new_tc & 0xf0) >> 4;
        self.version_class_flow[1] &= 0x0f;
        self.version_class_flow[1] |= (new_tc & 0x0f) << 4;
    }

    fn get_dscp_unshifted(&self) -> u8 {
        self.get_traffic_class() & 0b11111100
    }

    pub fn get_dscp(&self) -> u8 {
        self.get_dscp_unshifted() >> 2
    }

    pub fn set_dscp(&mut self, new_dscp: u8) {
        let ecn = self.get_ecn();
        self.set_traffic_class(ecn | ((new_dscp << 2) & 0b11111100));
    }

    pub fn get_ecn(&self) -> u8 {
        self.get_traffic_class() & 0b11
    }

    pub fn set_ecn(&mut self, new_ecn: u8) {
        let dscp_unshifted = self.get_dscp_unshifted();
        self.set_traffic_class(dscp_unshifted | (new_ecn & 0b11));
    }

    // This returns the flow label as the lower 20 bits of a u32
    pub fn get_flow_label(&self) -> u32 {
        let mut flow_label: u32 = 0;
        flow_label |= ((self.version_class_flow[1] & 0x0f) as u32) << 16;
        flow_label |= (self.version_class_flow[2] as u32) << 8;
        flow_label |= self.version_class_flow[3] as u32;
        flow_label
    }

    pub fn set_flow_label(&mut self, new_fl_val: u32) {
        self.version_class_flow[1] &= 0xf0;
        self.version_class_flow[1] |= ((new_fl_val >> 16) & 0x0f) as u8;
        self.version_class_flow[2] = (new_fl_val >> 8) as u8;
        self.version_class_flow[3] = new_fl_val as u8;
    }

    // TODO: Is this in network byte order?
    pub fn get_payload_len(&self) -> u16 {
        ntohs(self.payload_len)
    }

    // TODO: 40 = size of IP6header - find idiomatic way to compute
    pub fn get_total_len(&self) -> u16 {
        (40 + ntohs(self.payload_len))
    }

    // TODO: Is this in network byte order?
    pub fn set_payload_len(&mut self, new_len: u16) {
        self.payload_len = htons(new_len);
    }

    pub fn get_next_header(&self) -> u8 {
        self.next_header
    }

    pub fn set_next_header(&mut self, new_nh: u8) {
        self.next_header = new_nh;
    }

    pub fn get_hop_limit(&self) -> u8 {
        self.hop_limit
    }

    pub fn set_hop_limit(&mut self, new_hl: u8) {
        self.hop_limit = new_hl;
    }
}

#[allow(unused_variables,dead_code)]
pub struct IP6ExtHeader {
    next_header: u8,
    header_len_or_reserved: u8,
    options: [u8; 2],
    options_1: [u8; 4],
    additional_options: TakeCell<'static, [u8]>,
}

// TODO: Make this more full-featured
impl IP6ExtHeader {
    pub fn new(next_header: u8) -> IP6ExtHeader {
        IP6ExtHeader {
            next_header: next_header,
            header_len_or_reserved: 0,
            options: [0, 0],
            options_1: [0, 0, 0, 0],
            additional_options: TakeCell::empty(),
        }
    }
}

#[allow(unused_variables,dead_code)]
pub struct IP6Packet {
    header: Cell<IP6Header>,
    hop_opts: Cell<Option<IP6ExtHeader>>,
    dest_opts: Cell<Option<IP6ExtHeader>>,
    routing: Cell<Option<IP6ExtHeader>>,
    fragment: Cell<Option<IP6ExtHeader>>,
    auth: Cell<Option<IP6ExtHeader>>,
    esp: Cell<Option<IP6ExtHeader>>,
    dest_opts_second: Cell<Option<IP6ExtHeader>>,
    mobility: Cell<Option<IP6ExtHeader>>,
    payload: TakeCell<'static, [u8]>,
}


impl IP6Packet {
    pub fn new() -> IP6Packet {
        IP6Packet {
            header: Cell::new(IP6Header::default()),
            hop_opts: Cell::new(None),
            dest_opts: Cell::new(None),
            routing: Cell::new(None),
            fragment: Cell::new(None),
            auth: Cell::new(None),
            esp: Cell::new(None),
            dest_opts_second: Cell::new(None),
            mobility: Cell::new(None),
            payload: TakeCell::empty(),
        }
    }

    // Returns number of bytes written to buf
    // TODO: We currently do not support Jumbograms
    pub fn prepare_packet(&self, buf: &mut [u8]) -> usize {
        /*
        let mut offset = 0;
        offset
        buf[0]
        */
        0
    }
}
