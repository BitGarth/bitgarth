//! Hand-coded protobuf message encoding/decoding for Trezor Bridge communication.
//!
//! This module implements the minimal subset of Trezor protobuf messages needed
//! for exporting extended public keys. We avoid using prost to keep dependencies minimal.
//!
//! Message format over Bridge:
//! - 2 bytes: message type (big-endian u16)
//! - 4 bytes: payload length (big-endian u32)
//! - N bytes: protobuf-encoded payload

use super::types::{TrezorError, TrezorErrorDetail};
use crate::wallets::AddressScheme;

/// Trezor message type IDs from trezor-common/protob/messages.proto
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub(crate) enum MessageType {
    /// Initialize device session (resets state, returns Features)
    Initialize = 0,
    /// Failure response from device
    Failure = 3,
    /// GetPublicKey request
    GetPublicKey = 11,
    /// PublicKey response
    PublicKey = 12,
    /// Features response from Initialize
    Features = 17,
    /// Device requests PIN entry (on-device for Safe 3/5)
    PinMatrixRequest = 18,
    /// Not used for Safe 3/5 (they have on-device PIN)
    PinMatrixAck = 19,
    /// Device requests button confirmation
    ButtonRequest = 26,
    /// Acknowledge button request
    ButtonAck = 27,
    /// Device requests passphrase
    PassphraseRequest = 41,
    /// Passphrase acknowledgement
    PassphraseAck = 42,
}

impl MessageType {
    pub(crate) fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(MessageType::Initialize),
            3 => Some(MessageType::Failure),
            11 => Some(MessageType::GetPublicKey),
            12 => Some(MessageType::PublicKey),
            17 => Some(MessageType::Features),
            18 => Some(MessageType::PinMatrixRequest),
            19 => Some(MessageType::PinMatrixAck),
            26 => Some(MessageType::ButtonRequest),
            27 => Some(MessageType::ButtonAck),
            41 => Some(MessageType::PassphraseRequest),
            42 => Some(MessageType::PassphraseAck),
            _ => None,
        }
    }
}

/// Protobuf wire types
#[derive(Debug, Clone, Copy)]
enum WireType {
    Varint = 0,
    LengthDelimited = 2,
}

/// Protobuf encoder
pub(crate) struct ProtoEncoder {
    buffer: Vec<u8>,
}

impl ProtoEncoder {
    pub(crate) fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Encode a varint (used for field tags and integer values)
    fn encode_varint(&mut self, mut value: u64) {
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                self.buffer.push(byte);
                break;
            } else {
                self.buffer.push(byte | 0x80);
            }
        }
    }

    /// Encode a field tag (field number + wire type)
    fn encode_tag(&mut self, field_number: u32, wire_type: WireType) {
        let tag = ((field_number as u64) << 3) | (wire_type as u64);
        self.encode_varint(tag);
    }

    /// Encode a repeated uint32 field (used for BIP32 path)
    pub(crate) fn encode_repeated_uint32(&mut self, field_number: u32, values: &[u32]) {
        for value in values {
            self.encode_tag(field_number, WireType::Varint);
            self.encode_varint(*value as u64);
        }
    }

    /// Encode a string field
    pub(crate) fn encode_string(&mut self, field_number: u32, value: &str) {
        self.encode_tag(field_number, WireType::LengthDelimited);
        self.encode_varint(value.len() as u64);
        self.buffer.extend_from_slice(value.as_bytes());
    }

    /// Encode a bool field
    pub(crate) fn encode_bool(&mut self, field_number: u32, value: bool) {
        self.encode_tag(field_number, WireType::Varint);
        self.encode_varint(if value { 1 } else { 0 });
    }

    /// Encode a uint32 field
    pub(crate) fn encode_uint32(&mut self, field_number: u32, value: u32) {
        self.encode_tag(field_number, WireType::Varint);
        self.encode_varint(value as u64);
    }

    /// Get the encoded bytes
    pub(crate) fn finish(self) -> Vec<u8> {
        self.buffer
    }
}

