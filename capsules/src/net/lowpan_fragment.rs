extern crate kernel;
use kernel::ReturnCode;
use kernel::common::take_cell::{TakeCell, MapCell};
use kernel::common::list::{List, ListLink, ListNode};
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

pub struct Bitmap {
    map: [u8; 20],
}

impl Bitmap {
    pub fn new() -> Bitmap {
        Bitmap {
            map: [0; 20]
        }
    }

    fn clear(&mut self) {
        for i in 0..self.map.len() {
            self.map[i] = 0;
        }
    }

    fn clear_bit(&mut self, idx: usize) {
        let map_idx = idx / 8;
        self.map[map_idx] &= !(1 << (idx % 8));
    }

    fn set_bit(&mut self, idx: usize) {
        let map_idx = idx / 8;
        self.map[map_idx] |= 1 << (idx % 8);
    }

    // Returns true if successfully set bits, returns false if the bits
    // overlapped with already set bits
    fn set_bits(&mut self, start_idx: usize, end_idx: usize) -> bool {
        if start_idx > end_idx {
            return false;
        }
        let start_map_idx = start_idx / 8;
        let end_map_idx = end_idx / 8;
        let first = 0xff << (start_idx % 8);
        let second = 0xff >> (end_idx % 8);
        // TODO: Confirm this is correct
        if start_map_idx == end_map_idx {
            let result = (self.map[start_map_idx] & (first & second)) == 0;
            self.map[start_map_idx] |= first & second;
            result
        } else {
            let mut result = (self.map[start_map_idx] & first) == 0;
            result = result && ((self.map[end_map_idx] & second) == 0);
            self.map[start_map_idx] |= first;
            self.map[end_map_idx] |= second;
            for i in 1..end_map_idx - start_map_idx - 1 {
                result = result && (self.map[i] == 0);
                self.map[i] = 0xff;
            }
            result
        }
    }

    fn is_complete(&self) -> bool {
        let mut result = true;
        for i in 0..20 {
            result = result && (self.map[i] == 0xff);
        }
        result
    }
}

pub struct TxState<'a> {
    packet: TakeCell<'static, [u8]>,
    src_mac_addr: Cell<MacAddr>,
    dst_mac_addr: Cell<MacAddr>,
    source_long: Cell<bool>,
    dgram_tag: Cell<u16>, // TODO: This can probably be elided
    dgram_size: Cell<u16>,
    dgram_offset: Cell<usize>,
    fragment: Cell<bool>,
    client: Cell<Option<&'static TxClient>>,

    next: ListLink<'a, TxState<'a>>,
}

impl<'a> ListNode<'a, TxState<'a>> for TxState<'a> {
    fn next(&'a self) -> &'a ListLink<TxState<'a>> {
        &self.next
    }
}

