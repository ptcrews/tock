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
use capsules::net::deluge::program_state::{ProgramState, ProgramStateClient, DelugeProgramState};
use capsules::net::deluge::transmit_layer;
use capsules::net::deluge::transmit_layer::{DelugeTransmitLayer, DelugeTransmit};
use kernel::common::take_cell::TakeCell;
use kernel::returncode::ReturnCode;
use capsules::net::ieee802154::{Header, PanID, MacAddress};
use kernel::hil::radio;
use core::cell::Cell;

pub struct DelugeTest<'a> {
    dummy_data: Cell<Option<&'a u8>>,
}

static mut FIRST_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut TX_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut RX_PAGE: [u8; program_state::PAGE_SIZE] = [0 as u8; program_state::PAGE_SIZE];
static mut TX_RADIO_BUF: [u8; radio::MAX_BUF_SIZE] = [0 as u8; radio::MAX_BUF_SIZE];

const SRC_PAN_ADDR: PanID = 0xABCD;
const SRC_MAC_ADDR: MacAddress = MacAddress::Short(0xabcd);

pub unsafe fn initialize_all(radio_mac: &'static Mac,
                             mux_alarm: &'static MuxAlarm<'static, sam4l::ast::Ast>)
        -> &'static DelugeTest<'static> {

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

    let program_state = static_init!(
        ProgramState<'static>,
        ProgramState::new(0, FIRST_PAGE.len(), &mut TX_PAGE, &mut RX_PAGE)
    );

    let deluge_data = static_init!(
        DelugeData<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
        DelugeData::new(program_state, transmit_layer, trickle_data, VirtualMuxAlarm::new(mux_alarm))
    );
    deluge_data.init();
    transmit_layer.set_tx_client(deluge_data);
    transmit_layer.set_rx_client(deluge_data);
    radio_mac.set_receive_client(transmit_layer);
    radio_mac.set_transmit_client(transmit_layer);
    trickle_data.set_client(deluge_data);

    let deluge_test = static_init!(
        DelugeTest<'static>,
        DelugeTest::new()
    );
    program_state.set_client(deluge_test);

}

impl<'a> DelugeTest<'a> {
    pub fn new() -> DelugeTest<'a> {
        DelugeTest {
            dummy_data: Cell::new(None),
        }
    }

    pub fn start(&self) {
        //self.deluge_data.init();
    }
}

impl<'a> ProgramStateClient for DelugeTest<'a> {
    fn get_next_page(&self) {
    }

    fn get_page(&self, page_num: usize) -> &mut [u8] {
        unimplemented!();
    }

    fn page_completed(&self, completed_page: &mut [u8]) {
    }
}