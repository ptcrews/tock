/* Though TCP has not yet been implemented for the Tock Networking stackm
   this file defines the structure of the TCPHeader and TCPPacket structs
   so that TCPPacket can be included for clarity as part of the
   TransportPacket enum */

pub struct TCPHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq_num: u32,
    pub ack_num: u32,
    pub offset_and_control: u16,
    pub window: u16,
    pub cksum: u16,
    pub urg_ptr: u16,
}

pub struct TCPPacket<'a> { /* TCP Packet Struct */
    pub head: TCPHeader,
    pub payload: &'a mut [u8],
    pub len: u16, // length of payload
}
