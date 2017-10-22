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
//! User Interface
//! --------------
//! This layer exposes IP transmit and receive functionality to the upper
//! layers. The main interface to this layer is the `Sixlowpan` struct and
//! the `SixlowpanClient` trait. In general terms, the `Sixlowpan` struct
//! exposes a way to send IPv6 packets, while the `SixlowpanClient` trait
//! is responsible for delivering the `send_done` and `received` callbacks,
//! which are invoked when a full IPv6 packet has been sent or received
//! respectively. Note that this layer should be treated as a synchronous
//! interface; only one object should be transmitting or receiving packets
//! (the SixlowpanClient object). Any virtualization is performed above this
//! layer, along with additional packet processing (e.g. dispatching to some
//! receive client based on IP address). At this time, this layer is not
//! exposed to userspace.
//!
//! The high-level control flow looks as follows:
//!
//! Transmit:
//!           -------------
//!           |Upper Layer|
//!           -------------
//!                 |
//!       transmit_packet(..packet..)
//!                 |
//!                 v
//!            -----------
//!            |Sixlowpan|
//!            -----------
//! ...
//!         -----------------
//!         |SixlowpanClient|
//!         -----------------
//!                 ^
//!                 |
//!            send_done(..) 
//!                 |
//!            -----------
//!            |Sixlowpan|
//!            -----------
//!
//! Receive:
//!         -----------------
//!         |SixlowpanClient|
//!         -----------------
//!                ^
//!                |
//!          receive(..buf..)
//!                |
//!           -----------
//!           |Sixlowpan|
//!           -----------
//!
//! Initialization:
//!           -------------
//!           |Upper Layer|
//!           -------------
//!                 |
//!          set_client(client) 
//!                 |
//!                 v
//!            -----------
//!            |Sixlowpan|
//!            -----------
//!
//!
//! The interface is explored in more detail below.
//!
//! *Sixlowpan Struct:* For the `Sixlowpan` struct, we divide the
//! user interface into two parts: standard usage and initialization. First,
//! for standard usage (transmission), the `Sixlowpan` struct supplies the
//! following method:
//!
//! ```
//! Sixlowpan::transmit_packet(&self,
//!                            src_mac_addr: MacAddress,
//!                            dst_mac_addr: MacAddress,
//!                            ip6_packet: &'static mut [u8],
//!                            ip6_packet_len: usize,
//!                            security: Option<(SecurityLevel, KeyId)>,
//!                            fragment: bool,
//!                            compress: bool)
//!                            -> Result<ReturnCode, ReturnCode>;
//! ```
//! This function exposes the primary packet transmission fuctionality. The
//! source and destination mac address arguments specify the link-layer
//! addresses of the packet, while the `security`, `fragment`, and `compress`
//! options specify the security level (if any), whether to fragment the packet
//! if it is too large, and whether to compress the packet respectively. These
//! different options are exposed to provide the caller with more control over
//! how this layer modifies packets; no compression and no fragmentation can
//! be set, and the packet will be sent to the radio as a raw IPv6 packet.
//!
//! The `ip6_packet` argument contains a pointer to a buffer containing a valid
//! IPv6 packet, while the `ip6_packet_len` argument specifies the number of
//! bytes to send. Note that `ip6_packet.len() > ip6_packet_len`, but we check
//! the invariant that `ip6_packet_len <= ip6_packet.len()`.
//!
//! ```
//!
//! To initialize the `Sixlowpan` struct, the following methods are exposed:
//!
//! ```
//! Sixlowpan::new(radio: &'a Mac<'a>,
//!                ctx_store: C,
//!                tx_buf: &'static mut [u8],
//!                clock: &'a A)
//!                -> Sixlowpan<'a, A, C>;
//! ```
//! The new function returns a new Sixlowpan struct. The radio argument is any
//! object implementing the Mac trait, while the clock implements the Alarm
//! functionality. The ctx_store argument is any object that implements the
//! ContextStore trait.
//!
//! ```
//! Sixlowpan::add_rx_state(&self, rx_state: &'a RxState<'a>);
//! ```
//! This method adds an RxState to the RxState pool maintained by the Sixlowpan
//! object. Each RxState struct represents the ability to reconstruct a single
//! IP packet at a time; if two RxState structs are allocated, two IPv6 packets
//! can be reconstructed simultaneously (likewise for 3+ RxStates). If only
//! a single RxState is allocated, only one IP packet can be reconstructed at
//! a time. Note that if no RxStates are initialized, no IP packets can be
//! received.
//!
//! ```
//! Sixlowpan::set_client(&'a self, client: &'a SixlowpanClient);
//! ```
//! The `set_client` method sets the SixlowpanClient for the Sixlowpan object.
//! Whatever object is set as the client receives both `receive` and `send_done`
//! callbacks.
//!
//! Usage
//! -----
//! Examples of how to interface and use this layer are included in the file
//! `boards/imixv1/src/lowpan_frag_dummy.rs`. Significant set-up is required
//! in `boards/imixv1/src/main.rs` to initialize the various state for the
//! layer and its clients.