impl<'a> TxState<'a> {
    fn new() -> TxState<'a> {
        TxState {
            packet: TakeCell::empty(),
            src_mac_addr: Cell::new(MacAddr::ShortAddr(0)),
            dst_mac_addr: Cell::new(MacAddr::ShortAddr(0)),
            source_long: Cell::new(false),
            dgram_tag: Cell::new(0),
            dgram_size: Cell::new(0),
            dgram_offset: Cell::new(0),
            fragment: Cell::new(false),
            client: Cell::new(None),
            next: ListLink::empty(),
        }
    }

    fn is_transmit_done(&self) -> bool {
        self.dgram_size.get() as usize <= self.dgram_offset.get()
    }

    fn init_transmit(&self,
                     src_mac_addr: MacAddr,
                     dst_mac_addr: MacAddr,
                     packet: &'static mut [u8],
                     source_long: bool,
                     fragment: bool) {

        let packet_len = packet.len();
        self.src_mac_addr.set(src_mac_addr);
        self.dst_mac_addr.set(dst_mac_addr);
        self.source_long.set(source_long);
        self.fragment.set(fragment);
        self.packet.replace(packet);
        self.dgram_size.set(packet_len as u16);
    }

    // To cut down on the number of necessary buffers, we do compression here
    // Takes ownership of frag_buf and gives it to the radio
    fn start_transmit<'b, C: ContextStore<'b>>(&self,
                          dgram_tag: u16,
                          mut frag_buf: &'static mut [u8],
                          radio: &'b Radio,
                          lowpan: &'b LoWPAN<'b, C>) 
                          -> Result<ReturnCode, ReturnCode> {
        self.dgram_tag.set(dgram_tag);
        let ip6_packet = self.packet.take().ok_or(ReturnCode::ENOMEM)?;
        // Here, we assume that the compressed headers fit in the first MTU
        // fragment. This is consistent with RFC 6282.
        let mut lowpan_packet = [0 as u8; MAX_PAYLOAD_SIZE]; // TODO: Fix size
        let (consumed, written) = lowpan.compress(&ip6_packet,
                                                  self.src_mac_addr.get(),
                                                  self.dst_mac_addr.get(),
                                                  &mut lowpan_packet)
                                        .map_err(|_| ReturnCode::FAIL)?;
        // This gives the remaining, uncompressed bytes of the packet
        let remaining = ip6_packet.len() - consumed;
        let lowpan_len = written + remaining;

        // We can transmit in a single frame
        if lowpan_len <= MAX_PAYLOAD_SIZE {
            // Copy over the compressed header
            frag_buf[0..written].copy_from_slice(&lowpan_packet[0..written]);
            // Copy over the remaining payload
            frag_buf[written..written+remaining]
                .copy_from_slice(&ip6_packet[consumed..consumed+remaining]);
            // Setting the offset makes it so the callback knows there are no
            // more pending frames.
            self.dgram_offset.set(lowpan_len);
            self.transmit_frame(frag_buf, (lowpan_len) as u8, radio)
        // Otherwise, need to fragment
        } else if self.fragment.get() {
            // TODO: Confirm offset == consumed
            self.prepare_transmit_first_fragment(&lowpan_packet, frag_buf,
                                                 written, consumed, radio)
        // Otherwise, cannot transmit as packet is too large
        } else {
            Err(ReturnCode::ESIZE)
        }
    }

    // TODO: Should we copy over additional payload for frag1 as well?
    fn prepare_transmit_first_fragment(&self,
                                       lowpan_packet: &[u8],
                                       mut frag_buf: &'static mut [u8],
                                       lowpan_len: usize,
                                       offset: usize,
                                       radio: &Radio) 
                                        -> Result<ReturnCode, ReturnCode> {
        let (radio_header_len, max_payload_len) = /*radio.construct_header(..)*/ (0, 0);
        // This gives the offset to the start of the payload
        let header_len = lowpan_frag::FRAG1_HDR_SIZE + radio_header_len;
        // Assumes dgram_size and dgram_tag fields are properly set, and that
        // dgram_offset == 0
        set_frag_hdr(self.dgram_size.get(), self.dgram_tag.get(),
                     self.dgram_offset.get(), &mut frag_buf[radio_header_len..header_len], true);
        // Copy over the 'payload' (compressed lowpan header)
        frag_buf[header_len..header_len + lowpan_len]
            .copy_from_slice(&lowpan_packet[0..lowpan_len]);
        self.dgram_offset.set(offset);
        self.transmit_frame(frag_buf, (lowpan_len + header_len) as u8, radio)
    }

    fn prepare_transmit_next_fragment(&self,
                                      mut frag_buf: &'static mut [u8],
                                      radio: &Radio) -> Result<ReturnCode, ReturnCode> {
        let (radio_header_len, max_payload_len) = /*radio.construct_header(..)*/ (0, 0);
        // This gives the offset to the start of the payload
        let header_len = lowpan_frag::FRAGN_HDR_SIZE + radio_header_len;
        let dgram_offset = self.dgram_offset.get();

        // This rounds payload_len down to the nearest multiple of 8
        let payload_len = min(max_payload_len,
                              (self.dgram_size.get() as usize) - dgram_offset) & !0b111;

        let mut packet = self.packet.take().ok_or(ReturnCode::ENOMEM)?;
        set_frag_hdr(self.dgram_size.get(), self.dgram_tag.get(),
                     dgram_offset, &mut frag_buf[radio_header_len..header_len], false);
        frag_buf[header_len..header_len + payload_len]
            .copy_from_slice(&packet[dgram_offset..dgram_offset+payload_len]);
        self.packet.replace(packet);

        // Update the offset to be used for the next fragment
        self.dgram_offset.set(dgram_offset + payload_len);
        // TODO: Include full header_size here? Or only lowpan_frag::FRAGN_HDR_SIZE?
        self.transmit_frame(frag_buf, (header_len + payload_len) as u8, radio)
    }

    fn transmit_frame(&self, mut frame: &'static mut [u8], len: u8,
                          radio: &Radio) -> Result<ReturnCode, ReturnCode> {
        let res = match self.dst_mac_addr.get() {
            MacAddr::ShortAddr(addr)
                => radio.transmit(addr, frame,
                                  len, self.source_long.get()),
            MacAddr::LongAddr(addr)
                => radio.transmit_long(addr, frame,
                                       len, self.source_long.get()),
        };
        match res {
            ReturnCode::SUCCESS => Ok(res),
            _                   => Err(res),
        }
    }

    fn end_transmit(&self) -> Result<&'static mut [u8], ReturnCode> {
        self.packet.take().ok_or(ReturnCode::ENOMEM)
    }
}

