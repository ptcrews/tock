/// Note that in order for this test suite to work, the flash layer should be
/// configured

extern crate sam4l;
use capsules;
use capsules::ieee802154::mac::Mac;
use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use capsules::net::deluge::trickle::{Trickle, TrickleData};
use capsules::net::deluge::deluge::{DelugeData};
use capsules::net::deluge::program_state;
use capsules::net::deluge::program_state::{ProgramState, DelugeProgramState};
use capsules::net::deluge::transmit_layer::{DelugeTransmitLayer, DelugeTransmit};
use capsules::net::deluge::flash_layer::{FlashState, DelugeFlashState, DelugeFlashClient};
use capsules::virtual_flash::MuxFlash;
use sam4l::flashcalw::Sam4lPage;
use capsules::net::ieee802154::{PanID, MacAddress};
use kernel::hil::radio;
use kernel::hil::time;
use kernel::hil::time::Frequency;
use kernel::hil::flash::HasClient;
use kernel::common::take_cell::TakeCell;
use core::cell::Cell;

pub struct DelugeTest<'a, A: time::Alarm + 'a> {
    deluge_data: &'a DelugeData<'a, A>,
    program_state: &'a DelugeProgramState<'a>,
    flash_client: &'a DelugeFlashClient,
    flash_driver: &'a DelugeFlashState<'a>,
    flash_region_len: Cell<usize>,
    init_page_number: Cell<usize>,
    is_sender: Cell<bool>,
    self_flash_client: Cell<Option<&'a DelugeFlashClient>>,
    alarm: &'a A,
}

static mut FIRST_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut TX_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut RX_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut TX_RADIO_BUF: [u8; radio::MAX_BUF_SIZE] = [0 as u8; radio::MAX_BUF_SIZE];

static mut FLASH_BUFFER: Sam4lPage = Sam4lPage::new();

const SRC_PAN_ADDR: PanID = 0xABCD;
const SRC_MAC_ADDR: MacAddress = MacAddress::Short(0xabcd);

const DELAY_IN_S: u32 = 420;

const UPDATED_APP_VERSION: usize = 0x2;

pub unsafe fn initialize_all(radio_mac: &'static Mac,
                             mux_alarm: &'static MuxAlarm<'static, sam4l::ast::Ast>,
                             mux_flash: &'static MuxFlash<'static, sam4l::flashcalw::FLASHCALW>)
        -> &'static DelugeTest<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast<'static>>> {

    // Allocate flash storage section
    // NOTE: This macro allocates in 1024-byte chunks; this may not be
    // the same as the number of pages
    storage_volume!(DELUGE_FLASH_REGION, 4);
    let deluge_flash_region_addr = (&DELUGE_FLASH_REGION).as_ptr() as usize;

    // Allocate DelugeData + appropriate structs

    let trickle_alarm = static_init!(
        VirtualMuxAlarm<'static, sam4l::ast::Ast>,
        VirtualMuxAlarm::new(mux_alarm)
    );

    let trickle_data = static_init!(
        TrickleData<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
        TrickleData::new(&sam4l::trng::TRNG, trickle_alarm)
    );
    sam4l::trng::TRNG.set_client(trickle_data);
    trickle_alarm.set_client(trickle_data);

    // Everything that then uses the virtualized flash must use one of these.
    let virtual_flash = static_init!(
        capsules::virtual_flash::FlashUser<'static, sam4l::flashcalw::FLASHCALW>,
        capsules::virtual_flash::FlashUser::new(mux_flash));

    let flash_layer = static_init!(
        FlashState<'static, capsules::virtual_flash::FlashUser<'static, sam4l::flashcalw::FLASHCALW>>,
        FlashState::new(virtual_flash, &mut FLASH_BUFFER, deluge_flash_region_addr, DELUGE_FLASH_REGION.len()));

    let transmit_layer = static_init!(
        DelugeTransmitLayer<'static>,
        DelugeTransmitLayer::new(SRC_MAC_ADDR, SRC_PAN_ADDR, &mut TX_RADIO_BUF, radio_mac)
    );

    let program_state = static_init!(
        ProgramState<'static>,
        ProgramState::new(flash_layer, 0, &mut TX_PAGE, &mut RX_PAGE)
    );

    let deluge_alarm = static_init!(
        VirtualMuxAlarm<'static, sam4l::ast::Ast>,
        VirtualMuxAlarm::new(mux_alarm)
    );

    let deluge_data = static_init!(
        DelugeData<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
        DelugeData::new(program_state, transmit_layer, trickle_data, deluge_alarm)
    );
    deluge_alarm.set_client(deluge_data);
    transmit_layer.set_tx_client(deluge_data);
    transmit_layer.set_rx_client(deluge_data);
    radio_mac.set_receive_client(transmit_layer);
    radio_mac.set_transmit_client(transmit_layer);
    trickle_data.set_client(deluge_data);
    virtual_flash.set_client(flash_layer);

    let deluge_test_alarm = static_init!(
        VirtualMuxAlarm<'static, sam4l::ast::Ast>,
        VirtualMuxAlarm::new(mux_alarm)
    );

    let deluge_test = static_init!(
        DelugeTest<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
        DelugeTest::new(deluge_data, program_state, program_state, flash_layer,
                        DELUGE_FLASH_REGION.len(), deluge_test_alarm)
    );
    deluge_test_alarm.set_client(deluge_test);
    deluge_test.set_self_flash_client(deluge_test);
    program_state.set_client(deluge_data);

    // To write initial pages, we set the test suite to be the client initally
    flash_layer.set_client(deluge_test);
    deluge_test
}

