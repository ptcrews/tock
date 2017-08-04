//! Implements 802.15.4 MAC device functionality as an abstraction layer over
//! the raw radio transceiver hardware. The abstraction difference between a MAC
//! device and a raw radio transceiver is that the MAC devices exposes a
//! frame-oriented interface to its users, whereas the radio transceiver
//! transmits raw byte sequences. There is some abstraction breaking here,
//! though because the following are still implemented at the hardware level:
//! - CSMA-CA backoff
//! - FCS generation and verification
//!
//! TODO: Encryption/decryption
//! TODO: Sending beacon frames
//! TODO: Channel scanning

use core::cell::Cell;
use kernel::ReturnCode;
use kernel::common::take_cell::TakeCell;
use kernel::hil::radio;
use net::ieee802154::*;
use net::stream::{encode_u8, encode_u16, encode_u32, encode_bytes};
use net::stream::SResult;

/// This is auxiliary data to keep track of the state of a radio buffer (the
/// buffer that will eventually be sent to the radio). It contains exactly the
/// information that is needed in the security procedures, both incoming and
/// outgoing. It also exposes a limited interface for appending the payload in
/// the right place.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct FrameInfo {
    frame_type: FrameType,

    // Offsets are relative to buf[radio::PSDU_OFFSET..].
    // The MAC payload, including Payload IEs
    mac_payload_offset: usize,
    // The data payload, not including Payload IEs
    data_offset: usize,
    // The length of the data payload, not including MIC and FCS
    data_len: usize,
    // The length of the MIC
    mic_len: usize,

    // Security header, key, and nonce
    security_params: Option<(SecurityLevel, [u8; 16], [u8; 13])>,
}

impl FrameInfo {
    pub fn unsecured_frame_length(&self) -> usize {
        self.data_offset + self.data_len
    }

    pub fn secured_frame_length(&self) -> usize {
        self.data_offset + self.data_len + self.mic_len
    }

    pub fn remaining_data_capacity(&self, buf: &[u8]) -> usize {
        buf.len() - radio::PSDU_OFFSET - radio::MFR_SIZE - self.secured_frame_length()
    }

    pub fn append_payload(&mut self, buf: &mut [u8], payload: &[u8]) -> ReturnCode {
        if payload.len() > self.remaining_data_capacity(buf.as_ref()) {
            return ReturnCode::ENOMEM;
        }
        let begin = radio::PSDU_OFFSET + self.data_offset + self.data_len;
        buf[begin..begin + payload.len()].copy_from_slice(payload);
        self.data_len += payload.len();

        ReturnCode::SUCCESS
    }

    // Compute the offsets in the buffer for the a data and m/c data fields in
    // the CCM* authentication and encryption procedures which depends on the
    // frame type and security levels. Returns the (offset, len) of the m/c data
    // fields, not including the MIC, The a data is always the remaining prefix
    // of the header, so it can be determined implicitly.
    fn get_ccm_encrypt_ranges(&self, level: &SecurityLevel) -> (usize, usize) {
        // IEEE 802.15.4-2015: Table 9-1. Exceptions to Private Payload field
        // The boundary between open and private payload fields depends
        // on the type of frame.
        let private_payload_offset = match self.frame_type {
            FrameType::Beacon => {
                // Beginning of beacon payload field
                unimplemented!()
            }
            FrameType::MACCommand => {
                // Beginning of MAC command content field
                unimplemented!()
            }
            _ => {
                // MAC payload field, which includes payload IEs
                self.mac_payload_offset
            }
        };

        // IEEE 802.15.4-2015: Table 9-3. a data and m data
        if !level.encryption_needed() {
            // If only integrity is need, a data is the whole frame
            (radio::PSDU_OFFSET + self.unsecured_frame_length(), 0)
        } else {
            // Otherwise, a data is the header and the open payload, and
            // m data is the private payload field
            (radio::PSDU_OFFSET + private_payload_offset,
             self.unsecured_frame_length() - private_payload_offset)
        }
    }
}

// The needed buffer size might be bigger than an MTU, because
// the CCM* authentication procedure
// - adds an extra 16-byte block in front of the a and m data
// - prefixes the a data with a length encoding and pads the result
// - pads the m data to 16-byte blocks
pub const CRYPT_BUF_SIZE: usize = radio::MAX_MTU + 3 * 16;

