extern crate kernel;
use kernel::ReturnCode;
use kernel::common::take_cell::TakeCell;
use kernel::hil::radio::{Radio, TxClient, RxClient, ConfigClient};
use kernel::hil::time;
use core::cell::Cell;
use core::cmp::min;
use net::lowpan::{LoWPAN, ContextStore};
use net::ip::MacAddr;
use net::util::{slice_to_u16, u16_to_slice};

const MAX_PAYLOAD_SIZE: usize = 128;

// TODO: Where to put these constants?
pub mod lowpan_frag {
    pub const FRAGN_HDR: u8 = 0b11100000;
    pub const FRAG1_HDR: u8 = 0b11000000;
    pub const FRAG1_HDR_SIZE: usize = 4;
    pub const FRAGN_HDR_SIZE: usize = 5;
}

// TODO: Network byte order stuff
fn set_frag_hdr(dgram_size: u16, dgram_tag: u16, dgram_offset: usize, hdr: &mut [u8],
                is_frag1: bool) {
    let mask = if is_frag1 {
        lowpan_frag::FRAG1_HDR
    } else {
        lowpan_frag::FRAGN_HDR
    };
    u16_to_slice(dgram_size, &mut hdr[0..2]);
    hdr[0] = mask | (hdr[0] & !mask);
    u16_to_slice(dgram_tag, &mut hdr[2..4]);
    if !is_frag1 {
        hdr[4] = (dgram_offset / 8) as u8;
    }
}

// TODO: Network byte order stuff
fn get_frag_hdr(hdr: &[u8]) -> (bool, u16, u16, usize) {
    let is_frag1 = match hdr[0] & lowpan_frag::FRAGN_HDR {
        lowpan_frag::FRAG1_HDR => true,
        // TODO: Error handling?
        _ => false,
    };
    // Zero out upper bits
    let dgram_size = slice_to_u16(&hdr[0..2]) & !(lowpan_frag::FRAGN_HDR as u16) << 8;
    let dgram_tag = slice_to_u16(&hdr[2..4]);
    let dgram_offset = if is_frag1 {
        0
    } else {
        hdr[4]
    };
    (is_frag1, dgram_size, dgram_tag, (dgram_offset as usize) * 8)
}

// TODO: Correct?
fn is_fragment(packet: &[u8]) -> bool {
    (packet[0] & lowpan_frag::FRAGN_HDR == lowpan_frag::FRAGN_HDR) 
        || (packet[0] & lowpan_frag::FRAG1_HDR == lowpan_frag::FRAG1_HDR)
}

pub struct LoWPANFragState <'a, R: Radio + 'a, C: ContextStore<'a> + 'a,
                            A: time::Alarm + 'a> {
    radio: &'a R,
    lowpan: &'a LoWPAN<'a, C>,
    alarm: &'a A,

    // Transmit state
    tx_packet: TakeCell<'static, [u8]>,
    tx_frag_buf: TakeCell<'static, [u8]>,
    tx_dst_mac_addr: Cell<MacAddr>,
    tx_source_long: Cell<bool>,
    tx_datagram_tag: Cell<u16>,
    tx_datagram_size: Cell<u16>,
    tx_datagram_offset: Cell<usize>,
    tx_busy: Cell<bool>,
 //   tx_abort: Cell<bool>,
    tx_client: Cell<Option<&'static TxClient>>,

    // Receive state
    rx_packet: TakeCell<'static, [u8]>,
    rx_src_mac_addr: Cell<MacAddr>,
    rx_dst_mac_addr: Cell<MacAddr>, // TODO: Needed?
    rx_datagram_tag: Cell<u16>,
    rx_datagram_size: Cell<u16>,
    rx_busy: Cell<bool>,
    //rx_abort: Cell<bool>,
    rx_client: Cell<Option<&'static RxClient>>,
}

/*
pub trait AbortReceive {
    fn rx_abort(&self) {
        self.tx_abort.set(true);
        // TODO: Clean up stuff
    }
}
*/

