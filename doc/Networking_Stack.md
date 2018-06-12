Tock Networking Stack Design Document
=====================================

_NOTE: This document is a work in progress._

TODOS:

-  Implement enough of this design to successfully send UDP packets
   in a manner similar to what was done using the old code
-  Include Receive Paths...
-  More fully flush out how send_done callbacks will work and how
   clients will be assigned/tracked
-  Explanation of Queuing Section


This document describes the design of the Networking stack on Tock.

The design described in this document is based off of ideas contributed by
Phil Levis, Amit Levy, Paul Crews, Hubert Teo, Mateo Garcia, Daniel Giffin, and
Hudson Ayers.

### Table of Contents

This document is split into several sections. These are as follows:

1. Principles - Describes the main principles which the design should meet,
   along with some justification of why these principles matter. Ultimately,
   the design should follow from these principles.
2. Stack Diagram - Graphically depicts the layout of the stack
3. Explanation of queuing - Describes where packets are queued prior to
   transmission.
4. List of Traits - Describes the traits which will exist at each layer of the
   stack. For traits that may seem surprisingly complex, provide examples of
   specific messages that require this more complex trait as opposed to the
   more obvious, simpler trait that might be expected.
5. Implementation Details - Describes how certain implementations of these
   traits will work, providing some examples with pseudocode or commented
   explanations of functionality
6. Example Message Traversals - Shows how different example messages (Thread or
   otherwise) will traverse the stack
7. Suggested File Naming Convention - Currently capsules associated with the
   networking stack are named by a variety of conventions. This section
   proposes a unifying convention.

## Principles

1. Keep the simple case simple
   - Sending an IP packet via an established Thread network should not
     require a more complicated interface than send(destination, packet)
   - If functionality were added to allow for the transmission of IP packets over
     the BLE interface, this IP send function should not have to deal with any
     options or MessageInfo structs that include 802.15.4 layer information.
   - This principle reflects a desire to limit the complexity of Thread/the
     tock networking stack to the capsules that implement the stack. This
     prevents the burden of this complexity from being passed up to whatever
     applications use Thread

2. Layering is separate from encapsulation
   - Libraries that handle encapsulation should not be contained within any
     specific layering construct. For example, If the Thread control unit wants
     to encapsulate a UDP payload inside of a UDP packet inside of an IP packet,
     it should be able to do so using encapsulation libraries and get the
     resulting IP packet without having to pass through all of the protocol layers
   - Accordingly, implementations of layers can take advantage of these
     encapsulation libraries, but are not required to.

3. Dataplane traits are Thread-independent
   - For example, the IP trait should not make any assumption that send()
     will be called for a message that will be passed down to the 15.4 layer, in
     case this IP trait is used on top of an implementation that passes IP
     packets down to be sent over a BLE link layer. Accordingly the IP trait
     can not expose any arguments regarding 802.15.4 security parameters.
   - Even for instances where the only implementation of a trait in the near
     future will be a Thread-based implementation, the traits should not
     require anything that limit such a trait to Thread-based implementations

4. Transmission and reception APIs are decoupled
   - This allows for instances where Receive and send_done callbacks should
     be delivered to different clients (ex: Server listening on all addresses
     but also sending messages from specific addresses)
   - Prevents send path from having to navigate the added complexity required
     for Thread to determine whether to forward received messages up the stack

## Stack Diagram

```
IPv6 over ethernet:      Non-Thread 15.4:   Thread Stack:                                       Encapsulation Libraries
+-------------------+-------------------+----------------------------+
|                         Application                                |-------------------\
----------------------------------------+-------------+---+----------+                    \
|TCP Send| UDP Send |TCP Send| UDP Send |  | TCP Send |   | UDP Send |--\                  v
+--------+----------+--------+----------+  +----------+   +----------+   \               +------------+  +------------+
|     IP Send       |     IP Send       |  |         IP Send         |    \      ----->  | UDP Packet |  | TCP Packet |
|                   |                   |  +-------------------------+     \    /        +------------+  +------------+
|                   |                   |                            |      \  /         +-----------+
|                   |                   |                            |       -+------->  | IP Packet |
|                   |                   |       THREAD               |       /           +-----------+
| IP Send calls eth | IP Send calls 15.4|                   <--------|------>            +-------------------------+
| 6lowpan libs with | 6lowpan libs with |                            |       \ ------->  | 6lowpan compress_Packet |
| default values    | default values    |                            |        \          +-------------------------+
|                   |                   |                            |         \         +-------------------------+
|                   |                   +                +-----------|          ------>  | 6lowpan fragment_Packet |
|                   |                   |                | 15.4 Send |                   +-------------------------+
|-------------------|-------------------+----------------------------+
|     ethernet      |          IEEE 802.15.4 Link Layer              |
+-------------------+------------------------------------------------+
```

