extern crate sam4l;
use capsules;
use capsules::rng::SimpleRng;
use capsules::ieee802154::mac::{Mac, TxClient, RxClient};
use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use capsules::net::deluge::trickle;
use capsules::net::deluge::trickle::{Trickle, TrickleData, TrickleClient};
use kernel::common::take_cell::TakeCell;
use kernel::returncode::ReturnCode;
use capsules::net::ieee802154::{Header, PanID, MacAddress};
use kernel::hil::radio;
use core::cell::Cell;

pub struct TrickleTest<'a> {
    value: Cell<u8>,
    tx_buf: TakeCell<'static, [u8]>,
    trickle: &'a Trickle<'a>,
    radio: &'a Mac<'a>,
}

static mut TX_BUF: [u8; radio::MAX_BUF_SIZE] = [0 as u8; radio::MAX_BUF_SIZE];
const MAGIC_TRICKLE_NUMBER: u8 = 0xf1;
const K: usize = 1;
const I_MIN: usize = 1;
const I_MAX: usize = 8; // Number of doublings of I_MIN
const INITIAL_VALUE: u8 = 0x13;

const DST_PAN_ADDR: PanID = 0xABCD;
const SRC_PAN_ADDR: PanID = 0xABCD;

pub const SRC_MAC_ADDR: MacAddress =
    MacAddress::Long([0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17]);
pub const DST_MAC_ADDR: MacAddress =
    MacAddress::Long([0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f]);

pub unsafe fn initialize_all(radio_mac: &'static Mac,
                             mux_alarm: &'static MuxAlarm<'static, sam4l::ast::Ast>)
        -> &'static TrickleTest<'static> {

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

    let trickle_test = static_init!(
        TrickleTest<'static>,
        TrickleTest::new(&mut TX_BUF, trickle_data, radio_mac)
    );

    trickle_data.set_client(trickle_test);
    radio_mac.set_receive_client(trickle_test);
    radio_mac.set_transmit_client(trickle_test);
    trickle_test
}

impl<'a> TrickleTest<'a> {
    pub fn new(tx_buf: &'static mut [u8], trickle: &'a Trickle<'a>, radio: &'a Mac<'a>) -> TrickleTest<'a> {
        TrickleTest {
            value: Cell::new(INITIAL_VALUE),
            tx_buf: TakeCell::new(tx_buf),
            trickle: trickle,
            radio: radio,
        }
    }

    pub fn start(&self) {
        self.trickle.set_default_parameters(I_MAX, I_MIN, K);
        self.trickle.initialize();
    }

    fn transmit_packet(&self) -> ReturnCode {
        if self.tx_buf.is_none() {
            return ReturnCode::ENOMEM;
        }
        debug!("Transmit packet!");
        let buf: [u8; 2] = [MAGIC_TRICKLE_NUMBER, self.value.get()];
        match self.radio.prepare_data_frame(
            self.tx_buf.take().unwrap(),
            DST_PAN_ADDR,
            DST_MAC_ADDR,
            SRC_PAN_ADDR,
            SRC_MAC_ADDR,
            None) {
            Err(frame) => {
                self.tx_buf.replace(frame);
                ReturnCode::FAIL
            },
            Ok(mut frame) => {
                frame.append_payload(&buf);
                let (result, buf) = self.radio.transmit(frame);
                buf.map(|buf| {
                    self.tx_buf.replace(buf);
                    result
                }).unwrap_or(ReturnCode::SUCCESS)
            },
        }
    }

    fn is_packet_valid(&self, buf: &[u8]) -> bool {
        if buf.len() < 2 || buf[0] != MAGIC_TRICKLE_NUMBER {
            return false;
        }
        true
    }

    fn is_packet_consistent(&self, buf: &[u8]) -> bool {
        self.value.get() == buf[1]
    }
}

impl<'a> TrickleClient for TrickleTest<'a> {
    fn transmit(&self) {
        self.transmit_packet();
    }

    fn new_interval(&self) {
        // TODO: Do nothing?
    }
}

impl<'a> RxClient for TrickleTest<'a> {
    fn receive<'b>(&self, buf: &'b [u8], header: Header<'b>, data_offset: usize, data_len: usize) {
        let buffer = &buf[data_offset..];
        debug!("Received packet!");
        if self.is_packet_valid(buffer) {
            debug!("Received valid packet!");
            self.trickle.received_transmission(self.is_packet_consistent(buffer));
        }
    }
}

impl<'a> TxClient for TrickleTest<'a> {
    fn send_done(&self, tx_buf: &'static mut [u8], _acked: bool, result: ReturnCode) {
        self.tx_buf.replace(tx_buf);
    }
}
