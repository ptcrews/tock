extern crate kernel;
use kernel::ReturnCode;
use kernel::common::take_cell::{TakeCell, MapCell};
use kernel::common::list::{List, ListLink, ListNode};
//use kernel::hil::radio::{TxClient, RxClient/*, ConfigClient */};
use kernel::hil::radio;
use kernel::hil::time;
use kernel::hil::time::Frequency;
use core::cell::Cell;
use core::cmp::min;
use net::lowpan::{LoWPAN, ContextStore, is_lowpan};
use net::util::{slice_to_u16, u16_to_slice};
use net::frag_utils::Bitmap;
use net::ieee802154::{PanID, MacAddress, SecurityLevel, KeyId, Header};
use mac::{Mac, FrameInfo, TxClient, RxClient};

// Timer fire rate in seconds
const TIMER_RATE: usize = 10;
// Reassembly timeout in seconds
const FRAG_TIMEOUT: usize = 60;

pub trait ReceiveClient {
    fn receive(&self, buf: &'static mut [u8], len: u16, result: ReturnCode)
        -> &'static mut [u8];
}

pub trait TransmitClient {
    fn send_done(&self, buf: &'static mut [u8], state: &TxState, acked: bool, result: ReturnCode);
}

pub mod lowpan_frag {
    pub const FRAGN_HDR: u8 = 0b11100000;
    pub const FRAG1_HDR: u8 = 0b11000000;
    pub const FRAG1_HDR_SIZE: usize = 4;
    pub const FRAGN_HDR_SIZE: usize = 5;
}

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

fn get_frag_hdr(hdr: &[u8]) -> (bool, u16, u16, usize) {
    let is_frag1 = match hdr[0] & lowpan_frag::FRAGN_HDR {
        lowpan_frag::FRAG1_HDR => true,
        // TODO: Error handling?
        _ => false,
    };
    // Zero out upper bits
    let dgram_size = slice_to_u16(&hdr[0..2]) & !(0xf << 12);
    let dgram_tag = slice_to_u16(&hdr[2..4]);
    let dgram_offset = if is_frag1 {
        0
    } else {
        hdr[4]
    };
    (is_frag1, dgram_size, dgram_tag, (dgram_offset as usize) * 8)
}

fn is_fragment(packet: &[u8]) -> bool {
    let mask = packet[0] & lowpan_frag::FRAGN_HDR;
    (mask == lowpan_frag::FRAGN_HDR) || (mask == lowpan_frag::FRAG1_HDR)
}

pub struct TxState<'a> {
    packet: TakeCell<'static, [u8]>,
    src_pan: Cell<PanID>,
    dst_pan: Cell<PanID>,
    src_mac_addr: Cell<MacAddress>,
    dst_mac_addr: Cell<MacAddress>,
    source_long: Cell<bool>,
    security: Cell<Option<(SecurityLevel, KeyId)>>,
    dgram_tag: Cell<u16>,
    dgram_size: Cell<u16>,
    dgram_offset: Cell<usize>,
    fragment: Cell<bool>,
    client: Cell<Option<&'static TransmitClient>>,

    next: ListLink<'a, TxState<'a>>,
}

impl<'a> ListNode<'a, TxState<'a>> for TxState<'a> {
    fn next(&'a self) -> &'a ListLink<TxState<'a>> {
        &self.next
    }
}

impl<'a> TxState<'a> {
    pub fn new() -> TxState<'a> {
        TxState {
            packet: TakeCell::empty(),
            src_pan: Cell::new(0),
            dst_pan: Cell::new(0),
            src_mac_addr: Cell::new(MacAddress::Short(0)),
            dst_mac_addr: Cell::new(MacAddress::Short(0)),
            source_long: Cell::new(false),
            security: Cell::new(None),
            dgram_tag: Cell::new(0),
            dgram_size: Cell::new(0),
            dgram_offset: Cell::new(0),
            fragment: Cell::new(false),
            client: Cell::new(None),
            next: ListLink::empty(),
        }
    }

    pub fn set_transmit_client(&self, client: &'static TransmitClient) {
        self.client.set(Some(client));
    }

    fn is_transmit_done(&self) -> bool {
        self.dgram_size.get() as usize <= self.dgram_offset.get()
    }

