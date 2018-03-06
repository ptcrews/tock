use core::cell::Cell;
use kernel::ReturnCode;
use kernel::hil::time;
use kernel::hil::radio;
use kernel::hil::time::Frequency;
use kernel::common::take_cell::TakeCell;
use ieee802154::mac::{RxClient, TxClient};
use net::sixlowpan::{Sixlowpan, SixlowpanClient};
use net::sixlowpan_compression::Context;
use net::ieee802154::{MacAddress, PanID, Header};
use ieee802154::mac::Mac;

pub trait DelugeTransmit<'a> {
    // TODO: Add destination eventually
    fn transmit_packet(&self, buffer: &[u8]) -> ReturnCode;
    fn set_tx_client(&self, tx_client: &'a DelugeTxClient);
    fn set_rx_client(&self, rx_client: &'a DelugeRxClient);
}

pub trait DelugeTxClient{
    fn transmit_done(&self, buffer: &'static mut [u8], result: ReturnCode);
}

pub trait DelugeRxClient {
    fn receive(&self, buffer: &[u8]);
}

pub struct DelugeTransmitLayer<'a> {
    src_addr: Cell<MacAddress>,
    src_pan: Cell<PanID>,
    tx_buffer: TakeCell<'static, [u8]>,
    tx_client: Cell<Option<&'a DelugeTxClient>>,
    rx_client: Cell<Option<&'a DelugeRxClient>>,
    radio: &'a Mac<'a>,
}

const DST_MAC_ADDR: MacAddress = MacAddress::Short(0xffff);
const DST_PAN_ADDR: PanID = 0xABCD;

impl<'a> DelugeTransmit<'a> for DelugeTransmitLayer<'a> {
    fn transmit_packet(&self, buffer: &[u8]) -> ReturnCode {
        match self.radio.prepare_data_frame(
            self.tx_buffer.take().unwrap(),
            DST_PAN_ADDR,
            DST_MAC_ADDR,
            self.src_pan.get(),
            self.src_addr.get(),
            None
            ) {
            Err(frame) => {
                self.tx_buffer.replace(frame);
                ReturnCode::FAIL
            },
            Ok(mut frame) => {
                frame.append_payload(buffer);
                let (result, buf) = self.radio.transmit(frame);
                buf.map(|buf| {
                    self.tx_buffer.replace(buf);
                    result
                }).unwrap_or(ReturnCode::SUCCESS)
            }
        }
    }

    fn set_tx_client(&self, tx_client: &'a DelugeTxClient) {
        self.tx_client.set(Some(tx_client));
    }

    fn set_rx_client(&self, rx_client: &'a DelugeRxClient) {
        self.rx_client.set(Some(rx_client));
    }
}

impl<'a> TxClient for DelugeTransmitLayer<'a> {
    fn send_done(&self, tx_buf: &'static mut [u8], _acked: bool, result: ReturnCode) {
        self.tx_client.get().map(move |tx_client| tx_client.transmit_done(tx_buf, result));
    }
}

impl<'a> RxClient for DelugeTransmitLayer<'a> {
    fn receive<'b>(&self, buf: &'b [u8], header: Header<'b>, data_offset: usize, data_len: usize) {
        let data = &buf[data_offset..data_offset + data_len];
        self.rx_client.get().map(|rx_client| rx_client.receive(data));
    }
}

impl<'a> DelugeTransmitLayer<'a> {
    pub fn new(src_addr: MacAddress,
               src_pan: PanID,
               tx_buffer: &'static mut [u8],
               radio: &'a Mac<'a>) -> DelugeTransmitLayer<'a> {
        DelugeTransmitLayer {
            src_addr: Cell::new(src_addr),
            src_pan: Cell::new(src_pan),
            tx_buffer: TakeCell::new(tx_buffer),
            tx_client: Cell::new(None),
            rx_client: Cell::new(None),
            radio: radio,
        }
    }
}
