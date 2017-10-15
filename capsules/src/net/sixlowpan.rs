//! Implements 6LoWPAN transmission and reception, including compression,
//! fragmentation, and reassembly. This layer exposes a send/receive interface
//! which converts between fully-formed IPv6 packets and Mac-layer frames.
//! This layer fragments any packet larger than the 802.15.4 MTU size, and
//! will issue a callback when the entire packet has been transmitted.
//! Similarly, this layer issues a callback when an entire IPv6 packet has been
//! received and reassembled.
//!
//! This layer relies on the specifications contained in RFC 4944 and RFC 6282
//!
//! Remaining Tasks and Known Problems
//! ----------------------------------
//! TODO: Implement and expose a ConfigClient interface?
//!
//! Problem: The receiving Imix sometimes fails to receive a fragment. This
//!     occurs below the Mac layer, and prevents the packet from being fully
//!     reassembled.
//!
//! Design
//! ------
//! At a high level, this layer exposes a transmit and receive functionality
//! that takes IPv6 packets and converts them into chunks that are passed to
//! the 802.15.4 MAC layer. The Sixlowpan struct represents the global
//! transmission state, and contains tag information, queued TxState structs,
//! and in-process RxState structs. In order for a client to send via this
//! interface, it must supply a TxState struct, the IPv6 packet, and arguments
//! relating to lower layers. This layer then fragments and compresses the
//! packet if necessary, then transmits it over a Mac-layer device. In order
//! for a packet to be received, the client must call set_receive_client
//! on the Sixlowpan struct. Currently, there is a single, global receive
//! client that receives callbacks for all reassembled packets (unlike for
//! the transmit path, where each TxState struct contains a separate client).
//! The Sixlowpan struct contains a list of RxState structs which are statically
//! allocated and added to the list; these structs represent the number of
//! concurrent reassembly operations that can be in progress at the same time.
//!
//! This layer adds several new structs, Sixlowpan, TxState, and RxState,
//! as well as interfaces for them.
//!
//! Sixlowpan:
//! - Methods:
//! -- new(..): Initializes a new Sixlowpan struct
//! -- transmit_packet(..): Transmits the given IPv6 packet, using the provided
//!      TxState struct to track its progress, fragmenting if necessary
//! -- set_receive_client(..): Sets the global receive client, which receives
//!      a callback whenever a packet is fully reassembled
//!
//! The Sixlowpan struct represents a single, global struct that tracks the state
//! of transmission and reception for the various clients. This struct manages
//! global state, including references to the radio and lowpan structs, along
//! with lists of TxStates and RxStates, buffers, and various other state.
//!
//! TxState:
//! - Methods:
//! -- new(..): Initializes a new TxState struct
//! -- set_transmit_client(..): Sets the per-state transmit client, which
//!      receives a callback after an entire IPv6 packet has been transmitted
//!
//! In order to send a packet, each client must allocate space for a TxState
//! struct. Whenever a client sends a packet, it must pass in a reference to
//! its TxState struct, and clients should not directly modify any fields within
//! the TxState struct. The fragmentation layer uses this struct to store state
//! about packets currently being sent.
//!
//! RxState:
//! - Methods:
//! -- new(..): Initializes a new RxState struct
//!
//! The RxState struct contains information relating to packet reception and
//! reassembly. Unlike with the TxState structs, there is no concept of
//! individual clients "owning" these structs; instead, some number of RxStates
//! are statically allocated and added to the Sixlowpan's RxState list. These
//! states are then used to keep track of different reassembly flows; the
//! number of simultaneous packet receptions is dependent on the number of
//! allocated RxState structs. When a packet is fully reassembled, the global
//! receive client inside Sixlowpan receives a callback.
//!
//! In addition to structs and their methods, this layer also defines several
//! traits for the transmit and receive callbacks.
//!
//! TransmitClient Trait:
//! - send_done(..): Called after the entire IPv6 packet has been sent. Returns
//!     the IPv6 buffer, a reference to the TxState struct, and additional
//!     information relating to the sent packet
//!
//! ReceiveClient Trait:
//! - receive(..): Called after an entire IPv6 packet has been reassembled.
//!     Returns the IPv6 buffer containing the decompressed packet, as well as
//!     additional information. Note that this function is required to return a
//!     static buffer, as the buffer passed into this function is owned by the
//!     RxState and not the client
//!
//! Usage
//! -----
//! Examples of how to interface and use this layer are included in the file
//! `boards/imixv1/src/lowpan_frag_dummy.rs`. Significant set-up is required
//! in `boards/imixv1/src/main.rs` to initialize the various state for the
//! layer and its clients.