pub struct RxState<'a> {
    packet: TakeCell<'static, [u8]>,
    bitmap: MapCell<Bitmap>,
    dst_mac_addr: Cell<MacAddr>,
    src_mac_addr: Cell<MacAddr>,
    dgram_tag: Cell<u16>,
    dgram_size: Cell<u16>,
    client: Cell<Option<&'static RxClient>>,
    busy: Cell<bool>,
    timeout_counter: Cell<usize>,

    next: ListLink<'a, RxState<'a>>,
}

impl<'a> ListNode<'a, RxState<'a>> for RxState<'a> {
    fn next(&'a self) -> &'a ListLink<RxState<'a>> {
        &self.next
    }
}

impl<'a> RxState<'a> {
    fn new(packet: &'static mut [u8]) -> RxState<'a> {
        RxState {
            packet: TakeCell::new(packet),
            bitmap: MapCell::new(Bitmap::new()),
            dst_mac_addr: Cell::new(MacAddr::ShortAddr(0)),
            src_mac_addr: Cell::new(MacAddr::ShortAddr(0)),
            dgram_tag: Cell::new(0),
            dgram_size: Cell::new(0),
            client: Cell::new(None), //TODO: Do individual clients make sense?
            busy: Cell::new(false),
            timeout_counter: Cell::new(0),
            next: ListLink::empty(),
        }
    }

    fn is_my_fragment(&self, src_mac_addr: MacAddr, dst_mac_addr: MacAddr,
                      dgram_size: u16, dgram_tag: u16) -> bool {
        self.busy.get() && self.dgram_tag.get() == dgram_tag
            && self.dgram_size.get() == dgram_size
            //&& TODO: Mac addrs match
    }

    fn start_receive(&self, src_mac_addr: MacAddr, dst_mac_addr: MacAddr,
                     dgram_size: u16, dgram_tag: u16) {
        self.dst_mac_addr.set(dst_mac_addr);
        self.src_mac_addr.set(src_mac_addr);
        self.dgram_tag.set(dgram_tag);
        self.dgram_size.set(dgram_size);
        self.busy.set(true);
    }

    // TODO: Assumes payload a slice starting from the actual payload
    // (no 802.15.4 headers, no fragmentation headers)
    // Returns true if the packet is completely reassembled
    fn receive_next_frame(&self,
                          payload: &[u8],
                          payload_len: usize,
                          dgram_offset: usize) -> bool {
        // TODO: Unwrap
        let mut packet = self.packet.take().unwrap();
        packet[dgram_offset..dgram_offset+payload_len]
            .copy_from_slice(&payload[0..payload_len]);
        self.packet.replace(packet);
        if !self.bitmap
            .map(|bitmap| bitmap.set_bits(dgram_offset, dgram_offset+payload_len))
            .unwrap() {
            // If this fails, we found an overlapping fragment; reset
            // everything minus this fragment
            // TODO
        }
        self.bitmap.map(|bitmap| bitmap.is_complete()).unwrap()
    }

    fn end_receive(&self) -> Option<&'static mut [u8]> {
        self.busy.set(false);
        self.packet.take()
    }
}

