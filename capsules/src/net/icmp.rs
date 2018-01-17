use net::ip_utils::IPAddr;
use kernel::ReturnCode;

pub struct ICMPHeader {
    pub type: u8,
    pub code: u8,
    pub cksum: u16,
}

pub struct ICMPSocketExample {
    pub src_ip: IPAddr,
}

pub trait ICMPSocket:ICMPSend {
    fn bind(&self, src_ip: IPAddr) -> ReturnCode;
    fn send(&self, dest: IPAddr, icmp_packet: &'static mut ICMPPacket) -> ReturnCode;
    // TODO: What is this function used for??
    fn send_done(&self, icmp_packet: &'static mut ICMPPacket, result: ReturnCode);
}

pub struct ICMPPacket<'a> {
    pub header: ICMPHeader,
    pub payload: &'a mut [u8],
    pub len: u16,
}

impl<'a> ICMPPacket<'a> {
    pub fn reset(&self){}
    pub fn get_offset(&self) -> usize{8}
}