use core::cell::Cell;
use ieee802154::mac::{Mac, Frame, TxClient, RxClient};
use kernel::ReturnCode;
use kernel::common::list::{List, ListLink, ListNode};
use kernel::common::take_cell::{TakeCell, MapCell};
use kernel::hil::radio;
use kernel::hil::time;
use kernel::hil::time::Frequency;
use net::frag_utils::Bitmap;
use net::ieee802154::{PanID, MacAddress, SecurityLevel, KeyId, Header};
use net::sixlowpan_compression;
use net::sixlowpan_compression::{ContextStore, is_lowpan};
use net::util::{slice_to_u16, u16_to_slice};

// Reassembly timeout in seconds
const FRAG_TIMEOUT: u32 = 60;

pub trait SixlowpanClient {
    fn receive<'a>(&self, buf: &'a [u8], len: u16, result: ReturnCode);
    fn send_done(&self, buf: &'static mut [u8], acked: bool, result: ReturnCode);
}

pub mod lowpan_frag {
    pub const FRAGN_HDR: u8 = 0b11100000;
    pub const FRAG1_HDR: u8 = 0b11000000;
    pub const FRAG1_HDR_SIZE: usize = 4;
    pub const FRAGN_HDR_SIZE: usize = 5;
}

fn set_frag_hdr(dgram_size: u16,
                dgram_tag: u16,
                dgram_offset: usize,
                hdr: &mut [u8],
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
        _ => false,
    };
    // Zero out upper bits
    let dgram_size = slice_to_u16(&hdr[0..2]) & !(0xf << 12);
    let dgram_tag = slice_to_u16(&hdr[2..4]);
    let dgram_offset = if is_frag1 { 0 } else { hdr[4] };
    (is_frag1, dgram_size, dgram_tag, (dgram_offset as usize) * 8)
}

fn is_fragment(packet: &[u8]) -> bool {
    let mask = packet[0] & lowpan_frag::FRAGN_HDR;
    (mask == lowpan_frag::FRAGN_HDR) || (mask == lowpan_frag::FRAG1_HDR)
}

/// struct TxState
/// --------------
/// This struct tracks the per-client transmit state for a single IPv6 packet.
/// The Sixlowpan struct maintains a list of TxState structs, sending each in
/// order.
pub struct TxState {
    // State for a single transmission
    packet: TakeCell<'static, [u8]>,
    src_pan: Cell<PanID>,
    dst_pan: Cell<PanID>,
    src_mac_addr: Cell<MacAddress>,
    dst_mac_addr: Cell<MacAddress>,
    security: Cell<Option<(SecurityLevel, KeyId)>>,
    dgram_tag: Cell<u16>,
    dgram_size: Cell<u16>,
    dgram_offset: Cell<usize>,
    fragment: Cell<bool>,
    compress: Cell<bool>,

    // Global transmit state
    tx_dgram_tag: Cell<u16>,
    tx_busy: Cell<bool>, // TODO: Can remove?
    tx_buf: TakeCell<'static, [u8]>,
}

impl TxState {
    /// TxState::new
    /// ------------
    /// This constructs a new, default TxState struct.
    pub fn new(tx_buf: &'static mut [u8]) -> TxState {
        TxState {
            packet: TakeCell::empty(),
            src_pan: Cell::new(0),
            dst_pan: Cell::new(0),
            src_mac_addr: Cell::new(MacAddress::Short(0)),
            dst_mac_addr: Cell::new(MacAddress::Short(0)),
            security: Cell::new(None),
            dgram_tag: Cell::new(0),
            dgram_size: Cell::new(0),
            dgram_offset: Cell::new(0),
            fragment: Cell::new(false),
            compress: Cell::new(false),

            tx_dgram_tag: Cell::new(0),
            tx_busy: Cell::new(false),
            tx_buf: TakeCell::new(tx_buf),
        }
    }

    fn is_transmit_done(&self) -> bool {
        self.dgram_size.get() as usize <= self.dgram_offset.get()
    }

