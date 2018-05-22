/// This test suite acts as a virtual flash layer, allowing for the Deluge
/// state machine itself to be tested.

extern crate sam4l;
use capsules::ieee802154::mac::Mac;
use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use capsules::net::deluge::trickle::{Trickle, TrickleData};
use capsules::net::deluge::deluge::{DelugeData};
use capsules::net::deluge::program_state;
use capsules::net::deluge::program_state::{ProgramState, DelugeProgramState};
use capsules::net::deluge::transmit_layer::{DelugeTransmitLayer, DelugeTransmit};
use capsules::net::deluge::flash_layer::{DelugeFlashState, DelugeFlashClient};
use capsules::net::ieee802154::{PanID, MacAddress};
use kernel::hil::radio;
use kernel::hil::time;
use kernel::ReturnCode;
use kernel::common::take_cell::TakeCell;
use core::cell::Cell;

pub struct DelugeStateTest<'a, A: time::Alarm + 'a> {
    deluge_data: Cell<Option<&'a DelugeData<'a, A>>>,
    program_state: Cell<Option<&'a DelugeProgramState<'a>>>,
    flash_client: Cell<Option<&'a DelugeFlashClient>>,
    buffer: TakeCell<'static, [u8]>,
    test_number: Cell<usize>,
    is_sender: Cell<bool>,
}

const N_PAGES: usize = 4;
const FLASH_SIZE: usize = program_state::PAGE_SIZE * N_PAGES;
static mut VIRTUAL_FLASH: [u8; FLASH_SIZE] = [0 as u8; FLASH_SIZE];

static mut TX_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut RX_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut TX_RADIO_BUF: [u8; radio::MAX_BUF_SIZE] = [0 as u8; radio::MAX_BUF_SIZE];

const SRC_PAN_ADDR: PanID = 0xABCD;
const SRC_MAC_ADDR: MacAddress = MacAddress::Short(0xabcd);

const UPDATED_APP_VERSION: usize = 0x1;

pub unsafe fn initialize_all(radio_mac: &'static Mac,
                             mux_alarm: &'static MuxAlarm<'static, sam4l::ast::Ast>)
        -> &'static DelugeStateTest<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast<'static>>> {

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

    let transmit_layer = static_init!(
        DelugeTransmitLayer<'static>,
        DelugeTransmitLayer::new(SRC_MAC_ADDR, SRC_PAN_ADDR, &mut TX_RADIO_BUF, radio_mac)
    );

    let deluge_state_test = static_init!(
        DelugeStateTest<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
        DelugeStateTest::new(&mut VIRTUAL_FLASH)
    );

    let program_state = static_init!(
        ProgramState<'static>,
        ProgramState::new(deluge_state_test, 0, &mut TX_PAGE, &mut RX_PAGE)
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

    program_state.set_client(deluge_data);


    // To write initial pages, we set the test suite to be the client initally
    deluge_state_test.set_client(program_state);
    deluge_state_test.set_test_clients(deluge_data, program_state);
    deluge_state_test
}

impl<'a, A: time::Alarm + 'a> DelugeStateTest<'a, A> {
    pub fn new(buffer: &'static mut[u8]) -> DelugeStateTest<'a, A> {
        DelugeStateTest {
            deluge_data: Cell::new(None),
            program_state: Cell::new(None),
            flash_client: Cell::new(None),
            buffer: TakeCell::new(buffer),

            test_number: Cell::new(0),
            is_sender: Cell::new(false),
        }
    }

    fn set_test_clients(&self,
                        deluge_data: &'a DelugeData<'a, A>,
                        program_state: &'a DelugeProgramState<'a>) {
        self.deluge_data.set(Some(deluge_data));
        self.program_state.set(Some(program_state));
    }

    pub fn start(&self, is_sender: bool) {
        // Really just initializes Trickle
        self.deluge_data.get().map(|deluge_data| deluge_data.init());
        self.is_sender.set(is_sender);

        // Initialize ourselves
        if is_sender {
            self.start_next_test();
        }
    }

    fn start_next_test(&self) {
        let next_test_number = self.test_number.get() + 1;
        self.test_number.set(next_test_number);
        debug!("Starting next test: {}", next_test_number);
        self.buffer.map(|buffer| {
            for i in 0..buffer.len() {
                buffer[i] = next_test_number as u8;
            }
            self.program_state.get().map(|program_state|
                                         program_state.updated_application(next_test_number,
                                                                           N_PAGES));
        });
    }
}

impl<'a, A: time::Alarm + 'a> DelugeFlashState<'a> for DelugeStateTest<'a, A> {
    fn get_page(&self, page_num: usize) -> ReturnCode {
        debug!("Getting next page: {}", page_num);
        let offset = page_num * program_state::PAGE_SIZE;
        self.buffer.map(|buffer| {
            self.flash_client.get().map(|flash_client| {
                flash_client.read_complete(&buffer[offset..offset+program_state::PAGE_SIZE]);
            });
        });
        ReturnCode::SUCCESS
    }

    fn page_completed(&self, page_num: usize, completed_page: &[u8]) -> ReturnCode {
        debug!("Page completed: {}", page_num);
        let offset = page_num * program_state::PAGE_SIZE;
        self.buffer.map(|buffer| {
            buffer[offset..offset+program_state::PAGE_SIZE].copy_from_slice(completed_page);
        });
        self.flash_client.get().map(|flash_client| flash_client.write_complete());
        ReturnCode::SUCCESS
    }

    fn set_client(&self, client: &'a DelugeFlashClient) {
        self.flash_client.set(Some(client));
    }
}
