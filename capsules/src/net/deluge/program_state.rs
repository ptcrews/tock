use core::cell::Cell;
use kernel::returncode::ReturnCode;
use kernel::common::take_cell::TakeCell;
use net::deluge::flash_layer::DelugeFlashTrait;

pub trait DelugeFlashState {
    fn get_page(&self, page_num: usize) -> ReturnCode;
    fn page_completed(&self, page_num: usize, completed_page: &[u8]) -> ReturnCode;
}

pub trait ProgramStateClient {
    fn program_state_read_complete(&self, page_numbuffer: &[u8]);
    fn program_state_write_complete(&self, page_num: usize, packet_num: usize);
}

pub trait DelugeProgramState<'a> {
    fn received_new_version(&self, version: usize);
    fn receive_packet(&self, version: usize, page_num: usize, packet_num: usize, payload: &[u8]) -> bool;
    fn current_page_number(&self) -> usize;
    fn current_version_number(&self) -> usize;
    fn current_packet_number(&self) -> usize;
    fn get_requested_packet(&self, page_num: usize, packet_num: usize) -> bool;
    fn set_flash_client(&self, flash_driver: &'a DelugeFlashState);
    fn set_client(&self, client: &'a ProgramStateClient);
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

    //tx_page_vector: Cell<[u8; BIT_VECTOR_SIZE]>,
    tx_requested_packet: Cell<bool>, // Change to bitvector eventually
    tx_requested_packet_num: Cell<usize>,
    tx_page_num: Cell<usize>,
    tx_page: TakeCell<'static, [u8; PAGE_SIZE]>,  // Page

    //rx_page_vector: Cell<[u8; BIT_VECTOR_SIZE]>,
    rx_largest_packet: Cell<usize>, // Change to bitvector eventually
    rx_page_num: Cell<usize>,       // Also largest page num ready for transfer
    rx_page: TakeCell<'static, [u8; PAGE_SIZE]>,  // Page

    flash_driver: Cell<Option<&'a DelugeFlashState>>,
    client: Cell<Option<&'a ProgramStateClient>>,

}

impl<'a> ProgramState<'a> {
    // We load the first page on initialization
    pub fn new(unique_id: usize,
               tx_page: &'static mut [u8; PAGE_SIZE],
               rx_page: &'static mut [u8; PAGE_SIZE]) -> ProgramState<'a> {
        ProgramState {
            unique_id: unique_id,
            version: Cell::new(1),

            tx_requested_packet: Cell::new(false),
            tx_requested_packet_num: Cell::new(0),
            tx_page_num: Cell::new(0),
            tx_page: TakeCell::new(tx_page),

            rx_largest_packet: Cell::new(0),
            rx_page_num: Cell::new(0),
            rx_page: TakeCell::new(rx_page),

            flash_driver: Cell::new(None),
            client: Cell::new(None),
        }
    }

    fn page_completed(&self) -> ReturnCode {
        let rx_page = self.rx_page.take().unwrap();
        self.flash_driver.get().map(|flash_driver| {
            // TODO: Might be busy
            flash_driver.page_completed(self.rx_page_num.get(), rx_page);
            self.rx_page_num.set(self.rx_page_num.get() + 1);
            self.rx_largest_packet.set(0);
        });
        self.rx_page.replace(rx_page);
    }
}

impl<'a> DelugeFlashTrait for ProgramState<'a> {
    fn read_complete(&self, buffer: &[u8]) {
        self.client.map(|client| client.program_state_read_complete(buffer));
    }

    fn write_complete(&self) {
        self.client.map(|client| client.program_state_write_complete());
    }
}

impl<'a> DelugeProgramState<'a> for ProgramState<'a> {
    fn received_new_version(&self, version: usize) {
        // If we receive a data packet with a greater version than us and it is
        // the first page, reset our reception state and start receiving the
        // updated information
        if version > self.version.get() {
            self.version.set(version);
            // Reset TX state
            self.tx_requested_packet.set(false);
            self.tx_requested_packet_num.set(0);
            self.tx_page_num.set(0);
            // Reset RX state
            self.rx_largest_packet.set(0);
            self.rx_page_num.set(0);
        }
    }

    // TODO: Currently only supports sequential reception
    fn receive_packet(&self, version: usize, page_num: usize, packet_num: usize, payload: &[u8]) -> bool {
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
            self.page_completed();
            return true;
        }
        false
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

    fn get_requested_packet(&self, page_num: usize, packet_num: usize, buf: &mut [u8]) -> bool {
        debug!("Get requested packet: {}", packet_num);
        // If we haven't received the latest page
        if page_num > self.rx_page_num.get() {
            return false;
        }

        // TODO: Check for specific length
        let offset = packet_num * PACKET_SIZE;
        if offset + PACKET_SIZE > PAGE_SIZE {
            return false;
        }

        // If the page is a different page than the one we currently have
        if page_num != self.tx_page_num.get() {
            // TODO: Will panic
            let tx_page = self.tx_page.take().unwrap();
            //self.client.get().map(|client| client.get_page(page_num, tx_page)).unwrap();
            self.tx_page.replace(tx_page);
            self.tx_page_num.set(page_num);
            return true;
        }
        self.tx_page.map(|tx_page| buf.copy_from_slice(&tx_page[offset..offset+PACKET_SIZE]));
        true
    }

    fn set_flash_client(&self, flash_driver: &'a DelugeFlashState) {
        self.flash_driver.set(Some(flash_driver));
    }
}