impl<'a, A: time::Alarm + 'a> DelugeTest<'a, A> {
    pub fn new(deluge_data: &'a DelugeData<'a, A>,
               program_state: &'a DelugeProgramState<'a>,
               flash_client: &'a DelugeFlashClient,
               flash_driver: &'a DelugeFlashState<'a>,
               flash_region_len: usize,
               alarm: &'a A) -> DelugeTest<'a, A> {
        DelugeTest {
            deluge_data: deluge_data,
            program_state: program_state,
            // We must keep a reference to the real flash client, as we need to
            // first write a bunch of pages to the flash before setting
            // the program state struct (the real flash client) as the client
            flash_client: flash_client,

            flash_driver: flash_driver,
            flash_region_len: Cell::new(flash_region_len),
            init_page_number: Cell::new(0),
            is_sender: Cell::new(false),
            self_flash_client: Cell::new(None),

            alarm: alarm,
        }
    }

    pub fn start(&self, is_sender: bool) {
        // Really just initializes Trickle
        self.deluge_data.init();

        self.is_sender.set(is_sender);

        if is_sender {
            // First, write the test data
            self.write_complete();
        } else {
            self.init_done();
        }
    }

    fn init_done(&self) {
        self.flash_driver.set_client(self.flash_client);
        if self.is_sender.get() {
            // TODO: Use an alarm
            let num_pages = self.flash_region_len.get() / program_state::PAGE_SIZE;
            self.program_state.updated_application(UPDATED_APP_VERSION, num_pages);
        } else {
            // Set an alarm to check pages later
            let delta = A::Frequency::frequency() * DELAY_IN_S;
            let delay = self.alarm.now().wrapping_add(delta);
            self.alarm.set_alarm(delay);
        }
    }

    fn set_self_flash_client(&self, self_flash_client: &'a DelugeFlashClient) {
        self.self_flash_client.set(Some(self_flash_client));
    }
}

impl<'a, A: time::Alarm + 'a> DelugeFlashClient for DelugeTest<'a, A> {
    fn read_complete(&self, _buffer: &[u8]) {
        // We are now verifying the different pages
        for i in 0.._buffer.len() {
            if _buffer[i] != self.init_page_number.get() as u8 {
                debug!("Error: Page differs at {}. Should be {} but is {}",
                       i, self.init_page_number.get(), _buffer[i]);
                break;
            }
        }
        let num_pages = self.flash_region_len.get() / program_state::PAGE_SIZE;
        let next_page_number = self.init_page_number.get() + 1;
        if next_page_number >= num_pages {
            // We are done!
            self.flash_driver.set_client(self.flash_client);
            return;
        }
        self.init_page_number.set(next_page_number);
        let result = self.flash_driver.get_page(next_page_number);
        debug!("Requested page {} with return value: {:?}", next_page_number, result);
    }

    fn write_complete(&self) {
        let num_pages = self.flash_region_len.get() / program_state::PAGE_SIZE;
        let next_page_number = self.init_page_number.get() + 1;
        if next_page_number >= num_pages {
            // We are done initializing the pages
            self.init_done();
            return;
        }
        self.init_page_number.set(next_page_number);
        let next_page: [u8; program_state::PAGE_SIZE] = [next_page_number as u8; program_state::PAGE_SIZE];
        let result = self.flash_driver.page_completed(next_page_number, &next_page);
        debug!("Wrote page {} with return value: {:?}", next_page_number, result);
    }
}

impl<'a, A: time::Alarm + 'a> time::Client for DelugeTest<'a, A> {
    fn fired(&self) {
        debug!("Timer fired");
        // Set ourselves as the flash client again
        self.flash_driver.set_client(self.self_flash_client.get().unwrap());
        self.init_page_number.set(0);
        let result = self.flash_driver.get_page(0);
        debug!("Requested page {} with return value: {:?}", 0, result);
    }
}