    fn init_transmit(&self,
                     src_mac_addr: MacAddress,
                     dst_mac_addr: MacAddress,
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

    // Takes ownership of frag_buf and gives it to the radio
    fn start_transmit<'b, C: ContextStore<'b>>(&self,
                          dgram_tag: u16,
                          mut frag_buf: &'static mut [u8],
                          radio: &'b Mac,
                          lowpan: &'b LoWPAN<'b, C>) 
                          -> Result<ReturnCode,
                          (ReturnCode, &'static mut [u8])> {
        self.dgram_tag.set(dgram_tag);
        let ip6_packet_option = self.packet.take();
        if ip6_packet_option.is_none() {
            return Err((ReturnCode::ENOMEM, frag_buf));
        }
        let ip6_packet = ip6_packet_option.unwrap();
        let frame_info = radio.prepare_data_frame(frag_buf,
                                                      self.dst_pan.get(),
                                                      self.dst_mac_addr.get(),
                                                      self.src_pan.get(),
                                                      self.src_mac_addr.get(),
                                                      self.security.get());
        if frame_info.is_err() {
            return Err((ReturnCode::FAIL, frag_buf));
        }
        let result = self.prepare_transmit_first_fragment(frag_buf,
                                                          ip6_packet,
                                                          frame_info.unwrap(),
                                                          radio,
                                                          lowpan);
        self.packet.replace(ip6_packet);
        result
    }

    fn prepare_transmit_first_fragment<'b, C: ContextStore<'b>>(&self,
                                       mut frag_buf: &'static mut [u8],
                                       ip6_packet: &[u8],
                                       mut frame_info: FrameInfo,
                                       radio: &'b Mac,
                                       lowpan: &'b LoWPAN<'b, C>)
                                       -> Result<ReturnCode,
                                       (ReturnCode, &'static mut [u8])>{

        // Here, we assume that the compressed headers fit in the first MTU
        // fragment. This is consistent with RFC 6282.
        let mut lowpan_packet = [0 as u8; radio::MAX_FRAME_SIZE as usize];
        let lowpan_result = lowpan.compress(&ip6_packet,
                                                  self.src_mac_addr.get(),
                                                  self.dst_mac_addr.get(),
                                                  &mut lowpan_packet);
        if lowpan_result.is_err() {
            return Err((ReturnCode::FAIL, frag_buf));
        }
        let (consumed, written) = lowpan_result.unwrap();
        let remaining_payload = ip6_packet.len() - consumed;
        let lowpan_len = written + remaining_payload;
        let mut remaining_capacity = frame_info.remaining_data_capacity(frag_buf);

        // Unable to fragment and packet too large
        if !self.fragment.get() && (lowpan_len > remaining_capacity) {
            return Err((ReturnCode::ESIZE, frag_buf));
        }

        // Need to fragment
        if self.fragment.get() && (lowpan_len <= remaining_capacity) {
            let mut frag_header = [0 as u8; lowpan_frag::FRAG1_HDR_SIZE];
            set_frag_hdr(self.dgram_size.get(), self.dgram_tag.get(),
                /*offset = */ 0, &mut frag_header, true);
            frame_info.append_payload(frag_buf, &frag_header[..lowpan_frag::FRAG1_HDR_SIZE]);
            remaining_capacity -= lowpan_frag::FRAG1_HDR_SIZE;
        }

        // Write the 6lowpan header
        if written <= remaining_capacity {
            frame_info.append_payload(frag_buf, &lowpan_packet[0..written]);
            remaining_capacity -= written;
        } else {
            return Err((ReturnCode::ESIZE, frag_buf));
        }

        // Write the remainder of the payload
        frame_info.append_payload(frag_buf, &ip6_packet[consumed..consumed+remaining_capacity]);
        self.dgram_offset.set(consumed+remaining_capacity);
        let (result, buf) = radio.transmit(frag_buf, frame_info);
        Ok(ReturnCode::SUCCESS)
    }

    fn prepare_transmit_next_fragment(&self,
                                      mut frag_buf: &'static mut [u8],
                                      radio: &Mac) -> Result<ReturnCode, ReturnCode> {
        let mut frame_info = radio.prepare_data_frame(frag_buf,
                                                  self.dst_pan.get(),
                                                  self.dst_mac_addr.get(),
                                                  self.src_pan.get(),
                                                  self.src_mac_addr.get(),
                                                  self.security.get())
            .map_err(|_| ReturnCode::FAIL)?;
        let dgram_offset = self.dgram_offset.get();
        let remaining_capacity = frame_info.remaining_data_capacity(frag_buf)
            - lowpan_frag::FRAGN_HDR_SIZE;
        // This rounds payload_len down to the nearest multiple of 8
        let payload_len = min(remaining_capacity, (self.dgram_size.get() as usize)
                              - dgram_offset) & !0b111;

        let mut packet = self.packet.take().ok_or(ReturnCode::ENOMEM)?;
        let mut frag_header = [0 as u8; lowpan_frag::FRAGN_HDR_SIZE];
        set_frag_hdr(self.dgram_size.get(), self.dgram_tag.get(),
                     dgram_offset, &mut frag_header, false);
        frame_info.append_payload(frag_buf, &frag_header);
        frame_info.append_payload(frag_buf, &packet[dgram_offset..dgram_offset+payload_len]);
        self.packet.replace(packet);

        // Update the offset to be used for the next fragment
        self.dgram_offset.set(dgram_offset + payload_len);
        let (result, buf) = radio.transmit(frag_buf, frame_info);
        Ok(ReturnCode::SUCCESS)
    }

    fn end_transmit(&self, acked: bool, result: ReturnCode) {
        // TODO: Error handling
        let mut packet = self.packet.take().unwrap();
        // Note that if a null client is valid, then we lose the packet buffer
        self.client.get().map(move |client|
                              client.send_done(packet, self, acked, result));
    }
}

pub struct RxState<'a> {
    packet: TakeCell<'static, [u8]>,
    bitmap: MapCell<Bitmap>,
    dst_mac_addr: Cell<MacAddress>,
    src_mac_addr: Cell<MacAddress>,
    dgram_tag: Cell<u16>,
    dgram_size: Cell<u16>,
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
    pub fn new(packet: &'static mut [u8]) -> RxState<'a> {
        RxState {
            packet: TakeCell::new(packet),
            bitmap: MapCell::new(Bitmap::new()),
            dst_mac_addr: Cell::new(MacAddress::Short(0)),
            src_mac_addr: Cell::new(MacAddress::Short(0)),
            dgram_tag: Cell::new(0),
            dgram_size: Cell::new(0),
            busy: Cell::new(false),
            timeout_counter: Cell::new(0),
            next: ListLink::empty(),
        }
    }

    fn is_my_fragment(&self, src_mac_addr: MacAddress, dst_mac_addr: MacAddress,
                      dgram_size: u16, dgram_tag: u16) -> bool {
        self.busy.get() && (self.dgram_tag.get() == dgram_tag)
            && (self.dgram_size.get() == dgram_size)
            && (self.src_mac_addr.get() == src_mac_addr)
            && (self.dst_mac_addr.get() == dst_mac_addr)
    }

    fn start_receive(&self, src_mac_addr: MacAddress, dst_mac_addr: MacAddress,
                     dgram_size: u16, dgram_tag: u16) {
        self.dst_mac_addr.set(dst_mac_addr);
        self.src_mac_addr.set(src_mac_addr);
        self.dgram_tag.set(dgram_tag);
        self.dgram_size.set(dgram_size);
        self.busy.set(true);
        self.bitmap.map(|bitmap| bitmap.clear());
        self.timeout_counter.set(0);
    }

    // This function assumes that the payload is a slice starting from the
    // actual payload (no 802.15.4 headers, no fragmentation headers), and
    // returns true if the packet is completely reassembled.
    fn receive_next_frame<'b, C: ContextStore<'b>>(&self,
                          payload: &[u8],
                          payload_len: usize,
                          dgram_size: u16,
                          dgram_offset: usize,
                          lowpan: &'b LoWPAN<'b, C>) -> Result<bool, ReturnCode> {
        let mut packet = self.packet.take().ok_or(ReturnCode::ENOMEM)?;
        let uncompressed_len = if dgram_offset == 0 {
            let (consumed, written) = lowpan.decompress(&payload[0..payload_len as usize],
                                                        self.src_mac_addr.get(),
                                                        self.dst_mac_addr.get(),
                                                        &mut packet,
                                                        dgram_size,
                                                        true)
                                     .map_err(|_| ReturnCode::FAIL)?;
            let remaining = payload_len - consumed;
            packet[written..written+remaining]
                .copy_from_slice(&payload[consumed..consumed+remaining]);
            written+remaining
                
        } else {
            packet[dgram_offset..dgram_offset+payload_len]
                .copy_from_slice(&payload[0..payload_len]);
            payload_len
        };
        self.packet.replace(packet);
        if !self.bitmap
            .map(|bitmap| bitmap.set_bits(dgram_offset / 8, (dgram_offset+uncompressed_len) / 8))
            .ok_or(ReturnCode::FAIL)? {
            // If this fails, we received an overlapping fragment. We can simply
            // drop the packet in this case.
            Err(ReturnCode::FAIL)
        } else {
            self.bitmap.map(|bitmap| bitmap.is_complete((dgram_size as usize) / 8))
                .ok_or(ReturnCode::FAIL)
        }
    }

    fn end_receive(&self, client: Option<&'static ReceiveClient>, result: ReturnCode) {
        self.busy.set(false);
        self.bitmap.map(|bitmap| bitmap.clear());
        self.timeout_counter.set(0);
        if client.is_some() {
            let mut buffer = self.packet.take().unwrap();
            self.packet.replace(
                client.unwrap().receive(buffer, self.dgram_size.get(), result)
            );
        }
    }
}

pub struct FragState <'a, R: Mac + 'a, C: ContextStore<'a> + 'a,
                            A: time::Alarm + 'a> {
    pub radio: &'a R,
    lowpan: &'a LoWPAN<'a, C>,
    alarm: &'a A,

