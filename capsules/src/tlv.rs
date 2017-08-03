//! Implements Thread Type-Length-Value (TLV) formats

use net::stream::SResult;

pub enum Tlv {
    SourceAddress(u16),   // Sender's 16-bit MAC address
    Mode(u8),
    /*
    Timeout(u32),
    Challenge,            // TODO: constructor will generate random byte string, 4 to 8 bytes in length
    Response([u8; 8]),
    LinkLayerFrameCounter(u32),
    LinkQuality,                  // Not used in Thread
    NetworkParameter,             // Not used in Thread
    MleFrameCounter(u32),
    Route64 /* TODO: (params) */,          // NOTE: Not required to implement SED
    Address16(u16),
    LeaderData { partitionId: u32, weighting: u8, dataVersion: u8,
                 stableDataVersion: u8, leaderRouterId: u8 },
    NetworkData /* TODO: (params) */,      // NOTE: Not required to implement SED
    TlvRequest,
    ScanMask(/* TODO */),   // Same as mode, neds to take variable number of enum options
    Connectivity { parentPriority: ParentPriority, linkQuality3: u8, linkQuality2: u8, linkQuality1: u8 }
    */
}

impl Tlv {
  pub fn encode(&self, buf: &mut [u8]) -> SResult {
    match *self {
      Tlv::SourceAddress(ref macAddress) => {
        // TODO:
        // - put this in new branch off of better_mac_headers
        // - use stream_cond to confirm parameters are as expected
      }
    }
  }
}

#[repr(u8)]
pub enum LinkMode {
  ReceiverOnWhenIdle      = 0b00001000,
  SecureDataRequests      = 0b00000100,
  FullThreadDevice        = 0b00000010,         // Vs. Minimal Thread Device
  FullNetworkDataRequired = 0b00000001,  // Required by this sender
}

// TODO: Constructor for link mode that takes variable length array of link mode options

/*
// Used when creating a Scan Mask TLV
pub enum MulticastResponders {
  Router,
  EndDevice,
}

// Used in Connectivity TLV
pub enum ParentPriority {
  High = 0b01,
  Medium = 0b00,
  Low = 0b11,
  // Reserved = 0b10
}
*/

macro_rules! len_cond {
    ($buf:expr, $bytes:expr) => $buf.len() >= $bytes
}

// QUESTION: How are we handling failure? Something like 'SResult'?