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
use net::stream::{encode_u8, encode_u16, encode_u32, encode_u64, encode_bytes_be};
use net::stream::SResult;

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct FrameInfo {
    // These offsets are relative to buf[radio::PSDU_OFFSET..] so that
    // the unsecured mac frame length is data_offset + data_len
    frame_type: FrameType,
    mac_payload_offset: usize,
    data_offset: usize,
    data_len: usize,
    mic_len: usize,
    // Security header, key, nonce
    security_params: Option<(Security, [u8; 16], [u8; 13])>,
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

    // Compute the offsets in the buffer for the a data and m data fields
    // in the CCM* authentication and encryption transformation,
    // which depends on the frame type and security levels. Returns
    // (a_offset, m_offset, m_len) relative to the buffer.
    fn get_ccm_encrypt_ranges(&self, encrypt: bool) -> (usize, usize, usize) {
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

        if !encrypt {
            // If only integrity is need, a data is the whole frame
            (radio::PSDU_OFFSET,
             radio::PSDU_OFFSET + self.unsecured_frame_length(),
             0)
        } else {
            // Otherwise, a data is the header and the open payload, and
            // m data is the private payload field
            (radio::PSDU_OFFSET,
             radio::PSDU_OFFSET + private_payload_offset,
             self.unsecured_frame_length() - private_payload_offset)
        }
    }
}

// Buffer size might be bigger than an MTU due to padding
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
                   data_offset: usize,
                   data_len: usize,
                   result: ReturnCode);
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum TxState {
    Idle,
    ReadyToSecure,
    Mic,
    EncMic1,
    EncMic2,
    ReadyToTransmit,
}

pub struct MacDevice<'a, R: radio::Radio + 'a> {
    radio: &'a R,
    data_sequence: Cell<u8>,
    config_in_progress: Cell<bool>,

    // State for the transmit pathway
    tx_buf: TakeCell<'static, [u8]>,
    tx_info: Cell<Option<FrameInfo>>,
    tx_state: Cell<TxState>,
    tx_client: Cell<Option<&'static TxClient>>,

    // State for the receive pathway
    rx_client: Cell<Option<&'static RxClient>>,