    fn init_transmit(&self,
                     src_mac_addr: MacAddress,
                     dst_mac_addr: MacAddress,
                     packet: &'static mut [u8],
                     packet_len: usize,
                     security: Option<(SecurityLevel, KeyId)>,
                     fragment: bool,
                     compress: bool) {

        self.src_mac_addr.set(src_mac_addr);
        self.dst_mac_addr.set(dst_mac_addr);
        self.fragment.set(fragment);
        self.compress.set(compress);
        self.security.set(security);
        self.packet.replace(packet);
        self.dgram_size.set(packet_len as u16);
    }

    // Takes ownership of frag_buf and gives it to the radio
    fn start_transmit(&self,
                      dgram_tag: u16,
                      frag_buf: &'static mut [u8],
                      radio: &Mac,
                      ctx_store: &ContextStore)
                      -> Result<ReturnCode, (ReturnCode, &'static mut [u8])> {
        self.dgram_tag.set(dgram_tag);
        match self.packet.take() {
            None => Err((ReturnCode::ENOMEM, frag_buf)),
            Some(ip6_packet) => {
                let result = match radio.prepare_data_frame(frag_buf,
                                                            self.dst_pan.get(),
                                                            self.dst_mac_addr.get(),
                                                            self.src_pan.get(),
                                                            self.src_mac_addr.get(),
                                                            self.security.get()) {
                    Err(frame) => Err((ReturnCode::FAIL, frame)),
                    Ok(frame) => {
                        self.prepare_transmit_first_fragment(ip6_packet, frame, radio, ctx_store)
                    }
                };
                // If the ip6_packet is Some, always want to replace even in
                // case of errors
                self.packet.replace(ip6_packet);
                result
            }
        }
    }

    fn prepare_transmit_first_fragment(&self,
                                       ip6_packet: &[u8],
                                       mut frame: Frame,
                                       radio: &Mac,
                                       ctx_store: &ContextStore)
                                       -> Result<ReturnCode, (ReturnCode, &'static mut [u8])> {

        // Here, we assume that the compressed headers fit in the first MTU
        // fragment. This is consistent with RFC 6282.
        let mut lowpan_packet = [0 as u8; radio::MAX_FRAME_SIZE as usize];
        let (consumed, written) = if self.compress.get() {
            let lowpan_result = sixlowpan_compression::compress(ctx_store,
                                                 &ip6_packet,
                                                 self.src_mac_addr.get(),
                                                 self.dst_mac_addr.get(),
                                                 &mut lowpan_packet);
            match lowpan_result {
                Err(_) => return Err((ReturnCode::FAIL, frame.into_buf())),
                Ok(result) => result,
            }
        } else {
            (0, 0)
        };
        let remaining_payload = ip6_packet.len() - consumed;
        let lowpan_len = written + remaining_payload;
        // TODO: This -2 is added to account for the FCS; this should be changed
        // in the MAC code
        let mut remaining_capacity = frame.remaining_data_capacity() - 2;

        // Need to fragment
        if lowpan_len > remaining_capacity {
            if self.fragment.get() {
                let mut frag_header = [0 as u8; lowpan_frag::FRAG1_HDR_SIZE];
                set_frag_hdr(self.dgram_size.get(),
                             self.dgram_tag.get(),
                             /*offset = */
                             0,
                             &mut frag_header,
                             true);
                frame.append_payload(&frag_header[0..lowpan_frag::FRAG1_HDR_SIZE]);
                remaining_capacity -= lowpan_frag::FRAG1_HDR_SIZE;
            } else {
                // Unable to fragment and packet too large
                return Err((ReturnCode::ESIZE, frame.into_buf()));
            }
        }

        // Write the 6lowpan header
        if self.compress.get() {
            if written <= remaining_capacity {
                frame.append_payload(&lowpan_packet[0..written]);
                remaining_capacity -= written;
            } else {
                return Err((ReturnCode::ESIZE, frame.into_buf()));
            }
        }

        // Write the remainder of the payload, rounding down to a multiple
        // of 8 if the entire payload won't fit
        let payload_len = if remaining_payload > remaining_capacity {
            remaining_capacity & !0b111
        } else {
            remaining_payload
        };
        frame.append_payload(&ip6_packet[consumed..consumed + payload_len]);
        self.dgram_offset.set(consumed + payload_len);
        let (result, buf) = radio.transmit(frame);
        // If buf is returned, then map the error; otherwise, we return success
        buf.map(|buf| Err((result, buf))).unwrap_or(Ok(ReturnCode::SUCCESS))
    }

