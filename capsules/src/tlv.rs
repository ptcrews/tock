//! Implements Thread Type-Length-Value (TLV) formats

/*
pub enum Tlv {
    SourceAddress(u16),   // Sender's 16-bit MAC address
    Mode(/* TODO */),
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
}

pub enum LinkMode {
  ReceiverOnWhenIdle,
  SecureDataRequests,
  FullThreadDevice,         // Vs. Minimal Thread Device
  FullNetworkDataRequired,  // Required by this sender
}

// TODO: Constructor for link mode that takes variable length array of link mode options

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