Notes on the stack:
- IP messages sent via Thread networks are sent through Thread using an IP Send
  method that exposes only the parameters specified in the IP_Send trait.
  Other parameters of the message (6lowpan decisions, link layer parameters,
  many IP header options) are decided by Thread.
- The stack provides an interface for the application layer to send
  raw IPv6 packets over Thread.
- When the Thread control plane generates messages (MLE messages etc.), they are
  formatted using calls to the encapsulation libraries and then delivered to the
  802.15.4 layer using the 15.4 send function
- This stack design allows Thread to control header elements from transport down
  to link layer, and to set link layer security parameters and more as required
  for certain packers
- The application can either directly send IP messages using the IP Send
  implementation exposed by the Thread stack or it can use the UDP Send
  and TCP send implementation exposed by the Thread stack. If the application
  uses the TCP or UDP send implementations it must use the transport packet library
  to insert its payload inside a packet and set certain header fields.
  The transport send method uses the IP Packet library to set certain
  IP fields before handing the packet off to Thread. Thread then sets other
  parameters at other layers as needed before sending the packet off via the
  15.4 send function implemented for Thread.
- Note that currently this design leaves it up to the application layer to
  decide what interface any given packet will be transmitted from. This is
  because currently we are working towards a minimum functional stack.
  However, once this is working we intend to add a layer below the application
  layer that would handle interface multiplexing by destination address via a
  forwarding table. This should be straightforward to add in to our current
  design.
- This stack does not demonstrate a full set of functionality we are planning to
  implement now. Rather it demonstrates how this setup allows for multiple
  implementations of each layer based off of traits and libraries such that a
  flexible network stack can be configured, rather than creating a network
  stack designed such that applications can only use Thread.


## Explanation of Queuing

**TODO**

Basically, queuing will happen at the application layer in this stack.
Probably we will use a pushing based queuing structure, and since we are
transitioning to more of a scatter-gather implementation this should not be
too expensive from a memory standpoint.

HELP: I am still not entirely clear how this queuing will work in terms of
effectively dealing with multiple interfaces - especially for the instance of
having some packets sent out on the 15.4 radio that are not supposed to pass
through Thread. I guess one important question here is whether there would
ever be an instance where a device was sending some messages through Thread
and other 15.4 messages not through Thread. Why would this happen?

## List of Traits

This section describes a number of traits which must be implemented by any
network stack. It is expected that multiple implementations of some of these
traits may exist to allow for Tock to support more than just Thread networking.

Before discussing these traits - a note on buffers:

>    Prior implementations of the tock networking stack passed around references
>    to 'static mut [u8] to pass packets along the stack. This is not ideal from a
>    standpoint of wanting
>    to be able to prevent as many errors as possible at compile time. The next iteration
>    of code will pass 'typed' buffers up and down the stack. There are a number
>    of packet library traits defined below (e.g. IPPacket, UDPPacket, etc.).
>    Transport Layer traits will be implemented by a struct that will contain at least one field -
>    a [u8] buffer with lifetime 'a. Lower level traits will simply contain
>    payload fields that are Transport Level packet traits (thanks to a
>    TransportPacket enum). This design allows for all buffers passed to
>    be passed as type 'UDPPacket', 'IPPacket', etc. An added advantage of this
>    design is that each buffer can easily be operated on using the library
>    functions associated with this buffer type.


The traits below are organized by the network layer they would typically be
associated with.

### Transport Layer

TODO: TCP, ICMP, RawIP

```rust
pub struct UDPHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub len: u16,
    pub cksum: u16,
}

pub struct UDPSocketExample { /* Example UDP socket implementation */
    pub src_ip: IPAddr,
    pub src_port: u16,
}

pub trait UDPSocket:UDPSend {
    fn bind(&self, src_ip: IPAddr, src_port: u16) -> ReturnCode;
    fn send(&self, dest: IPAddr, udp_packet: &'static mut UDPPacket) -> ReturnCode;
    fn send_done(&self, udp_packet: &'static mut UDPPacket, result: ReturnCode);
}

pub struct UDPPacket<'a> { /* Example UDP Packet struct */
    pub head: UDPHeader,
    pub payload: &'a mut [u8],
    pub len: u16, // length of payload
}

impl<'a> UDPPacket<'a> {
    pub fn reset(&self){} //Sets fields to appropriate defaults
    pub fn get_offset(&self) -> usize{8} //Always returns 8

    pub fn set_dest_port(&self, port: u16){}
    pub fn set_src_port(&self, port: u16){}
    pub fn set_len(&self, len: u16){}
    pub fn set_cksum(&self, cksum: u16){}
    pub fn get_dest_port(&self) -> u16{0}
    pub fn get_src_port(&self) -> u16{0}
    pub fn get_len(&self) -> u16{0}
    pub fn get_cksum(&self) -> u16{0}

    pub fn set_payload(&self, payload: &'a [u8]){}

}

pub trait UDPSend {
    fn send(dest: IPAddr, udp_packet: &'static mut UDPPacket); // dest rqrd
    fn send_done(buf: &'static mut UDPPacket, result: ReturnCode);
}
```