// Internal Design
// ---------------
// The overall 6LoWPAN protocol is non-trivial, and as a result, this layer
// is fairly complex. There are two main aspects of the 6LoWPAN layer; first
// is compression, which is abstracted as a distinct library (found at
// `capsules/src/net/sixlowpan_compression.rs`), and second is the
// fragmentation and reassembly layer, which is implemented in this file.
// The documentation below describes the different components of the
// fragmentation/reassembly functionality (for 6LoWPAN compression
// documentation, please consult `capsules/src/net/sixlowpan_compression.rs`).
//
// This layer adds several new structures; principally, it implements the
// Sixlowpan, TxState, and RxState structs. Further, this layer also defines
// the SixlowpanClient trait. The Sixlowpan struct is responsible
// for keeping track of the global state of this layer, and contains references
// to the TxState and the list of RxStates. The TxState is responsible for
// maintaining the current transmit state, and how much of the current
// IPv6 packet has been transmitted. The RxState structs maintain the
// reassembly state corresponding to a single IPv6 packet. Note that since
// they are maintained as a list, several RxStates can be allocated at compile
// time, and each RxState corresponds to a distinct IPv6 packet that can be
// reassembled simultaneously. Finally, the SixlowpanClient trait defines
// the interface between the upper (IP) layer and the Sixlowpan layer. 
// Each object is examined in greater detail below:
//
// Sixlowpan:
// Examination of the public methods on the `Sixlowpan` struct are examined
// above in the User Interface section. Instead, here we detail the internal
// design of the struct. First, the `Sixlowpan` object is designed to be a
// single, global object which sits between the Mac and IP layers. It receives
// and transmits frames through the Mac layer, while reassembling or
// fragmenting IPv6 packets via the IP layer. As a result, the `Sixlowpan`
// struct maintains the single, global state relevent for this layer, including
// a reference to the radio, the context store (for (de)compressing 6LoWPAN-
// compressed fragments), a clock, and the upper-layer client callback.
// Additionally, this object maintains references to a single TxState, and
// a list of RxStates.
//
// TxState:
// The TxState struct maintains the state necessary to incrementally fragment
// and send a full IPv6 packet. This includes the source/destination Mac
// addresses and PanIDs, frame-level security options, a total datagram size,
// and the current offset into the datagram. This struct also maintains some
// minimal global transmit state, including the global datagram tag and a
// buffer to pass to the radio. This object is visible only to the
// Sixlowpan struct, and abstracts away the details for transmitting and
// fragmenting packets.
//
// RxState:
// The RxState struct is analogous to the TxState struct, in that it maintains
// state specific to reassembling an IPv6 packet. Unlike the TxState struct
// however, the Sixlowpan object manages multiple RxState structs. These
// RxStates serve as a pool of objects, and when a fragment arrives, the
// Sixlowpan object either dispatches it to an in-progress packet reassembly
// managed by a busy RxState struct, or initializes a free RxState struct
// to start reassembling the rest of the fragments. Similar to TxState,
// RxState objects should only be visible to the Sixlowpan object, aside
// from one caveat - the initialization of RxStates must occur statically
// outside the Sixlowpan struct (this may change in the future).
//
// The RxState struct maintains the in-progress packet buffer, a bitmap
// indicating which 8-byte chunks have not yet been received, the source/dest
// mac address pair, datagram size and tag, and a start time (to lazily
// expire timed-out reassembly processes).
//
// SixlowpanClient:
// The SixlowpanClient trait has two functions; `send_done` and `receive`.
// The Sixlowpan struct maintains a reference to the (current) SixlowpanClient,
// and issues callbacks when transmissions have completed (`send_done`) or
// a full IPv6 packet has been reassembled (`receive`). Note that the
// Sixlowpan object allows for the client to change at runtime, but the
// current assumption is a single layer sitting above the 6LoWPAN layer.
//
//
// Design Decisions
// ----------------
// Throughout designing this layer, there were a number of critical design
// decisions made. Several of the most prominent are listed below, with a
// short rationale as to why they were necessary or the most optimal solution.
//
// Multiple RxStates:
// This design decision is one of the more complicated and contentious ones.
// Due to the wording of the 6LoWPAN specification and the data associated
// with 6LoWPAN fragments, it is entirely reasonable to expect that even
// an edge node (a node not doing routing) might receive 6LoWPAN fragments
// for different IP packets interleaved. In particular, a 6LoWPAN fragment
// header contains a datagram tag, which is different for each IPv6 packet
// fragmented even from the same layer 2 source/destination pairs. Thus,
// a single node could send multiple, distinct, fragmented IPv6 packets
// simultaneously (or at least, a node is not prohibited from doing so). In
// addition, the reassembly timeout for 6LoWPAN fragments is on the order of
// seconds, and with a single RxState, a single lost fragment could
// substantially hamper or delay the ability of a client to receive additional
// packets. As a result of these two issues, the ability to add several
// RxStates to the 6LoWPAN layer was provided. Unfortunately, this
// increased the complexity of this layer substantially, and further,
// necessitated additional initialization complexity by the upper layer.
//
// Single TxState:
// Although both the RxState and TxState structs are treated similarly by
// the Sixlowpan layer, many aspects of their control flow differ
// significantly. The final design decision was to have a single upper layer
// that serialized (or virtualized) both the reception and transmission of
// IPv6 packets. As a result, only a single outstanding transmission made
// sense, and thus the layer was designed to have a serial transmit path.
// Note that this differs greatly from the RxState model, but since we
// cannot serialize reception in the same way, it did not make sense to treat
// both RxState and TxState structs identically.
//
// SixlowpanClient Receives both Callbacks:
// Another major design decision was to combine both the `receive` and
// `send_done` callbacks into a single trait. This reduced overall complexity
// as only a single client was necessary, and further, the current design
// of the 6LoWPAN layer assumes a serialized, single-client model. Thus,
// combining both callbacks into a single interface represented no major
// drawbacks, and served to simplify the code. Note that this design may
// change as additional functionality is implemented on top of this layer.

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