    fn prepare_transmit_next_fragment(&self,
                                      frag_buf: &'static mut [u8],
                                      radio: &Mac)
                                      -> Result<ReturnCode, (ReturnCode, &'static mut [u8])> {
        match radio.prepare_data_frame(frag_buf,
                                       self.dst_pan.get(),
                                       self.dst_mac_addr.get(),
                                       self.src_pan.get(),
                                       self.src_mac_addr.get(),
                                       self.security.get()) {
            Err(frame) => Err((ReturnCode::FAIL, frame)),
            Ok(mut frame) => {
                let dgram_offset = self.dgram_offset.get();
                let remaining_capacity = frame.remaining_data_capacity() -
                                         lowpan_frag::FRAGN_HDR_SIZE;
                // This rounds payload_len down to the nearest multiple of 8 if it
                // is not the last fragment (per RFC 4944)
                let remaining_bytes = (self.dgram_size.get() as usize) - dgram_offset;
                let payload_len = if remaining_bytes > remaining_capacity {
                    remaining_capacity & !0b111
                } else {
                    remaining_bytes
                };

                // Take the packet temporarily
                match self.packet.take() {
                    None => Err((ReturnCode::ENOMEM, frame.into_buf())),
                    Some(packet) => {
                        let mut frag_header = [0 as u8; lowpan_frag::FRAGN_HDR_SIZE];
                        set_frag_hdr(self.dgram_size.get(),
                                     self.dgram_tag.get(),
                                     dgram_offset,
                                     &mut frag_header,
                                     false);
                        frame.append_payload(&frag_header);
                        frame.append_payload(&packet[dgram_offset..dgram_offset + payload_len]);
                        // Replace the packet
                        self.packet.replace(packet);

                        // Update the offset to be used for the next fragment
                        self.dgram_offset.set(dgram_offset + payload_len);
                        let (result, buf) = radio.transmit(frame);
                        // If buf is returned, then map the error; otherwise, we return success
                        buf.map(|buf| Err((result, buf))).unwrap_or(Ok(ReturnCode::SUCCESS))
                    }
                }
            }
        }
    }

    fn end_transmit(&self, client: Option<&'static SixlowpanClient>, acked: bool, result: ReturnCode) {
        client.map(move |client| {
            // The packet here should always be valid, as we borrow the packet
            // from the upper layer for the duration of the transmission. It
            // represents a significant bug if the packet is not there when
            // transmission completes.
            self.packet
                .take()
                .map(|packet| { client.send_done(packet, acked, result); })
                .expect("Error: `packet` is None in call to end_transmit.");
        });
    }
}

/// struct RxState
/// --------------
/// This struct tracks the reassembly process for a given packet. The `busy`
/// field marks whether the particular RxState is currently reassembling a
/// packet or if it is currently free. The Sixlowpan struct maintains a list of
/// RxState structs, which represents the number of packets that can be
/// concurrently reassembled.
pub struct RxState<'a> {
    packet: TakeCell<'static, [u8]>,
    bitmap: MapCell<Bitmap>,
    dst_mac_addr: Cell<MacAddress>,
    src_mac_addr: Cell<MacAddress>,
    dgram_tag: Cell<u16>,
    dgram_size: Cell<u16>,
    busy: Cell<bool>,
    start_time: Cell<u32>,

    next: ListLink<'a, RxState<'a>>,
}

impl<'a> ListNode<'a, RxState<'a>> for RxState<'a> {
    fn next(&'a self) -> &'a ListLink<RxState<'a>> {
        &self.next
    }
}