    // Transmit state
    tx_states: List<'a, TxState<'a>>,
    tx_dgram_tag: Cell<u16>,
    tx_busy: Cell<bool>,
    tx_buf: TakeCell<'static, [u8]>,

    // Receive state
    rx_states: List<'a, RxState<'a>>,
    rx_client: Cell<Option<&'static ReceiveClient>>,
}

impl <'a, R: Mac, C: ContextStore<'a>, A: time::Alarm> TxClient for FragState<'a, R, C, A> {
    fn send_done(&self, buf: &'static mut [u8], acked: bool, result: ReturnCode) {
        self.tx_buf.replace(buf);
        if result != ReturnCode::SUCCESS {
            self.end_packet_transmit(acked, result);
            return;
        }
        self.tx_states.head().map(move |head| {
            if head.is_transmit_done() {
                // This must return Some if we are in the closure - in particular,
                // tx_state == head
                self.end_packet_transmit(acked, result);
            } else {
                let tx_buf = self.tx_buf.take().unwrap();
                let retcode = head.prepare_transmit_next_fragment(tx_buf, self.radio);
                if retcode.is_err() {
                    self.end_packet_transmit(acked, retcode.unwrap_err());
                }
            }
        });
    }
}

impl <'a, R: Mac, C: ContextStore<'a>, A: time::Alarm>
RxClient for FragState<'a, R, C, A> {
    // TODO: Handle error (result != SUCCESS). Should we even propogate errors?
    // or just drop packet/frame?
    fn receive<'b>(&self, buf: &'b [u8],
                   header: Header<'b>,
                   data_offset: usize,
                   data_len: usize,
                   _: ReturnCode) {

        // TODO: Handle unwrap!
        let src_mac_addr = header.src_addr.unwrap_or(MacAddress::Short(0));
        let dst_mac_addr = header.dst_addr.unwrap_or(MacAddress::Short(0));

        let (rx_state, returncode) = self.receive_frame(&buf[data_offset..data_offset+data_len],
                                          data_len,
                                          src_mac_addr,
                                          dst_mac_addr);
        // Reception completed if rx_state is not None. Note that this can
        // also occur for some fail states (e.g. dropping an invalid packet)
        rx_state.map(|state| state.end_receive(self.rx_client.get(), returncode));
    }
}

