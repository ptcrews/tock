use core::cell::Cell;
use kernel::common::take_cell::TakeCell;

pub trait ProgramStateClient {
    fn get_next_page(&self);
    fn get_page_num(&self, page_num: usize) -> &mut [u8];
    fn page_completed(&self, completed_page: &mut [u8]);
}

pub trait DelugeProgramState {
    fn receive_packet(&self, version: usize, page_num: usize, packet_num: usize, payload: &[u8]) -> bool;
    fn current_page_number(&self) -> usize;
    fn current_version_number(&self) -> usize;
    fn current_packet_number(&self) -> usize;
    fn get_requested_packet(&self, version: usize, page_num: usize, packet_num: usize, buf: &mut [u8]) -> bool;
}

const PAGE_SIZE: usize = 512;
pub const PACKET_SIZE: usize = 64;
//const BIT_VECTOR_SIZE: usize = (PAGE_SIZE/PACKET_SIZE)/8;

// TODO: Support odd-sized last pages
pub struct ProgramState<'a> {
    unique_id: usize,               // Program ID (global across all nodes)
    version: Cell<usize>,           // Page version

    //tx_page_vector: Cell<[u8; BIT_VECTOR_SIZE]>,
    tx_requested_packet: Cell<bool>, // Change to bitvector eventually
    tx_requested_packet_num: Cell<usize>,
    tx_page_num: Cell<usize>,
    tx_page: TakeCell<'static, [u8; PAGE_SIZE]>,  // Page

    //rx_page_vector: Cell<[u8; BIT_VECTOR_SIZE]>,
    rx_largest_packet: Cell<usize>, // Change to bitvector eventually
    rx_page_num: Cell<usize>,       // Also largest page num ready for transfer
    rx_page: TakeCell<'static, [u8; PAGE_SIZE]>,  // Page

    client: &'a ProgramStateClient,

}

impl<'a> ProgramState<'a> {
    // We load the first page on initialization
    pub fn new(unique_id: usize,
               page_len: usize,
               tx_page: &'static mut [u8; PAGE_SIZE],
               rx_page: &'static mut [u8; PAGE_SIZE],
               client: &'a ProgramStateClient) -> ProgramState<'a> {
        ProgramState {
            unique_id: unique_id,
            version: Cell::new(0),

            tx_requested_packet: Cell::new(false),
            tx_requested_packet_num: Cell::new(0),
            tx_page_num: Cell::new(0),
            tx_page: TakeCell::new(tx_page),

            rx_largest_packet: Cell::new(0),
            rx_page_num: Cell::new(0),
            rx_page: TakeCell::new(rx_page),

            client: client,
        }
    }
}

impl<'a> DelugeProgramState for ProgramState<'a> {
    fn receive_packet(&self, version: usize, page_num: usize, packet_num: usize, payload: &[u8]) -> bool {
        // If we receive a data packet with a greater version than us and it is
        // the first page, reset our reception state and start receiving the
        // updated information
        if version > self.version.get() && page_num == 0 {
            self.version.set(version);
            // Reset TX state
            self.tx_requested_packet.set(false);
            self.tx_requested_packet_num.set(0);
            self.tx_page_num.set(0);
            // Reset RX state
            self.rx_largest_packet.set(0);
            self.rx_page_num.set(0);
        }
        let offset = packet_num * PACKET_SIZE;
        if offset + payload.len() > PAGE_SIZE {
            // TODO: Error
            return false;
        }
        if self.rx_page_num.get() != page_num {
            // TODO: Error
            return false;
        }
        if self.rx_largest_packet.get() + 1 != packet_num {
            // TODO: Error
            return false;
        }
        self.rx_largest_packet.set(packet_num);
        self.rx_page.map(|page| page[offset..].copy_from_slice(payload));
        true
    }

    fn current_page_number(&self) -> usize {
        self.rx_page_num.get()
    }

    fn current_version_number(&self) -> usize {
        self.version.get()
    }

    fn current_packet_number(&self) -> usize {
        self.rx_largest_packet.get()
    }

    fn get_requested_packet(&self, version: usize, page_num: usize, packet_num: usize, buf: &mut [u8]) -> bool {
        if version != self.version.get() {
            return false;
        }
        if page_num > self.rx_largest_packet.get() {
            return false;
        }
        if page_num != self.tx_page_num.get() {
            // TODO: Load page
        }
        // Check for specific length
        let offset = packet_num * PACKET_SIZE;
        if offset + PACKET_SIZE > PAGE_SIZE {
            return false;
        }
        self.tx_page.map(|tx_page| buf.copy_from_slice(&tx_page[offset..offset+PACKET_SIZE]));
        true
    }
}