impl<'a> RxState<'a> {
    /// RxState::new
    /// ------------
    /// This function constructs a new RxState struct.
    pub fn new(packet: &'static mut [u8]) -> RxState<'a> {
        RxState {
            packet: TakeCell::new(packet),
            bitmap: MapCell::new(Bitmap::new()),
            dst_mac_addr: Cell::new(MacAddress::Short(0)),
            src_mac_addr: Cell::new(MacAddress::Short(0)),
            dgram_tag: Cell::new(0),
            dgram_size: Cell::new(0),
            busy: Cell::new(false),
            start_time: Cell::new(0),
            next: ListLink::empty(),
        }
    }

    fn is_my_fragment(&self,
                      src_mac_addr: MacAddress,
                      dst_mac_addr: MacAddress,
                      dgram_size: u16,
                      dgram_tag: u16)
                      -> bool {
        self.busy.get() && (self.dgram_tag.get() == dgram_tag) &&
        (self.dgram_size.get() == dgram_size) &&
        (self.src_mac_addr.get() == src_mac_addr) &&
        (self.dst_mac_addr.get() == dst_mac_addr)
    }

    // Checks if a given RxState is free or expired (and thus, can be freed).
    // This function implements the reassembly timeout for 6LoWPAN lazily.
    fn is_busy(&self, frequency: u32, current_time: u32) -> bool {
        let expired = current_time >= (self.start_time.get()
                                        + FRAG_TIMEOUT * frequency); 
        if expired {
            self.end_receive(None, ReturnCode::FAIL);
        }
        self.busy.get()
    }

    fn start_receive(&self,
                     src_mac_addr: MacAddress,
                     dst_mac_addr: MacAddress,
                     dgram_size: u16,
                     dgram_tag: u16,
                     current_tics: u32) {
        self.dst_mac_addr.set(dst_mac_addr);
        self.src_mac_addr.set(src_mac_addr);
        self.dgram_tag.set(dgram_tag);
        self.dgram_size.set(dgram_size);
        self.busy.set(true);
        self.bitmap.map(|bitmap| bitmap.clear());
        self.start_time.set(current_tics);
    }

    // This function assumes that the payload is a slice starting from the
    // actual payload (no 802.15.4 headers, no fragmentation headers), and
    // returns true if the packet is completely reassembled.
    fn receive_next_frame(&self,
                          payload: &[u8],
                          payload_len: usize,
                          dgram_size: u16,
                          dgram_offset: usize,
                          ctx_store: &ContextStore)
                          -> Result<bool, ReturnCode> {
        let mut packet = self.packet.take().ok_or(ReturnCode::ENOMEM)?;
        let uncompressed_len = if dgram_offset == 0 {
            let (consumed, written) = sixlowpan_compression::decompress(ctx_store,
                                                         &payload[0..payload_len as usize],
                                                         self.src_mac_addr.get(),
                                                         self.dst_mac_addr.get(),
                                                         &mut packet,
                                                         dgram_size,
                                                         true).map_err(|_| ReturnCode::FAIL)?;
            let remaining = payload_len - consumed;
            packet[written..written + remaining]
                .copy_from_slice(&payload[consumed..consumed + remaining]);
            written + remaining

        } else {
            packet[dgram_offset..dgram_offset + payload_len]
                .copy_from_slice(&payload[0..payload_len]);
            payload_len
        };
        self.packet.replace(packet);
        if !self.bitmap.map_or(false, |bitmap| {
            bitmap.set_bits(dgram_offset / 8, (dgram_offset + uncompressed_len) / 8)
        }) {
            // If this fails, we received an overlapping fragment. We can simply
            // drop the packet in this case.
            Err(ReturnCode::FAIL)
        } else {
            self.bitmap
                .map(|bitmap| bitmap.is_complete((dgram_size as usize) / 8))
                .ok_or(ReturnCode::FAIL)
        }
    }

    fn end_receive(&self, client: Option<&'static SixlowpanClient>, result: ReturnCode) {
        self.busy.set(false);
        self.bitmap.map(|bitmap| bitmap.clear());
        self.start_time.set(0);
        client.map(move |client| {
            // Since packet is borrowed from the upper layer, failing to return it
            // in the callback represents a significant error that should never
            // occur - all other calls to `packet.take()` replace the packet,
            // and thus the packet should always be here.
            self.packet
                .map(|packet| { client.receive(&packet, self.dgram_size.get(), result); })
                .expect("Error: `packet` is None in call to end_receive.");
        });
    }
}

/// struct Sixlowpan
/// ----------------
/// This struct tracks the global sending/receiving state, and contains the
/// lists of RxStates and TxStates.
pub struct Sixlowpan<'a, A: time::Alarm + 'a> {
    pub radio: &'a Mac<'a>,
    ctx_store: &'a ContextStore,
    clock: &'a A,
    client: Cell<Option<&'static SixlowpanClient>>,

