/// Why do we want a separate flash layer?
/// Because although not *super* valuable, it does provide a nice abstraction
/// for dealing with the weird types (the Pages) and to remove unnecessary
/// references to hil stuff in upper layers. Also, can suppress/deal with
/// errors and lower-layer stuff + mux the flash layer easily
use core::cell::Cell;
use kernel::hil;
use kernel::common::take_cell::TakeCell;
use kernel::returncode::ReturnCode;

pub trait DelugeFlashClient {
    fn read_complete(&self, buffer: &[u8]);
    fn write_complete(&self);
}

pub trait DelugeFlashState<'a> {
    fn get_page(&self, page_num: usize) -> ReturnCode;
    fn page_completed(&self, page_num: usize, completed_page: &[u8]) -> ReturnCode;
    fn set_client(&self, &'a DelugeFlashClient);
}

pub struct FlashState<'a, F: hil::flash::Flash + 'static> {
    flash_driver: &'a F,
    client: Cell<Option<&'a DelugeFlashClient>>,
    buffer: TakeCell<'static, F::Page>,
    num_pages_offset: Cell<usize>,
}

impl<'a, F: hil::flash::Flash + 'a> FlashState<'a, F> {
    pub fn new(flash_driver: &'a F,
               buffer: &'static mut F::Page,
               num_pages_offset: usize) -> FlashState<'a, F> {
        FlashState {
            flash_driver: flash_driver,
            client: Cell::new(None),
            buffer: TakeCell::new(buffer),
            num_pages_offset: Cell::new(num_pages_offset),
        }
    }
}

impl<'a, F: hil::flash::Flash + 'a> hil::flash::Client<F> for FlashState<'a, F> {
    fn read_complete(&self, buffer: &'static mut F::Page, error: hil::flash::Error) {
        self.client.get().map(|client| client.read_complete(buffer.as_mut()));
        self.buffer.replace(buffer);
    }

    fn write_complete(&self, buffer: &'static mut F::Page, error: hil::flash::Error) {
        self.client.get().map(|client| client.write_complete());
        self.buffer.replace(buffer);
    }

    fn erase_complete(&self, error: hil::flash::Error) {
    }
}

impl<'a, F: hil::flash::Flash + 'a> DelugeFlashState<'a> for FlashState<'a, F> {
    fn get_page(&self, page_num: usize) -> ReturnCode {
        if self.buffer.is_none() {
            return ReturnCode::EBUSY;
        }
        let buffer = self.buffer.take().unwrap();
        self.flash_driver.read_page(self.num_pages_offset.get() + page_num, buffer);
        ReturnCode::SUCCESS
    }

    fn page_completed(&self, page_num: usize, completed_page: &[u8]) -> ReturnCode {
        if self.buffer.is_none() {
            return ReturnCode::EBUSY;
        }
        let buffer = self.buffer.take().unwrap();
        buffer.as_mut().copy_from_slice(completed_page);
        self.flash_driver.write_page(self.num_pages_offset.get() + page_num, buffer);
        ReturnCode::SUCCESS
    }

    fn set_client(&self, client: &'a DelugeFlashClient) {
        self.client.set(Some(client));
    }
}
