/// Implements the 6LoWPAN specification for sending IPv6 datagrams over
/// 802.15.4 packets efficiently, as detailed in RFC 6282.

use core::mem;
use core::result::Result;

use net::ip;
use net::ip::{IP6Header, MacAddr, IPAddr, ip6_nh};
use net::ip::{ntohs, htons, slice_to_u16, u16_to_slice};
use net::util;

/// Contains bit masks and constants related to the two-byte header of the
/// LoWPAN_IPHC encoding format.
mod iphc {
    pub const DISPATCH: [u8; 2]    = [0x60, 0x00];

    // First byte masks

    pub const TF_TRAFFIC_CLASS: u8 = 0x08;
    pub const TF_FLOW_LABEL: u8    = 0x10;

    pub const NH: u8               = 0x04;

    pub const HLIM_MASK: u8        = 0x03;
    pub const HLIM_INLINE: u8      = 0x00;
    pub const HLIM_1: u8           = 0x01;
    pub const HLIM_64: u8          = 0x02;
    pub const HLIM_255: u8         = 0x03;

    // Second byte masks

    pub const CID: u8              = 0x80;

    pub const SAC: u8              = 0x40;

    pub const SAM_MASK: u8         = 0x30;
    pub const SAM_INLINE: u8       = 0x00;
    pub const SAM_MODE1: u8        = 0x10;
    pub const SAM_MODE2: u8        = 0x20;
    pub const SAM_MODE3: u8        = 0x30;

    pub const MULTICAST: u8        = 0x08;

    pub const DAC: u8              = 0x04;
    pub const DAM_MASK: u8         = 0x03;
    pub const DAM_INLINE: u8       = 0x00;
    pub const DAM_MODE1: u8        = 0x01;
    pub const DAM_MODE2: u8        = 0x02;
    pub const DAM_MODE3: u8        = 0x03;

    // Address compression
    pub const MAC_BASE: [u8; 8]    = [0, 0, 0, 0xff, 0xfe, 0, 0, 0];
    pub const MAC_UL: u8           = 0x02;
}

/// Contains bit masks and constants related to LoWPAN_NHC encoding,
/// including some specific to UDP header encoding
mod nhc {
    pub const DISPATCH_NHC: u8           = 0xe0;
    pub const DISPATCH_UDP: u8           = 0xf0;
    pub const DISPATCH_MASK: u8          = 0xf0;

    pub const EID_MASK: u8               = 0x0e;
    pub const HOP_OPTS: u8               = 0 << 1;
    pub const ROUTING: u8                = 1 << 1;
    pub const FRAGMENT: u8               = 2 << 1;
    pub const DST_OPTS: u8               = 3 << 1;
    pub const MOBILITY: u8               = 4 << 1;
    pub const IP6: u8                    = 7 << 1;

    pub const NH: u8                     = 0x01;

    // UDP header compression

    pub const UDP_4BIT_PORT: u16         = 0xf0b0;
    pub const UDP_4BIT_PORT_MASK: u16    = 0xfff0;
    pub const UDP_8BIT_PORT: u16         = 0xf000;
    pub const UDP_8BIT_PORT_MASK: u16    = 0xff00;

    pub const UDP_CHECKSUM_FLAG: u8      = 0b100;
    pub const UDP_SRC_PORT_FLAG: u8      = 0b010;
    pub const UDP_DST_PORT_FLAG: u8      = 0b001;
}

#[derive(Copy,Clone,Debug)]
pub struct Context<'a> {
    pub prefix: &'a [u8],
    pub prefix_len: u8,
    pub id: u8,
    pub compress: bool,
}

/// LoWPAN encoding requires being able to look up the existence of contexts,
/// which are essentially IPv6 address prefixes. Any implementation must ensure
/// that context 0 is always available and contains the mesh-local prefix.
pub trait ContextStore<'a> {
    fn get_context_from_addr(&self, ip_addr: IPAddr) -> Option<Context<'a>>;
    fn get_context_from_id(&self, ctx_id: u8) -> Option<Context<'a>>;
    fn get_context_0(&self) -> Context<'a> {
        match self.get_context_from_id(0) {
            Some(ctx) => ctx,
            None => panic!("Context 0 not found"),
        }
    }
    fn get_context_from_prefix(&self, prefix: &[u8], prefix_len: u8) -> Option<Context<'a>>;
}

/// Computes the LoWPAN Interface Identifier from either the 16-bit short MAC or
/// the IEEE EUI-64 that is derived from the 48-bit MAC.
pub fn compute_iid(mac_addr: &MacAddr) -> [u8; 8] {
    match mac_addr {
        &MacAddr::ShortAddr(short_addr) => {
            // IID is 0000:00ff:fe00:XXXX, where XXXX is 16-bit MAC
            let mut iid: [u8; 8] = iphc::MAC_BASE;
            iid[6] = (short_addr >> 1) as u8;
            iid[7] = (short_addr & 0xff) as u8;
            iid
        },
        &MacAddr::LongAddr(long_addr) => {
            // IID is IEEE EUI-64 with universal/local bit inverted
            let mut iid: [u8; 8] = long_addr;
            iid[0] ^= iphc::MAC_UL;
            iid
        }
    }
}

/// Determines if the next header is LoWPAN_NHC compressible, which depends on
/// both the next header type and the length of the IPv6 next header extensions.
/// Returns `Ok((false, 0))` if the next header is not compressible or
/// `Ok((true, nh_len))`. `nh_len` is only meaningful when the next header type
/// is an IPv6 next header extension, in which case it is the number of bytes
/// after the first two bytes in the IPv6 next header extension. Returns
/// `Err(())` in the case of an invalid IPv6 packet.
fn is_ip6_nh_compressible(next_header: u8,
                          next_headers: &[u8]) -> Result<(bool, u8), ()> {
    match next_header {
        // IP6 encapsulated headers are always compressed
        ip6_nh::IP6 => Ok((true, 0)),
        // UDP headers are always compresed
        ip6_nh::UDP => Ok((true, 0)),
        ip6_nh::FRAGMENT
        | ip6_nh::HOP_OPTS
        | ip6_nh::ROUTING
        | ip6_nh::DST_OPTS
        | ip6_nh::MOBILITY => {
            let mut header_len: u32 = 6;
            if next_header != ip6_nh::FRAGMENT {
                // All compressible next header extensions except
                // for the fragment header have a length field
                if next_headers.len() < 2 {
                    return Err(());
                } else {
                    // The length field is the number of 8-octet
                    // groups after the first 8 octets
                    header_len += (next_headers[1] as u32) * 8;
                }
            }
            if header_len <= 255 {
                Ok((true, header_len as u8))
            } else {
                Ok((false, 0))
            }
        },
        _ => Ok((false, 0)),
    }
}