pub struct LoWPANFragState <'a, R: Radio + 'a, C: ContextStore<'a> + 'a,
                            A: time::Alarm + 'a> {
    radio: &'a R,
    lowpan: &'a LoWPAN<'a, C>,
    alarm: &'a A,

    // Transmit state
    tx_states: List<'a, TxState<'a>>,
    //tx_state: MapCell<TxState>,
    tx_dgram_tag: Cell<u16>,
    tx_busy: Cell<bool>,
    tx_buf: TakeCell<'static, [u8]>,

    // Receive state
    rx_states: List<'a, RxState<'a>>,

    //rx_abort: Cell<bool>,
    rx_client: Cell<Option<&'static RxClient>>,
}

impl <'a, R: Radio, C: ContextStore<'a>, A: time::Alarm> TxClient for LoWPANFragState<'a, R, C, A> {
    // TODO: Handle abort bool, ReturnCode stuff
    fn send_done(&self, buf: &'static mut [u8], acked: bool, result: ReturnCode) {
        self.tx_buf.replace(buf);
        // If we are done
        // TODO: Fix unwraps
        //if self.tx_state.map(|state| state.is_transmit_done()).unwrap() {
        if self.tx_states.head().unwrap().is_transmit_done() {
            let mut ret_buf = self.end_fragment_transmit().unwrap();
            // TODO: Be careful here, as need to transmit next fragment stuff
            // before callback
            // TODO: Need to pass the tx_state struct back as well
            // if exists pending tx_state:
            //      self.tx_state.replace(pending state)
            //      self.tx_state.map(|state| state.start_transmit(..)
            //TODO: Callback
            //self.tx_state.get().map(move |client| client.send_done(ret_buf, acked, result));
        } else {
            // TODO: Handle returncode
            let tx_buf = self.tx_buf.take().unwrap();
            let mut retval = self.tx_states.head()
                .unwrap().prepare_transmit_next_fragment(tx_buf, self.radio)
                .unwrap_or(ReturnCode::FAIL);
            // TODO: Call abort
        }
    }
}

impl <'a, R: Radio, C: ContextStore<'a>, A: time::Alarm> RxClient for LoWPANFragState<'a, R, C, A> {
    // TODO: Assumes len includes 802.15.4 header
    // TODO: Handle returncode
    // TODO: Give buffer back
    fn receive(&self, buf: &'static mut [u8], len: u8, result: ReturnCode) {
        // TODO: Generalize this
        let offset = self.radio.payload_offset(false, false);
        // TODO: Impl
        let (src_mac_addr, dst_mac_addr) = (MacAddr::ShortAddr(0), MacAddr::ShortAddr(0)); 
        //self.radio.get_mac_addrs();
        let retbuf = self.receive_packet(&buf[offset as usize..],
                                         len - offset as u8,
                                         src_mac_addr, dst_mac_addr);
        // Give the buffer back
        self.radio.set_receive_buffer(buf);
    }
}

// TODO: Need to implement config client?
/*
impl <'a, R: Radio, C: ContextStore<'a>, A: time::Alarm> ConfigClient for LoWPANFragState<'a, R, C, A> {
    fn config_done(&self, result: ReturnCode) {
    }
}
*/

// TODO: Should we have one timer that marks every rx context, or one timer for each
// rx context? The latter seems wasteful, the former inaccurate (since we are operating on the
// order of 60s)
impl <'a, R: Radio, C: ContextStore<'a>, A: time::Alarm> time::Client for LoWPANFragState<'a, R, C, A> {
    fn fired(&self) {
    }
}