Notes on this UDP implementation:
  - Want to require a socket be used to call UDPSend at the application level
  - May not want this requirement within Thread, so allow Thread to directly
    call UDPSend (this is the reason for separation between UDPSocket and
    UDPSend traits)

### Network Layer

```rust
pub struct IP6Header {
    pub version_class_flow: [u8; 4],
    pub payload_len: u16,
    pub next_header: u8,
    pub hop_limit: u8,
    pub src_addr: IPAddr,
    pub dst_addr: IPAddr,
}


pub enum TransportPacket<'a> {
    UDP(UDPPacket<'a>),
    TCP(TCPPacket<'a>), /* NOTE: TCP,ICMP,RawIP traits not yet detailed in this
                     * document, but follow logically from UDPPacket trait. */
    ICMP(ICMPPacket<'a>),
    Raw(RawIPPacket<'a>),
}

pub struct IP6Packet<'a> {
    pub header: IP6Header,
    pub payload: TransportPacket<'a>,
}


impl<'a> IP6Packet<'a> {
    pub fn new(&mut self, payload: TransportPacket<'a>) -> IP6Packet<'a>{}
/* An IP packet cannot be created without the TransportPacket that will serve
   as its payload. Recall that this transport packet could be a "RawIP" packet */

    pub fn reset(&self){} //Sets fields to appropriate defaults
    pub fn get_offset(&self) -> usize{40} //Always returns 40 until we add options support

    // Remaining functions are just getters and setters for the header fields
    pub fn set_traffic_class(&mut self, new_tc: u8){}
    pub fn set_dscp(&mut self, new_dscp: u8) {}
    pub fn set_ecn(&mut self, new_ecn: u8) {}
    pub fn set_flow_label(&mut self, flow_label: u32){}
    pub fn set_payload_len(&mut self, len: u16){}
    pub fn set_next_header(&mut self, new_nh: u8){}
    pub fn set_hop_limit(&mut self, new_hl: u8) {}
    pub fn set_dest_addr(&mut self, dest: IPAddr){}
    pub fn set_src_addr(&mut self, src: IPAddr){}
    pub fn get_traffic_class(&self) -> u8{}
    pub fn get_dscp(&self) -> u8{}
    pub fn get_ecn(&self) -> u8{}
    pub fn get_flow_label(&self)-> u32{}
    pub fn get_payload_len(&self) -> u16{}
    pub fn get_next_header(&self) -> u8{}
    pub fn get_dest_addr(&self) -> IPAddr{}
    pub fn get_src_addr(&self) -> IPAddr{}
    pub fn set_transpo_cksum(&mut self){} //Looks at internal buffer assuming
    // it contains a valid IP packet, checks the payload type. If the payload
    // type requires a cksum calculation, this function calculates the
    // psuedoheader cksum and calls the appropriate transport packet function
    // using this pseudoheader cksum to set the transport packet cksum

}

pub trait IP6Send {
    fn send_to(&self, dest: IPAddr, ip6_packet: IP6Packet); //Convenience fn, sets dest addr, sends
    fn send(&self, ip6_packet: IP6Packet); //Length can be determined from IP6Packet
    fn send_done(&self, ip6_packet: IP6Packet, result: ReturnCode);
}
```

### 6lowpan Layer

NOTE: At the initial meeting where we planned out the creation of this
document, Phil suggested that we should create a 6lowpan layer with a single
trait (interface) for which the function would be implemented differently
depending on whether 15.4 6lowpan was being used or BLE 6lowpan etc. For now,
there are a number of things that would make such an interface difficult. For
instance, different return types (15.4 fragments or BLE fragments?), and
different parameters for compression (15.4 6lowpan requires source/dest MAC
addresses, not sure whether BLE uses BT hardware addresses the same way).

