//! `ip_dummy.rs`: IPv6 Test Suite
//!
//! This implements a simple testing framework for sending and receiving
//! IPv6 packets. This framework follows the same conceptual design as the
//! other networking test suites. Two Imix boards run this code, one for
//! receiving and one for transmitting. The transmitting Imix then sends a
//! variety of packets to the receiving Imix, relying on the IPv6 encoding
//! and decoding layer. Note that this layer relies on the lower 6LoWPAN layer.
//! For 6LoWPAN-specific tests, consult the `lowpan_frag_dummy.rs` and
//! `sixlowpan_dummy.rs` files.

use capsules;
extern crate sam4l;
use capsules::net::sixlowpan_compression::{ContextStore, Context};

pub struct DummyStore {
    context0: Context,
}

impl DummyStore {
    pub fn new(context0: Context) -> DummyStore {
        DummyStore { context0: context0 }
    }
}

impl ContextStore for DummyStore {
    fn get_context_from_addr(&self, ip_addr: IPAddr) -> Option<Context> {
        if util::matches_prefix(&ip_addr.0, &self.context0.prefix, self.context0.prefix_len) {
            Some(self.context0)
        } else {
            None
        }
    }

    fn get_context_from_id(&self, ctx_id: u8) -> Option<Context> {
        if ctx_id == 0 {
            Some(self.context0)
        } else {
            None
        }
    }

    fn get_context_from_prefix(&self, prefix: &[u8], prefix_len: u8) -> Option<Context> {
        if prefix_len == self.context0.prefix_len &&
            util::matches_prefix(prefix, &self.context0.prefix, prefix_len) {
            Some(self.context0)
        } else {
            None
        }
    }
}