impl <'a, R: Radio, C: ContextStore<'a>, A: time::Alarm> LoWPANFragState<'a, R, C, A> {
    pub fn new(radio: &'a R, lowpan: &'a LoWPAN<'a, C>, tx_buf: &'static mut [u8],
               alarm: &'a A) -> LoWPANFragState<'a, R, C, A> {
        LoWPANFragState {
            radio: radio,
            lowpan: lowpan,
            alarm: alarm,

            tx_states: List::new(),
            //tx_state: MapCell::new(TxState::new()),
            tx_dgram_tag: Cell::new(0),
            tx_busy: Cell::new(false), // TODO: This can be elided if we can 
                                       // remove elements from the tx_states
                                       // list, and check if busy by seeing if
                                       // list is empty.
            tx_buf: TakeCell::new(tx_buf),

            rx_states: List::new(),
            rx_client: Cell::new(None),
        }
    }

    // TODO: We assume ip6_packet.len() == ip6_packet_len
    // TODO: Where to get src_mac_addr from?
    // TODO: Need to keep track of additional state: encryption bool, etc.
    pub fn transmit_packet(&self,
                           src_mac_addr: MacAddr,
                           dst_mac_addr: MacAddr,
                           ip6_packet: &'static mut [u8],
                           tx_state: &'a TxState<'a>,
                           source_long: bool,
                           fragment: bool) -> Result<ReturnCode, ReturnCode> {

        tx_state.init_transmit(src_mac_addr, dst_mac_addr, ip6_packet, 
                               source_long, fragment);
        if self.tx_busy.get() {
            // Queue tx_state
            // TODO: Need to enqueue this element as not the head element
            self.tx_states.push_head(tx_state);
            Ok(ReturnCode::SUCCESS)
        } else {
            // Set as current state and start transmit
            self.tx_states.push_head(tx_state);
            //self.tx_state.replace(tx_state);
            self.start_packet_transmit()
        }
    }

    fn start_packet_transmit(&self) -> Result<ReturnCode, ReturnCode> {
        let mut frag_buf = self.tx_buf.take().ok_or(ReturnCode::ENOMEM)?;
        self.tx_busy.set(true);
        // Apparently, a dgram_tag of 0 is invalid; therefore, we avoid it
        let dgram_tag = self.tx_dgram_tag.get() + 1;
        self.tx_dgram_tag.set( if dgram_tag == 0 { 1 } else { dgram_tag });
        self.tx_states.head().unwrap().start_transmit(dgram_tag, frag_buf,
                                                      self.radio, self.lowpan)
    }

    fn end_fragment_transmit(&self) -> Result<&'static mut [u8], ReturnCode> {
        self.tx_busy.set(false);
        self.tx_states.head().ok_or(ReturnCode::ENOMEM)?.end_transmit()
    }

    // TODO: Rename
    fn receive_packet(&self,
                      packet: &[u8],
                      packet_len: u8,
                      src_mac_addr: MacAddr,
                      dst_mac_addr: MacAddr) {
        if is_fragment(packet) {
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
                                  dgram_size,
                                  dgram_tag,
                                  dgram_offset);
        } else {
            //self.receive_single_packet();
        }
    }

    // TODO: Bounds check everything
    fn receive_fragment(&self,
                        frag_payload: &[u8],
                        payload_len: u8,
                        src_mac_addr: MacAddr,
                        dst_mac_addr: MacAddr,
                        dgram_size: u16,
                        dgram_tag: u16,
                        dgram_offset: usize) -> Option<&'static mut [u8]>{
        // TODO: Error handling
        let mut rx_state = self.rx_states
            .iter().find(|state| state.is_my_fragment(src_mac_addr,
                                                      dst_mac_addr,
                                                      dgram_tag,
                                                      dgram_size));
        if rx_state.is_none() {
            rx_state = self.rx_states.iter().find(|state| !state.busy.get());
            // Initialize new state
            rx_state.map(|state| state.start_receive(src_mac_addr, dst_mac_addr,
                                                     dgram_size, dgram_tag));
            if rx_state.is_none() {
                // TODO: Error
            }
        }
        rx_state.map(|state| {
            // Returns true if the full packet is reassembled
            if state.receive_next_frame(frag_payload,
                                        payload_len as usize,
                                        dgram_offset) {
                state.end_receive()
            } else {
                None
            }
        }).unwrap_or(None)
    }
}