// TODO: how to id transmit?
/*
pub trait AbortTransmit {
    fn tx_abort(&self, 
}
*/

impl <'a, R: Radio, C: ContextStore<'a>, A: time::Alarm> TxClient for LoWPANFragState<'a, R, C, A> {
    // TODO: Handle abort bool, ReturnCode stuff
    fn send_done(&self, buf: &'static mut [u8], acked: bool, result: ReturnCode) {
        self.tx_frag_buf.replace(buf);
        if self.transmit_done() {
            let mut ret_buf = self.end_fragment_transmit().unwrap(); // TODO: Fix
            // TODO: Be careful here, as need to transmit next fragment stuff
            // before callback
            self.tx_client.get().map(move |client| client.send_done(ret_buf, acked, result));
        } else {
            self.prepare_transmit_fragment(self.tx_datagram_offset.get(), 
                                                   false).unwrap(); // TODO: Fix this
        }
    }
}

impl <'a, R: Radio, C: ContextStore<'a>, A: time::Alarm> RxClient for LoWPANFragState<'a, R, C, A> {
    // TODO: Assumes len includes 802.15.4 header
    fn receive(&self, buf: &'static mut [u8], len: u8, result: ReturnCode) {
        // TODO: Handle returncode
        // TODO: Fix payload_offset to take an arbitrary frame, and return
        // the desired offset.
        let offset = self.radio.payload_offset(false, false);
        // TODO: Impl
        let (src_mac_addr, dst_mac_addr) = (MacAddr::ShortAddr(0), MacAddr::ShortAddr(0)); //self.radio.get_mac_addrs();
        let retbuf = self.receive_packet(&buf[offset as usize..],
                                         len - offset as u8,
                                         src_mac_addr, dst_mac_addr);
        // Give the buffer back
        self.radio.set_receive_buffer(buf);
    }
}

// TODO: Need to implement config client?
impl <'a, R: Radio, C: ContextStore<'a>, A: time::Alarm> ConfigClient for LoWPANFragState<'a, R, C, A> {
    fn config_done(&self, result: ReturnCode) {
    }
}

// TODO: Should we have one timer that marks every rx context, or one timer for each
// rx context? The latter seems wasteful, the former inaccurate (since we are operating on the
// order of 60s)
impl <'a, R: Radio, C: ContextStore<'a>, A: time::Alarm> time::Client for LoWPANFragState<'a, R, C, A> {
    fn fired(&self) {
    }
}

