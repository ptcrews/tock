//! ICMPv6 layer of the Tock networking stack.
//!
//! - Author: Conor McAvity <cmcavity@stanford.edu>

use core::cell::Cell;
use net::icmpv6::icmpv6::ICMP6Header;
use net::ipv6::ipv6::TransportHeader;
use net::ipv6::ip_utils::IPAddr;
use net::ipv6::ipv6_send::{IP6Sender, IP6Client};
use kernel::ReturnCode;

pub trait ICMP6SendClient {
    fn send_done(&self, result: ReturnCode);
}

pub struct ICMP6SendStruct<'a, T: IP6Sender<'a> + 'a> {
    ip_send_struct: T,
    client: Cell<Option<&'a ICMP6SendClient>>,
}

impl<'a, T: IP6Sender<'a>> ICMP6SendStruct<'a, T> {
    pub fn new(ip_send_struct: T) -> ICMP6SendStruct<'a, T> {
        ICMP6SendStruct {
            ip_send_struct: ip_send_struct,
            client: Cell::new(None),
        }
    }
    
    pub fn set_client(&self, client: &'a ICMP6SendClient) {
        self.client.set(Some(client));
    }

    pub fn send(&self, dest: IPAddr, icmp_header: ICMP6Header, buf: &'a [u8]) 
            -> ReturnCode {
        let transport_header = TransportHeader::ICMP(icmp_header);
        self.ip_send_struct.send_to(dest, transport_header, buf)
    }
}

impl<'a, T: IP6Sender<'a>> IP6Client for ICMP6SendStruct<'a, T> {
    fn send_done(&self, result: ReturnCode) {
        self.client.get().map(|client| client.send_done(result));
    }
}