    // Transmit state
    tx_state: TxState,
    // Receive state
    rx_states: List<'a, RxState<'a>>,
}

// This function is called after transmitting a frame
#[allow(unused_must_use)]
impl<'a, A: time::Alarm> TxClient for Sixlowpan<'a, A> {
    fn send_done(&self, tx_buf: &'static mut [u8], acked: bool, result: ReturnCode) {
        // If we are done sending the entire packet, or if the transmit failed,
        // end the transmit state and issue callbacks.
        if result != ReturnCode::SUCCESS || self.tx_state.is_transmit_done() {
            self.end_packet_transmit(tx_buf, acked, result);
        // Otherwise, send next fragment
        } else {
            let result = self.tx_state.prepare_transmit_next_fragment(tx_buf, self.radio);
            result.map_err(|(retcode, tx_buf)| {
                // If we have an error, abort
                self.end_packet_transmit(tx_buf, acked, retcode);
            });
        }
    }
}

// This function is called after receiving a frame
impl<'a, A: time::Alarm> RxClient for Sixlowpan<'a, A> {
    fn receive<'b>(&self, buf: &'b [u8], header: Header<'b>, data_offset: usize, data_len: usize) {
        // We return if retcode is not valid, as it does not make sense to issue
        // a callback for an invalid frame reception
        let data_offset = data_offset;
        // TODO: Handle the case where the addresses are None/elided - they
        // should not default to the zero address
        let src_mac_addr = header.src_addr.unwrap_or(MacAddress::Short(0));
        let dst_mac_addr = header.dst_addr.unwrap_or(MacAddress::Short(0));

        let (rx_state, returncode) = self.receive_frame(&buf[data_offset..data_offset + data_len],
                                                        data_len,
                                                        src_mac_addr,
                                                        dst_mac_addr);
        // Reception completed if rx_state is not None. Note that this can
        // also occur for some fail states (e.g. dropping an invalid packet)
        rx_state.map(|state| state.end_receive(self.client.get(), returncode));
    }
}

impl<'a, A: time::Alarm> Sixlowpan<'a, A> {
    /// Sixlowpan::new
    /// --------------
    /// This function initializes and returns a new Sixlowpan struct.
    pub fn new(radio: &'a Mac<'a>,
               ctx_store: &'a ContextStore,
               tx_buf: &'static mut [u8],
               clock: &'a A)
               -> Sixlowpan<'a, A> {
        Sixlowpan {
            radio: radio,
            ctx_store: ctx_store,
            clock: clock,
            client: Cell::new(None),

            tx_state: TxState::new(tx_buf),
            rx_states: List::new(),
        }
    }

    /// Sixlowpan::add_rx_state
    /// -----------------------
    /// This function prepends the passed in RxState struct to the list of
    /// RxStates maintained by the Sixlowpan struct. For the current use cases,
    /// some number of RxStates are statically allocated and immediately
    /// added to the list of RxStates.
    pub fn add_rx_state(&self, rx_state: &'a RxState<'a>) {
        self.rx_states.push_head(rx_state);
    }

    /// TODO
    pub fn set_client(&self, client: &'static SixlowpanClient) {
        self.client.set(Some(client));
    }

    /// Sixlowpan::transmit_packet
    /// --------------------------
    /// This function is called to send a fully-formed IPv6 packet. Arguments
    /// to this function are used to determine various aspects of the MAC
    /// layer frame and keep track of the transmission state.
    pub fn transmit_packet(&self,
                           src_mac_addr: MacAddress,
                           dst_mac_addr: MacAddress,
                           ip6_packet: &'static mut [u8],
                           ip6_packet_len: usize,
                           security: Option<(SecurityLevel, KeyId)>,
                           fragment: bool,
                           compress: bool)
                           -> Result<ReturnCode, ReturnCode> {

        self.tx_state.init_transmit(src_mac_addr,
                               dst_mac_addr,
                               ip6_packet,
                               ip6_packet_len,
                               security,
                               fragment,
                               compress);
        // TODO: Lose buffer if busy
        if self.tx_state.tx_busy.get() {
            Err(ReturnCode::EBUSY)
        } else {
            self.start_packet_transmit();
            Ok(ReturnCode::SUCCESS)
        }
    }

