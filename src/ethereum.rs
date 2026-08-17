//! Ethereum-specific types: address validation, amount encoding, transfer classification.

use serde::{Deserialize, Serialize};
#[cfg(feature = "server")]
use std::fmt;

// ============ Ethereum Address Types ============

/// Raw user-provided Ethereum address (unvalidated).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawEthAddress(String);

impl RawEthAddress {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    #[cfg(feature = "server")]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated Ethereum address (20 bytes).
///
/// Constructed via [`EthAddress::parse`], which enforces:
/// - Exactly 42 characters (0x + 40 hex digits)
/// - Valid hex characters
/// - EIP-55 checksum when mixed-case input is provided
///
/// Display produces EIP-55 checksummed form.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EthAddress {
    bytes: [u8; 20],
}

#[cfg(feature = "server")]
impl EthAddress {
    /// Parse and validate a raw Ethereum address.
    pub(crate) fn parse(raw: &RawEthAddress) -> Result<Self, EthAddressError> {
        let input = raw.as_str().trim();

        if input.len() != 42 {
            return Err(EthAddressError::WrongLength {
                actual: input.len(),
            });
        }

        if !input.starts_with("0x") && !input.starts_with("0X") {
            return Err(EthAddressError::MissingPrefix);
        }

        let hex_part = &input[2..];

        // Check all characters are valid hex
        if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(EthAddressError::InvalidHex);
        }

        // Decode hex to bytes
        let bytes: [u8; 20] = hex::decode(hex_part)
            .map_err(|_| EthAddressError::InvalidHex)?
            .try_into()
            .map_err(|_| EthAddressError::InvalidHex)?;

        // If mixed-case, validate EIP-55 checksum
        if is_mixed_case(hex_part) {
            let expected = eip55_checksum(&bytes);
            if hex_part != expected {
                return Err(EthAddressError::InvalidChecksum);
            }
        }

        Ok(Self { bytes })
    }

    /// EIP-55 checksummed hex string with 0x prefix.
    pub(crate) fn checksummed(&self) -> String {
        format!("0x{}", eip55_checksum(&self.bytes))
    }

    /// Lowercase hex string with 0x prefix (for DB `address_normalized` column).
    pub(crate) fn normalized(&self) -> String {
        format!("0x{}", hex::encode(self.bytes))
    }
}

#[cfg(feature = "server")]
impl fmt::Display for EthAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.checksummed())
    }
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EthAddressError {
    WrongLength { actual: usize },
    MissingPrefix,
    InvalidHex,
    InvalidChecksum,
}

#[cfg(feature = "server")]
impl fmt::Display for EthAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EthAddressError::WrongLength { actual } => {
                write!(
                    f,
                    "Ethereum address must be 42 characters (0x + 40 hex), got {actual}"
                )
            }
            EthAddressError::MissingPrefix => {
                write!(f, "Ethereum address must start with 0x")
            }
            EthAddressError::InvalidHex => {
                write!(f, "Ethereum address contains invalid hex characters")
            }
            EthAddressError::InvalidChecksum => {
                write!(f, "Ethereum address has an invalid EIP-55 checksum")
            }
        }
    }
}

#[cfg(feature = "server")]
impl std::error::Error for EthAddressError {}

/// Check whether a hex string (without 0x prefix) has mixed case.
/// All-lowercase and all-uppercase are NOT mixed-case.
#[cfg(feature = "server")]
fn is_mixed_case(hex: &str) -> bool {
    let has_upper = hex.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = hex.chars().any(|c| c.is_ascii_lowercase());
    has_upper && has_lower
}

/// Compute EIP-55 checksummed hex representation (without 0x prefix).
///
/// EIP-55 uses the Keccak-256 hash of the lowercase hex address to determine
/// which characters should be uppercase.
#[cfg(feature = "server")]
fn eip55_checksum(bytes: &[u8; 20]) -> String {
    use sha3::{Digest, Keccak256};

    let lowercase_hex = hex::encode(bytes);
    let hash = Keccak256::digest(lowercase_hex.as_bytes());

    lowercase_hex
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if c.is_ascii_alphabetic() {
                // Each hex char of the hash corresponds to 4 bits.
                // We check the high nibble of each byte for even indices,
                // the low nibble for odd indices.
                let hash_nibble = if i % 2 == 0 {
                    hash[i / 2] >> 4
                } else {
                    hash[i / 2] & 0x0f
                };
                if hash_nibble >= 8 {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            } else {
                c
            }
        })
        .collect()
}

// ============ Transfer Kind ============

/// Classification of an account-based transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) enum TransferKind {
    /// Top-level ETH transfer.
    Normal,
    /// Contract internal call (trace).
    Internal,
    /// SELFDESTRUCT opcode sending remaining ETH.
    SelfDestruct,
}

#[cfg(feature = "server")]
impl TransferKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            TransferKind::Normal => "normal",
            TransferKind::Internal => "internal",
            TransferKind::SelfDestruct => "self_destruct",
        }
    }

    #[cfg(all(test, not(bitgarth_db_unit_only)))]
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "normal" => Some(TransferKind::Normal),
            "internal" => Some(TransferKind::Internal),
            "self_destruct" => Some(TransferKind::SelfDestruct),
            _ => None,
        }
    }
}

#[cfg(feature = "server")]
impl fmt::Display for TransferKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============ Transfer Index ============

