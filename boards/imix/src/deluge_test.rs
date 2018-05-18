extern crate sam4l;
use capsules;
use capsules::rng::SimpleRng;
use capsules::ieee802154::mac::{Mac, TxClient, RxClient};
use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use capsules::net::deluge::trickle;
use capsules::net::deluge::trickle::{Trickle, TrickleData, TrickleClient};
use capsules::net::deluge::deluge;
use capsules::net::deluge::deluge::{DelugeData};
use capsules::net::deluge::program_state;
use capsules::net::deluge::program_state::{ProgramState, DelugeProgramStateClient, DelugeProgramState};
use capsules::net::deluge::transmit_layer;
use capsules::net::deluge::transmit_layer::{DelugeTransmitLayer, DelugeTransmit};
use capsules::net::deluge::flash_layer::FlashState;
use capsules::virtual_flash::MuxFlash;
use sam4l::flashcalw::Sam4lPage;
use kernel::common::take_cell::TakeCell;
use kernel::returncode::ReturnCode;
use capsules::net::ieee802154::{Header, PanID, MacAddress};
use kernel::hil::radio;
use kernel::hil::time;
use core::cell::Cell;

pub struct DelugeTest<'a, A: time::Alarm + 'a> {
    deluge_data: &'a DelugeData<'a, A>,
}

static mut FIRST_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut TX_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut RX_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut TX_RADIO_BUF: [u8; radio::MAX_BUF_SIZE] = [0 as u8; radio::MAX_BUF_SIZE];

static mut FLASH_BUFFER: Sam4lPage = Sam4lPage::new();

const SRC_PAN_ADDR: PanID = 0xABCD;
const SRC_MAC_ADDR: MacAddress = MacAddress::Short(0xabcd);

pub unsafe fn initialize_all(radio_mac: &'static Mac,
                             mux_alarm: &'static MuxAlarm<'static, sam4l::ast::Ast>,
                             mux_flash: &'static MuxFlash<'static, sam4l::flashcalw::FLASHCALW>)
        -> &'static DelugeTest<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast<'static>>> {

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
        FlashState::new(virtual_flash, &mut FLASH_BUFFER, 0));

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

    let deluge_test = static_init!(
        DelugeTest<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
        DelugeTest::new(deluge_data)
    );
    program_state.set_client(deluge_data);
    deluge_test
}

impl<'a, A: time::Alarm + 'a> DelugeTest<'a, A> {
    pub fn new(deluge_data: &'a DelugeData<'a, A>) -> DelugeTest<'a, A> {
        DelugeTest {
            deluge_data: deluge_data,
        }
    }

    pub fn start(&self) {
        self.deluge_data.init();
        /*
        if is_sender {
        }
        */
    }
}