trait OnesComplement {
    fn ones_complement_add(self, other: Self) -> Self;
}

/// Implements one's complement addition for use in calculating the UDP checksum
impl OnesComplement for u16 {
    fn ones_complement_add(self, other: u16) -> u16 {
        let (sum, overflow) = self.overflowing_add(other);
        if overflow {
            sum + 1
        } else {
            sum
        }
    }
}

/// Computes the UDP checksum for a UDP packet sent over IPv6.
/// Returns the checksum in host byte-order.
fn compute_udp_checksum(ip6_header: &IP6Header,
                        udp_header: &[u8],
                        udp_length: u16,
                        payload: &[u8]) -> u16 {
    // The UDP checksum is computed on the IPv6 pseudo-header concatenated
    // with the UDP header and payload, but with the UDP checksum field
    // zeroed out. Hence, this function assumes that `udp_header` has already
    // been filled with the UDP header, except for the ignored checksum.
    let mut checksum: u16 = 0;

    // IPv6 pseudo-header
    // +--16 bits--+--16 bits--+--16 bits--+--16 bits--+
    // |                                               |
    // +              Source IPv6 Address              +
    // |                                               |
    // +-----------+-----------+-----------+-----------+
    // |                                               |
    // +           Destination IPv6 Address            +
    // |                                               |
    // +-----------+-----------+-----------+-----------+
    // |      UDP Length       |     0     |  NH type  |
    // +-----------+-----------+-----------+-----------+

    // Source and destination addresses
    for two_bytes in ip6_header.src_addr.0.chunks(2) {
        checksum = checksum.ones_complement_add(slice_to_u16(two_bytes));
    }
    for two_bytes in ip6_header.dst_addr.0.chunks(2) {
        checksum = checksum.ones_complement_add(slice_to_u16(two_bytes));
    }

    // UDP length and UDP next header type. Note that we can avoid adding zeros,
    // but the pseudo header must be in network byte-order.
    checksum = checksum.ones_complement_add(htons(udp_length));
    checksum = checksum.ones_complement_add(htons(ip6_nh::UDP as u16));

    // UDP header without the checksum (which is the last two bytes)
    for two_bytes in udp_header[0..6].chunks(2) {
        checksum = checksum.ones_complement_add(slice_to_u16(two_bytes));
    }

    // UDP payload
    for bytes in payload.chunks(2) {
        checksum = checksum.ones_complement_add(
            if bytes.len() == 2 {
                slice_to_u16(bytes)
            } else {
                htons(bytes[0] as u16)
            }
        );
    }

    // Return the complement of the checksum, unless it is 0, in which case we
    // the checksum is one's complement -0 for a non-zero binary representation
    if !checksum != 0 {
        !checksum
    } else {
        checksum
    }
}

/// Maps values of a IPv6 next header field to a corresponding LoWPAN
/// NHC-encoding extension ID, if that next header type is NHC-compressible
fn ip6_nh_to_nhc_eid(next_header: u8) -> Option<u8> {
    match next_header {
        ip6_nh::HOP_OPTS => Some(nhc::HOP_OPTS),
        ip6_nh::ROUTING  => Some(nhc::ROUTING),
        ip6_nh::FRAGMENT => Some(nhc::FRAGMENT),
        ip6_nh::DST_OPTS => Some(nhc::DST_OPTS),
        ip6_nh::MOBILITY => Some(nhc::MOBILITY),
        ip6_nh::IP6      => Some(nhc::IP6),
        _ => None,
    }
}

/// Maps a LoWPAN_NHC header the corresponding IPv6 next header type,
/// or an error if the NHC header is invalid
fn nhc_to_ip6_nh(nhc: u8) -> Result<u8, ()> {
    match nhc & nhc::DISPATCH_MASK {
        nhc::DISPATCH_NHC => match nhc & nhc::EID_MASK {
            nhc::HOP_OPTS => Ok(ip6_nh::HOP_OPTS),
            nhc::ROUTING  => Ok(ip6_nh::ROUTING),
            nhc::FRAGMENT => Ok(ip6_nh::FRAGMENT),
            nhc::DST_OPTS => Ok(ip6_nh::DST_OPTS),
            nhc::MOBILITY => Ok(ip6_nh::MOBILITY),
            nhc::IP6      => Ok(ip6_nh::IP6),
            _ => Err(()),
        },
        nhc::DISPATCH_UDP => Ok(ip6_nh::UDP),
        _ => Err(()),
    }
}

pub struct LoWPAN<'a, C: ContextStore<'a> + 'a> {
    ctx_store: &'a C,
}