/// Non-negative index for ordering transfers within a single transaction.
/// 0 = top-level transfer, 1+ = internal transfers ordered by trace position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
pub(crate) struct TransferIndex(i64);

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
pub(crate) enum TransferIndexError {
    Negative(i64),
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
impl fmt::Display for TransferIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransferIndexError::Negative(value) => {
                write!(f, "transfer index must be non-negative, got {value}")
            }
        }
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
impl std::error::Error for TransferIndexError {}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
impl TransferIndex {
    pub(crate) fn try_new(value: i64) -> Result<Self, TransferIndexError> {
        if value < 0 {
            return Err(TransferIndexError::Negative(value));
        }
        Ok(Self(value))
    }

    pub(crate) fn value(self) -> i64 {
        self.0
    }

    pub(crate) const fn top_level() -> Self {
        Self(0)
    }
}

// ============ Tests ============

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    // ---- EthAddress tests ----

    #[test]
    fn parse_valid_lowercase_address() {
        let raw = RawEthAddress::new("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".to_string());
        let addr = EthAddress::parse(&raw);
        assert!(addr.is_ok());
        let addr = addr
            .map_err(|e| format!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            addr.normalized(),
            "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed"
        );
    }

    #[test]
    fn parse_valid_uppercase_address() {
        let raw = RawEthAddress::new("0x5AAEB6053F3E94C9B9A09F33669435E7EF1BEAED".to_string());
        let addr = EthAddress::parse(&raw);
        assert!(addr.is_ok());
    }

    #[test]
    fn parse_valid_eip55_checksummed_address() {
        // EIP-55 test vector
        let raw = RawEthAddress::new("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string());
        let addr = EthAddress::parse(&raw);
        assert!(addr.is_ok());
        let addr = addr
            .map_err(|e| format!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            addr.checksummed(),
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
        );
    }

    #[test]
    fn parse_invalid_eip55_checksum_rejected() {
        // Valid hex but wrong checksum (swapped one case character)
        let raw = RawEthAddress::new("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1Beaed".to_string());
        let result = EthAddress::parse(&raw);
        assert_eq!(result, Err(EthAddressError::InvalidChecksum));
    }

    #[test]
    fn parse_wrong_length_rejected() {
        let raw = RawEthAddress::new("0x5aAeb6053F3E94C9b9A09f33".to_string());
        let result = EthAddress::parse(&raw);
        assert!(matches!(result, Err(EthAddressError::WrongLength { .. })));
    }

    #[test]
    fn parse_missing_prefix_rejected() {
        let raw = RawEthAddress::new("5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed00".to_string());
        let result = EthAddress::parse(&raw);
        assert_eq!(result, Err(EthAddressError::MissingPrefix));
    }

    #[test]
    fn parse_invalid_hex_rejected() {
        let raw = RawEthAddress::new("0xZZZZb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string());
        let result = EthAddress::parse(&raw);
        assert_eq!(result, Err(EthAddressError::InvalidHex));
    }

    #[test]
    fn parse_display_roundtrip() {
        let raw = RawEthAddress::new("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".to_string());
        let addr = EthAddress::parse(&raw)
            .map_err(|e| format!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        let checksummed = addr.checksummed();
        // Parsing the checksummed output should succeed
        let raw2 = RawEthAddress::new(checksummed.clone());
        let addr2 = EthAddress::parse(&raw2)
            .map_err(|e| format!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(addr, addr2);
        assert_eq!(addr2.checksummed(), checksummed);
    }

    #[test]
    fn eip55_known_test_vectors() {
        // From EIP-55 specification
        let test_cases = [
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
            "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
            "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
        ];

        for expected in test_cases {
            let raw = RawEthAddress::new(expected.to_string());
            let addr = EthAddress::parse(&raw)
                .map_err(|e| format!("{e}"))
                .unwrap_or_else(|e| panic!("failed to parse {expected}: {e}"));
            assert_eq!(
                addr.checksummed(),
                expected,
                "checksum mismatch for {expected}"
            );
        }
    }

    #[test]
    fn normalized_is_always_lowercase() {
        let raw = RawEthAddress::new("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string());
        let addr = EthAddress::parse(&raw)
            .map_err(|e| format!("{e}"))
            .unwrap_or_else(|e| panic!("{e}"));
        let normalized = addr.normalized();
        assert!(normalized.starts_with("0x"));
        assert_eq!(normalized, normalized.to_ascii_lowercase());
    }

    // ---- TransferKind tests ----

    #[test]
    fn transfer_kind_roundtrip() {
        for kind in [
            TransferKind::Normal,
            TransferKind::Internal,
            TransferKind::SelfDestruct,
        ] {
            let s = kind.as_str();
            let parsed = TransferKind::from_str(s);
            assert_eq!(parsed, Some(kind), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn transfer_kind_from_str_rejects_unknown() {
        assert_eq!(TransferKind::from_str("unknown"), None);
    }

    // ---- TransferIndex tests ----

    #[test]
    fn transfer_index_rejects_negative() {
        assert!(TransferIndex::try_new(-1).is_err());
    }

    #[test]
    fn transfer_index_accepts_zero() {
        let idx = TransferIndex::try_new(0);
        assert!(idx.is_ok());
        assert_eq!(
            idx.map_err(|e| format!("{e}"))
                .unwrap_or_else(|e| panic!("{e}"))
                .value(),
            0
        );
    }

    #[test]
    fn transfer_index_top_level() {
        assert_eq!(TransferIndex::top_level().value(), 0);
    }
}