/// Protobuf decoder
pub(crate) struct ProtoDecoder<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ProtoDecoder<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn decode_varint(&mut self) -> Result<u64, TrezorError> {
        let mut result: u64 = 0;
        let mut shift = 0;
        loop {
            if self.pos >= self.data.len() {
                return Err(TrezorError::protocol_error(
                    "Unexpected end of varint".to_string(),
                ));
            }
            let byte = self.data[self.pos];
            self.pos += 1;
            result |= ((byte & 0x7F) as u64) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 63 {
                return Err(TrezorError::protocol_error("Varint too long".to_string()));
            }
        }
        Ok(result)
    }

    fn decode_tag(&mut self) -> Result<Option<(u32, WireType)>, TrezorError> {
        if self.pos >= self.data.len() {
            return Ok(None);
        }
        let tag = self.decode_varint()?;
        let field_number = (tag >> 3) as u32;
        let wire_type = match tag & 0x07 {
            0 => WireType::Varint,
            2 => WireType::LengthDelimited,
            other => {
                return Err(TrezorError::protocol_error(format!(
                    "Unsupported wire type: {other}"
                )));
            }
        };
        Ok(Some((field_number, wire_type)))
    }

    fn skip_field(&mut self, wire_type: WireType) -> Result<(), TrezorError> {
        match wire_type {
            WireType::Varint => {
                self.decode_varint()?;
            }
            WireType::LengthDelimited => {
                let len = self.decode_varint()? as usize;
                if self.pos + len > self.data.len() {
                    return Err(TrezorError::protocol_error(
                        "Length-delimited field extends beyond data".to_string(),
                    ));
                }
                self.pos += len;
            }
        }
        Ok(())
    }

    pub(crate) fn decode_string(&mut self) -> Result<String, TrezorError> {
        let len = self.decode_varint()? as usize;
        if self.pos + len > self.data.len() {
            return Err(TrezorError::protocol_error(
                "String extends beyond data".to_string(),
            ));
        }
        let s = String::from_utf8(self.data[self.pos..self.pos + len].to_vec())
            .map_err(|e| TrezorError::protocol_error(format!("Invalid UTF-8: {e}")))?;
        self.pos += len;
        Ok(s)
    }

    pub(crate) fn decode_uint32(&mut self) -> Result<u32, TrezorError> {
        let value = self.decode_varint()?;
        Ok(value as u32)
    }
}

/// Input script types for GetPublicKey
#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub(crate) enum InputScriptType {
    /// BIP44 (legacy P2PKH) — SPENDADDRESS = 0
    SpendAddress = 0,
    /// BIP84 (native segwit P2WPKH) — SPENDWITNESS = 3
    SpendWitness = 3,
    /// BIP49 (P2SH-P2WPKH) — SPENDP2SHWITNESS = 4
    SpendP2SHWitness = 4,
    /// BIP86 (taproot) — SPENDTAPROOT = 5
    SpendTaproot = 5,
}

impl InputScriptType {
    #[cfg(all(test, not(bitgarth_db_unit_only)))]
    pub(crate) fn from_u32(value: u32) -> Option<Self> {
        match value {
            0 => Some(InputScriptType::SpendAddress),
            3 => Some(InputScriptType::SpendWitness),
            4 => Some(InputScriptType::SpendP2SHWitness),
            5 => Some(InputScriptType::SpendTaproot),
            _ => None,
        }
    }
}

/// GetPublicKey request message
pub(crate) struct GetPublicKeyRequest {
    /// BIP32 path as hardened indices (e.g., [84' | 0x80000000, 0' | 0x80000000, 0' | 0x80000000])
    pub address_n: Vec<u32>,
    /// Coin name (e.g., "Bitcoin")
    pub coin_name: Option<String>,
    /// Show on device display
    pub show_display: bool,
    /// Script type for address derivation
    pub script_type: Option<InputScriptType>,
}

