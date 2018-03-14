use core::cell::Cell;
use kernel::hil;
use kernel::common::take_cell::TakeCell;
use net::deluge::program_state::ProgramStateClient;

pub trait DelugeFlashTrait {
    fn read_complete(&self, buffer: &[u8]);
    fn write_complete(&self);
}

pub struct FlashState<'a, F: hil::flash::Flash + 'static> {
    flash_driver: &'a F,
    program_state: &'a DelugeFlashTrait,
    buffer: TakeCell<'static, F::Page>,
    num_pages_offset: Cell<usize>,
}

impl<'a, F: hil::flash::Flash + 'a> FlashState<'a, F> {
    pub fn new(flash_driver: &'a F,
               program_state: &'a DelugeFlashTrait,
               buffer: &'static mut F::Page,
               num_pages_offset: usize) -> FlashState<'a, F> {
        FlashState {
            flash_driver: flash_driver,
            program_state: program_state,
            buffer: TakeCell::new(buffer),
            num_pages_offset: Cell::new(num_pages_offset),
        }
    }
}

impl<'a, F: hil::flash::Flash + 'a> hil::flash::Client<F> for FlashState<'a, F> {
    fn read_complete(&self, buffer: &'static mut F::Page, error: hil::flash::Error) {
        self.program_state.read_complete(buffer.as_mut());
    }

    fn write_complete(&self, buffer: &'static mut F::Page, error: hil::flash::Error) {
    }

    fn erase_complete(&self, error: hil::flash::Error) {
    }
}

impl<'a, F: hil::flash::Flash + 'a> ProgramStateClient for FlashState<'a, F> {
    fn get_page(&self, page_num: usize) {
    }

    fn page_completed(&self, page_num: usize, completed_page: &[u8]) {
        let buffer = self.buffer.take().unwrap();
        buffer.as_mut().copy_from_slice(completed_page);
    }
}