    fn start_packet_transmit(&self) {
        // TODO:
        // Already transmitting - this should never happen
        if self.tx_state.tx_busy.get() {
            return;
        }

        // Increment dgram_tag
        let dgram_tag = if (self.tx_state.tx_dgram_tag.get() + 1) == 0 {
            1
        } else {
            self.tx_state.tx_dgram_tag.get() + 1
        };

        let frag_buf = self.tx_state.tx_buf
            .take()
            .expect("Error: `tx_buf` is None in call to start_packet_transmit.");

        match self.tx_state.start_transmit(dgram_tag, frag_buf, self.radio, self.ctx_store) {
            // Successfully started transmitting
            Ok(_) => {
                self.tx_state.tx_dgram_tag.set(dgram_tag);
                self.tx_state.tx_busy.set(true);
            }
            // Otherwise, we failed
            Err((returncode, new_frag_buf)) => {
                self.tx_state.tx_buf.replace(new_frag_buf);
                self.tx_state.end_transmit(self.client.get(), false, returncode);
            }
        }
    }

    // This function ends the current packet transmission state, and starts
    // sending the next queued packet before calling the current callback.
    fn end_packet_transmit(&self, tx_buf: &'static mut [u8], acked: bool, returncode: ReturnCode) {
        self.tx_state.tx_busy.set(false);
        self.tx_state.tx_buf.replace(tx_buf);
        // TODO: Consider disassociation event case
        self.tx_state.end_transmit(self.client.get(), acked, returncode);
    }

    fn receive_frame(&self,
                     packet: &[u8],
                     packet_len: usize,
                     src_mac_addr: MacAddress,
                     dst_mac_addr: MacAddress)
                     -> (Option<&RxState<'a>>, ReturnCode) {
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
                             dst_mac_addr: MacAddress)
                             -> (Option<&RxState<'a>>, ReturnCode) {
        let rx_state = self.rx_states.iter().find(|state|
                                                  !state.is_busy(self.clock.now(), A::Frequency::frequency()));
        rx_state.map(|state| {
                state.start_receive(src_mac_addr, dst_mac_addr, payload_len as u16, 0, self.clock.now());
                // The packet buffer should *always* be there; in particular,
                // since this state is not busy, it must have the packet buffer.
                // Otherwise, we are in an inconsistent state and can fail.
                let mut packet = state.packet
                    .take()
                    .expect("Error: `packet` in RxState struct is `None` \
                            in call to `receive_single_packet`.");
                if is_lowpan(payload) {
                    let decompressed = sixlowpan_compression::decompress(self.ctx_store,
                                                          &payload[0..payload_len as usize],
                                                          src_mac_addr,
                                                          dst_mac_addr,
                                                          &mut packet,
                                                          0,
                                                          false);
                    match decompressed {
                        Ok((consumed, written)) => {
                            let remaining = payload_len - consumed;
                            packet[written..written + remaining]
                                .copy_from_slice(&payload[consumed..consumed + remaining]);
                        }
                        Err(_) => {
                            return (None, ReturnCode::FAIL);
                        }
                    }
                } else {
                    packet[0..payload_len].copy_from_slice(&payload[0..payload_len]);
                }
                state.packet.replace(packet);
                (Some(state), ReturnCode::SUCCESS)
            })
            .unwrap_or((None, ReturnCode::ENOMEM))
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
                        dgram_offset: usize)
                        -> (Option<&RxState<'a>>, ReturnCode) {
        // First try to find an rx_state in the middle of assembly
        let mut rx_state = self.rx_states
            .iter()
            .find(|state| state.is_my_fragment(src_mac_addr, dst_mac_addr, dgram_size, dgram_tag));

        // Else find a free state
        if rx_state.is_none() {
            rx_state = self.rx_states.iter().find(|state|
                                                  !state.is_busy(self.clock.now(), A::Frequency::frequency()));
            // Initialize new state
            rx_state.map(|state| {
                state.start_receive(src_mac_addr, dst_mac_addr, dgram_size, dgram_tag, self.clock.now())
            });
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
                                                   self.ctx_store);
                match res {
                    // Some error occurred
                    Err(_) => (Some(state), ReturnCode::FAIL),
                    Ok(complete) => {
                        if complete {
                            // Packet fully reassembled
                            (Some(state), ReturnCode::SUCCESS)
                        } else {
                            // Packet not fully reassembled
                            (None, ReturnCode::SUCCESS)
                        }
                    }
                }
            })
            .unwrap_or((None, ReturnCode::ENOMEM))
    }

    #[allow(dead_code)]
    // This function is called when a disassociation event occurs, as we need
    // to expire all pending state.
    fn discard_all_state(&self) {
        for rx_state in self.rx_states.iter() {
            rx_state.end_receive(None, ReturnCode::FAIL);
        }
        self.tx_state.end_transmit(self.client.get(), false, ReturnCode::FAIL);
    }
}