impl GetPublicKeyRequest {
    /// Create a request for master fingerprint (m/0')
    pub(crate) fn for_fingerprint() -> Self {
        Self {
            address_n: vec![0x80000000], // m/0' (hardened)
            coin_name: None,
            show_display: false,
            script_type: None,
        }
    }

    /// Create a request for a bitcoin account key at the given account index.
    pub(crate) fn for_address_scheme(account_index: u32, address_scheme: AddressScheme) -> Self {
        let (purpose, script_type) = match address_scheme {
            AddressScheme::Legacy => (44, InputScriptType::SpendAddress),
            AddressScheme::NestedSegwit => (49, InputScriptType::SpendP2SHWitness),
            AddressScheme::NativeSegwit => (84, InputScriptType::SpendWitness),
            AddressScheme::Taproot => (86, InputScriptType::SpendTaproot),
            AddressScheme::Standard => (44, InputScriptType::SpendAddress),
        };

        Self {
            address_n: vec![
                purpose | 0x80000000,       // purpose
                0x80000000,                 // coin_type: 0' (Bitcoin mainnet)
                account_index | 0x80000000, // account: N' (hardened)
            ],
            coin_name: Some("Bitcoin".to_string()),
            show_display: true,
            script_type: Some(script_type),
        }
    }

    /// Encode to protobuf bytes
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut encoder = ProtoEncoder::new();

        // Field 1: address_n (repeated uint32)
        encoder.encode_repeated_uint32(1, &self.address_n);

        // Field 3: show_display (optional bool)
        if self.show_display {
            encoder.encode_bool(3, true);
        }

        // Field 4: coin_name (optional string)
        if let Some(ref coin_name) = self.coin_name {
            encoder.encode_string(4, coin_name);
        }

        // Field 5: script_type (optional enum as uint32)
        if let Some(script_type) = self.script_type {
            encoder.encode_uint32(5, script_type as u32);
        }

        encoder.finish()
    }

    /// Decode from protobuf bytes
    #[cfg(all(test, not(bitgarth_db_unit_only)))]
    pub(crate) fn decode(data: &[u8]) -> Result<Self, TrezorError> {
        let mut decoder = ProtoDecoder::new(data);
        let mut address_n: Vec<u32> = Vec::new();
        let mut coin_name = None;
        let mut show_display = false;
        let mut script_type = None;

        while decoder.remaining() > 0 {
            let Some((field_number, wire_type)) = decoder.decode_tag()? else {
                break;
            };

            match field_number {
                // Field 1: address_n (repeated uint32)
                1 => {
                    let value = decoder.decode_uint32()?;
                    address_n.push(value);
                }
                // Field 3: show_display (bool)
                3 => {
                    let value = decoder.decode_uint32()?;
                    show_display = value != 0;
                }
                // Field 4: coin_name (string)
                4 => {
                    coin_name = Some(decoder.decode_string()?);
                }
                // Field 5: script_type (enum as uint32)
                5 => {
                    let raw = decoder.decode_uint32()?;
                    script_type = Some(InputScriptType::from_u32(raw).ok_or_else(|| {
                        TrezorError::protocol_error(format!("Unknown script type: {raw}"))
                    })?);
                }
                _ => {
                    decoder.skip_field(wire_type)?;
                }
            }
        }

        Ok(Self {
            address_n,
            coin_name,
            show_display,
            script_type,
        })
    }
}

/// PublicKey response message
#[derive(Debug)]
pub(crate) struct PublicKeyResponse {
    /// Extended public key in base58 format
    pub xpub: String,
    /// Root fingerprint (4 bytes as u32)
    pub root_fingerprint: Option<u32>,
}

