use kernel::ReturnCode;
use kernel::hil::radio;

pub struct RadioLoopback<'a, R: radio::Radio + 'a> {
    radio: &'a R
}

impl<'a, R: radio::Radio + 'a> RadioLoopback<'a, R> {
    pub fn new(radio: &'a R) -> RadioLoopback<'a, R> {
        RadioLoopback { radio: radio }
    }
}

impl<'a, R: radio::Radio + 'a> radio::Radio for RadioLoopback<'a, R> {}

impl<'a, R: radio::Radio + 'a> radio::RadioConfig for RadioLoopback<'a, R> {
    fn initialize(&self,
                  buf: &'static mut [u8],
                  reg_write: &'static mut [u8],
                  reg_read: &'static mut [u8])
                  -> ReturnCode {
        self.radio.initialize(buf, reg_write, reg_read)
    }

    fn reset(&self) -> ReturnCode {
        self.radio.reset()
    }

    fn start(&self) -> ReturnCode {
        self.radio.start()
    }

    fn stop(&self) -> ReturnCode {
        self.radio.stop()
    }

    fn is_on(&self) -> bool {
        self.radio.is_on()
    }

    fn busy(&self) -> bool {
        self.radio.busy()
    }

    fn set_config_client(&self, client: &'static radio::ConfigClient) {
        self.radio.set_config_client(client)
    }

    fn set_power_client(&self, client: &'static radio::PowerClient) {
        self.radio.set_power_client(client)
    }

    fn config_set_address(&self, addr: u16) {
        self.radio.config_set_address(addr)
    }

    fn config_set_address_long(&self, addr: [u8; 8]) {
        self.radio.config_set_address_long(addr)
    }

    fn config_set_pan(&self, id: u16) {
        self.radio.config_set_pan(id)
    }

    fn config_set_tx_power(&self, power: i8) -> ReturnCode {
        self.radio.config_set_tx_power(power)
    }

    fn config_set_channel(&self, chan: u8) -> ReturnCode {
        self.radio.config_set_channel(chan)
    }

    fn config_address(&self) -> u16 {
        self.radio.config_address()
    }

    fn config_address_long(&self) -> [u8; 8] {
        self.radio.config_address_long()
    }

    fn config_pan(&self) -> u16 {
        self.radio.config_pan()
    }

    fn config_tx_power(&self) -> i8 {
        self.radio.config_tx_power()
    }

    fn config_channel(&self) -> u8 {
        self.radio.config_channel()
    }

    fn config_commit(&self) -> ReturnCode {
        self.radio.config_commit()
    }
}

