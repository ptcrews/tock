extern crate kernel;
use kernel::{Callback, ReturnCode};
use kernel::common::take_cell::{TakeCell, MapCell};
use kernel::hil::radio::{Radio, TxClient, RxClient, ConfigClient};
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

pub struct LoWPANFragState <'a, R: Radio + 'a, C: ContextStore<'a> + 'a> {
    radio: &'a R,
    lowpan: &'a LoWPAN<'a, C>,

    // Transmit state
    tx_packet: TakeCell<'static, [u8]>,
    tx_frag_buf: TakeCell<'static, [u8]>,
    tx_dst_mac_addr: Cell<MacAddr>,
    tx_source_long: Cell<bool>,
    tx_datagram_tag: Cell<u16>,
    tx_datagram_size: Cell<u16>,
    tx_datagram_offset: Cell<usize>,
    tx_busy: Cell<bool>,
    tx_is_fragment: Cell<bool>,
    tx_client: Cell<Option<&'static TxClient>>,

    // Receive state
    rx_packet: TakeCell<'static, [u8]>,
    rx_src_mac_addr: Cell<MacAddr>,
    rx_dst_mac_addr: Cell<MacAddr>, // TODO: Needed?
    rx_datagram_tag: Cell<u16>,
    rx_datagram_size: Cell<u16>,
    rx_busy: Cell<bool>,
    rx_client: Cell<Option<&'static RxClient>>,
}

impl <'a, R: Radio, C: ContextStore<'a>> TxClient for LoWPANFragState<'a, R, C> {
    fn send_done(&self, buf: &'static mut [u8], acked: bool, result: ReturnCode) {
        self.tx_frag_buf.replace(buf);
        let packet_len = self.prepare_fragment(false);
        // TODO: Returncode handling
        // We have transmitted all the fragments of the packet or there are no
        // fragments
        if packet_len == 0 || !self.tx_is_fragment.get() {
            let mut ret_buf = self.end_fragment_transmit();
            // TODO: Transmit done
            self.tx_client.get().map(move |client| client.send_done(ret_buf, acked, result));
        }
        self.transmit_fragment(packet_len);
    }
}

impl <'a, R: Radio, C: ContextStore<'a>> RxClient for LoWPANFragState<'a, R, C> {
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
impl <'a, R: Radio, C: ContextStore<'a>> ConfigClient for LoWPANFragState<'a, R, C> {
    fn config_done(&self, result: ReturnCode) {
    }
}

impl <'a, R: Radio, C: ContextStore<'a>> LoWPANFragState<'a, R, C> {
    pub fn new(radio: &'a R, lowpan: &'a LoWPAN<'a, C>, tx_frag_buf: &'static mut [u8]) -> LoWPANFragState<'a, R, C> {
        LoWPANFragState {
            radio: radio,
            lowpan: lowpan,

            tx_packet: TakeCell::empty(),
            tx_frag_buf: TakeCell::new(tx_frag_buf),
            tx_dst_mac_addr: Cell::new(MacAddr::ShortAddr(0)),
            tx_source_long: Cell::new(false),
            tx_datagram_tag: Cell::new(0),
            tx_datagram_size: Cell::new(0),
            tx_datagram_offset: Cell::new(0),
            tx_busy: Cell::new(false),
            tx_is_fragment: Cell::new(false),
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
                           source_long: bool) -> Result<ReturnCode, ()> {
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

        // We can transmit in a single frame
        if remaining + written <= MAX_PAYLOAD_SIZE {
            // Copy over the remaining payload
            tx_frag_buf[written..written+remaining]
                .copy_from_slice(&ip6_packet[consumed..consumed+remaining]);
            self.tx_frag_buf.replace(tx_frag_buf);
            self.tx_is_fragment.set(false);
            Ok(self.transmit_fragment((remaining+written) as u8))
        // Otherwise, need to fragment
        } else {
            // Based on the IPv6 packet format, this must be a multiple of 8
            // TODO: Confirm this is the value for the second offset
            let dgram_offset = ((consumed + remaining) / 8) as u8; 
            let dgram_size = ip6_packet.len();
            self.tx_frag_buf.replace(tx_frag_buf);
            self.init_fragment_transmit(dst_mac_addr, ip6_packet, dgram_size,
                                        written, source_long);
            let frag_len = self.prepare_fragment(true);
            Ok(self.transmit_fragment(frag_len))
        }

    }

    fn init_fragment_transmit(&self,
                              dst_mac_addr: MacAddr,
                              packet: &'static mut [u8],
                              ip6_payload_len: usize,
                              initial_offset: usize,
                              source_long: bool) {

        self.tx_datagram_tag.set(self.tx_datagram_tag.get() + 1);
        self.tx_dst_mac_addr.set(dst_mac_addr);
        self.tx_source_long.set(source_long);
        self.tx_packet.replace(packet);
        self.tx_datagram_size.set(ip6_payload_len as u16);
        self.tx_datagram_offset.set(initial_offset);
        self.tx_busy.set(true);
        self.tx_is_fragment.set(true);

        // TODO: Remove comment
        // Note that we cannot actually change the radio transmit client
        // at this point, as the radio requires a static allocation, but the
        // compiler can't know that this will (eventually) be part of a static
        // allocation.
        //self.radio.set_transmit_client(self);
    }

    fn end_fragment_transmit(&self) -> &'static mut [u8] {
        self.tx_busy.set(false);
        self.tx_is_fragment.set(false);
        self.tx_packet.take().unwrap()
    }

    // Returns length of fragment or 0 if no remaining fragments
    fn prepare_fragment(&self, is_frag1: bool) -> u8 {
        // Offset in bytes; if this is the first fragment, **must** be zero
        let bytes_offset = if is_frag1 {
            0
        } else {
            self.tx_datagram_offset.get()
        };

        // All fragments sent
        if self.tx_datagram_size.get() <= bytes_offset as u16 {
            return 0;
        }
        let header_size = if is_frag1 {
            lowpan_frag::FRAG1_HDR_SIZE
        } else {
            lowpan_frag::FRAGN_HDR_SIZE
        };

        // TODO: This should round max_payload_len down to the nearest mutiple of 8
        let max_payload_len = (MAX_PAYLOAD_SIZE - header_size) & !0b111;
        let payload_len = min(max_payload_len,
                              (self.tx_datagram_size.get() as usize) - bytes_offset);

        // Check that tx_frag_buf is valid
        let mut frag_buf = self.tx_frag_buf.take().unwrap();
        let mut packet = self.tx_packet.take().unwrap();

        set_frag_hdr(self.tx_datagram_size.get(), self.tx_datagram_tag.get(),
                     self.tx_datagram_offset.get(), &mut frag_buf[0..5], is_frag1);
        frag_buf[0..payload_len].copy_from_slice(&packet[bytes_offset..bytes_offset+payload_len]);

        self.tx_packet.replace(packet);
        self.tx_frag_buf.replace(frag_buf);

        // Update the offset (in multiples of 8, to be used for the next frag)
        self.tx_datagram_offset.set(self.tx_datagram_offset.get() + payload_len);
        payload_len as u8

    }

    fn transmit_fragment(&self, len: u8) -> ReturnCode {
        let mut packet = self.tx_frag_buf.take().unwrap();
        match self.tx_dst_mac_addr.get() {
            MacAddr::ShortAddr(addr)
                => self.radio.transmit(addr, packet,
                                       len, self.tx_source_long.get()),
            MacAddr::LongAddr(addr)
                => self.radio.transmit_long(addr, packet,
                                            len, self.tx_source_long.get()),
        }
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
    // TODO: Handle dgram_size elision
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