impl PublicKeyResponse {
    /// Decode from protobuf bytes
    pub(crate) fn decode(data: &[u8]) -> Result<Self, TrezorError> {
        let mut decoder = ProtoDecoder::new(data);
        let mut xpub = String::new();
        let mut root_fingerprint = None;

        while decoder.remaining() > 0 {
            let Some((field_number, wire_type)) = decoder.decode_tag()? else {
                break;
            };

            match field_number {
                // Field 1: node (HDNodeType) - we need to extract xpub from inside
                1 => {
                    // Skip the nested message for now, we use field 2 (xpub) directly
                    decoder.skip_field(wire_type)?;
                }
                // Field 2: xpub (string)
                2 => {
                    xpub = decoder.decode_string()?;
                }
                // Field 3: root_fingerprint (uint32)
                3 => {
                    root_fingerprint = Some(decoder.decode_uint32()?);
                }
                _ => {
                    decoder.skip_field(wire_type)?;
                }
            }
        }

        if xpub.is_empty() {
            return Err(TrezorError::protocol_error(
                "Missing xpub in PublicKey response".to_string(),
            ));
        }

        Ok(Self {
            xpub,
            root_fingerprint,
        })
    }
}

/// Failure response message
#[derive(Debug)]
pub(crate) struct FailureResponse {
    /// Failure code
    pub code: Option<u32>,
    /// Error message
    pub message: String,
}

impl FailureResponse {
    /// Decode from protobuf bytes
    pub(crate) fn decode(data: &[u8]) -> Result<Self, TrezorError> {
        let mut decoder = ProtoDecoder::new(data);
        let mut code = None;
        let mut message = String::new();

        while decoder.remaining() > 0 {
            let Some((field_number, wire_type)) = decoder.decode_tag()? else {
                break;
            };

            match field_number {
                // Field 1: code (enum as uint32)
                1 => {
                    code = Some(decoder.decode_uint32()?);
                }
                // Field 2: message (string)
                2 => {
                    message = decoder.decode_string()?;
                }
                _ => {
                    decoder.skip_field(wire_type)?;
                }
            }
        }

        Ok(Self { code, message })
    }

    /// Convert to TrezorError
    pub(crate) fn to_error(&self) -> TrezorError {
        // Failure codes from trezor-common/protob/messages.proto
        match self.code {
            Some(4) => TrezorError::user_cancelled(), // Failure_ActionCancelled
            Some(6) => TrezorError::pin_required(),   // Failure_PinExpected
            Some(9) => TrezorError::user_cancelled(), // Failure_UserRejected (newer code)
            _ => {
                let detail = if self.message.is_empty() {
                    None
                } else {
                    Some(TrezorErrorDetail::new(self.message.clone()))
                };
                TrezorError::device_error(detail)
            }
        }
    }
}

/// ButtonRequest message (device wants user confirmation)
#[derive(Debug)]
pub(crate) struct ButtonRequestMessage;

impl ButtonRequestMessage {
    /// Decode from protobuf bytes
    pub(crate) fn decode(data: &[u8]) -> Result<Self, TrezorError> {
        let mut decoder = ProtoDecoder::new(data);

        while decoder.remaining() > 0 {
            let Some((field_number, wire_type)) = decoder.decode_tag()? else {
                break;
            };

            match field_number {
                1 => {
                    let _ = decoder.decode_uint32()?;
                }
                _ => {
                    decoder.skip_field(wire_type)?;
                }
            }
        }

        Ok(Self)
    }
}

/// ButtonAck message (acknowledge button request)
pub(crate) struct ButtonAckMessage;

impl ButtonAckMessage {
    /// Encode to protobuf bytes (empty message)
    pub(crate) fn encode() -> Vec<u8> {
        Vec::new()
    }
}

/// Encode a message with type header for Bridge API
pub(crate) fn encode_message(msg_type: MessageType, payload: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(6 + payload.len());
    // 2 bytes: message type (big-endian)
    result.extend_from_slice(&(msg_type as u16).to_be_bytes());
    // 4 bytes: payload length (big-endian)
    result.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    // N bytes: payload
    result.extend_from_slice(payload);
    result
}