impl<'a, R: radio::Radio + 'a> radio::RadioData for RadioLoopback<'a, R> {
    fn payload_offset(&self, long_src: bool, long_dest: bool) -> u8 {
        self.radio.payload_offset(long_src, long_dest)
    }

    fn header_size(&self, long_src: bool, long_dest: bool) -> u8 {
        self.radio.header_size(long_src, long_dest)
    }

    fn packet_header_size(&self, packet: &'static [u8]) -> u8 {
        self.radio.packet_header_size(packet)
    }

    fn packet_get_src(&self, packet: &'static [u8]) -> u16 {
        self.radio.packet_get_src(packet)
    }

    fn packet_get_dest(&self, packet: &'static [u8]) -> u16 {
        self.radio.packet_get_dest(packet)
    }

    fn packet_has_src_long(&self, packet: &'static [u8]) -> bool {
        self.radio.packet_has_src_long(packet)
    }

    fn packet_has_dest_long(&self, packet: &'static [u8]) -> bool {
        self.radio.packet_has_dest_long(packet)
    }

    fn packet_get_src_long(&self, packet: &'static [u8]) -> [u8; 8] {
        self.radio.packet_get_src_long(packet)
    }

    fn packet_get_dest_long(&self, packet: &'static [u8]) -> [u8; 8] {
        self.radio.packet_get_dest_long(packet)
    }

    fn packet_get_length(&self, packet: &'static [u8]) -> u8 {
        self.radio.packet_get_length(packet)
    }

    fn packet_get_pan(&self, packet: &'static [u8]) -> u16 {
        self.radio.packet_get_pan(packet)
    }

    fn set_transmit_client(&self, client: &'static radio::TxClient) {
        self.radio.set_transmit_client(client)
    }

    fn set_receive_client(&self, client: &'static radio::RxClient, buffer: &'static mut [u8]) {
        self.radio.set_receive_client(client, buffer)
    }

    fn set_receive_buffer(&self, buffer: &'static mut [u8]) {
        self.radio.set_receive_buffer(buffer)
    }

    fn transmit(&self,
                dest: u16,
                payload: &'static mut [u8],
                len: u8,
                source_long: bool)
                -> ReturnCode {
        // Transmit len is header + payload
        let offset = self.radio.payload_offset(source_long, false);
        let payload_len = (len - self.radio.header_size(source_long, false)) as usize;
        debug!("Transmitting radio packet: dest={:x}, len={}, source_long={}",
               dest, len, source_long);

        // All of this is because the debug macro adds a newline, and
        // we don't have strings.
        // Chunks bytes up into sequences of 8 bytes
        let payload_full: usize = payload_len >> 3;
        let payload_rem: usize = payload_len & 7;
        for i in 0..payload_full {
            let chunk_offset = i << 3;
            debug!("{} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                   if i == 0 {"hex:"} else { "    " },
                   payload[chunk_offset + 0],
                   payload[chunk_offset + 1],
                   payload[chunk_offset + 2],
                   payload[chunk_offset + 3],
                   payload[chunk_offset + 4],
                   payload[chunk_offset + 5],
                   payload[chunk_offset + 6],
                   payload[chunk_offset + 7]);
        }
        let chunk_offset = payload_full << 3;
        match payload_rem {
            7 => debug!("{} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                        if payload_full == 0 {"hex:"} else { "    " },
                        payload[chunk_offset + 0],
                        payload[chunk_offset + 1],
                        payload[chunk_offset + 2],
                        payload[chunk_offset + 3],
                        payload[chunk_offset + 4],
                        payload[chunk_offset + 5],
                        payload[chunk_offset + 6]),
            6 => debug!("{} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                        if payload_full == 0 {"hex:"} else { "    " },
                        payload[chunk_offset + 0],
                        payload[chunk_offset + 1],
                        payload[chunk_offset + 2],
                        payload[chunk_offset + 3],
                        payload[chunk_offset + 4],
                        payload[chunk_offset + 5]),
            5 => debug!("{} {:02x} {:02x} {:02x} {:02x} {:02x}",
                        if payload_full == 0 {"hex:"} else { "    " },
                        payload[chunk_offset + 0],
                        payload[chunk_offset + 1],
                        payload[chunk_offset + 2],
                        payload[chunk_offset + 3],
                        payload[chunk_offset + 4]),
            4 => debug!("{} {:02x} {:02x} {:02x} {:02x}",
                        if payload_full == 0 {"hex:"} else { "    " },
                        payload[chunk_offset + 0],
                        payload[chunk_offset + 1],
                        payload[chunk_offset + 2],
                        payload[chunk_offset + 3]),
            3 => debug!("{} {:02x} {:02x} {:02x}",
                        if payload_full == 0 {"hex:"} else { "    " },
                        payload[chunk_offset + 0],
                        payload[chunk_offset + 1],
                        payload[chunk_offset + 2]),
            2 => debug!("{} {:02x} {:02x}",
                        if payload_full == 0 {"hex:"} else { "    " },
                        payload[chunk_offset + 0],
                        payload[chunk_offset + 1]),
            1 => debug!("{} {:02x}",
                        if payload_full == 0 {"hex:"} else { "    " },
                        payload[chunk_offset + 0]),
            _ => {}
        };

        self.radio.transmit(dest, payload, len, source_long)
    }

    fn transmit_long(&self,
                     dest: [u8; 8],
                     payload: &'static mut [u8],
                     len: u8,
                     source_long: bool)
                     -> ReturnCode {
        self.radio.transmit_long(dest, payload, len, source_long)
    }
}