// TODO: Need to implement config client?
/*
impl <'a, R: Mac, C: ContextStore<'a>, A: time::Alarm> ConfigClient for FragState<'a, R, C, A> {
    fn config_done(&self, result: ReturnCode) {
    }
}
*/

impl <'a, R: Mac, C: ContextStore<'a>, A: time::Alarm> 
time::Client for FragState<'a, R, C, A> {
    fn fired(&self) {
        // Timeout any expired rx_states
        for state in self.rx_states.iter() {
            if state.busy.get() {
                state.timeout_counter.set(state.timeout_counter.get() + TIMER_RATE);
                if state.timeout_counter.get() >= FRAG_TIMEOUT {
                    state.end_receive(self.rx_client.get(), ReturnCode::FAIL);
                }
            }
        }
        self.schedule_next_timer();
    }
}

impl <'a, R: Mac, C: ContextStore<'a>, A: time::Alarm> FragState<'a, R, C, A> {
    pub fn new(radio: &'a R, lowpan: &'a LoWPAN<'a, C>, tx_buf: &'static mut [u8],
               alarm: &'a A) -> FragState<'a, R, C, A> {
        FragState {
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

    pub fn schedule_next_timer(&self) {
        let seconds = A::Frequency::frequency() * (TIMER_RATE as u32);
        let next = self.alarm.now().wrapping_add(seconds);
        self.alarm.set_alarm(next);
    }

    pub fn add_rx_state(&self, rx_state: &'a RxState<'a>) {
        self.rx_states.push_head(rx_state);
    }

    pub fn set_receive_client(&self, client: &'static ReceiveClient) {
        self.rx_client.set(Some(client));
    }

    // TODO: We assume ip6_packet.len() == ip6_packet_len
    // TODO: Need to keep track of additional state: encryption bool, etc.
    pub fn transmit_packet(&self,
                           src_mac_addr: MacAddress, // TODO: Can get this from radio
                           dst_mac_addr: MacAddress,
                           mut ip6_packet: &'static mut [u8],
                           tx_state: &'a TxState<'a>,
                           source_long: bool,
                           fragment: bool) -> Result<ReturnCode, ReturnCode> {

        tx_state.init_transmit(src_mac_addr, dst_mac_addr, ip6_packet, 
                               source_long, fragment);
        // Queue tx_state
        self.tx_states.push_tail(tx_state);
        if self.tx_busy.get() {
            Ok(ReturnCode::SUCCESS)
        } else {
            // Set as current state and start transmit
            self.start_packet_transmit();
            Ok(ReturnCode::SUCCESS)
        }
    }

    #[allow(unused_must_use)]
    // TODO: Handle failure case
    fn start_packet_transmit(&self) {
        // TODO: Below will not work until we retrieve the buffer from the
        // radio on error - any iteration > 2 will not have the buffer anymore
        // as it was lost in the transmit call
        /*
        let mut tx_state = self.tx_states.head();
        while tx_state.is_some() {
            let result = tx_state.map(move |state|
                state.start_transmit(dgram_tag, frag_buf, self.radio, self.lowpan)
            ).unwrap();
            // Successfully started transmitting
            if result.is_ok() {
                self.tx_busy.set(true);
                break;
            }
            // Failed to start transmitting; issue error callbacks and remove
            // TxState from the list
            self.tx_states.pop_head().map(|head| {
                head.end_transmit(false, result.unwrap_err());
            });
            // TODO: Get frag_buf back -- requires modifying the radio
            // This will *not* compile until frag_buf is updated (as the value
            // moved)
            // frag_buf = ...
            // Updates tx_state
            tx_state = self.tx_states.head();
        }
        */

        self.tx_states.head().map(move |state| {
            // We panic here, as it should never be the case that we start
            // transmitting without the tx_buf
            let mut frag_buf = self.tx_buf.take().unwrap();
            let dgram_tag = self.tx_dgram_tag.get() + 1;
            self.tx_dgram_tag.set( if dgram_tag == 0 { 1 } else { dgram_tag});
            self.tx_busy.set(true);
            state.start_transmit(dgram_tag, frag_buf, self.radio, self.lowpan)
        }).unwrap_or(Ok(ReturnCode::SUCCESS));
    }

    // This function ends the current packet transmission state, and starts
    // sending the next queued packet before calling the current callback.
    fn end_packet_transmit(&self, acked: bool, returncode: ReturnCode) {
        self.tx_busy.set(false);
        // Note that tx_state can be None if a disassociation event occurred,
        // in which case end_transmit was already called.
        self.tx_states.pop_head().map(|tx_state| {
            self.start_packet_transmit();
            tx_state.end_transmit(acked, returncode);
        });
    }

    fn receive_frame(&self,
                      packet: &[u8],
                      packet_len: usize,
                      src_mac_addr: MacAddress,
                      dst_mac_addr: MacAddress) -> (Option<&RxState<'a>>, ReturnCode) {
        if is_fragment(packet) {
            let (is_frag1, dgram_size, dgram_tag, dgram_offset) = get_frag_hdr(&packet[0..5]);
            let offset_to_payload = if is_frag1 {
                lowpan_frag::FRAG1_HDR_SIZE
            } else {
                lowpan_frag::FRAGN_HDR_SIZE
            };
            self.receive_fragment(&packet[offset_to_payload..],
                                  packet_len - offset_to_payload,
                                  src_mac_addr,
                                  dst_mac_addr,
                                  dgram_size,
                                  dgram_tag,
                                  dgram_offset)
        } else {
            self.receive_single_packet(&packet, packet_len, src_mac_addr, dst_mac_addr)
        }
    }

    fn receive_single_packet(&self,
                             payload: &[u8],
                             payload_len: usize,
                             src_mac_addr: MacAddress,
                             dst_mac_addr: MacAddress) -> (Option<&RxState<'a>>, ReturnCode) {
        let rx_state = self.rx_states.iter().find(|state| !state.busy.get());
        rx_state.map(|state| {
            state.start_receive(src_mac_addr, dst_mac_addr,
                                payload_len as u16, 0);
            // The packet buffer should *always* be there, so we can panic if
            // unwrap fails
            let mut packet = state.packet.take().unwrap();
            if is_lowpan(payload) {
                let decompressed = self.lowpan.decompress(&payload[0..payload_len as usize],
                                                          src_mac_addr,
                                                          dst_mac_addr,
                                                          &mut packet,
                                                          0,
                                                          false);
                if decompressed.is_err() {
                    return (None, ReturnCode::FAIL);
                }
                let (consumed, written) = decompressed.unwrap();
                let remaining = payload_len - consumed;
                packet[written..written+remaining]
                    .copy_from_slice(&payload[consumed..consumed+remaining]);

            } else {
                packet[0..payload_len]
                    .copy_from_slice(&payload[0..payload_len]);
            }
            state.packet.replace(packet);
            (Some(state), ReturnCode::SUCCESS)
        }).unwrap_or((None, ReturnCode::ENOMEM))
    }

    // This function returns an Err if an error occurred, returns Ok(Some(RxState))
    // if the packet has been fully reassembled, or returns Ok(None) if there
    // are still pending fragments
    fn receive_fragment(&self,
                        frag_payload: &[u8],
                        payload_len: usize,
                        src_mac_addr: MacAddress,
                        dst_mac_addr: MacAddress,
                        dgram_size: u16,
                        dgram_tag: u16,
                        dgram_offset: usize) -> (Option<&RxState<'a>>, ReturnCode) {
        let mut rx_state = self.rx_states.iter().find(
            |state| state.is_my_fragment(src_mac_addr, dst_mac_addr, dgram_size, dgram_tag)
        );

        if rx_state.is_none() { 
            rx_state = self.rx_states.iter().find(|state| !state.busy.get());
            // Initialize new state
            rx_state.map(|state| state.start_receive(src_mac_addr, dst_mac_addr,
                                                     dgram_size, dgram_tag));
            if rx_state.is_none() {
                return (None, ReturnCode::ENOMEM);
            }
        }
        rx_state.map(|state| {
            // Returns true if the full packet is reassembled
            let res = state.receive_next_frame(frag_payload,
                                               payload_len,
                                               dgram_size,
                                               dgram_offset,
                                               &self.lowpan);
            if res.is_err() {
                // Some error occurred
                (Some(state), ReturnCode::FAIL)
            } else if res.unwrap() {
                // Packet fully reassembled
                (Some(state), ReturnCode::SUCCESS)
            } else {
                // Packet not fully reassembled
                (None, ReturnCode::SUCCESS)
            }
        }).unwrap_or((None, ReturnCode::ENOMEM))
    }

    #[allow(dead_code)]
    // This function is called when a disassociation event occurs, as we need
    // to expire all pending state.
    fn discard_all_state(&self) {
        for rx_state in self.rx_states.iter() {
            rx_state.end_receive(None, ReturnCode::FAIL);
        }
        for tx_state in self.tx_states.iter() {
            tx_state.end_transmit(false, ReturnCode::FAIL);
        }
    }
}