```rust
pub struct Context { //Required for compression traits
    pub prefix: [u8; 16],
    pub prefix_len: u8,
    pub id: u8,
    pub compress: bool,
}

// A simple enum that encodes the various return states
// for the next_fragment function in the SixlowpanFragment trait
pub enum FragReturn {
  Fail((ReturnCode, &'static mut [u8])),
  Success(Frame),
  Done(Frame),
}

trait Sixlowpan {
  // This is the cleanest way to expose the fact that 6LoWPAN requires
  // some global state - namely, that the dgram tag be incremented for
  // each packet fragmented globally
  fn next_dgram_tag(&self) -> u16;

  // Likewise, this seemed to be the cleanest way to access/store the
  // ContextStore object. Note that having multiple ContextStores is
  // problematic for receiving/decoding packets.
  fn get_ctx_store(&self) -> &ContextStore;
}

trait SixlowpanFragment {
  // This function initializes the SixlowpanFragment object. Note that we
  // (currently) need to pass in a reference to a Sixlowpan object, as that
  // is how we access the ContextStore and update the global dgram tag counter
  // The radio allows us to construct the frames when producing fragments.
  fn new(sixlowpan: &'a Sixlowpan, radio: &'a Mac) -> SixlowpanFragment<'a>;

  // This implementation assumes the user calls init before calling next_fragment
  // for the first time. This makes next_fragment cleaner, as we do not need
  // to keep passing in the same state. Note that some of these arguments can
  // be elided (e.g. security) and passed in via a different method, but
  // this seems clean enough.
  fn init(&self, dst_mac_addr: MacAddress, security: Option<(SecurityLevel, KeyId)>);

  // This function is called repeatedly on the same packet buffer, and produces
  // subsequent frames representing the next fragment to send. frag_buf is
  // consumed in the returned Frame, and the FragReturn enum encodes the
  // different possible return states from this function (Fail, Success, Done).
  fn next_fragment<'b>(&self, packet: &'b IPPacket, frag_buf: &'static mut [u8]) -> FragReturn;
}

pub trait ContextStore {
    fn get_context_from_addr(&self, ip_addr: IPAddr) -> Option<Context>;
    fn get_context_from_id(&self, ctx_id: u8) -> Option<Context>;
    fn get_context_0(&self) -> Context;
    fn get_context_from_prefix(&self, prefix: &[u8], prefix_len: u8) -> Option<Context>;
}

trait SixlowpanCompress {

    /* This function takes in an ipv6 packet and mac addresses and returns the
       compressed header. It is written as a recursive function and requires a
       context store. This is unchanged from the current library
       implementation except that now this will be a trait.
       Note that this function requires link layer MAC Addresses bc these are
       used in computing compressed source/dest IP addresses */

    fn compress(ctx_store: &ContextStore,
                ip6_datagram: &'static mut IPPacket,
                src_mac_addr,
                dst_mac_addr,
                mut buf: &mut [u8]) -> Result<(usize, usize));
}
```

### Link Layer

Note: For now, this description does not provide details of any link layer
other than 802.15.4

The below functions merely describe the already implemented interface for
IEEE 802.15.4 link layer. The stack design that this document
details likely requires virtualization of this interface. This could be
implemented via a method similar to the one that currently exists in
virtual_mac.rs but I believe that some changes will be needed so that
file is current with our other link layer files.