/// Decode message type and payload from Bridge response
pub(crate) fn decode_message(data: &[u8]) -> Result<(MessageType, Vec<u8>), TrezorError> {
    if data.len() < 6 {
        return Err(TrezorError::protocol_error(format!(
            "Message too short: {} bytes",
            data.len()
        )));
    }

    let msg_type_raw = u16::from_be_bytes([data[0], data[1]]);
    let msg_type = MessageType::from_u16(msg_type_raw).ok_or_else(|| {
        TrezorError::protocol_error(format!("Unknown message type: {msg_type_raw}"))
    })?;

    let payload_len = u32::from_be_bytes([data[2], data[3], data[4], data[5]]) as usize;

    if data.len() < 6 + payload_len {
        return Err(TrezorError::protocol_error(format!(
            "Message truncated: expected {} payload bytes, got {}",
            payload_len,
            data.len() - 6
        )));
    }

    let payload = data[6..6 + payload_len].to_vec();
    Ok((msg_type, payload))
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn test_encode_get_public_key_for_fingerprint() {
        let request = GetPublicKeyRequest::for_fingerprint();
        let encoded = request.encode();
        // Should encode: field 1 (address_n) with value 0x80000000
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_encode_get_public_key_for_native_segwit() {
        let request = GetPublicKeyRequest::for_address_scheme(0, AddressScheme::NativeSegwit);
        let encoded = request.encode();
        // Should encode: field 1 (address_n) with [84', 0', 0'], coin_name, show_display, script_type
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_decode_get_public_key_for_fingerprint() {
        let request = GetPublicKeyRequest::for_fingerprint();
        let encoded = request.encode();
        let decoded = GetPublicKeyRequest::decode(&encoded).expect("decode should succeed");

        assert_eq!(decoded.address_n, vec![0x80000000]);
        assert_eq!(decoded.coin_name, None);
        assert!(!decoded.show_display);
        assert!(decoded.script_type.is_none());
    }

    #[test]
    fn test_decode_get_public_key_for_native_segwit() {
        let request = GetPublicKeyRequest::for_address_scheme(2, AddressScheme::NativeSegwit);
        let encoded = request.encode();
        let decoded = GetPublicKeyRequest::decode(&encoded).expect("decode should succeed");

        assert_eq!(
            decoded.address_n,
            vec![84 | 0x80000000, 0x80000000, 2 | 0x80000000]
        );
        assert_eq!(decoded.coin_name.as_deref(), Some("Bitcoin"));
        assert!(decoded.show_display);
        assert!(matches!(
            decoded.script_type,
            Some(InputScriptType::SpendWitness)
        ));
    }

    #[test]
    fn test_decode_get_public_key_for_nested_segwit() {
        let request = GetPublicKeyRequest::for_address_scheme(3, AddressScheme::NestedSegwit);
        let encoded = request.encode();
        let decoded = GetPublicKeyRequest::decode(&encoded).expect("decode should succeed");

        assert_eq!(
            decoded.address_n,
            vec![49 | 0x80000000, 0x80000000, 3 | 0x80000000]
        );
        assert_eq!(decoded.coin_name.as_deref(), Some("Bitcoin"));
        assert!(decoded.show_display);
        assert!(matches!(
            decoded.script_type,
            Some(InputScriptType::SpendP2SHWitness)
        ));
    }

    #[test]
    fn test_message_header_encode_decode() {
        let payload = vec![1, 2, 3, 4];
        let encoded = encode_message(MessageType::GetPublicKey, &payload);

        assert_eq!(encoded.len(), 6 + payload.len());
        assert_eq!(&encoded[0..2], &[0, 11]); // GetPublicKey = 11
        assert_eq!(&encoded[2..6], &[0, 0, 0, 4]); // length = 4

        let (msg_type, decoded_payload) = decode_message(&encoded).expect("decode should succeed");
        assert_eq!(msg_type, MessageType::GetPublicKey);
        assert_eq!(decoded_payload, payload);
    }
}