pub trait Mac {
    fn get_address(&self) -> u16; //....... The local 16-bit address
    fn get_address_long(&self) -> [u8; 8]; // 64-bit address
    fn get_pan(&self) -> u16; //........... The 16-bit PAN ID
    fn get_channel(&self) -> u8;
    fn get_tx_power(&self) -> i8;

    fn set_address(&self, addr: u16);
    fn set_address_long(&self, addr: [u8; 8]);
    fn set_pan(&self, id: u16);
    fn set_channel(&self, chan: u8) -> ReturnCode;
    fn set_tx_power(&self, power: i8) -> ReturnCode;

    fn config_commit(&self) -> ReturnCode;

    fn is_on(&self) -> bool;
    fn prepare_data_frame(&self,
                          buf: &mut [u8],
                          dst_pan: PanID,
                          dst_addr: MacAddress,
                          src_pan: PanID,
                          src_addr: MacAddress,
                          security_needed: Option<(SecurityLevel, KeyId)>)
                          -> Result<FrameInfo, ()>;
    fn transmit(&self,
                buf: &'static mut [u8],
                frame_info: FrameInfo)
                -> (ReturnCode, Option<&'static mut [u8]>);
}

pub trait TxClient {
    fn send_done(&self, spi_buf: &'static mut [u8], acked: bool, result: ReturnCode);
}

pub trait RxClient {
    fn receive<'a>(&self,
                   buf: &'a [u8],
                   header: Header<'a>,
                   // This data_offset is relative to the PSDU in the buffer and
                   // does not include the two bytes before the PSDU.
                   data_offset: usize,
                   data_len: usize,
                   result: ReturnCode);
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum TxState {
    Idle,
    ReadyToSecure,
    AuthDone,
    EncDone,
    ReadyToTransmit,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum RxState {
    Idle,
    ReadyToUnsecure,
    DecDone,
    AuthDone,
    ReadyToReturn,
}

pub struct MacDevice<'a, R: radio::Radio + 'a> {
    radio: &'a R,
    data_sequence: Cell<u8>,
    config_in_progress: Cell<bool>,

    // State for the transmit pathway
    tx_buf: TakeCell<'static, [u8]>,
    tx_info: Cell<Option<FrameInfo>>,
    tx_m_off: Cell<usize>,
    tx_m_len: Cell<usize>,
    tx_state: Cell<TxState>,
    tx_client: Cell<Option<&'static TxClient>>,

    // State for the receive pathway
    rx_buf: TakeCell<'static, [u8]>,
    rx_info: Cell<Option<FrameInfo>>,
    rx_c_off: Cell<usize>,
    rx_c_len: Cell<usize>,
    rx_state: Cell<RxState>,
    rx_client: Cell<Option<&'static RxClient>>,

    // State for CCM* authentication/encryption
    crypt_buf: TakeCell<'static, [u8]>,
    crypt_buf_len: Cell<usize>,
    crypt_iv: TakeCell<'static, [u8]>,
    crypt_busy: Cell<bool>,
}

impl<'a, R: radio::Radio + 'a> MacDevice<'a, R> {
    pub fn new(radio: &'a R,
               crypt_buf: &'static mut [u8],
               crypt_iv: &'static mut [u8]) -> MacDevice<'a, R> {
        MacDevice {
            radio: radio,
            data_sequence: Cell::new(0),
            config_in_progress: Cell::new(false),

            tx_buf: TakeCell::empty(),
            tx_info: Cell::new(None),
            tx_m_off: Cell::new(0),
            tx_m_len: Cell::new(0),
            tx_state: Cell::new(TxState::Idle),
            tx_client: Cell::new(None),

            rx_buf: TakeCell::empty(),
            rx_info: Cell::new(None),
            rx_c_off: Cell::new(0),
            rx_c_len: Cell::new(0),
            rx_state: Cell::new(RxState::Idle),
            rx_client: Cell::new(None),

            crypt_buf: TakeCell::new(crypt_buf),
            crypt_buf_len: Cell::new(0),
            crypt_iv: TakeCell::new(crypt_iv),
            crypt_busy: Cell::new(false),
        }
    }

    pub fn set_transmit_client(&self, client: &'static TxClient) {
        self.tx_client.set(Some(client));
    }

    pub fn set_receive_client(&self, client: &'static RxClient) {
        self.rx_client.set(Some(client));
    }

    // TODO: Look up the key in the list of thread neighbors
    fn lookup_key(&self, level: SecurityLevel, key_id: KeyId)
        -> Option<([u8; 16])> {
        let fake_key = [0xC0, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xCB, 0xCC, 0xCD, 0xCE, 0xCF];
        if level == SecurityLevel::None {
            None
        } else {
            Some(fake_key)
        }
    }

    // TODO: Look up the extended device address from a short address
    // Not sure if more information is needed
    fn lookup_addr_long(&self, src_addr: Option<MacAddress>) -> Option<([u8; 8])> {
        let fake_addr = [0xac, 0xde, 0x48, 0, 0, 0, 0, 1];
        Some(fake_addr)
    }

    fn encode_ccm_nonce(&self,
                        buf: &mut [u8],
                        addr_long: &[u8; 8],
                        frame_counter: u32,
                        level: SecurityLevel) -> SResult {
        // IEEE 802.15.4-2015: 9.3.4, CCM* nonce
        let off = enc_consume!(buf; encode_bytes, addr_long);
        let off = enc_consume!(buf, off; encode_u32, frame_counter);
        let off = enc_consume!(buf, off; encode_u8, level as u8);
        stream_done!(off);
    }

    // Prepares crypt_buf with the input for the CCM* authentication
    // transformation. Assumes that self.crypt_buf, self.crypt_iv are present.
    fn prepare_ccm_auth(&self, nonce: &[u8], mic_len: usize, a_data: &[u8], m_data: &[u8]) {
        if nonce.len() != 13 {
            panic!("CCM* nonce must be 13 bytes long");
        }

        // IEEE 802.15.4-2015: Appendix B.4.1.2, CCM* authentication
        // The authentication tag T is computed with AES128-CBC-MAC on
        // B_0 | AuthData, where
        //   B_0 = Flags (1 byte) | nonce (13 bytes) | m length (2 bytes)
        //   Flags = 0 | A data present? (1 bit) | M (3 bits) | L (3 bits)
        //   AuthData = AddAuthData | PlaintextData
        //   AddAuthData = L(a) (encoding of a_data.len()) | a_data
        //   PlaintextData = m_data
        //   Both AddAuthData and PlaintextData are 0-padded to 16-byte blocks.
        // The following code places B_0 | AuthData into crypt_buf.
        let a_len = a_data.len();
        let m_len = m_data.len();

        // Set IV = 0 for CBC-MAC
        self.crypt_iv.map(|iv| {
            for b in iv.iter_mut() {
                *b = 0;
            }
        });

        self.crypt_buf.map(|cbuf| {
            // flags = reserved | Adata | (M - 2) / 2 | (L - 1)
            let mut flags: u8 = 0;
            if a_len != 0 {
                flags |= 1 << 6;
            }
            if mic_len != 0 {
                flags |= (((mic_len - 2) / 2) as u8) << 3;
            }
            flags |= 1;

            // The first block is flags | nonce | m length
            cbuf[0] = flags;
            cbuf[1..14].copy_from_slice(&nonce);
            encode_u16(&mut cbuf[14..], (m_len as u16).to_le()).done().unwrap();

            // After that comes L(a) | a, where L(a) is the following
            // encoding of a_len:
            let mut off = 16;
            if a_len == 0 {
                // L(a) is empty, and the Adata flag is zero
            } else if a_len < 0xff00 as usize {
                // L(a) is l(a) in 2 bytes of little-endian
                encode_u16(&mut cbuf[off..], (a_len as u16).to_le())
                    .done().unwrap();
                off += 2;
            } else if a_len <= 0xffffffff as usize {
                // This length encoding branch is defined in the specification
                // but should never be reached because our MTU is 127.
                panic!("CCM* authentication data is larger than MTU");

                // L(a) is 0xfffe | l(a) in 4 bytes of little-endian
                // cbuf[off] = 0xff;
                // cbuf[off + 1] = 0xfe;
                // encode_u32(&mut cbuf[off + 2..], (a_len as u32).to_le())
                //     .done().unwrap();
                // off += 6;
            } else {
                // This length encoding branch is defined in the specification
                // but should never be reached because our MTU is 127.
                panic!("CCM* authentication data is larger than MTU");

                // L(a) is 0xffff | l(a) in 8 bytes of little-endian
                // cbuf[off] = 0xff;
                // cbuf[off + 1] = 0xff;
                // encode_u64(&mut cbuf[off + 2..], (a_len as u64).to_le())
                //     .done().unwrap();
                // off += 10;
            }

            // Append the auth data and 0-pad to a multiple of 16 bytes
            cbuf[off..off + a_len].copy_from_slice(a_data);
            off += a_len;
            while off % 16 != 0 {
                cbuf[off] = 0;
                off += 1;
            }

            // Append plaintext data and 0-pad to a multiple of 16 bytes
            cbuf[off..off + m_len].copy_from_slice(m_data);
            off += m_len;
            while off % 16 != 0 {
                cbuf[off] = 0;
                off += 1;
            }

            self.crypt_buf_len.set(off);
        });
    }

    fn start_ccm_auth(&self) {
        // TODO: call aes_crypt_cbc
    }

    // Prepares crypt_buf with the input for the CCM* encryption transformation.
    // Assumes that self.crypt_buf, self.crypt_iv are present.  Note that since
    // the confidentiality mode is CTR, encryption is the same as decryption.
    fn prepare_ccm_encrypt(&self, nonce: &[u8], m_data: &[u8]) {
        if nonce.len() != 13 {
            panic!("CCM* nonce must be 13 bytes long");
        }

        // IEEE 802.15.4-2015: Appendix B.4.1.3, CCM* encryption
        // Let A_i = flags | nonce | i (2 bytes)
        //     M_1, M_2, ... = m_data 0-padded to 16-byte blocks
        // The CCM* ciphertext is computed with AES128-CTR on the 0-padded
        // plaintext, with initial counter A_1, followed by the encrypted MIC
        // tag U.
        //
        // The encrypted MIC tag U is computed from the unencrypted MIC tag T by
        // U = E(Key, A_0) xor T. Hence, let M_0 = 0. By computing AES128-CTR on
        // M with initial counter A_0, the first block in the resulting
        // ciphertext will be C_0 = E(Key, A_0) xor M_0 = E(Key, A_0). U can
        // then be computed easily by T xor C_0.
        //
        // The following code places the message in the buffer as described
        // above. It also prepares A_0 in crypt_iv.
        self.crypt_iv.map(|iv| {
            // flags = reserved | reserved | 0 | (L - 1)
            // Since L = 2, flags = 1.
            iv[0] = 1;
            iv[1..14].copy_from_slice(nonce);
            iv[14] = 0;
            iv[15] = 0;
        });

        self.crypt_buf.map(|cbuf| {
            for b in cbuf[..16].iter_mut() {
                *b = 0;
            }
            cbuf[16..16 + m_data.len()].copy_from_slice(m_data);
            let mut off = 16 + m_data.len();
            while off % 16 != 0 {
                cbuf[off] = 0;
                off += 1;
            }

            self.crypt_buf_len.set(off);
        });
    }

    fn start_ccm_encrypt(&self) {
        // TODO: call aes_crypt_ctr
    }

    // The first step in the procedure to transmit a frame is to perform the
    // outgoing frame security procedure. Returns the next state to enter.
    fn outgoing_frame_security(&self,
                               buf: &'static mut [u8],
                               frame_info: FrameInfo) -> TxState {
        self.tx_buf.replace(buf);
        self.tx_info.set(Some(frame_info));

        // IEEE 802.15.4-2015: 9.2.1, outgoing frame security
        // Steps a-e have already been performed in the frame preparation step,
        // so we only need to dispatch on the security parameters in the frame info
        match frame_info.security_params {
            Some((level, _, _)) => {
                if level == SecurityLevel::None {
                    // This case should never occur if the FrameInfo was
                    // prepared by prepare_data_frame
                    TxState::ReadyToTransmit
                } else {
                    TxState::ReadyToSecure
                }
            }
            None => TxState::ReadyToTransmit,
        }
    }

    fn step_transmit_state(&self, next_state: TxState) -> (ReturnCode, Option<&'static mut [u8]>) {
        self.tx_state.set(next_state);
        match next_state {
            TxState::Idle => (ReturnCode::SUCCESS, None),
            TxState::ReadyToSecure => {
                // If hardware encryption is busy, the callback will continue
                // this operation when it is done.
                if self.crypt_busy.get() {
                    return (ReturnCode::SUCCESS, None);
                }

                let frame_info = self.tx_info.get().unwrap();
                let (ref level, ref key, ref nonce) =
                    frame_info.security_params.unwrap();

                // Get positions of a and m data
                let (m_off, m_len) = frame_info.get_ccm_encrypt_ranges(level);
                self.tx_m_off.set(m_off);
                self.tx_m_len.set(m_len);

                // Prepare for CCM* authentication
                self.tx_buf.map(|buf| {
                    self.prepare_ccm_auth(nonce,
                                          frame_info.mic_len,
                                          &buf[radio::PSDU_OFFSET..m_off],
                                          &buf[m_off..m_off + m_len]);
                });

                // Set state before starting CCM* in case callback
                // fires immediately
                self.tx_state.set(TxState::AuthDone);
                // TODO: self.crypto.set_key(key, 16);
                self.crypt_busy.set(true);
                self.start_ccm_auth();

                // Wait for crypt_done to trigger the next transmit state
                (ReturnCode::SUCCESS, None)
            }
            TxState::AuthDone => {
                // The authentication tag T is now the first mic_len bytes of
                // the last 16-byte block in crypt_buf. We append that to the
                // frame, and then encrypt the message data.
                let crypt_t_off = self.crypt_buf_len.get() - 16;

                let frame_info = self.tx_info.get().unwrap();
                let mic_len = frame_info.mic_len;
                let m_off = self.tx_m_off.get();
                let m_len = self.tx_m_len.get();
                self.crypt_buf.map(|cbuf| {
                    self.tx_buf.map(|buf| {
                        let t_off = m_off + m_len;
                        buf[t_off..t_off + mic_len]
                            .copy_from_slice(&cbuf[crypt_t_off..crypt_t_off + mic_len]);
                    });
                });

                // Start the encryption transformation
                let (_, _, ref nonce) = frame_info.security_params.unwrap();
                self.tx_buf.map(|buf| {
                    self.prepare_ccm_encrypt(nonce,
                                             &buf[m_off..m_off + m_len]);
                });

                self.tx_state.set(TxState::EncDone);
                self.start_ccm_encrypt();
                (ReturnCode::SUCCESS, None)
            }
            TxState::EncDone => {
                // The first block of crypt_buf is now E(Key, A_0), and T is
                // already appended to the frame in tx_buf, so we should xor
                // the first mic_len bytes of crypt_buf with that to produce the
                // encrypted MIC, U. Then, we should copy the first m_len bytes
                // of the remaining blocks in crypt_buf over to the frame
                // payload in tx_buf.

                let frame_info = self.tx_info.get().unwrap();
                let mic_len = frame_info.mic_len;
                let m_off = self.tx_m_off.get();
                let m_len = self.tx_m_len.get();
                self.crypt_buf.map(|cbuf| {
                    self.tx_buf.map(|buf| {
                        let t_off = m_off + m_len;
                        for (b, c) in buf[t_off..].iter_mut()
                            .zip(cbuf.iter()).take(mic_len) {
                            *b ^= *c;
                        }

                        buf[m_off..m_off + m_len]
                            .copy_from_slice(&cbuf[16..16 + m_len]);
                    });
                });

                self.crypt_busy.set(false);
                self.step_transmit_state(TxState::ReadyToTransmit)
            }
            TxState::ReadyToTransmit => {
                if self.config_in_progress.get() {
                    // We will continue when the configuration is done.
                    (ReturnCode::SUCCESS, None)
                } else {
                    let frame_info = self.tx_info.get().unwrap();
                    let buf = self.tx_buf.take().unwrap();
                    self.tx_state.set(TxState::Idle);
                    self.tx_info.set(None);
                    self.radio.transmit(buf, frame_info.secured_frame_length())
                }
            }
        }
    }

    // The procedure to verify and unsecure incoming frames
    fn incoming_frame_security(&self, buf: &[u8], frame_len: usize) -> RxState {
        if let Some((data_offset, (header, mac_payload_offset))) =
            Header::decode(&buf[radio::PSDU_OFFSET..]).done() {
            let mic_len = header.security.map_or(0, |sec| sec.level.mic_len());
            let data_len = frame_len - data_offset - mic_len;
            if let Some(security) = header.security {
                // IEEE 802.15.4-2015: 9.2.3, incoming frame security procedure
                // for security-enabled headers
                if header.version == FrameVersion::V2003 {
                    // Legacy frames are not supported
                    RxState::ReadyToReturn
                } else {
                    // Step e: Lookup the key.
                    let key = match self.lookup_key(security.level, security.key_id) {
                        Some(key) => key,
                        None => { return RxState::ReadyToReturn; }
                    };

                    // Step f: Obtain the extended source address
                    let device_addr = match self.lookup_addr_long(header.src_addr) {
                        Some(addr) => addr,
                        None => { return RxState::ReadyToReturn; }
                    };

                    // Step g, h: Check frame counter
                    let frame_counter = match security.frame_counter {
                        Some(frame_counter) => {
                            if frame_counter == 0xffffffff {
                                // Counter error
                                return RxState::ReadyToReturn;
                            }
                            // TODO: Check frame counter against source device
                            frame_counter
                        }
                        // TSCH mode, where ASN is used instead, not supported
                        None => { return RxState::ReadyToReturn; }
                    };

                    let mut nonce = [0; 13];
                    self.encode_ccm_nonce(&mut nonce,
                                          &device_addr,
                                          frame_counter,
                                          security.level).done().unwrap();

                    let mic_len = security.level.mic_len();
                    self.rx_info.set(Some(FrameInfo {
                        frame_type: header.frame_type,
                        mac_payload_offset: mac_payload_offset,
                        data_offset: data_offset,
                        data_len: data_len,
                        mic_len: mic_len,
                        security_params: Some((security.level, key, nonce)),
                    }));
                    RxState::ReadyToUnsecure
                }
            } else {
                // No security needed, can yield the frame immediately
                self.rx_client.get().map(|client| {
                    client.receive(&buf,
                                   header,
                                   data_offset,
                                   data_len,
                                   ReturnCode::SUCCESS);
                });
                RxState::ReadyToReturn
            }
        } else {
            // Drop frames with invalid headers
            RxState::ReadyToReturn
        }
    }

    fn step_receive_state(&self, next_state: RxState) {
        self.rx_state.set(next_state);
        match next_state {
            RxState::Idle => {}
            RxState::ReadyToUnsecure => {
                // If hardware encryption is busy, the callback will continue
                // this operation when it is done.
                if self.crypt_busy.get() { return; }

                let frame_info = self.rx_info.get().unwrap();
                let (ref level, ref key, ref nonce) =
                    frame_info.security_params.unwrap();

                // Get positions of a and c data
                let (c_off, c_len) = frame_info.get_ccm_encrypt_ranges(level);
                self.rx_c_off.set(c_off);
                self.rx_c_len.set(c_len);

                // CCM* decryption (which is the same as encryption)
                self.rx_buf.map(|buf| {
                    self.prepare_ccm_encrypt(nonce,
                                             &buf[c_off..c_off + c_len]);
                });

                // Set state before starting CCM*
                self.rx_state.set(RxState::DecDone);
                // TODO: self.crypto.set_key(key, 16);
                self.crypt_busy.set(true);
                self.start_ccm_encrypt();
            }
            RxState::DecDone => {
                // The first block of crypt_buf is now E(Key, A_0), and U is
                // already at the end of the frame in rx_buf, so we should xor
                // the first mic_len bytes of crypt_buf with that to produce the
                // unencrypted MIC, T = U xor E(key, A_0). Then, we should copy the
                // decrypted private data payload from the first m_len bytes
                // of the remaining blocks in crypt_buf over to the frame
                // payload in rx_buf.
                let frame_info = self.rx_info.get().unwrap();
                let mic_len = frame_info.mic_len;
                let c_off = self.rx_c_off.get();
                let c_len = self.rx_c_len.get();
                self.crypt_buf.map(|cbuf| {
                    self.rx_buf.map(|buf| {
                        let t_off = c_off + c_len;
                        for (b, c) in buf[t_off..].iter_mut()
                            .zip(cbuf.iter()).take(mic_len) {
                            *b ^= *c;
                        }

                        buf[c_off..c_off + c_len]
                            .copy_from_slice(&cbuf[16..16 + c_len]);
                    });
                });

                // Now, rx_buf contains the unsecured frame with the unencrypted
                // MIC at the end. Hence, the procedure for verifying the tag is
                // exactly the same as if starting from an unencrypted but
                // integrity-protected frame.
                // At this point, rx_buf contains the plaintext authentication
                // data and plaintext payload data, followed by the
                // authentication tag at the end.
                let (_, _, ref nonce) = frame_info.security_params.unwrap();
                self.rx_buf.map(|buf| {
                    self.prepare_ccm_auth(nonce,
                                          frame_info.mic_len,
                                          &buf[radio::PSDU_OFFSET..c_off],
                                          &buf[c_off..c_off + c_len]);
                });

                self.rx_state.set(RxState::AuthDone);
                self.start_ccm_auth();
            }
            RxState::AuthDone => {
                // The recomputed MAC tag T' is the first mic_len bytes of the
                // last 16-byte block of crypt_buf. Compare that with the
                // transmitted MAC tag T to verify the integrity of the frame.

                let mut verified = false;
                let frame_info = self.rx_info.get().unwrap();
                let mic_len = frame_info.mic_len;
                let crypt_t_off = self.crypt_buf_len.get() - 16;
                let t_off = self.rx_c_off.get() + self.rx_c_len.get();
                self.crypt_buf.map(|cbuf| {
                    self.rx_buf.map(|buf| {
                        verified = cbuf[crypt_t_off..crypt_t_off + mic_len]
                            .iter().eq(buf[t_off..t_off + mic_len].iter());
                    });
                });
                self.crypt_busy.set(false);

                // If authentication failed, we drop the frame and return it to
                // the radio without passing it to the client.
                if !verified {
                    self.step_receive_state(RxState::ReadyToReturn);
                }

                // Otherwise, we continue the incoming frame security procedure
                // TODO: Steps j-o: In particular, we need to update the frame
                // counter for the source device

                // Re-parse the now-unsecured frame and expose it to the client.
                self.rx_buf.map(|buf| {
                    if let Some((_, (header, _))) =
                        Header::decode(&buf[radio::PSDU_OFFSET..]).done() {
                        // Show rx_client the unsecured frame without the MIC
                        self.rx_client.get().map(|client| {
                            client.receive(&buf,
                                           header,
                                           frame_info.data_offset,
                                           frame_info.data_len,
                                           ReturnCode::SUCCESS);
                        });
                    }
                });

                // Return the buffer to the radio
                self.step_receive_state(RxState::ReadyToReturn);
            }
            RxState::ReadyToReturn => {
                self.rx_state.set(RxState::Idle);
                self.rx_info.set(None);
                let buf = self.rx_buf.take().unwrap();
                self.radio.set_receive_buffer(buf);
            }
        }
    }

    fn trigger_states(&self) {
        let (rval, buf) = self.step_transmit_state(self.tx_state.get());
        if let Some(buf) = buf {
            // Return the buffer to the transmit client
            self.tx_client.get().map(move |client| { client.send_done(buf, false, rval); });
        }
        self.step_receive_state(self.rx_state.get());
    }
}

impl<'a, R: radio::Radio + 'a> Mac for MacDevice<'a, R> {
    fn get_address(&self) -> u16 {
        self.radio.get_address()
    }

    fn get_address_long(&self) -> [u8; 8] {
        self.radio.get_address_long()
    }

    fn get_pan(&self) -> u16 {
        self.radio.get_pan()
    }

    fn get_channel(&self) -> u8 {
        self.radio.get_channel()
    }

    fn get_tx_power(&self) -> i8 {
        self.radio.get_tx_power()
    }

    fn set_address(&self, addr: u16) {
        self.radio.set_address(addr)
    }

    fn set_address_long(&self, addr: [u8; 8]) {
        self.radio.set_address_long(addr)
    }

    fn set_pan(&self, id: u16) {
        self.radio.set_pan(id)
    }

    fn set_channel(&self, chan: u8) -> ReturnCode {
        self.radio.set_channel(chan)
    }

    fn set_tx_power(&self, power: i8) -> ReturnCode {
        self.radio.set_tx_power(power)
    }

    fn config_commit(&self) -> ReturnCode {
        let rval = if !self.config_in_progress.get() {
            self.radio.config_commit()
        } else {
            ReturnCode::EBUSY
        };
        if rval == ReturnCode::SUCCESS {
            self.config_in_progress.set(true)
        }
        rval
    }

    fn is_on(&self) -> bool {
        self.radio.is_on()
    }

    fn prepare_data_frame(&self,
                          buf: &mut [u8],
                          dst_pan: PanID,
                          dst_addr: MacAddress,
                          src_pan: PanID,
                          src_addr: MacAddress,
                          security_needed: Option<(SecurityLevel, KeyId)>)
                          -> Result<FrameInfo, ()> {
        // IEEE 802.15.4-2015: 9.2.1 outgoing frame security
        // Steps a-e of the security procedure are implemented here.
        let src_addr_long = self.get_address_long();
        let security_desc = security_needed.and_then(|(level, key_id)| {
            self.lookup_key(level, key_id).map(|key| {
                // TODO: lookup frame counter for device
                let frame_counter = 0;
                let mut nonce = [0; 13];
                self.encode_ccm_nonce(&mut nonce,
                                      &src_addr_long,
                                      frame_counter,
                                      level).done().unwrap();
                (Security {
                    level: level,
                    asn_in_nonce: false,
                    frame_counter: Some(frame_counter),
                    key_id: key_id,
                 },
                 key,
                 nonce)
            })
        });
        if security_needed.is_some() && security_desc.is_none() {
            // If security was requested, fail when desired key was not found.
            return Err(());
        }

        // Construct MAC header
        let security = security_desc.map(|(sec, _, _)| sec);
        let mic_len = security.map_or(0, |sec| sec.level.mic_len());
        let header = Header {
            frame_type: FrameType::Data,
            /* TODO: determine this by looking at queue */
            frame_pending: false,
            // Unicast data frames request acknowledgement
            ack_requested: true,
            version: FrameVersion::V2015,
            seq: Some(self.data_sequence.get()),
            dst_pan: Some(dst_pan),
            dst_addr: Some(dst_addr),
            src_pan: Some(src_pan),
            src_addr: Some(src_addr),
            security: security,
            header_ies: Default::default(),
            header_ies_len: 0,
            payload_ies: Default::default(),
            payload_ies_len: 0,
        };

        header.encode(&mut buf[radio::PSDU_OFFSET..], true)
            .done()
            .map(|(data_offset, mac_payload_offset)| {
                FrameInfo {
                    frame_type: FrameType::Data,
                    mac_payload_offset: mac_payload_offset,
                    data_offset: data_offset,
                    data_len: 0,
                    mic_len: mic_len,
                    security_params: security_desc
                        .map(|(sec, key, nonce)| (sec.level, key, nonce)),
                }
            })
            .ok_or(())
    }

    fn transmit(&self,
                buf: &'static mut [u8],
                frame_info: FrameInfo)
                -> (ReturnCode, Option<&'static mut [u8]>) {
        if self.tx_state.get() != TxState::Idle {
            return (ReturnCode::EBUSY, Some(buf));
        }

        let next_state = self.outgoing_frame_security(buf, frame_info);
        self.step_transmit_state(next_state)
    }
}

impl<'a, R: radio::Radio + 'a> radio::TxClient for MacDevice<'a, R> {
    fn send_done(&self, buf: &'static mut [u8], acked: bool, result: ReturnCode) {
        self.data_sequence.set(self.data_sequence.get() + 1);
        self.tx_info.set(None);
        self.tx_client.get().map(move |client| { client.send_done(buf, acked, result); });
    }
}

impl<'a, R: radio::Radio + 'a> radio::RxClient for MacDevice<'a, R> {
    fn receive(&self, buf: &'static mut [u8], frame_len: usize, crc_valid: bool, _: ReturnCode) {
        // Drop all frames with invalid CRC
        if !crc_valid {
            self.radio.set_receive_buffer(buf);
            return;
        }

        if self.rx_state.get() != RxState::Idle {
            // This should never occur unless something other than this MAC
            // layer provided a receive buffer to the radio, but if
            // this occurs then we have no choice but to drop the frame.
            self.radio.set_receive_buffer(buf);
        } else {
            let next_state = self.incoming_frame_security(buf.as_ref(), frame_len);
            self.rx_buf.replace(buf);
            self.step_receive_state(next_state);
        }
    }
}

impl<'a, R: radio::Radio + 'a> radio::ConfigClient for MacDevice<'a, R> {
    fn config_done(&self, _: ReturnCode) {
        if self.config_in_progress.get() {
            self.config_in_progress.set(false);
            self.trigger_states();
        }
    }
}

// impl<'a, R: radio::Radio + 'a, C: SymmetricEncryption + 'a>
//     symmetric_encryption::Client for MacDevice<'a, R, C> {
//     fn crypt_done(&self, buf: &'static mut [u8], iv: &'static mut [u8], len: usize) -> ReturnCode {
//         self.crypt_buf.replace(buf);
//         self.crypt_iv.replace(iv);
//         self.trigger_states();
//         ReturnCode::SUCCESS
//     }
// }
