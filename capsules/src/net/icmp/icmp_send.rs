//! ICMP layer of the Tock networking stack.
//!
//! - Author: Conor McAvity <cmcavity@stanford.edu>

use core::cell::Cell;
use net::icmp::icmp::ICMPHeader;
use net::ipv6::ipv6::TransportHeader;
use net::ipv6::ip_utils::IPAddr;
use net::ipv6::ipv6_send::{IP6SendStruct, IP6Client};
use kernel::ReturnCode;

pub trait ICMPSendClient {
    fn send_done(&self, result: ReturnCode);
}

pub struct ICMPSendStruct<'a> {
    ip_send_struct: &'a IP6SendStruct<'a>,
    client: Cell<Option<&'a ICMPSendClient>>,
}

impl<'a> ICMPSendStruct<'a> {
    pub fn new(ip_send_struct: &'a IP6SendStruct<'a>) -> ICMPSendStruct<'a> {
        ICMPSendStruct {
            ip_send_struct: ip_send_struct,
            client: Cell::new(None),
        }
    }
    
    pub fn set_client(&self, client: &'a ICMPSendClient) {
        self.client.set(Some(client));
    }

    pub fn send(&self, dest: IPAddr, icmp_header: ICMPHeader, buf: &'a [u8]) -> ReturnCode {
        let transport_header = TransportHeader::ICMP(icmp_header);
        self.ip_send_struct.send_to(dest, transport_header, buf)
    }
}

impl<'a> IP6Client for ICMPSendStruct<'a> {
    fn send_done(&self, result: ReturnCode) {
        self.client.get().map(|client| client.send_done(result));
    }
}
