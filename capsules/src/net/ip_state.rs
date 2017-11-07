//! Current implementation details:
//! - Performs single-dispatch semantics; will not deliver a received packet
//!   to multiple IPStates (even if they match)
//! - Does not understand subnet equality
//! - Does *not* perform fair scheduling on the ready "queue" - simply sends
//!   the next packet immediately. Should be changed to do something more
//!   round-robin style

use core::cell::Cell;
use net::ip;
use net::ip::{IPAddr, IP6Header};
use net::sixlowpan;
use net::sixlowpan::{SixlowpanClient, Sixlowpan};
use net::sixlowpan_compression::ContextStore;
use net::ieee802154::MacAddress;
use kernel::ReturnCode;
use kernel::hil::time;
use kernel::common::list::{List, ListLink, ListNode};
use kernel::common::take_cell::{TakeCell, MapCell};

// TODO: Remove
pub const SRC_MAC_ADDR: MacAddress = MacAddress::Long([0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17]);
pub const DST_MAC_ADDR: MacAddress = MacAddress::Long([0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
                                                       0x1f]);

#[derive(Eq,PartialEq,Debug)]
enum IPSendingState {
    Idle,
    Ready(&'static mut [u8], usize),
    Sending,
}

pub trait IPClient {
    fn receive<'a>(&self, buf: &'a [u8], len: u16, result: ReturnCode);
    fn send_done(&self, buf: &'static mut [u8], acked: bool, result: ReturnCode);
}

pub struct IPState<'a> {
    addr: Cell<IPAddr>,
    client: Cell<Option<&'a IPClient>>,
    state: MapCell<IPSendingState>,
    next: ListLink<'a, IPState<'a>>,
}

impl<'a> ListNode<'a, IPState<'a>> for IPState<'a> {
    fn next(&'a self) -> &'a ListLink<IPState<'a>> {
        &self.next
    }
}

impl<'a> IPState<'a> {
    pub fn new(addr: IPAddr) -> IPState<'a> {
        IPState {
            addr: Cell::new(addr),
            client: Cell::new(None),
            state: MapCell::new(IPSendingState::Idle),
            next: ListLink::empty(),
        }
    }

    // This function allows an application to set or change the IPv6 address
    // corresponding to the IPState instance.
    pub fn set_addr(&self, addr: IPAddr) {
        self.addr.set(addr);
    }

    // This function allows an application to set which IPClient should receive
    // the `send_done` and `receive` callbacks.
    pub fn set_client(&self, client: &'a IPClient) {
        self.client.set(Some(client));
    }

    // This helper function determines address equality; at some point, this
    // should be expanded to include subnet equality
    fn is_my_addr(&self, addr: IPAddr) -> bool {
        self.addr.get().is_equal(addr)
    }

    // TODO: This should return an error? Yes
    fn initialize_packet<'b>(&self, ip6_packet: &'b mut [u8], payload: &[u8], payload_len: usize)
            -> usize {
        let mut ip6_header = IP6Header::new();
        // TODO: THIS IS WRONG - needs to be in octets
        ip6_header.set_payload_len(payload_len as u16);
        ip6_header.src_addr = self.addr.get();
        ip::IP6Header::encode(ip6_packet, ip6_header);
        ip6_packet[40..40+payload_len].copy_from_slice(&payload[0..payload_len]);
        // TODO: Get from ip6_header
        40 + payload_len
    }

    fn received_packet<'b>(&self, ip6_header: &IP6Header, buf: &'b [u8]) {
    }

    fn send_done(&self, buf: &'static mut [u8], acked: bool, result: ReturnCode) {
        self.client.get().map(move |client| client.send_done(buf, acked, result));
    }
}

pub struct IPLayer<'a, A: time::Alarm + 'a, C: ContextStore> {
    ip_states: List<'a, IPState<'a>>,
    sending: Cell<bool>,
    ip6_buffer: TakeCell<'static, [u8]>,
    // TODO: I think that the ContextStore should be a Thread-level (or
    // application level) thing, and so passed-in during intialization
    sixlowpan: Sixlowpan<'a, A, C>,
}

impl<'a, A: time::Alarm, C: ContextStore> SixlowpanClient for IPLayer<'a, A, C> {
    fn receive<'b>(&self, buf: &'b [u8], len: u16, result: ReturnCode) {
        // If the decode fails, silently drop the packet
        // TODO: Decode should also perform sanity-checking on the input
        IP6Header::decode(buf).done().map(|(_, ip6_header)| {
            // TODO: Check if IP header is valid
            let addr = ip6_header.dst_addr;
            let ip_state = self.ip_states.iter().find(|state| state.is_my_addr(addr));
            // If there is no matching `IPState`, silently drop the packet
            ip_state.map(|ip_state| ip_state.received_packet(&ip6_header, buf));
        });
    }

    // TODO: In order to determine *who* sent the packet, we need to maintain
    // the invariant that buf is not modified by lower layers
    // TODO: Or change the callback to include state data
    fn send_done(&self, buf: &'static mut [u8], acked: bool, result: ReturnCode) {
        // The IPLayer is no longer sending
        self.sending.set(false);

        IP6Header::decode(buf).done().map(|(_, ip6_header)| {
            // TODO: Check validity of IP header
            let addr = ip6_header.src_addr;
            // If there is no matching `IPState`, silently drop the packet
            let ip_state = self.ip_states.iter().find(|state| state.is_my_addr(addr));
            ip_state.map(move |ip_state| ip_state.send_done(buf, acked, result));
        });

        // Start transmitting next packet - note that this *might not* succeed
        // as the client may have called `send` again in the `send_done`
        // callback
        // TODO: Is this desired behavior?
        let mut ip6_buf = self.ip6_buffer.take().unwrap();
        self.send_pending_packets(ip6_buf);
    }
}

impl<'a, A: time::Alarm, C: ContextStore> IPLayer<'a, A, C> {
    pub fn new(ip6_buffer: &'static mut [u8], sixlowpan: Sixlowpan<'a, A, C>)
            -> IPLayer<'a, A, C> {
        IPLayer {
            ip_states: List::new(),
            sending: Cell::new(false),
            ip6_buffer: TakeCell::new(ip6_buffer),
            sixlowpan: sixlowpan,
        }
    }

    pub fn add_ip_state(&self, ip_state: &'a IPState<'a>) {
        self.ip_states.push_head(ip_state);
    }

    pub fn send(&self, ip_state: &'a IPState<'a>, buf: &'static mut [u8], len: usize) {
        // TODO: Return err if not idle
        ip_state.state.map(move |state| {
            match *state {
                IPSendingState::Idle => { *state = IPSendingState::Ready(buf, len); },
                _ => {},
            };
        });
        // If we are not currently transmitting
        if !self.sending.get() {
            self.sending.set(true);
        }
    }

    // On error, ip6_packet is returned
    fn send_pending_packets(&self, ip6_packet: &'static mut [u8]) {
        self.ip_states.iter().for_each(|ip_state| {
            ip_state.state.map(move |state| {
                match *state {
                    IPSendingState::Ready(ref buf, len) => {
                        let mut payload = buf;
                        let total_len = ip_state.initialize_packet(ip6_packet, payload, len);
                        self.sixlowpan.transmit_packet(SRC_MAC_ADDR,
                                                       DST_MAC_ADDR,
                                                       ip6_packet,
                                                       total_len,
                                                       None,
                                                       true,
                                                       true);
                        *state = IPSendingState::Sending;
                        return;
                    },
                    // If not Ready, continue
                    _ => {},
                };
            });
        });
    }
}