impl<'a, C: ContextStore<'a> + 'a> LoWPAN<'a, C> {
    pub fn new(ctx_store: &'a C) -> LoWPAN<'a, C> {
        LoWPAN { ctx_store: ctx_store }
    }

    /// Constructs a 6LoWPAN header in `buf` from the given IPv6 datagram and
    /// 16-bit MAC addresses. If the compression was successful, returns
    /// `Ok((consumed, written))`, where `consumed` is the number of header
    /// bytes consumed from the IPv6 datagram `written` is the number of
    /// compressed header bytes written into `buf`. Payload bytes and
    /// non-compressed next headers are not written, so the remaining `buf.len()
    /// - consumed` bytes must still be copied over to `buf`.
    pub fn compress(&self,
                    ip6_datagram: &[u8],
                    src_mac_addr: MacAddr,
                    dst_mac_addr: MacAddr,
                    mut buf: &mut [u8])
                    -> Result<(usize, usize), ()> {
        let ip6_header: &IP6Header = unsafe {
            mem::transmute(ip6_datagram.as_ptr())
        };
        let mut consumed: usize = mem::size_of::<IP6Header>();
        let mut next_headers: &[u8] = &ip6_datagram[consumed..];

        // The first two bytes are the LOWPAN_IPHC header
        let mut offset: usize = 2;

        // Initialize the LOWPAN_IPHC header
        buf[0..2].copy_from_slice(&iphc::DISPATCH);

        let mut src_ctx: Option<Context> = self.ctx_store
            .get_context_from_addr(ip6_header.src_addr);
        let mut dst_ctx: Option<Context> = if ip6_header.dst_addr.is_multicast() {
            let prefix_len: u8 = ip6_header.dst_addr.0[3];
            let prefix: &[u8] = &ip6_header.dst_addr.0[4..12];
            // This also implicitly verifies that prefix_len <= 64
            if util::verify_prefix_len(prefix, prefix_len) {
                self.ctx_store.get_context_from_prefix(prefix, prefix_len)
            } else {
                None
            }
        } else {
            self.ctx_store.get_context_from_addr(ip6_header.dst_addr)
        };

        // Do not contexts that are not marked to be available for compression
        src_ctx = src_ctx.and_then(|ctx| if ctx.compress { Some(ctx) } else { None });
        dst_ctx = dst_ctx.and_then(|ctx| if ctx.compress { Some(ctx) } else { None });

        // Context Identifier Extension
        self.compress_cie(&src_ctx, &dst_ctx, &mut buf, &mut offset);

        // Traffic Class & Flow Label
        self.compress_tf(ip6_header, &mut buf, &mut offset);

        // Next Header
        let (mut is_nhc, mut nh_len): (bool, u8) =
            is_ip6_nh_compressible(ip6_header.next_header, next_headers)?;
        self.compress_nh(ip6_header, is_nhc, &mut buf, &mut offset);

        // Hop Limit
        self.compress_hl(ip6_header, &mut buf, &mut offset);

        // Source Address
        self.compress_src(&ip6_header.src_addr,
                          &src_mac_addr,
                          &src_ctx,
                          &mut buf,
                          &mut offset);

        // Destination Address
        if ip6_header.dst_addr.is_multicast() {
            self.compress_multicast(&ip6_header.dst_addr,
                                    &dst_ctx,
                                    &mut buf,
                                    &mut offset);
        } else {
            self.compress_dst(&ip6_header.dst_addr,
                              &dst_mac_addr,
                              &dst_ctx,
                              &mut buf,
                              &mut offset);
        }

        // Next Headers
        let mut ip6_nh_type: u8 = ip6_header.next_header;
        while is_nhc {
            match ip6_nh_type {
                ip6_nh::IP6 => {
                    // For IPv6 encapsulation, the NH bit in the NHC ID is 0
                    let nhc_header = nhc::DISPATCH_NHC | nhc::IP6;
                    buf[offset] = nhc_header;
                    offset += 1;

                    // Recursively place IPHC-encoded IPv6 after the NHC ID
                    let (encap_consumed, encap_offset) =
                        self.compress(next_headers,
                                      src_mac_addr,
                                      dst_mac_addr,
                                      &mut buf[offset..])?;
                    consumed += encap_consumed;
                    offset += encap_offset;

                    // The above recursion handles the rest of the packet
                    // headers, so we are done
                    break;
                },
                ip6_nh::UDP => {
                    let mut nhc_header = nhc::DISPATCH_UDP;

                    // Leave a space for the UDP LoWPAN_NHC byte
                    let udp_nh_offset = offset;
                    offset += 1;

                    // Compress ports and checksum
                    let udp_header = &next_headers[0..8];
                    nhc_header |= self.compress_udp_ports(udp_header,
                                                          &mut buf,
                                                          &mut offset);
                    nhc_header |= self.compress_udp_checksum(udp_header,
                                                             &mut buf,
                                                             &mut offset);

                    // Write the UDP LoWPAN_NHC byte
                    buf[udp_nh_offset] = nhc_header;
                    consumed += 8;

                    // There cannot be any more next headers after UDP
                    break;
                },
                ip6_nh::FRAGMENT
                | ip6_nh::HOP_OPTS
                | ip6_nh::ROUTING
                | ip6_nh::DST_OPTS
                | ip6_nh::MOBILITY => {
                    // The NHC EID is guaranteed not to be 0 here.
                    let mut nhc_header = nhc::DISPATCH_NHC
                        | ip6_nh_to_nhc_eid(ip6_nh_type).unwrap_or(0);
                    // next_nh_offset includes the next header field and the
                    // length byte, while nh_len does not
                    let next_nh_offset = 2 + (nh_len as usize);

                    // Determine if the next header is compressible
                    let (next_is_nhc, next_nh_len) =
                        is_ip6_nh_compressible(next_headers[0],
                                               &next_headers[next_nh_offset..])?;
                    if next_is_nhc {
                        nhc_header |= nhc::NH;
                    }

                    // Place NHC ID in buffer
                    buf[offset] = nhc_header;
                    if ip6_nh_type != ip6_nh::FRAGMENT {
                        // Fragment extension does not have a length field
                        buf[offset + 1] = nh_len;
                    }
                    offset += 2;

                    self.compress_and_elide_padding(ip6_nh_type,
                                                    nh_len as usize,
                                                    &next_headers,
                                                    &mut buf,
                                                    &mut offset);

                    ip6_nh_type = next_headers[0];
                    is_nhc = next_is_nhc;
                    nh_len = next_nh_len;
                    next_headers = &next_headers[next_nh_offset..];
                    consumed += next_nh_offset;
                },
                // This case should not be reachable because
                // is_ip6_nh_compressible guarantees that is_nhc is true
                // only if ip6_nh_type is one of the types matched above
                _ => panic!("Unreachable case"),
            }
        }

        Ok((consumed, offset))
    }

    fn compress_cie(&self,
                    src_ctx: &Option<Context>,
                    dst_ctx: &Option<Context>,
                    buf: &mut [u8],
                    offset: &mut usize) {
        let mut cie: u8 = 0;

        src_ctx.as_ref().map(|ctx| if ctx.id != 0 {
            cie |= ctx.id << 4;
        });
        dst_ctx.as_ref().map(|ctx| if ctx.id != 0 {
            cie |= ctx.id;
        });

        if cie != 0 {
            buf[1] |= iphc::CID;
            buf[*offset] = cie;
            *offset += 1;
        }
    }

    fn compress_tf(&self, ip6_header: &IP6Header, buf: &mut [u8], offset: &mut usize) {
        let ecn = ip6_header.get_ecn();
        let dscp = ip6_header.get_dscp();
        let flow = ip6_header.get_flow_label();

        let mut tf_encoding = 0;
        let old_offset = *offset;

        // If ECN != 0 we are forced to at least have one byte,
        // otherwise we can elide dscp
        if dscp == 0 && (ecn == 0 || flow != 0) {
            tf_encoding |= iphc::TF_TRAFFIC_CLASS;
        } else {
            buf[*offset] = dscp;
            *offset += 1;
        }

        // We can elide flow if it is 0
        if flow == 0 {
            tf_encoding |= iphc::TF_FLOW_LABEL;
        } else {
            buf[*offset]     = ((flow >> 16) & 0x0f) as u8;
            buf[*offset + 1] = (flow >> 8) as u8;
            buf[*offset + 2] = flow as u8;
            *offset += 3;
        }

        if *offset != old_offset {
            buf[old_offset] |= ecn << 6;
        }
        buf[0] |= tf_encoding;
    }

    fn compress_nh(&self,
                   ip6_header: &IP6Header,
                   is_nhc: bool,
                   buf: &mut [u8],
                   offset: &mut usize) {
        if is_nhc {
            buf[0] |= iphc::NH;
        } else {
            buf[*offset] = ip6_header.next_header;
            *offset += 1;
        }
    }

    fn compress_hl(&self, ip6_header: &IP6Header, buf: &mut [u8], offset: &mut usize) {
        let hop_limit_flag = {
            match ip6_header.hop_limit {
                // Compressed
                1 => iphc::HLIM_1,
                64 => iphc::HLIM_64,
                255 => iphc::HLIM_255,
                // Uncompressed
                _ => {
                    buf[*offset] = ip6_header.hop_limit;
                    *offset += 1;
                    iphc::HLIM_INLINE
                }
            }
        };
        buf[0] |= hop_limit_flag;
    }

    // TODO: We should check to see whether context or link local compression
    // schemes gives the better compression; currently, we will always match
    // on link local even if we could get better compression through context.
    fn compress_src(&self,
                    src_ip_addr: &IPAddr,
                    src_mac_addr: &MacAddr,
                    src_ctx: &Option<Context>,
                    buf: &mut [u8],
                    offset: &mut usize) {
        if src_ip_addr.is_unspecified() {
            // SAC = 1, SAM = 00
            buf[1] |= iphc::SAC;
        } else if src_ip_addr.is_unicast_link_local() {
            // SAC = 0, SAM = 01, 10, 11
            self.compress_iid(src_ip_addr, src_mac_addr, true, buf, offset);
        } else if src_ctx.is_some() {
            // SAC = 1, SAM = 01, 10, 11
            buf[1] |= iphc::SAC;
            self.compress_iid(src_ip_addr, src_mac_addr, true, buf, offset);
        } else {
            // SAC = 0, SAM = 00
            buf[*offset..*offset + 16].copy_from_slice(&src_ip_addr.0);
            *offset += 16;
        }
    }

    // TODO: For the SAC=0, SAM=11 case, we must also consider computing the
    // address from an encapsulating IPv6 packet (e.g. when we recurse), not
    // just from a 802.15.4 frame.
    fn compress_iid(&self,
                    ip_addr: &IPAddr,
                    mac_addr: &MacAddr,
                    is_src: bool,
                    buf: &mut [u8],
                    offset: &mut usize) {
        let iid: [u8; 8] = compute_iid(mac_addr);
        if ip_addr.0[8..16] == iid {
            // SAM/DAM = 11, 0 bits
            buf[1] |= if is_src {
                iphc::SAM_MODE3
            } else {
                iphc::DAM_MODE3
            };
        } else if ip_addr.0[8..14] == iphc::MAC_BASE[0..6] {
            // SAM/DAM = 10, 16 bits
            buf[1] |= if is_src {
                iphc::SAM_MODE2
            } else {
                iphc::DAM_MODE2
            };
            buf[*offset..*offset + 2].copy_from_slice(&ip_addr.0[14..16]);
            *offset += 2;
        } else {
            // SAM/DAM = 01, 64 bits
            buf[1] |= if is_src {
                iphc::SAM_MODE1
            } else {
                iphc::DAM_MODE1
            };
            buf[*offset..*offset + 8].copy_from_slice(&ip_addr.0[8..16]);
            *offset += 8;
        }
    }

    // Compresses non-multicast destination address
    // TODO: We should check to see whether context or link local compression
    // schemes gives the better compression; currently, we will always match
    // on link local even if we could get better compression through context.
    fn compress_dst(&self,
                    dst_ip_addr: &IPAddr,
                    dst_mac_addr: &MacAddr,
                    dst_ctx: &Option<Context>,
                    buf: &mut [u8],
                    offset: &mut usize) {
        // Assumes dst_ip_addr is not a multicast address (prefix ffXX)
        if dst_ip_addr.is_unicast_link_local() {
            // Link local compression
            // M = 0, DAC = 0, DAM = 01, 10, 11
            self.compress_iid(dst_ip_addr, dst_mac_addr, false, buf, offset);
        } else if dst_ctx.is_some() {
            // Context compression
            // DAC = 1, DAM = 01, 10, 11
            buf[1] |= iphc::DAC;
            self.compress_iid(dst_ip_addr, dst_mac_addr, false, buf, offset);
        } else {
            // Full address inline
            // DAC = 0, DAM = 00
            buf[*offset..*offset + 16].copy_from_slice(&dst_ip_addr.0);
            *offset += 16;
        }
    }

    // Compresses multicast destination addresses
    fn compress_multicast(&self,
                          dst_ip_addr: &IPAddr,
                          dst_ctx: &Option<Context>,
                          buf: &mut [u8],
                          offset: &mut usize) {
        // Assumes dst_ip_addr is indeed a multicast address (prefix ffXX)
        buf[1] |= iphc::MULTICAST;
        if dst_ctx.is_some() {
            // M = 1, DAC = 1, DAM = 00
            buf[1] |= iphc::DAC;
            buf[*offset..*offset + 2].copy_from_slice(&dst_ip_addr.0[1..3]);
            buf[*offset + 2..*offset + 6].copy_from_slice(&dst_ip_addr.0[12..16]);
            *offset += 6;
        } else {
            // M = 1, DAC = 0
            if dst_ip_addr.0[1] == 0x02 && util::is_zero(&dst_ip_addr.0[2..15]) {
                // DAM = 11
                buf[1] |= iphc::DAM_MODE3;
                buf[*offset] = dst_ip_addr.0[15];
                *offset += 1;
            } else {
                if !util::is_zero(&dst_ip_addr.0[2..11]) {
                    // DAM = 00
                    buf[1] |= iphc::DAM_INLINE;
                    buf[*offset..*offset + 16].copy_from_slice(&dst_ip_addr.0);
                    *offset += 16;
                } else if !util::is_zero(&dst_ip_addr.0[11..13]) {
                    // DAM = 01, ffXX::00XX:XXXX:XXXX
                    buf[1] |= iphc::DAM_MODE1;
                    buf[*offset] = dst_ip_addr.0[1];
                    buf[*offset + 1..*offset + 6].copy_from_slice(&dst_ip_addr.0[11..16]);
                    *offset += 6;
                } else {
                    // DAM = 10, ffXX::00XX:XXXX
                    buf[1] |= iphc::DAM_MODE2;
                    buf[*offset] = dst_ip_addr.0[1];
                    buf[*offset + 1..*offset + 4].copy_from_slice(&dst_ip_addr.0[13..16]);
                    *offset += 4;
                }
            }
        }
    }

    fn compress_udp_ports(&self,
                          udp_header: &[u8],
                          buf: &mut [u8],
                          offset: &mut usize) -> u8 {
        let src_port: u16 = ntohs(slice_to_u16(&udp_header[0..2]));
        let dst_port: u16 = ntohs(slice_to_u16(&udp_header[2..4]));

        let mut udp_port_nhc = 0;
        if (src_port & nhc::UDP_4BIT_PORT_MASK) == nhc::UDP_4BIT_PORT
            && (dst_port & nhc::UDP_4BIT_PORT_MASK) == nhc::UDP_4BIT_PORT {
            // Both can be compressed to 4 bits
            udp_port_nhc |= nhc::UDP_SRC_PORT_FLAG | nhc::UDP_DST_PORT_FLAG;
            // This should compress the ports to a single 8-bit value,
            // with the source port before the destination port
            buf[*offset] = (((src_port & !nhc::UDP_4BIT_PORT_MASK) << 4)
                           | (dst_port & !nhc::UDP_4BIT_PORT_MASK)) as u8;
            *offset += 1;
        } else if (src_port & nhc::UDP_8BIT_PORT_MASK) == nhc::UDP_8BIT_PORT {
            // Source port compressed to 8 bits, destination port uncompressed
            udp_port_nhc |= nhc::UDP_SRC_PORT_FLAG;
            buf[*offset] = (src_port & !nhc::UDP_8BIT_PORT_MASK) as u8;
            u16_to_slice(htons(dst_port), &mut buf[*offset + 1..*offset + 3]);
            *offset += 3;
        } else if (dst_port & nhc::UDP_8BIT_PORT_MASK) == nhc::UDP_8BIT_PORT {
            udp_port_nhc |= nhc::UDP_DST_PORT_FLAG;
            u16_to_slice(htons(src_port), &mut buf[*offset..*offset + 2]);
            buf[*offset + 3] = (dst_port & !nhc::UDP_8BIT_PORT_MASK) as u8;
            *offset += 3;
        } else {
            buf[*offset..*offset + 4].copy_from_slice(&udp_header[0..4]);
            *offset += 4;
        }
        return udp_port_nhc;
    }

    fn compress_udp_checksum(&self,
                             udp_header: &[u8],
                             buf: &mut [u8],
                             offset: &mut usize) -> u8 {
        // TODO: Checksum is always inline, elision is currently not supported
        buf[*offset] = udp_header[6];
        buf[*offset + 1] = udp_header[7];
        *offset += 2;
        // Inline checksum corresponds to the 0 flag
        0
    }

    fn compress_and_elide_padding(&self,
                                  nh_type: u8,
                                  nh_len: usize,
                                  next_headers: &[u8],
                                  buf: &mut [u8],
                                  offset: &mut usize) {
        // true if the header length is a multiple of 8-octets
        let total_len = nh_len + 2;
        let is_multiple = (total_len % 8) == 0;
        let correct_type = (nh_type == ip6_nh::HOP_OPTS)
            || (nh_type == ip6_nh::DST_OPTS);
        // opt_offset points to the start of the end padding (if it exists)
        let mut opt_offset = 2;
        let mut prev_was_padding = false;
        let mut is_padding = false;
        if correct_type && is_multiple {
            while opt_offset < total_len {
                let opt_type = next_headers[opt_offset];
                // This is the last byte
                if opt_offset == total_len - 1 {
                    // If last option is Pad1
                    if !prev_was_padding && opt_type == 0 {
                        is_padding = true;
                    }
                    break;
                }
                if opt_type == 0 {
                    prev_was_padding = true;
                    opt_offset += 1;
                    continue;
                }
                let opt_len = next_headers[opt_offset + 1] as usize;
                let new_opt_offset = opt_offset + opt_len + 2;
                // PadN
                if new_opt_offset == total_len {
                    if opt_type == 1 && !prev_was_padding {
                        is_padding = true;
                    }
                    break;
                }
                if opt_type == 1 {
                    prev_was_padding = true;
                } else {
                    prev_was_padding = false;
                }
                opt_offset = new_opt_offset;
            }
        }

        // We only elide the padding if: 1) Encapsulating packet is a multiple
        // of 8 octets in length, 2) the header is either hop options or dest
        // options, and 3) if there is a single Pad1 or PadN trailing padding.
        if is_multiple && correct_type && is_padding {
            buf[*offset..*offset + opt_offset - 2]
                .copy_from_slice(&next_headers[2..opt_offset]);
            *offset += opt_offset - 2;
        } else {
            // Copy over the remaining packet data
            buf[*offset..*offset + nh_len].copy_from_slice(&next_headers[2..2 + nh_len]);
            *offset += nh_len;
        }
    }

    /// Decodes a compressed header into a full IPv6 header given the 16-bit MAC
    /// addresses. `buf` is expected to be a slice containing only the 6LowPAN
    /// packet along with its payload.  If the decompression was successful,
    /// returns `Ok((consumed, written))`, where `consumed` is the number of
    /// header bytes consumed from the 6LoWPAN header and `written` is the
    /// number of uncompressed header bytes written into `out_buf`. Payload
    /// bytes and non-compressed next headers are not written, so the remaining
    /// `buf.len() - consumed` bytes must still be copied over to `out_buf`.
    pub fn decompress(&self,
                      buf: &[u8],
                      src_mac_addr: MacAddr,
                      dst_mac_addr: MacAddr,
                      mut out_buf: &mut [u8])
                      -> Result<(usize, usize), ()> {
        // Get the LOWPAN_IPHC header (the first two bytes are the header)
        let iphc_header_1: u8 = buf[0];
        let iphc_header_2: u8 = buf[1];
        let mut offset: usize = 2;

        let mut ip6_header: &mut IP6Header = unsafe {
            mem::transmute(out_buf.as_mut_ptr())
        };
        let mut bytes_written: usize = mem::size_of::<IP6Header>();
        *ip6_header = IP6Header::new();

        // Decompress CID and CIE fields if they exist
        let (src_ctx, dst_ctx) =
            self.decompress_cie(iphc_header_1, &buf, &mut offset)?;

        // Traffic Class & Flow Label
        self.decompress_tf(&mut ip6_header, iphc_header_1, &buf, &mut offset);

        // Next Header
        let (mut is_nhc, mut next_header) = self.decompress_nh(iphc_header_1,
                                                               &buf,
                                                               &mut offset);

        // Decompress hop limit field
        self.decompress_hl(&mut ip6_header, iphc_header_1, &buf, &mut offset)?;

        // Decompress source address
        self.decompress_src(&mut ip6_header, iphc_header_2,
                            &src_mac_addr, &src_ctx, &buf, &mut offset)?;

        // Decompress destination address
        if (iphc_header_2 & iphc::MULTICAST) != 0 {
            self.decompress_multicast(&mut ip6_header, iphc_header_2, &dst_ctx,
                                      &buf, &mut offset)?;
        } else {
            self.decompress_dst(&mut ip6_header, iphc_header_2,
                                &dst_mac_addr, &dst_ctx, &buf, &mut offset)?;
        }

        // Note that next_header is already set only if is_nhc is false
        if is_nhc {
            next_header = nhc_to_ip6_nh(buf[offset])?;
        }
        ip6_header.set_next_header(next_header);
        // While the next header is still compressed
        // Note that at each iteration, offset points to the NHC header field
        // and next_header refers to the type of this field.
        while is_nhc {
            // Advance past the LoWPAN NHC byte
            let nhc_header = buf[offset];
            offset += 1;

            // Scoped mutable borrow of out_buf
            let mut next_headers: &mut [u8] = &mut out_buf[bytes_written..];

            match next_header {
                ip6_nh::IP6 => {
                    let (encap_written, encap_processed) =
                        self.decompress(&buf[offset..],
                                        src_mac_addr,
                                        dst_mac_addr,
                                        &mut next_headers[bytes_written..])?;
                    bytes_written += encap_written;
                    offset += encap_processed;
                    break;
                },
                ip6_nh::UDP => {
                    // UDP length includes UDP header and data in bytes
                    let udp_length = (8 + (buf.len() - offset)) as u16;
                    // Decompress UDP header fields
                    let (src_port, dst_port) =
                        self.decompress_udp_ports(nhc_header,
                                                  &buf,
                                                  &mut offset);
                    // Fill in uncompressed UDP header
                    u16_to_slice(htons(src_port), &mut next_headers[0..2]);
                    u16_to_slice(htons(dst_port), &mut next_headers[2..4]);
                    u16_to_slice(htons(udp_length), &mut next_headers[4..6]);
                    // Need to fill in header values before computing the checksum
                    let udp_checksum =
                        self.decompress_udp_checksum(nhc_header,
                                                     &next_headers[0..8],
                                                     udp_length,
                                                     &ip6_header,
                                                     &buf,
                                                     &mut offset);
                    u16_to_slice(htons(udp_checksum), &mut next_headers[6..8]);

                    bytes_written += 8;
                    break;
                },
                ip6_nh::FRAGMENT
                | ip6_nh::HOP_OPTS
                | ip6_nh::ROUTING
                | ip6_nh::DST_OPTS
                | ip6_nh::MOBILITY => {
                    // True if the next header is also compressed
                    is_nhc = (nhc_header & nhc::NH) != 0;

                    // len is the number of octets following the length field
                    let len = buf[offset] as usize;
                    offset += 1;

                    // Check that there is a next header in the buffer,
                    // which must be the case if the last next header specifies
                    // NH = 1
                    if offset + len >= buf.len() {
                        return Err(());
                    }

                    // Length in 8-octet units after the first 8 octets
                    // (per the IPv6 ext hdr spec)
                    let mut hdr_len_field = (len - 6) / 8;
                    if (len - 6) % 8 != 0 {
                        hdr_len_field += 1;
                    }

                    // Gets the type of the subsequent next header.  If is_nhc
                    // is true, there must be a LoWPAN NHC header byte,
                    // otherwise there is either an uncompressed next header.
                    next_header = if is_nhc {
                        // The next header is LoWPAN NHC-compressed
                        nhc_to_ip6_nh(buf[offset + len])?
                    } else {
                        // The next header is uncompressed
                        buf[offset + len]
                    };

                    // Fill in the extended header in uncompressed IPv6 format
                    next_headers[0] = next_header;
                    next_headers[1] = hdr_len_field as u8;
                    // Copies over the remaining options.
                    next_headers[2..2 + len]
                        .copy_from_slice(&buf[offset..offset + len]);

                    // Fill in padding
                    let pad_bytes = hdr_len_field * 8 - len + 6;
                    if pad_bytes == 1 {
                        // Pad1
                        next_headers[2 + len] = 0;
                    } else {
                        // PadN, 2 <= pad_bytes <= 7
                        next_headers[2 + len] = 1;
                        next_headers[2 + len + 1] = pad_bytes as u8 - 2;
                        for i in 2..pad_bytes {
                            next_headers[2 + len + i] = 0;
                        }
                    }

                    bytes_written += 8 + hdr_len_field * 8;
                    offset += len;
                },
                _ => panic!("Unreachable case"),
            }
        }

        // The IPv6 header length field is the size of the IPv6 payload,
        // including extension headers. This is thus the uncompressed
        // size of the IPv6 packet - the fixed IPv6 header.
        let payload_len = bytes_written + (buf.len() - offset)
                          - mem::size_of::<IP6Header>();
        ip6_header.payload_len = ip::htons(payload_len as u16);
        Ok((offset, bytes_written))
    }

    fn decompress_cie(&self,
                      iphc_header: u8,
                      buf: &[u8],
                      offset: &mut usize) -> Result<(Context, Context), ()> {
        let ctx_0 = self.ctx_store.get_context_0();
        let (mut src_ctx, mut dst_ctx) = (ctx_0, ctx_0);
        if iphc_header & iphc::CID != 0 {
            let sci = buf[*offset] >> 4;
            let dci = buf[*offset] & 0xf;
            *offset += 1;

            if sci != 0 {
                src_ctx = self.ctx_store.get_context_from_id(sci).ok_or(())?;
            }
            if dci != 0 {
                dst_ctx = self.ctx_store.get_context_from_id(dci).ok_or(())?;
            }
        }
        Ok((src_ctx, dst_ctx))
    }

    fn decompress_tf(&self,
                     ip6_header: &mut IP6Header,
                     iphc_header: u8,
                     buf: &[u8],
                     offset: &mut usize) {
        let fl_compressed = (iphc_header & iphc::TF_FLOW_LABEL) != 0;
        let tc_compressed = (iphc_header & iphc::TF_TRAFFIC_CLASS) != 0;

        // Determine ECN and DSCP separately because the order is different
        // from the IPv6 traffic class field.
        if !fl_compressed || !tc_compressed {
            let ecn = buf[*offset] >> 6;
            ip6_header.set_ecn(ecn);
        }
        if !tc_compressed {
            let dscp = buf[*offset] & 0b111111;
            ip6_header.set_dscp(dscp);
            *offset += 1;
        }

        // Flow label is always in the same bit position relative to the last
        // three bytes in the inline fields
        if fl_compressed {
            ip6_header.set_flow_label(0);
        } else {
            let flow = (((buf[*offset] & 0x0f) as u32) << 16)
                      | ((buf[*offset + 1] as u32) << 8)
                      |  (buf[*offset + 2] as u32);
            *offset += 3;
            ip6_header.set_flow_label(flow);
        }
    }

    fn decompress_nh(&self,
                     iphc_header: u8,
                     buf: &[u8],
                     offset: &mut usize) -> (bool, u8) {
        let is_nhc = (iphc_header & iphc::NH) != 0;
        let mut next_header: u8 = 0;
        if !is_nhc {
            next_header = buf[*offset];
            *offset += 1;
        }
        return (is_nhc, next_header);
    }

    fn decompress_hl(&self, 
                     ip6_header: &mut IP6Header,
                     iphc_header: u8,
                     buf: &[u8],
                     offset: &mut usize) -> Result<(), ()> {
        let hop_limit = match iphc_header & iphc::HLIM_MASK {
            iphc::HLIM_1      => 1,
            iphc::HLIM_64     => 64,
            iphc::HLIM_255    => 255,
            iphc::HLIM_INLINE => {
                let hl = buf[*offset];
                *offset += 1;
                hl
            },
            _ => panic!("Unreachable case"),
        };
        ip6_header.set_hop_limit(hop_limit);
        Ok(())
    }

    fn decompress_src(&self,
                      ip6_header: &mut IP6Header,
                      iphc_header: u8,
                      mac_addr: &MacAddr,
                      ctx: &Context,
                      buf: &[u8],
                      offset: &mut usize) -> Result<(), ()> {
        let uses_context = (iphc_header & iphc::SAC) != 0;
        let sam_mode = iphc_header & iphc::SAM_MASK;
        if uses_context && sam_mode == iphc::SAM_INLINE {
            // SAC = 1, SAM = 00: UNSPECIFIED (::), which is already the default
        } else if uses_context {
            // SAC = 1, SAM = 01, 10, 11
            self.decompress_iid_context(sam_mode,
                                        &mut ip6_header.src_addr,
                                        mac_addr,
                                        ctx,
                                        buf,
                                        offset)?;
        } else {
            // SAC = 0, SAM = 00, 01, 10, 11
            self.decompress_iid_link_local(sam_mode,
                                           &mut ip6_header.src_addr,
                                           mac_addr,
                                           buf,
                                           offset)?;
        }
        Ok(())
    }

    fn decompress_dst(&self,
                      ip6_header: &mut IP6Header,
                      iphc_header: u8,
                      mac_addr: &MacAddr,
                      ctx: &Context,
                      buf: &[u8],
                      offset: &mut usize) -> Result<(), ()> {
        let uses_context = (iphc_header & iphc::DAC) != 0;
        let dam_mode = iphc_header & iphc::DAM_MASK;
        if uses_context && dam_mode == iphc::DAM_INLINE {
            // DAC = 1, DAM = 00: Reserved
            return Err(());
        } else if uses_context {
            // DAC = 1, DAM = 01, 10, 11
            self.decompress_iid_context(dam_mode,
                                        &mut ip6_header.dst_addr,
                                        mac_addr,
                                        ctx,
                                        buf,
                                        offset)?;
        } else {
            // DAC = 0, DAM = 00, 01, 10, 11
            self.decompress_iid_link_local(dam_mode,
                                           &mut ip6_header.dst_addr,
                                           mac_addr,
                                           buf,
                                           offset)?;
        }
        Ok(())
    }

    fn decompress_multicast(&self,
                            ip6_header: &mut IP6Header,
                            iphc_header: u8,
                            ctx: &Context,
                            buf: &[u8],
                            offset: &mut usize) -> Result<(), ()> {
        let uses_context = (iphc_header & iphc::DAC) != 0;
        let dam_mode = iphc_header & iphc::DAM_MASK;
        let mut ip_addr: &mut IPAddr = &mut ip6_header.dst_addr;
        if uses_context {
            match dam_mode {
                iphc::DAM_INLINE => {
                    // DAC = 1, DAM = 00: 48 bits
                    // ffXX:XXLL:PPPP:PPPP:PPPP:PPPP:XXXX:XXXX
                    let prefix_bytes = ((ctx.prefix_len + 7) / 8) as usize;
                    if prefix_bytes > 8 {
                        // The maximum prefix length for this mode is 64 bits.
                        // If the specified prefix exceeds this length, the
                        // compression is invalid.
                        return Err(());
                    }
                    ip_addr.0[0] = 0xff;
                    ip_addr.0[1] = buf[*offset];
                    ip_addr.0[2] = buf[*offset + 1];
                    ip_addr.0[3] = ctx.prefix_len;
                    ip_addr.0[4..4 + prefix_bytes]
                        .copy_from_slice(&ctx.prefix[0..prefix_bytes]);
                    ip_addr.0[12..16].copy_from_slice(&buf[*offset + 2..*offset + 6]);
                    *offset += 6;
                },
                _ => {
                    // DAC = 1, DAM = 01, 10, 11: Reserved
                    return Err(());
                },
            }
        } else {
            match dam_mode {
                // DAC = 0, DAM = 00: Inline
                iphc::DAM_INLINE => {
                    ip_addr.0.copy_from_slice(&buf[*offset..*offset + 16]);
                    *offset += 16;
                },
                // DAC = 0, DAM = 01: 48 bits
                // ffXX::00XX:XXXX:XXXX
                iphc::DAM_MODE1  => {
                    ip_addr.0[0] = 0xff;
                    ip_addr.0[1] = buf[*offset];
                    *offset += 1;
                    ip_addr.0[11..16].copy_from_slice(&buf[*offset..*offset + 5]);
                    *offset += 5;
                },
                // DAC = 0, DAM = 10: 32 bits
                // ffXX::00XX:XXXX
                iphc::DAM_MODE2  => {
                    ip_addr.0[0] = 0xff;
                    ip_addr.0[1] = buf[*offset];
                    *offset += 1;
                    ip_addr.0[13..16].copy_from_slice(&buf[*offset..*offset + 3]);
                    *offset += 3;
                },
                // DAC = 0, DAM = 11: 8 bits
                // ff02::00XX
                iphc::DAM_MODE3  => {
                    ip_addr.0[0] = 0xff;
                    ip_addr.0[1] = 0x02;
                    ip_addr.0[15] = buf[*offset];
                    *offset += 1;
                },
                _ => panic!("Unreachable case"),
            }
        }
        Ok(())
    }

    fn decompress_iid_link_local(&self,
                                 addr_mode: u8,
                                 ip_addr: &mut IPAddr,
                                 mac_addr: &MacAddr,
                                 buf: &[u8],
                                 offset: &mut usize) -> Result<(), ()> {
        let mode = addr_mode & (iphc::SAM_MASK | iphc::DAM_MASK);
        match mode {
            // SAM, DAM = 00: Inline
            iphc::SAM_INLINE => {
                // SAM_INLINE is equivalent to DAM_INLINE
                ip_addr.0.copy_from_slice(&buf[*offset..*offset + 16]);
                *offset += 16;
            },
            // SAM, DAM = 01: 64 bits
            // Link-local prefix (64 bits) + 64 bits carried inline
            iphc::SAM_MODE1 | iphc::DAM_MODE1 => {
                ip_addr.set_unicast_link_local();
                ip_addr.0[8..16].copy_from_slice(&buf[*offset..*offset + 8]);
                *offset += 8;
            },
            // SAM, DAM = 11: 16 bits
            // Link-local prefix (112 bits) + 0000:00ff:fe00:XXXX
            iphc::SAM_MODE2 | iphc::DAM_MODE2 => {
                ip_addr.set_unicast_link_local();
                ip_addr.0[11..13].copy_from_slice(&iphc::MAC_BASE[3..5]);
                ip_addr.0[14..16].copy_from_slice(&buf[*offset..*offset + 2]);
                *offset += 2;
            },
            // SAM, DAM = 11: 0 bits
            // Linx-local prefix (64 bits) + IID from outer header (64 bits)
            iphc::SAM_MODE3 | iphc::DAM_MODE3 => {
                ip_addr.set_unicast_link_local();
                ip_addr.0[8..16].copy_from_slice(&compute_iid(mac_addr));
            },
            _ => panic!("Unreachable case"),
        }
        Ok(())
    }

    fn decompress_iid_context(&self,
                              addr_mode: u8,
                              ip_addr: &mut IPAddr,
                              mac_addr: &MacAddr,
                              ctx: &Context,
                              buf: &[u8],
                              offset: &mut usize) -> Result<(), ()> {
        let mode = addr_mode & (iphc::SAM_MASK | iphc::DAM_MASK);
        match mode {
            // DAM = 00: Reserved
            // SAM = 0 is handled separately outside this method
            iphc::DAM_INLINE => {
                return Err(());
            },
            // SAM, DAM = 01: 64 bits
            // Suffix is the 64 bits carried inline
            iphc::SAM_MODE1 | iphc::DAM_MODE1 => {
                ip_addr.0[8..16].copy_from_slice(&buf[*offset..*offset + 8]);
                *offset += 8;
            },
            // SAM, DAM = 10: 16 bits
            // Suffix is 0000:00ff:fe00:XXXX
            iphc::SAM_MODE2 | iphc::DAM_MODE2 => {
                ip_addr.0[8..16].copy_from_slice(&iphc::MAC_BASE);
                ip_addr.0[14..16].copy_from_slice(&buf[*offset..*offset + 2]);
                *offset += 2;
            },
            // SAM, DAM = 11: 0 bits
            // Suffix is the IID computed from the encapsulating header
            iphc::SAM_MODE3 | iphc::DAM_MODE3 => {
                let iid = compute_iid(mac_addr);
                ip_addr.0[8..16].copy_from_slice(&iid[0..8]);
            },
            _ => panic!("Unreachable case"),
        }
        // The bits covered by the provided context are always used, so we copy
        // the context bits into the address after the non-context bits are set.
        ip_addr.set_prefix(&ctx.prefix, ctx.prefix_len);
        Ok(())
    }

    // Returns the UDP ports in host byte-order
    fn decompress_udp_ports(&self,
                            udp_nhc: u8,
                            buf: &[u8],
                            offset: &mut usize) -> (u16, u16) {
        let src_compressed = (udp_nhc & nhc::UDP_SRC_PORT_FLAG) != 0;
        let dst_compressed = (udp_nhc & nhc::UDP_DST_PORT_FLAG) != 0;

        let src_port: u16;
        let dst_port: u16;
        if src_compressed && dst_compressed {
            // Both src and dst are compressed to 4 bits
            let src_short = ((buf[*offset] >> 4) & 0xf) as u16;
            let dst_short = (buf[*offset] & 0xf) as u16;
            src_port = nhc::UDP_4BIT_PORT | src_short;
            dst_port = nhc::UDP_4BIT_PORT | dst_short;
            *offset += 1;
        } else if src_compressed {
            // Source port is compressed to 8 bits
            src_port = nhc::UDP_8BIT_PORT | (buf[*offset] as u16);
            // Destination port is uncompressed
            dst_port = ntohs(slice_to_u16(&buf[*offset + 1..*offset + 3]));
            *offset += 3;
        } else if dst_compressed {
            // Source port is uncompressed
            src_port = ntohs(slice_to_u16(&buf[*offset..*offset + 2]));
            // Destination port is compressed to 8 bits
            dst_port = nhc::UDP_8BIT_PORT | (buf[*offset + 2] as u16);
            *offset += 3;
        } else {
            // Both ports are uncompressed
            src_port = ntohs(slice_to_u16(&buf[*offset..*offset + 2]));
            dst_port = ntohs(slice_to_u16(&buf[*offset + 2..*offset + 4]));
            *offset += 4;
        }
        (src_port, dst_port)
    }

    // Returns the UDP checksum in host byte-order
    fn decompress_udp_checksum(&self,
                               udp_nhc: u8,
                               udp_header: &[u8],
                               udp_length: u16,
                               ip6_header: &IP6Header,
                               buf: &[u8],
                               offset: &mut usize) -> u16 {
        if (udp_nhc & nhc::UDP_CHECKSUM_FLAG) != 0 {
            // TODO: Need to verify that the packet was sent with *some* kind
            // of integrity check at a lower level (otherwise, we need to drop
            // the packet)
            compute_udp_checksum(ip6_header, udp_header, udp_length,
                                 &buf[*offset..])
        } else {
            let checksum = ntohs(slice_to_u16(&buf[*offset..*offset + 2]));
            *offset += 2;
            checksum
        }
    }
}