impl <'a, R: Radio, C: ContextStore<'a>, A: time::Alarm> LoWPANFragState<'a, R, C, A> {
    pub fn new(radio: &'a R, lowpan: &'a LoWPAN<'a, C>, tx_frag_buf: &'static mut [u8],
               alarm: &'a A) -> LoWPANFragState<'a, R, C, A> {
        LoWPANFragState {
            radio: radio,
            lowpan: lowpan,
            alarm: alarm,

            tx_packet: TakeCell::empty(),
            tx_frag_buf: TakeCell::new(tx_frag_buf),
            tx_dst_mac_addr: Cell::new(MacAddr::ShortAddr(0)),
            tx_source_long: Cell::new(false),
            tx_datagram_tag: Cell::new(0),
            tx_datagram_size: Cell::new(0),
            tx_datagram_offset: Cell::new(0), // This should always be a multiple of 8
            tx_busy: Cell::new(false),
            tx_client: Cell::new(None), // TODO: Where to set tx_client?

            rx_packet: TakeCell::empty(),
            rx_src_mac_addr: Cell::new(MacAddr::ShortAddr(0)),
            rx_dst_mac_addr: Cell::new(MacAddr::ShortAddr(0)),
            rx_datagram_tag: Cell::new(0),
            rx_datagram_size: Cell::new(0),
            rx_busy: Cell::new(false),
            rx_client: Cell::new(None),
        }
    }

    // TODO: We assumed ip6_packet.len() == ip6_packet_len
    // TODO: Where to get src_mac_addr from?
    pub fn transmit_packet(&self,
                           dst_mac_addr: MacAddr,
                           src_mac_addr: MacAddr,
                           ip6_packet: &'static mut [u8],
                           source_long: bool,
                           fragment: bool) -> Result<ReturnCode, ()> {
        // If we're already transmitting, return
        // TODO: In this case, we perminantly loose the packet buffer
        if self.tx_busy.get() {
            return Ok(ReturnCode::EBUSY);
        }

        let mut tx_frag_buf = self.tx_frag_buf.take().ok_or(())?;
        // Here, we assume that the compressed headers fit in the first MTU
        // fragment. This is consistent with RFC 6282.
        let (consumed, written) = self.lowpan.compress(&ip6_packet,
                                                       src_mac_addr,
                                                       dst_mac_addr,
                                                       &mut tx_frag_buf)?;
        // This gives the remaining, uncompressed bytes of the packet
        let remaining = ip6_packet.len() - consumed;
        let dgram_size = ip6_packet.len();
        let lowpan_len = written + remaining;

        // We can transmit in a single frame
        if lowpan_len <= MAX_PAYLOAD_SIZE {
            // Copy over the remaining payload
            tx_frag_buf[written..written+remaining]
                .copy_from_slice(&ip6_packet[consumed..consumed+remaining]);
            self.tx_frag_buf.replace(tx_frag_buf);
            self.init_fragment_transmit(dst_mac_addr, ip6_packet, dgram_size,
                                        source_long);
            // Setting the offset makes it so the callback knows there are no
            // more pending frames.
            self.tx_datagram_offset.set(lowpan_len);
            self.transmit_frame((lowpan_len) as u8)
        // Otherwise, need to fragment
        } else if fragment {
            // Based on the IPv6 packet format, this must be a multiple of 8
            self.tx_frag_buf.replace(tx_frag_buf);
            self.init_fragment_transmit(dst_mac_addr, ip6_packet, dgram_size,
                                        source_long);
            // TODO: Confirm offset == consumed
            self.prepare_transmit_fragment(consumed, true)
        // Otherwise, cannot transmit as packet is too large
        } else {
            Ok(ReturnCode::ESIZE)
        }
    }

    fn init_fragment_transmit(&self,
                              dst_mac_addr: MacAddr,
                              packet: &'static mut [u8],
                              packet_len: usize,
                              source_long: bool) {

        let dgram_tag = self.tx_datagram_tag.get() + 1;
        // Apparently, a dgram_tag of 0 is invalid; therefore, we avoid
        // it here.
        self.tx_datagram_tag.set(
            if dgram_tag == 0 {
                1
            } else {
                dgram_tag
            });
        self.tx_datagram_tag.set(self.tx_datagram_tag.get() + 1);
        self.tx_dst_mac_addr.set(dst_mac_addr);
        self.tx_source_long.set(source_long);
        self.tx_packet.replace(packet);
        self.tx_datagram_size.set(packet_len as u16);
        self.tx_busy.set(true);
    }

    fn end_fragment_transmit(&self) -> Result<&'static mut [u8], ()> {
        self.tx_busy.set(false);
        self.tx_packet.take().ok_or(())
    }

    fn transmit_done(&self) -> bool {
        self.tx_datagram_size.get() as usize <= self.tx_datagram_offset.get()
    }

    // Assumptions about state: 1) If this is frag1, the field self.tx_datagram_offset == 0
    //
    // Note that this will fill in any remaining space with payload in the frag1 frame
    fn prepare_transmit_fragment(&self, offset: usize, is_frag1: bool) -> Result<ReturnCode, ()> {
        let header_size = if is_frag1 { lowpan_frag::FRAG1_HDR_SIZE } else { lowpan_frag::FRAGN_HDR_SIZE };

        // TODO: This should round max_payload_len down to the nearest mutiple of 8
        let max_payload_len = (MAX_PAYLOAD_SIZE - header_size) & !0b111;
        let payload_len = min(max_payload_len,
                              (self.tx_datagram_size.get() as usize) - offset);
        let mut frag_buf = self.tx_frag_buf.take().ok_or(())?;
        let mut packet = self.tx_packet.take().ok_or(())?;
        set_frag_hdr(self.tx_datagram_size.get(), self.tx_datagram_tag.get(),
                     self.tx_datagram_offset.get(), &mut frag_buf[0..5], is_frag1);
        frag_buf[header_size..payload_len+header_size]
            .copy_from_slice(&packet[offset..offset+payload_len]);

        self.tx_packet.replace(packet);
        self.tx_frag_buf.replace(frag_buf);

        // Update the offset to be used for the next fragment
        self.tx_datagram_offset.set(self.tx_datagram_offset.get() + payload_len);
        self.transmit_frame(payload_len as u8)
    }

    fn transmit_frame(&self, len: u8) -> Result<ReturnCode, ()> {
        let mut packet = self.tx_frag_buf.take().ok_or(())?;
        Ok(match self.tx_dst_mac_addr.get() {
            MacAddr::ShortAddr(addr)
                => self.radio.transmit(addr, packet,
                                       len, self.tx_source_long.get()),
            MacAddr::LongAddr(addr)
                => self.radio.transmit_long(addr, packet,
                                            len, self.tx_source_long.get()),
        })
    }

    fn receive_packet(&self,
                      packet: &[u8],
                      packet_len: u8,
                      src_mac_addr: MacAddr,
                      dst_mac_addr: MacAddr) {
        let is_frag = is_fragment(packet);
        if is_frag {
            let (is_frag1, dgram_size, dgram_tag, dgram_offset) = get_frag_hdr(&packet[0..5]);
            let offset_to_payload = if is_frag1 {
                lowpan_frag::FRAG1_HDR_SIZE
            } else {
                lowpan_frag::FRAGN_HDR_SIZE
            };
            self.receive_fragment(&packet[offset_to_payload..],
                                  packet_len - offset_to_payload as u8,
                                  src_mac_addr,
                                  dst_mac_addr,
                                  is_frag1,
                                  dgram_size,
                                  dgram_tag,
                                  dgram_offset);
        } else {
            //self.receive_single_packet();
        }
    }

    fn receive_fragment(&self,
                        frag_payload: &[u8],
                        payload_len: u8,
                        src_mac_addr: MacAddr,
                        dst_mac_addr: MacAddr,
                        is_frag1: bool,
                        dgram_size: u16,
                        dgram_tag: u16,
                        dgram_offset: usize) {
        if self.rx_busy.get() {
            if dgram_tag == self.rx_datagram_tag.get() /*&& 
                src_mac_addr == self.rx_src_mac_addr.get() &&
                dst_mac_addr == self.rx_dst_mac_addr.get()*/ {
                self.receive_next_fragment(frag_payload, payload_len, is_frag1,
                                           dgram_size, dgram_offset);
            } // If busy and did not match, discard packet
        } else {
            self.init_fragment_receive(src_mac_addr, dst_mac_addr, dgram_size,
                                       dgram_tag);
            self.receive_next_fragment(frag_payload, payload_len, is_frag1,
                                       dgram_size, dgram_offset);
        }
    }

    // Sets busy to false if received last packet(?) TODO
    fn receive_next_fragment(&self, payload: &[u8], payload_len: u8,
                             is_frag1: bool, dgram_size: u16, dgram_offset: usize) {
        // TODO: Implement

    }

    fn init_fragment_receive(&self, src_mac_addr: MacAddr, dst_mac_addr: MacAddr,
                             dgram_size: u16, dgram_tag: u16) {
        self.rx_src_mac_addr.set(src_mac_addr);
        self.rx_dst_mac_addr.set(dst_mac_addr);
        self.rx_datagram_tag.set(dgram_tag);
        self.rx_datagram_size.set(dgram_size);
        self.rx_busy.set(true);
    }

    fn end_fragment_receive(&self) {
        self.rx_busy.set(false);
    }
}