pub enum TransmitState {
    Idle { tx_buf: &'static [u8] },
    Transmitting { packet: &'static [u8] }
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
    dgram_tag: Cell<u16>, // TODO: Redundant
    dgram_size: Cell<u16>,
    dgram_offset: Cell<usize>,
    fragment: Cell<bool>, // TODO: Unnecessary?
    compress: Cell<bool>, // TODO: Unnecessary?

    // Global transmit state
    tx_dgram_tag: Cell<u16>,
    tx_busy: Cell<bool>, // TODO: Can remove?
    tx_buf: TakeCell<'static, [u8]>,

    //state: MapCell<TransmitState>,
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

            //state: MapCell::new(TransmitState::Idle { tx_buf: tx_buf),
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

    fn end_transmit<'a>(&self,
                    tx_buf: &'static mut [u8],
                    client: Option<&'a SixlowpanClient>,
                    acked: bool,
                    result: ReturnCode) {

        self.tx_busy.set(false);
        self.tx_buf.replace(tx_buf);
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

    fn end_receive(&self, client: Option<&'a SixlowpanClient>, result: ReturnCode) {
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
pub struct Sixlowpan<'a, A: time::Alarm + 'a, C: ContextStore> {
    pub radio: &'a Mac<'a>,
    ctx_store: C,
    clock: &'a A,
    client: Cell<Option<&'a SixlowpanClient>>,

    // Transmit state
    tx_state: TxState,
    // Receive state
    rx_states: List<'a, RxState<'a>>,
}

// This function is called after transmitting a frame
#[allow(unused_must_use)]
impl<'a, A: time::Alarm, C: ContextStore> TxClient for Sixlowpan<'a, A, C> {
    fn send_done(&self, tx_buf: &'static mut [u8], acked: bool, result: ReturnCode) {
        // If we are done sending the entire packet, or if the transmit failed,
        // end the transmit state and issue callbacks.
        if result != ReturnCode::SUCCESS || self.tx_state.is_transmit_done() {
            self.tx_state.end_transmit(tx_buf, self.client.get(), acked, result);
        // Otherwise, send next fragment
        } else {
            let result = self.tx_state.prepare_transmit_next_fragment(tx_buf, self.radio);
            result.map_err(|(retcode, tx_buf)| {
                // If we have an error, abort
                self.tx_state.end_transmit(tx_buf, self.client.get(), acked, retcode);
            });
        }
    }
}

// This function is called after receiving a frame
impl<'a, A: time::Alarm, C: ContextStore> RxClient for Sixlowpan<'a, A, C> {
    fn receive<'b>(&self, buf: &'b [u8], header: Header<'b>, data_offset: usize, data_len: usize) {
        // We return if retcode is not valid, as it does not make sense to issue
        // a callback for an invalid frame reception
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

impl<'a, A: time::Alarm, C: ContextStore> Sixlowpan<'a, A, C> {
    /// Sixlowpan::new
    /// --------------
    /// This function initializes and returns a new Sixlowpan struct.
    pub fn new(radio: &'a Mac<'a>,
               ctx_store: C,
               tx_buf: &'static mut [u8],
               clock: &'a A)
               -> Sixlowpan<'a, A, C> {
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
    pub fn set_client(&'a self, client: &'a SixlowpanClient) {
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

        // TODO: Lose buffer if busy
        if self.tx_state.tx_busy.get() {
            Err(ReturnCode::EBUSY)
        } else if ip6_packet_len > ip6_packet.len() {
            Err(ReturnCode::ENOMEM)
        } else {
            self.tx_state.init_transmit(src_mac_addr,
                                        dst_mac_addr,
                                        ip6_packet,
                                        ip6_packet_len,
                                        security,
                                        fragment,
                                        compress);
            self.start_packet_transmit();
            Ok(ReturnCode::SUCCESS)
        }
    }

    fn start_packet_transmit(&self) {
        // Increment dgram_tag
        let dgram_tag = if (self.tx_state.tx_dgram_tag.get() + 1) == 0 {
            1
        } else {
            self.tx_state.tx_dgram_tag.get() + 1
        };

        let frag_buf = self.tx_state.tx_buf
            .take()
            .expect("Error: `tx_buf` is None in call to start_packet_transmit.");

        match self.tx_state.start_transmit(dgram_tag, frag_buf, self.radio, &self.ctx_store) {
            // Successfully started transmitting
            Ok(_) => {
                self.tx_state.tx_dgram_tag.set(dgram_tag);
                self.tx_state.tx_busy.set(true);
            }
            // Otherwise, we failed
            Err((returncode, new_frag_buf)) => {
                self.tx_state.end_transmit(new_frag_buf, self.client.get(), false, returncode);
            }
        }
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
                    let decompressed = sixlowpan_compression::decompress(&self.ctx_store,
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
                                                   &self.ctx_store);
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
        // TODO: May lose tx_buf here
        // TODO: Need to get buffer back from Mac layer on disassociation
        //self.tx_state.end_transmit(self.client.get(), false, ReturnCode::FAIL);
    }
}
