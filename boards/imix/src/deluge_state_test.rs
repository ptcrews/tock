/// This test suite acts as a virtual flash layer, allowing for the Deluge
/// state machine itself to be tested.

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
use kernel::ReturnCode;
use core::cell::Cell;

pub struct DelugeStateTest<'a, A: time::Alarm + 'a> {
    deluge_data: Cell<Option<&'a DelugeData<'a, A>>>,
    program_state: Cell<Option<&'a DelugeFlashClient>>,
    is_sender: Cell<bool>,
}

static mut FIRST_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut TX_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut RX_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut TX_RADIO_BUF: [u8; radio::MAX_BUF_SIZE] = [0 as u8; radio::MAX_BUF_SIZE];

static mut FLASH_BUFFER: Sam4lPage = Sam4lPage::new();

const SRC_PAN_ADDR: PanID = 0xABCD;
const SRC_MAC_ADDR: MacAddress = MacAddress::Short(0xabcd);

const UPDATED_APP_VERSION: usize = 0x1;

pub unsafe fn initialize_all(radio_mac: &'static Mac,
                             mux_alarm: &'static MuxAlarm<'static, sam4l::ast::Ast>,
                             mux_flash: &'static MuxFlash<'static, sam4l::flashcalw::FLASHCALW>)
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
        DelugeStateTest::new()
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
    deluge_state_test.set_test_client(deluge_data);
    deluge_state_test
}

impl<'a, A: time::Alarm + 'a> DelugeStateTest<'a, A> {
    pub fn new() -> DelugeStateTest<'a, A> {
        DelugeStateTest {
            deluge_data: Cell::new(None),
            program_state: Cell::new(None),

            is_sender: Cell::new(false),
        }
    }

    fn set_test_client(&self, client: &'a DelugeData<'a, A>) {
        self.deluge_data.set(Some(client));
    }

    pub fn start(&self, is_sender: bool) {
        // Really just initializes Trickle
        self.deluge_data.get().map(|deluge_data| deluge_data.init());
        self.is_sender.set(is_sender);

    }
}

impl<'a, A: time::Alarm + 'a> DelugeFlashState<'a> for DelugeStateTest<'a, A> {
    fn get_page(&self, page_num: usize) -> ReturnCode {
        ReturnCode::SUCCESS
    }

    fn page_completed(&self, page_num: usize, completed_page: &[u8]) -> ReturnCode {
        ReturnCode::SUCCESS
    }

    fn set_client(&self, client: &'a DelugeFlashClient) {
        self.program_state.set(Some(client));
    }
}
