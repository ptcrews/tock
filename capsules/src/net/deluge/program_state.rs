use core::cell::Cell;
use kernel::returncode::ReturnCode;
use kernel::common::take_cell::TakeCell;
use net::deluge::flash_layer::{DelugeFlashClient, DelugeFlashState};

pub trait DelugeProgramStateClient {
    fn read_complete(&self, page_num: usize, packet_num: usize, buffer: &[u8]);
    fn write_complete(&self, page_completed: bool);
}

pub trait DelugeProgramState<'a> {
    // This is called externally, when something updates our binary
    // TODO: Should this only be for testing?
    fn updated_application(&self, new_version: usize, page_count: usize);

    fn received_new_version(&self, version: usize);
    fn receive_packet(&self, version: usize, page_num: usize, packet_num: usize, payload: &[u8]) -> bool;
    fn current_page_number(&self) -> usize;
    fn current_version_number(&self) -> usize;
    fn current_packet_number(&self) -> usize;
    // Result return asynchronously
    fn get_requested_packet(&self, page_num: usize, packet_num: usize) -> bool;
    fn set_client(&self, client: &'a DelugeProgramStateClient);
}

pub const PAGE_SIZE: usize = 512;
pub const PACKET_SIZE: usize = 64;
//const BIT_VECTOR_SIZE: usize = (PAGE_SIZE/PACKET_SIZE)/8;

pub enum ProgramStateReturnType {
    ERROR,
    OUTDATED,
    INVALID_PACKET,
    BUSY,
}

// TODO: Support odd-sized last pages
pub struct ProgramState<'a> {
    unique_id: usize,               // Program ID (global across all nodes)
    version: Cell<usize>,           // Page version

    // State for requested packet
    // Note that since we can only have one outstanding request to
    // the flash driver, we only have one state. We keep the state here
    // as the flash driver has no concept of packet numbers
    requested_packet_num: Cell<usize>,
    requested_page_num: Cell<usize>,

    //tx_page_vector: Cell<[u8; BIT_VECTOR_SIZE]>,
    tx_page_num: Cell<usize>,
    tx_page: TakeCell<'static, [u8; PAGE_SIZE]>,  // Page
    tx_page_is_stale: Cell<bool>,

    //rx_page_vector: Cell<[u8; BIT_VECTOR_SIZE]>,
    rx_largest_packet: Cell<usize>, // Change to bitvector eventually
    rx_page_num: Cell<usize>,       // Also largest page num ready for transfer
    rx_page: TakeCell<'static, [u8; PAGE_SIZE]>,  // Page

    flash_driver: &'a DelugeFlashState<'a>,
    client: Cell<Option<&'a DelugeProgramStateClient>>,

}

impl<'a> ProgramState<'a> {
    // We load the first page on initialization
    pub fn new(flash_driver: &'a DelugeFlashState<'a>,
               unique_id: usize,
               tx_page: &'static mut [u8; PAGE_SIZE],
               rx_page: &'static mut [u8; PAGE_SIZE]) -> ProgramState<'a> {
        ProgramState {
            unique_id: unique_id,
            version: Cell::new(1),

            requested_packet_num: Cell::new(0),
            requested_page_num: Cell::new(0),

            tx_page_num: Cell::new(0),
            tx_page: TakeCell::new(tx_page),
            tx_page_is_stale: Cell::new(false),

            rx_largest_packet: Cell::new(0),
            rx_page_num: Cell::new(0),
            rx_page: TakeCell::new(rx_page),

            flash_driver: flash_driver,
            client: Cell::new(None),
        }
    }

    fn page_completed(&self) -> ReturnCode {
        let ret_code = self.rx_page.map(|rx_page|
                                        self.flash_driver.page_completed(self.rx_page_num.get(), rx_page)
                                       ).unwrap_or(ReturnCode::ENOMEM);
        if ret_code == ReturnCode::SUCCESS {
            // TODO: Should these be here, or in the callback?
            self.rx_page_num.set(self.rx_page_num.get() + 1);
            self.rx_largest_packet.set(0);
        }
        ret_code
    }
}

impl<'a> DelugeFlashClient for ProgramState<'a> {
    fn read_complete(&self, buffer: &[u8]) {
        // NOTE: We previously checked the validity of packet_num, so we
        // can just index into the received page
        let packet_num = self.requested_packet_num.get();
        let page_num = self.requested_page_num.get();
        // Update tx_page_num here
        self.tx_page_num.set(page_num);
        // TODO: The tx_page should **REALLY** be here
        self.tx_page.map(|tx_page| {
            // buffer and tx_page *should* be the same size
            tx_page.copy_from_slice(&buffer[0..PAGE_SIZE]);
            let offset = packet_num * PAGE_SIZE;
            self.client.get().map(|client|
                                  client.read_complete(page_num,
                                                       packet_num,
                                                       &tx_page[offset..offset+PAGE_SIZE]));
        }).unwrap(); // Force the panic
    }