    // State for CCM* authentication/encryption
    crypt_buf: TakeCell<'static, [u8]>,
    crypt_buf_len: Cell<usize>,
    crypt_iv: TakeCell<'static, [u8]>,
    tx_a_off: Cell<usize>,
    tx_m_off: Cell<usize>,
    tx_m_len: Cell<usize>,
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
            tx_state: Cell::new(TxState::Idle),
            tx_client: Cell::new(None),
            rx_client: Cell::new(None),
            crypt_buf: TakeCell::new(crypt_buf),
            crypt_buf_len: Cell::new(0),
            crypt_iv: TakeCell::new(crypt_iv),
            tx_a_off: Cell::new(0),
            tx_m_off: Cell::new(0),
            tx_m_len: Cell::new(0),
        }
    }

    pub fn set_transmit_client(&self, client: &'static TxClient) {
        self.tx_client.set(Some(client));
    }

    pub fn set_receive_client(&self, client: &'static RxClient) {
        self.rx_client.set(Some(client));
    }

    fn lookup_key(&self, level: SecurityLevel, key_id: KeyId)
        -> Option<(Security, [u8; 16])> {
        let security = Security {
            level: level,
            asn_in_nonce: false,
            frame_counter: None,
            key_id: key_id,
        };
        Some((security, [0; 16]))
    }

    // Compute the CCM* nonce.
    fn get_ccm_nonce(&self, security: &Security) -> [u8; 13] {
        let mut nonce = [0; 13];
        self.encode_ccm_nonce(&mut nonce, security).done().unwrap();
        nonce
    }

    fn encode_ccm_nonce(&self, buf: &mut [u8], security: &Security) -> SResult {
        let off = enc_consume!(buf; encode_bytes_be, &self.get_address_long());
        // TSCH mode, where ASN is used for the nonce, is unsupported
        stream_cond!(security.frame_counter.is_some());
        let off = enc_consume!(buf, off; encode_u32,
                           security.frame_counter.unwrap().to_be());
        let off = enc_consume!(buf, off; encode_u8, security.level as u8);
        stream_done!(off);
    }

    // Prepares crypt_buf with the input for the CCM* authentication
    // transformation. Assumes that self.crypt_buf, self.crypt_iv are present.
    fn prepare_ccm_auth(&self, nonce: &[u8], mic_len: usize, a_data: &[u8], m_data: &[u8]) {
        if nonce.len() != 13 {
            panic!("CCM* nonce must be 13 bytes long");
        }

        // IEEE 802.15.4-2015, Appendix B.4.1.2: CCM* authentication
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
                // L(a) is 0xfffe | l(a) in 4 bytes of little-endian
                cbuf[off] = 0xff;
                cbuf[off + 1] = 0xfe;
                encode_u32(&mut cbuf[off + 2..], (a_len as u32).to_le())
                    .done().unwrap();
                off += 6;
            } else {
                // L(a) is 0xffff | l(a) in 4 bytes of little-endian
                cbuf[off] = 0xff;
                cbuf[off + 1] = 0xff;
                encode_u64(&mut cbuf[off + 2..], (a_len as u64).to_le())
                    .done().unwrap();
                off += 10;
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
        // TODO: call aes.crypt_cbc
    }

    // Prepares crypt_buf with the input for the CCM* encryption
    // transformation. Assumes that self.crypt_buf, self.crypt_iv are present.
    fn prepare_ccm_encrypt(&self, nonce: &[u8], m_data: &[u8]) {
        if nonce.len() != 13 {
            panic!("CCM* nonce must be 13 bytes long");
        }

        // IEEE 802.15.4-2015, Appendix B.4.1.3, CCM* encryption
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
        // TODO: call aes.crypt_ctr
    }

    fn step_transmit_state(&self) -> (ReturnCode, Option<&'static mut [u8]>) {
        match self.tx_state.get() {
            TxState::Idle => (ReturnCode::SUCCESS, None),
            TxState::ReadyToSecure => {
                // If hardware encryption is busy, the callback will continue
                // this operation when it is done.
                if self.crypt_buf.is_none() {
                    return (ReturnCode::SUCCESS, None);
                }

                let frame_info = self.tx_info.get().unwrap();
                let (ref security, ref key, ref nonce) =
                    frame_info.security_params.unwrap();
                let encrypt = match security.level {
                    SecurityLevel::None => {
                        self.tx_state.set(TxState::ReadyToTransmit);
                        return self.step_transmit_state();
                    }
                    SecurityLevel::Mic32
                    | SecurityLevel::Mic64
                    | SecurityLevel::Mic128 => false,
                    SecurityLevel::EncMic32
                    | SecurityLevel::EncMic64
                    | SecurityLevel::EncMic128 => true,
                };

                // Get positions of a and m data
                let (a_off, m_off, m_len) =
                    frame_info.get_ccm_encrypt_ranges(encrypt);
                self.tx_a_off.set(a_off);
                self.tx_m_off.set(m_off);
                self.tx_m_len.set(m_len);

                // Prepare for CCM* authentication
                self.tx_buf.map(|buf| {
                    self.prepare_ccm_auth(nonce,
                                          frame_info.mic_len,
                                          &buf[a_off..m_off],
                                          &buf[m_off..m_off + m_len]);
                });

                // Set state before starting CCM* in case callback
                // fires immediately
                if !encrypt {
                    self.tx_state.set(TxState::Mic);
                } else {
                    self.tx_state.set(TxState::EncMic1);
                }
                // TODO: set encryption key to the security key
                self.start_ccm_auth();

                // Wait for crypt_done to trigger the next transmit state
                (ReturnCode::SUCCESS, None)
            }
            TxState::Mic => {
                // The authentication tag is now the first mic_len bytes of
                // the last 16-byte block in crypt_buf. Append that to the
                // frame and it is ready to transmit.
                let crypt_t_off = self.crypt_buf_len.get() - 16;

                let frame_info = self.tx_info.get().unwrap();
                let mic_len = frame_info.mic_len;
                let t_off = self.tx_m_off.get() + self.tx_m_len.get();
                self.crypt_buf.map(|cbuf| {
                    self.tx_buf.map(|buf| {
                        buf[t_off..t_off + mic_len]
                            .copy_from_slice(&cbuf[crypt_t_off..crypt_t_off + mic_len]);
                    });
                });

                self.tx_state.set(TxState::ReadyToTransmit);
                self.step_transmit_state()
            }
            TxState::EncMic1 => {
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
                self.tx_state.set(TxState::EncMic2);
                self.start_ccm_encrypt();
                (ReturnCode::SUCCESS, None)
            }
            TxState::EncMic2 => {
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

                self.tx_state.set(TxState::ReadyToTransmit);
                self.step_transmit_state()
            }
            TxState::ReadyToTransmit => {
                if self.config_in_progress.get() {
                    // We will continue when the configuration is done.
                    (ReturnCode::SUCCESS, None)
                } else {
                    let frame_info = self.tx_info.get().unwrap();
                    let buf = self.tx_buf.take().unwrap();
                    self.tx_state.set(TxState::Idle);
                    self.radio.transmit(buf, frame_info.secured_frame_length())
                }
            }
        }
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
        let security_params = security_needed
            .and_then(|(level, key_id)| self.lookup_key(level, key_id))
            .map(|(sec, key)| (sec, key, self.get_ccm_nonce(&sec)));
        if security_needed.is_some() && security_params.is_none() {
            // If security was requested, fail when desired key was not found.
            return Err(());
        }

        let mic_len = match security_params {
            Some((security, _, _)) => {
                match security.level {
                    SecurityLevel::Mic32
                    | SecurityLevel::EncMic32 => 4,
                    SecurityLevel::Mic64
                    | SecurityLevel::EncMic64 => 8,
                    SecurityLevel::Mic128
                    | SecurityLevel::EncMic128 => 16,
                    _ => 0,
                }
            }
            None => 0,
        };

        // Construct MAC header
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
            security: security_params.map(|(sec, _, _)| sec),
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
                    security_params: security_params,
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

        self.tx_buf.replace(buf);
        self.tx_info.set(Some(frame_info));
        match frame_info.security_params {
            Some(_) => self.tx_state.set(TxState::ReadyToSecure),
            None => self.tx_state.set(TxState::ReadyToTransmit),
        }
        self.step_transmit_state()
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

        // Try to read the MAC headers of the frame to determine if decryption is
        // needed. Otherwise, dispatch the parsed headers directly to the client
        let decrypt = if let Some((data_offset, (header, mac_payload_offset))) =
            Header::decode(&buf[radio::PSDU_OFFSET..]).done() {
            // 802.15.4 Incoming frame security procedure
            let buf_data_offset = radio::PSDU_OFFSET + data_offset;
            let data_len = frame_len - data_offset;
            if let Some(security) = header.security {
                if header.version == FrameVersion::V2003 || security.level == SecurityLevel::None {
                    // Version must not be 2003 (legacy) and the security level must
                    // not be none, otherwise incoming security is undefined.
                    // Hence, we drop the frame
                    false
                } else {
                    // TODO: Implement decryption
                    self.rx_client.get().map(|client| {
                        client.receive(&buf,
                                       header,
                                       buf_data_offset,
                                       data_len,
                                       ReturnCode::ENOSUPPORT);
                    });
                    false
                }
            } else {
                // No security needed, can yield the frame immediately
                self.rx_client.get().map(|client| {
                    client.receive(&buf,
                                   header,
                                   buf_data_offset,
                                   data_len,
                                   ReturnCode::ENOSUPPORT);
                });
                false
            }
        } else {
            false
        };

        // If decryption is needed, we begin the decryption process, otherwise,
        // we can return the buffer immediately to the radio.
        if decrypt {
            // TODO: Implement decryption
            self.radio.set_receive_buffer(buf);
        } else {
            self.radio.set_receive_buffer(buf);
        }
    }
}

impl<'a, R: radio::Radio + 'a> radio::ConfigClient for MacDevice<'a, R> {
    fn config_done(&self, _: ReturnCode) {
        if self.config_in_progress.get() {
            self.config_in_progress.set(false);
            let (rval, buf) = self.step_transmit_state();
            if let Some(buf) = buf {
                // Return the buffer to the transmit client
                self.tx_client.get().map(move |client| { client.send_done(buf, false, rval); });
            }
        }
    }
}