```rust
pub trait MacDevice<'a> {
    /// Sets the transmission client of this MAC device
    fn set_transmit_client(&self, client: &'a TxClient);
    /// Sets the receive client of this MAC device
    fn set_receive_client(&self, client: &'a RxClient);

    /// The short 16-bit address of the MAC device
    fn get_address(&self) -> u16;
    /// The long 64-bit address (EUI-64) of the MAC device
    fn get_address_long(&self) -> [u8; 8];
    /// The 16-bit PAN ID of the MAC device
    fn get_pan(&self) -> u16;

    /// Set the short 16-bit address of the MAC device
    fn set_address(&self, addr: u16);
    /// Set the long 64-bit address (EUI-64) of the MAC device
    fn set_address_long(&self, addr: [u8; 8]);
    /// Set the 16-bit PAN ID of the MAC device
    fn set_pan(&self, id: u16);

    /// This method must be called after one or more calls to `set_*`. If
    /// `set_*` is called without calling `config_commit`, there is no guarantee
    /// that the underlying hardware configuration (addresses, pan ID) is in
    /// line with this MAC device implementation.
    fn config_commit(&self);

    /// Returns if the MAC device is currently on.
    fn is_on(&self) -> bool;

    /// Prepares a mutable buffer slice as an 802.15.4 frame by writing the appropriate
    /// header bytes into the buffer. This needs to be done before adding the
    /// payload because the length of the header is not fixed.
    ///
    /// - `buf`: The mutable buffer slice to use
    /// - `dst_pan`: The destination PAN ID
    /// - `dst_addr`: The destination MAC address
    /// - `src_pan`: The source PAN ID
    /// - `src_addr`: The source MAC address
    /// - `security_needed`: Whether or not this frame should be secured. This
    /// needs to be specified beforehand so that the auxiliary security header
    /// can be pre-inserted.
    ///
    /// Returns either a Frame that is ready to have payload appended to it, or
    /// the mutable buffer if the frame cannot be prepared for any reason
    fn prepare_data_frame(
        &self,
        buf: &'static mut [u8],
        dst_pan: PanID,
        dst_addr: MacAddress,
        src_pan: PanID,
        src_addr: MacAddress,
        security_needed: Option<(SecurityLevel, KeyId)>,
    ) -> Result<Frame, &'static mut [u8]>;

    /// Transmits a frame that has been prepared by the above process. If the
    /// transmission process fails, the buffer inside the frame is returned so
    /// that it can be re-used.
    fn transmit(&self, frame: Frame) -> (ReturnCode, Option<&'static mut [u8]>);
}

/// Trait to be implemented by any user of the IEEE 802.15.4 device that
/// transmits frames. Contains a callback through which the static mutable
/// reference to the frame buffer is returned to the client.
pub trait TxClient {
    /// When transmission is complete or fails, return the buffer used for
    /// transmission to the client. `result` indicates whether or not
    /// the transmission was successful.
    ///
    /// - `spi_buf`: The buffer used to contain the transmitted frame is
    /// returned to the client here.
    /// - `acked`: Whether the transmission was acknowledged.
    /// - `result`: This is `ReturnCode::SUCCESS` if the frame was transmitted,
    /// otherwise an error occured in the transmission pipeline.
    fn send_done(&self, spi_buf: &'static mut [u8], acked: bool, result: ReturnCode);
}

/// Trait to be implemented by users of the IEEE 802.15.4 device that wish to
/// receive frames. The callback is triggered whenever a valid frame is
/// received, verified and unsecured (via the IEEE 802.15.4 security procedure)
/// successfully.
pub trait RxClient {
    /// When a frame is received, this callback is triggered. The client only
    /// receives an immutable borrow of the buffer. Only completely valid,
    /// unsecured frames that have passed the incoming security procedure are
    /// exposed to the client.
    ///
    /// - `buf`: The entire buffer containing the frame, including extra bytes
    /// in front used for the physical layer.
    /// - `header`: A fully-parsed representation of the MAC header, with the
    /// caveat that the auxiliary security header is still included if the frame
    /// was previously secured.
    /// - `data_offset`: Offset of the data payload relative to
    /// `buf`, so that the payload of the frame is contained in
    /// `buf[data_offset..data_offset + data_len]`.
    /// - `data_len`: Length of the data payload
    fn receive<'a>(&self, buf: &'a [u8], header: Header<'a>, data_offset: usize, data_len: usize);
}

// The below code is modified from virtual_mac.rs and provides insight into
// how virtualization of the mac layer might work

// This is one approach, but Phil suggest we probably want to go with the more
// generalize approach (using the virtualizer queue)
pub struct MacUser<'a> {
    mux: &'a MuxMac<'a>,
    operation: MapCell<Op>,
    next: ListLink<'a, MacUser<'a>>,
    tx_client: Cell<Option<&'a mac::TxClient>>,
}

trait MacUserTrait { //a MAC user would implement this and the mac trait
// Alternatively, this send_done function could just be added to the mac trait
// I did not show that here so that the above mac trait would be unchanged
// from the current implementation.
    send_done(spi_buf: &'static mut [u8], acked: bool, result: ReturnCode);

}
```

## Implementation Details

Ultimately, this section will include pseudocode examples of how different
implementation of these traits should look for different Thread messages that
might be sent, and for other messages (non-thread) that might be sent using
this messaging stack.

One Example Implementation of IP6Send:

```rust
/* Implementation of IP6Send Specifically for sending MLE messages. This
implementation is incomplete and not entirely syntactically correct. However it
is useful in that it provides insight into the benefit of having IP6Send
merely be implemented as a trait instead of a layer. This function assumes
that the buffer passed in contains an already formatted IP message. (A
previous function would have been used to create the IP Header and place a UDP
message with an MLE payload inside of it). This message then uses the
appropriate 6lowpan trait implementation to compress/fragment this IP message,
then sets the 15_4 link layer headers and settings as required. Accordingly
this function reveals how an implementation of IP6Send could give control to
Thread at the IP layer, 6lowpan layer, and 15.4 layer. */

impl IP6Send for ThreadMLEIP6Send{
    fn sendTo(&self, dest: IP6Addr, ip6_packet: IP6Packet) {
        ip6_packet.setDestAddr(dest);
        self.send(ip6_packet);
    }

    fn send(&self, ip6_packet: IP6Packet) {
        ip6_packet.setTranspoCksum(); //If packet is UDP etc., this sets the cksum
        ctx_store = sixlowpan_comp::ContextStore::new();
        fragState = sixlowpan_frag::fragState::new(ip6_packet);

        /* Note: the below loop should be replaced with repetitions on callbacks, but
        you get the idea - multiple calls to the frag library are required to
        send all of the link layer frames */

        while(fragState != Done) {
            let fragToSend: 15_4_frag_buf = 15_4_6lowpan_frag(&ip6_packet, fragState);
            fragToSend.setSrcPANID(threadPANID);
            if(ip6_packet.is_disc_request()) { // One example of a thread
                                               // decision that affects link layer parameters
                fragToSend.setSrcMAC(MAC::generate_random());
            }
            // etc.... (More Thread decision making)
            let security = securityType::MLESecurity;
            15_4_link_layer_send(fragToSend, security, len);
        }
    }
}

/* Implementation of IP6Send for an application sitting on top of Thread which
simply wants to send an IP message through Thread. For such an instance the
user does not need to worry about setting parameters below the IP layer, as
Thread handles this. This function reflects Thread making those decisions in
such a scenario */
impl IP6Send for IP6SendThroughThread {
    fn sendTo(&self, dest: IP6Addr, ip6_packet: IP6Packet) {
        setDestAddr(ip6_packet, dest);
        self.send(ip6_packet);
    }

    fn send(&self, ip6_packet: IP6Packet) {
        ip6_packet.setTranspoCksum(); //If packet is UDP, this sets the cksum
        fragState = new fragState(ip6_packet);
        while(fragState != Done) {
            let fragToSend: 15_4_frag_buf = 15_4_6lowpan_frag(&ip6_packet, fragState);
            fragToSend.setSrcPANID(threadPANID);
            fragToSend.setSrcMAC(getSrcMacFromSrcIPaddr(ip6_packet.getSrcIP));
            // etc....
            let security = securityType::LinkLayerSec;
            15_4_link_layer_send(fragToSend, security, len);
        }
    }
}

/* Implementation of UDPSend for an application sitting on top of Thread which
simply wants to send a UDP message through Thread. This simply calls on the
appropriate implementation of IP6Send sitting beneath it. Recall that this
function assumes it is passed an already formatted UDP Packet. Also recall the
assumption that the IPSend function will calculate and set the UDP cksum. */

impl UDPSend for UDPSendThroughThread {
    fn send(&self, dest, udp_packet: UDPPacket) {

        let trans_pkt = TransportPacket::UDP(udp_packet);

        ip6_packet = IPPacket::new(trans_pkt);

        /* First, library calls to format IP Packet */
        ip6_packet.setDstAddr(dest);
        ip6_packet.setSrcAddr(THREAD_GLOBAL_SRC_IP_CONST); /* this fn only
          called for globally destined packets sent over Thread network */
        ip6_packet.setTF(0);
        ip6_packet.setHopLimit(64);
        ip6_packet.setProtocol(UDP_PROTO_CONST);
        ip6_packet.setLen(40 + trans_pkt.get_len());
        /* Now, send the packet */
        IP6SendThroughThread.sendTo(dest, ip6_packet);
    }
}
```

The above implementations are not meant to showcase accurate code, but rather
give an example as to how multiple implementations of a given trait can be
useful in the creation of a flexible network stack. Right now this section
does not contain much, as actually writing all of this example code seems less
productive than simply writing and testing actual code in Tock. These examples
are merely intended to give an idea of how traits will be used in this stack,
so please don't bother nitpicking the examples (for instance, I realize it
doesn't make sense that the function doesn't set all of the IP Header fields,
and that there should be decision making occurring to set the source address,
etc.)

## Example Message Traversals

The Thread specification determines an entire control plane that spans many
different layers in the OSI networking model. To adequately understand the
interactions and dependencies between these layers' behaviors, it might help to
trace several types of messages and see how each layer processes the different
types of messages. Let's trace carefully the way OpenThread handles messages.

We begin with the most fundamental message: a data-plane message that does not
interact with the Thread control plane save for passing through a
Thread-defined network interface. Note that some of the procedures in the below
traces will not make sense when taken independently: the responsibility-passing
will only make sense when all the message types are taken as a whole.
Additionally, no claim is made as to whether or not this sequence of callbacks
is the optimal way to express these interactions: it is just OpenThread's way
of doing it.

### Data plane: IPv6 datagram

1. Upper layer (application) wants to send a payload
  - Provides payload
  - Specifies the IP6 interface to send it on (via some identifier)
  - Specifies protocol (IP6 next header field)
  - Specifies destination IP6 address
  - Possibly doesn't specify source IP6 address
2. IP6 interface dispatcher (with knowledge of all the interfaces) fills in the
  IP6 header and produces an IP6 message
  - Payload, protocol, and destination address used directly from the upper layer
  - Source address is more complicated
    - If the address is specified and is not multicast, it is used directly
    - If the address is unspecified or multicast, source address is determined
      from the specific IP6 selected AND the destination address via a matching scheme on
      the addresses associated with the interface.
  - Now that the addresses are determined, the IP6 layer computes the pseudoheader
    checksum.
    - If the application layer's payload has a checksum that includes the pseudoheader
      (UDP, ICMP6), this partial checksum is now used to update the checksum field in the payload.
