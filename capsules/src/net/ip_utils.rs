use net::stream::{decode_u16, decode_u8, decode_bytes};
use net::stream::{encode_u16, encode_u8, encode_bytes};
use net::stream::SResult;
use net::udp::udp::{UDPHeader};
use net::util::{slice_to_u16};

#[derive(Copy,Clone,PartialEq)]
pub enum MacAddr {
    ShortAddr(u16),
    LongAddr([u8; 8]),
}

pub mod ip6_nh {
    pub const HOP_OPTS: u8 = 0;
    pub const TCP: u8 = 6;
    pub const UDP: u8 = 17;
    pub const IP6: u8 = 41;
    pub const ROUTING: u8 = 43;
    pub const FRAGMENT: u8 = 44;
    pub const ICMP: u8 = 58;
    pub const NO_NEXT: u8 = 59;
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
        self.0.iter().all(|&b| b == 0)
    }

    pub fn is_unicast_link_local(&self) -> bool {
        self.0[0] == 0xfe && (self.0[1] & 0xc0) == 0x80 && (self.0[1] & 0x3f) == 0 &&
        self.0[2..8].iter().all(|&b| b == 0)
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

#[repr(C, packed)]
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
            dst_addr: IPAddr::new(),
        }
    }
}

impl IP6Header {
    pub fn new() -> IP6Header {
        IP6Header::default()
    }

    pub fn decode(buf: &[u8]) -> SResult<IP6Header> {
        // TODO: Let size of header be a constant
        stream_len_cond!(buf, 40);

        let mut ip6_header = Self::new();
        // Note that `dec_consume!` uses the length of the output buffer to
        // determine how many bytes are to be read.
        let off = dec_consume!(buf, 0; decode_bytes, &mut ip6_header.version_class_flow);
        let (off, payload_len_be) = dec_try!(buf, off; decode_u16);
        ip6_header.payload_len = u16::from_be(payload_len_be);
        let (off, next_header) = dec_try!(buf, off; decode_u8);
        ip6_header.next_header = next_header;
        let (off, hop_limit) = dec_try!(buf, off; decode_u8);
        ip6_header.hop_limit = hop_limit;
        let off = dec_consume!(buf, off; decode_bytes, &mut ip6_header.src_addr.0);
        let off = dec_consume!(buf, off; decode_bytes, &mut ip6_header.dst_addr.0);
        stream_done!(off, ip6_header);
    }

    // Returns the offset wrapped in an SResult
    pub fn encode(&self, buf: &mut [u8]) -> SResult<usize> {
        stream_len_cond!(buf, 40);

        let mut off = enc_consume!(buf, 0; encode_bytes, &self.version_class_flow);
        off = enc_consume!(buf, off; encode_u16, self.payload_len.to_be());
        off = enc_consume!(buf, off; encode_u8, self.next_header);
        off = enc_consume!(buf, off; encode_u8, self.hop_limit);
        off = enc_consume!(buf, off; encode_bytes, &self.src_addr.0);
        off = enc_consume!(buf, off; encode_bytes, &self.dst_addr.0);
        stream_done!(off, off);
    }

    // Version should always be 6
    pub fn get_version(&self) -> u8 {
        (self.version_class_flow[0] & 0xf0) >> 4
    }

    // TODO: Confirm order
    pub fn get_traffic_class(&self) -> u8 {
        (self.version_class_flow[0] & 0x0f) << 4 | (self.version_class_flow[1] & 0xf0) >> 4
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

    pub fn get_payload_len(&self) -> u16 {
        u16::from_be(self.payload_len)
    }

    // TODO: 40 = size of IP6header - find idiomatic way to compute
    pub fn get_total_len(&self) -> u16 {
        40 + self.get_payload_len()
    }

    pub fn set_payload_len(&mut self, new_len: u16) {
        self.payload_len = new_len.to_be();
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


pub fn compute_udp_checksum(ip6_header: &IP6Header,
                            udp_header: &UDPHeader,
                            udp_length: u16,
                            payload: &[u8])
                            -> u16 {

    //This checksum is calculated according to some of the recommendations found in RFC 1071.

    let src_port = udp_header.src_port;
    let dst_port = udp_header.dst_port;
    let mut sum: u32 = 0;
    {
        //First, iterate through src/dst address and add them to the sum
        let mut i = 0;
        while i <= 14 { 
            let msb_src: u16 = ((ip6_header.src_addr.0[i]) as u16) << 8;
            let lsb_src: u16 = ip6_header.src_addr.0[i+1] as u16;
            let temp_src: u16 = msb_src + lsb_src;
            sum += temp_src as u32;

            let msb_dst: u16 = ((ip6_header.dst_addr.0[i]) as u16) << 8;
            let lsb_dst: u16 = ip6_header.dst_addr.0[i+1] as u16;
            let temp_dst: u16 = msb_dst + lsb_dst;
            sum += temp_dst as u32;

            i += 2; //Iterate two bytes at a time bc 16 bit checksum
        }

    }
    sum += udp_header.len as u32;
    //Finally, add UDP next header
    sum += 17; //was "padded next header"

    //return sum as u16;
    //Next, add the UDP header elements to the sum
    sum += src_port as u32;
    sum += dst_port as u32;
    sum += udp_header.len as u32; 
    //Now just need to iterate thru data and add it to the sum
    {
        let mut i: usize = 0;
        while i < ((udp_length - 8) as usize) {
            let msb_dat: u16 = ((payload[i]) as u16) << 8;
            let lsb_dat: u16 = payload[i + 1] as u16;
            let temp_dat: u16 = msb_dat + lsb_dat;
            sum += temp_dat as u32;

            i += 2; //Iterate two bytes at a time bc 16 bit checksum
        }
        //debug!("Checksum is currently: {:?}", sum);
    }
    //now all 16 bit addition has occurred

    while sum > 65535 {
        let sum_high: u32 = sum >> 16; //upper 16 bits of sum
        let sum_low: u32 = sum & 65535; //lower 16 bits of sum
        sum = sum_high + sum_low;
    }

    //Finally, flip all bits
    sum = !sum;
    sum = sum & 65535; //Remove upper 16 bits (which should be FFFF after flip)
    (sum as u16) //Return result as u16 in host byte order

}