    // We receive this after writing a page. This happens when receiving
    // a packet, and this was the last packet in the page.
    fn write_complete(&self) {
        // Must be for an outstanding write request, meaning that the last
        // write triggered a page_write
        // TODO: Do we actually care about the page/packet number here?
        // - Don't think so, because we update the state *before* calling
        // - write on the flash driver
        self.client.get().map(|client| client.write_complete(true));
    }
}

impl<'a> DelugeProgramState<'a> for ProgramState<'a> {
    // TODO: Note that this is slightly dangerous, as the tx_page buffer will
    // now be stale. Even though we go and fetch it, we still have a race
    // condition here -> should probably move "waiting" state tracking into
    // this level
    fn updated_application(&self, new_version: usize, page_count: usize) {
        self.version.set(new_version);
        // Minus one here since rx_page_num is 0-indexed
        self.rx_page_num.set(page_count-1);
        self.tx_page_is_stale.set(true);
    }

    fn received_new_version(&self, version: usize) {
        // If we receive a data packet with a greater version than us and it is
        // the first page, reset our reception state and start receiving the
        // updated information
        if version > self.version.get() {
            self.version.set(version);
            // Reset TX state
            self.tx_page_num.set(0);
            // Reset RX state
            self.rx_largest_packet.set(0);
            self.rx_page_num.set(0);
        }
    }

    // TODO: Currently only supports sequential reception
    fn receive_packet(&self,
                      version: usize,
                      page_num: usize,
                      packet_num: usize,
                      payload: &[u8]) -> bool {

        debug!("ProgramState: receive_packet: {}, {}, {}", version, page_num, packet_num);
        if version > self.version.get() {
            debug!("ProgramState: new version");
            self.received_new_version(version);
        }
        let offset = packet_num * PACKET_SIZE;
        if offset + payload.len() > PAGE_SIZE {
            // TODO: Error
            // Packet too large
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

        // TODO: Mark complete
        if packet_num * PACKET_SIZE == PAGE_SIZE {
            // This triggers a write to the flash layer, and the client will
            // receive the callback asynchronously
            // TODO: Should make this entire function return ReturnCode
            if self.page_completed() == ReturnCode::SUCCESS {
                return true;
            } else {
                return false;
            }
        }
        self.client.get().map(|client| client.write_complete(false));
        true
    }

    fn current_page_number(&self) -> usize {
        self.rx_page_num.get()
    }

    fn current_version_number(&self) -> usize {
        self.version.get()
    }

    fn current_packet_number(&self) -> usize {
        debug!("Current packet number: {}", self.rx_largest_packet.get());
        self.rx_largest_packet.get()
    }

    // TODO: Make this an asynchrounous request to the flash layer
    // Make all retrieved pages be passed via the asynch interface
    fn get_requested_packet(&self, page_num: usize, packet_num: usize) -> bool {
        debug!("Get requested packet: {}", packet_num);
        // If we haven't received the latest page
        // NOTE: This is absolutely crucial to the correctness of the algorithm,
        // as we can receive in any state - if we attempt to transmit while
        // receiving the same *page*, we can get into an inconsistent state
        if page_num > self.rx_page_num.get() {
            return false;
        }

        // TODO: Check for specific length
        let offset = packet_num * PACKET_SIZE;
        if offset + PACKET_SIZE > PAGE_SIZE {
            return false;
        }

        // If the page is a different page than the one we currently have, need
        // to asynchronously read from flash. Note that the is_stale variable
        // is only set when we manually force an update by calling
        // updated_application
        if self.tx_page_is_stale.get() || page_num != self.tx_page_num.get() {
            match self.flash_driver.get_page(page_num) {
                ReturnCode::SUCCESS => {
                    // Set state for request
                    self.requested_packet_num.set(packet_num);
                    self.requested_page_num.set(page_num);
                },
                _ => {
                    // Some issue with the flash driver
                    return false;
                }
            }
            // Successfully issued the asynchronous request
            return true;
        }

        // We have the page in our buffer
        self.tx_page.map(|tx_page| {
            self.client.get().map(|client|
                                  client.read_complete(page_num,
                                                       packet_num,
                                                       &tx_page[offset..offset+PACKET_SIZE]));
            true
        }).unwrap_or(false)
        // Return true or false if the buffer didn't exist
    }

    fn set_client(&self, client: &'a DelugeProgramStateClient) {
        self.client.set(Some(client));
    }
}