3. The actual IP6 interface (Thread-controlled) tries to send that message
  - First step is to determine whether the message can be sent immediately or not (sleepy child or not).
       This passes the message to the scheduler. This is important for sleepy children where there is a
       control scheme that determines when messages are sent.
  - Next, determine the MAC src/dest addresses.
    - If this is a direct transmission, there is a source matching scheme to determine if the destination address
      used should be short or long. The same length is used for the source MAC address, obtained from the MAC interface.
  - Notify the MAC layer to notify you that your message can be sent.
4. The MAC layer schedules its transmissions and determines that it can send the above message
  - MAC sets the transmission power
  - MAC sets the channel differently depending on the message type
5. The IP6 interface fills up the frame. This is the chance for the IP6 interface to do things like
  fragmentation, retransmission, and so on. The MAC layer just wants a frame.
  - XXX: The IP6 interface fills up the MAC header. This should really be the responsibility of the MAC layer.
    Anyway, here is what is done:
    - Channel, source PAN ID, destination PAN ID, and security modes are determined by message type.
      Note that the channel set by the MAC layer is sometimes overwritten.
    - A mesh extension header is added for some messages. (eg. indirect transmissions)
  - The IP6 message is then 6LoWPAN-compressed/fragmented into the payload section of the frame.
6. The MAC layer receives the raw frame and tries to send it
  - MAC sets the sequence number of the frame (from the previous sequence number for the correct link neighbor),
    if it is not a retransmission
  - The frame is secured if needed. This is another can of worms:
    - Frame counter is dependent on the link neighbor and whether or not the frame is a retransmission
    - Key is dependent on which key id mode is selected, and also the link neighbor's key sequence
    - Key sequence != frame counter
    - One particular mode requires using a key, source and frame counter that is a Thread-defined constant.
  - The frame is transmitted, an ACK is waited for, and the process completes.

As you can see, the data dependencies are nowhere as clean as the OSI model
dictates. The complexity mostly arises because

- Layer 4 checksum can include IPv6 pseudoheader
- IP6 source address (mesh local? link local? multicast?) is determined by
  interface and destination address
- MAC src/dest addresses are dependent on the next device on the route to the
  IP6 destination address
- Channel, src/dest PAN ID, security is dependent on message type
- Mesh extension header presence is dependent on message type
- Sequence number is dependent on message type and destination

Note that all of the MAC layer dependencies in step 5 can be pre-decided so
that the MAC layer is the only one responsible for writing the MAC header.

This gives a pretty good overview of what minimally needs to be done to even be
able to send normal IPv6 datagrams, but does not cover all of Thread's
complexities. Next, we look at some control-plane messages.

### Control plane: MLE messages

1. The MLE layer encapsulates its messages in UDP on a constant port
  - Security is determined by MLE message type. If MLE-layer security is
    required, the frame is secured using the same CCM* encryption scheme used
    in the MAC layer, but with a different key discipline.
  - MLE key sequence is global across a single Thread device
  - MLE sets IP6 source address to the interface's link local address
2. This UDP-encapsulated MLE message is sent to the IP6 dispatch again
3. The actual IP6 interface (Thread-controlled) tries to send that message
4. The MAC layer schedules the transmission
5. The IP6 interface fills up the frame.
  - MLE messages disable link-layer security when MLE-layer security is
    present. However, if link-layer security is disabled and the MLE message
    doesn't fit in a single frame, link-layer security is enabled so that
    fragmentation can proceed.
6. The MAC layer receives the raw frame and tries to send it

The only cross-layer dependency introduced by the MLE layer is the dependency
between MLE-layer security and link-layer security. Whether or not the MLE
layer sits atop an actual UDP socket is an implementation detail.

### Control plane: Mesh forwarding

If Thread REED devices are to be eventually supported in Tock, then we must
also consider this case. If a frame is sent to a router which is not its final
destination, then the router must forward that message to the next hop.

1. The MAC layer receives a frame, decrypts it and passes it to the IP6 interface
2. The IP6 reception reads the frame and realizes that it is an indirect
   transmission that has to be forwarded again
  - The frame must contain a mesh header, and the HopsLeft field in it should
    be decremented
  - The rest of the payload remains the same
  - Hence, the IP6 interface needs to send a raw 6LoWPAN-compressed frame
3. The IP6 transmission interface receives a raw 6LoWPAN-compressed frame to be
   transmitted again
  - This frame must still be scheduled: it might be destined for a sleepy
    device that is not yet awake
4. The MAC layer schedules the transmission
5. The IP6 transmission interface copies the frame to be retransmitted
   verbatim, but with the modified mesh header and a new MAC header
6. The MAC layer receives the raw frame and tries to send it

This example shows that the IP6 transmission interface may need to handle more
message types than just IP6 datagrams: there is a case where it is convenient
to be able to handle a datagram that is already 6LoWPAN compressed.

### Control plane: MAC data polling

From time to time, a sleepy edge device will wake up and begin polling its
parent to check if any frames are available for it. This is done via a MAC
command frame, which must still be sent through the transmission pipeline with
link security enabled (Key ID mode 1).  OpenThread does this by routing it
through the IP6 transmission interface, which arguably isn't the right choice.

1. Data poll manager send a data poll message directly to the IP6 transmission
   interface, skipping the IP6 dispatch
2. The IP6 transmission interface notices the different type of message, which
   always warrants a direct transmission.
3. The MAC layer schedules the transmission
4. The IP6 transmission interface fills in the frame
  - The MAC dest is set to the parent of this node and the MAC src is set to be
    the same length as the address of the parent
  - The payload is filled up to contain the Data Request MAC command
  - The MAC security level and key ID mode is also fixed for MAC commands under
    the Thread specification
5. The MAC layer secures the frame and sends it out

We could imagine giving the data poll manager direct access as a client of the
MAC layer to avoid having to shuffle data through the IP6 transmission
interface. This is only justified because MAC command frames are never
6LoWPAN-compressed or fragmented, nor do they depend on the IP6 interface in
any way.

### Control plane: Child supervision

This type of message behaves similarly to the MAC data polls. The message is
essentially and empty MAC frame, but OpenThread chooses to also route it
through the IP6 transmission interface. It would be far better to allow a child
supervision implementation to be a direct client of the MAC interface.

### Control plane: Joiner entrust and MLE announce

These two message types are also explicitly marked, because they require a
specific Key ID Mode to be selected when producing the frame for the MAC
interface.

### Caveat about MAC layer security

So far, it seems like we can expect the MAC layer to have no cross-layer
dependencies: it receives frames with a completely specified description of how
they are to be secured and transmitted, and just does so. However, this is not
entirely the case.

When the frame is being secured, the key ID mode has been set by the upper
layers as described above, and this key ID mode is used to select between a few
different key disciplines. For example, mode 0 is only used by Joiner entrust
messages and uses the Thread KEK sequence. Mode 1 uses the MAC key sequence and
Mode 2 is a constant key used only in MLE announce messages. Hence, this key ID
mode selection is actually enabling an upper layer to determine the specific
key being used in the link layer.

Note that we cannot just reduce this dependency by allowing the upper layer to
specify the key used in MAC encryption. During frame reception, the MAC layer
itself has to know which key to use in order to decrypt the frames correctly.

TODO: Receive path dependencies

## Suggested File Names

Currently capsules associated with the networking stack are named according to a
variety of conventions. This section proposes some changes.

`Thread` directory should eventually contain any layers implemented with
specifically Thread in mind - files such as ip_thread.rs, which would contain
implementations of the traits found in net::ip.rs. (For instance, this file
would contain the implementations of the IPSend trait such as IPSendMLE.)
The tlv.rs file which currently resides here should remain here as well.

`net::udp.rs` - Should contain functions and traits associated with the generic UDP layer.
For instance, this file would include the UDPPacket and UDPSend traits, the
UDPHeader struct, and functions associated with calculating the UDP checksum.
This file should contain basic library implementations for the UDPPacket trait.

`net::ip.rs` - Should contain functions associated with the generic IP layer. For
instance, this file would include the IPPacket and IPSend traits, the IPPacket
struct, and any other functions which any IP implementation would have to
implement. This function would also provide documentation regarding how this
generic layer should be implemented. This file should contain basic library
implementations for the IPPacket Trait.

`net::ip_utils.rs` - The functions contained in the file currently named ip.rs
should be moved into a file named ip_utils.rs to better indicate the purpose
of this file.

`net::sixlowpan_utils.rs` - The function currently found at net::util.rs should
be renamed to sixlowpan_utils.rs to better reflect its purpose. Further, it
seems as though the functions/structs currently contained in
net::frag_utils.rs should simply be moved into this file as well.

`net::sixlowpan_frag.rs` - Contains functions and interfaces pertaining to the
sixlowpan fragmentation library. (What is currently called sixlowpan.rs)

`net::sixlowpan_comp.rs` - Contains functions and interfaces pertaining to the
sixlowpan compression library.

`ieee802154_enc_dec.rs` - Implements 15.4 header encoding and decoding.
Currently should found at net::ieee802154.rs - should be renamed and
moved within the greater ieee802154 directory in capsules.

ieee802154 directory can be left as is.


Once the traits have been defined and placed in udp.rs/ip.rs etc. , the
various implementations of these traits should be defined in their own files,
such as udp_through_thread.rs (could contain the UDPSend trait implementations to
be used by applications sending UDP across an established Thread network).
Another file could simultaneously exist called 'udp_no_thread.rs' which is
used for an implementation that allowed for sending through a stack which was
totally separate from the Thread stack.



