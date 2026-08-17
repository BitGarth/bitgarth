use serde::{Deserialize, Serialize};

#[cfg(feature = "server")]
use super::primitives::{AddressScheme, Network};
#[cfg(feature = "server")]
use std::fmt;

/// Raw user-provided Bitcoin address (unvalidated).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawBtcAddress(String);

impl RawBtcAddress {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    #[cfg(feature = "server")]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated Bitcoin address with network and address scheme.
///
/// Constructed via [`BtcAddress::parse`], which enforces:
/// - Valid Bitcoin address encoding (base58check or bech32/bech32m)
/// - Correct network (mainnet, testnet, etc.)
/// - Recognized address type (P2PKH, P2SH, P2WPKH, P2WSH, P2TR)
///
/// The address scheme is auto-detected from the parsed address type.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BtcAddress {
    canonical: String,
    address_scheme: AddressScheme,
}

#[cfg(feature = "server")]
impl BtcAddress {
    /// Parse and validate a raw Bitcoin address for the given network.
    pub(crate) fn parse(raw: &RawBtcAddress, network: Network) -> Result<Self, BtcAddressError> {
        let input = raw.as_str().trim();
        if input.is_empty() {
            return Err(BtcAddressError::Empty);
        }

        let unchecked = input
            .parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>()
            .map_err(|e| BtcAddressError::ParseFailed(format!("{e}")))?;

        let bitcoin_network = match network {
            Network::Mainnet => bitcoin::Network::Bitcoin,
            Network::Testnet => bitcoin::Network::Testnet,
            Network::Signet => bitcoin::Network::Signet,
            Network::Regtest => bitcoin::Network::Regtest,
        };

        let checked = unchecked
            .require_network(bitcoin_network)
            .map_err(|e| BtcAddressError::WrongNetwork(format!("{e}")))?;

        let address_type = checked
            .address_type()
            .ok_or(BtcAddressError::UnrecognizedType)?;

        let address_scheme = match address_type {
            bitcoin::AddressType::P2pkh => AddressScheme::Legacy,
            bitcoin::AddressType::P2sh => AddressScheme::NestedSegwit,
            bitcoin::AddressType::P2wpkh | bitcoin::AddressType::P2wsh => {
                AddressScheme::NativeSegwit
            }
            bitcoin::AddressType::P2tr => AddressScheme::Taproot,
            _ => return Err(BtcAddressError::UnrecognizedType),
        };

        Ok(Self {
            canonical: checked.to_string(),
            address_scheme,
        })
    }

    /// Canonical string representation (the standard display form).
    pub(crate) fn canonical(&self) -> &str {
        &self.canonical
    }

    /// Detected address scheme based on the address type.
    pub(crate) fn address_scheme(&self) -> AddressScheme {
        self.address_scheme
    }

    /// Normalized form for duplicate detection in the database.
    ///
    /// For base58check addresses (Legacy, NestedSegwit): the canonical encoding
    /// is case-sensitive and deterministic, so it serves as the normalized form.
    /// For bech32/bech32m addresses (NativeSegwit, Taproot): already lowercase.
    pub(crate) fn normalized(&self) -> &str {
        &self.canonical
    }
}

#[cfg(feature = "server")]
impl fmt::Display for BtcAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.canonical)
    }
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BtcAddressError {
    Empty,
    ParseFailed(String),
    WrongNetwork(String),
    UnrecognizedType,
}

#[cfg(feature = "server")]
impl fmt::Display for BtcAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BtcAddressError::Empty => write!(f, "Bitcoin address cannot be empty"),
            BtcAddressError::ParseFailed(msg) => {
                write!(f, "Invalid Bitcoin address: {msg}")
            }
            BtcAddressError::WrongNetwork(msg) => {
                write!(f, "Bitcoin address is for a different network: {msg}")
            }
            BtcAddressError::UnrecognizedType => {
                write!(f, "Unrecognized Bitcoin address type")
            }
        }
    }
}

#[cfg(feature = "server")]
impl std::error::Error for BtcAddressError {}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    // ============ BtcAddress Tests ============

    #[test]
    fn test_btc_address_parse_valid_legacy() {
        let raw = RawBtcAddress::new("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string());
        let addr = BtcAddress::parse(&raw, Network::Mainnet).expect("should parse legacy address");
        assert_eq!(addr.address_scheme(), AddressScheme::Legacy);
        assert!(addr.canonical().starts_with('1'));
    }

    #[test]
    fn test_btc_address_parse_valid_native_segwit() {
        let raw = RawBtcAddress::new("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string());
        let addr =
            BtcAddress::parse(&raw, Network::Mainnet).expect("should parse native segwit address");
        assert_eq!(addr.address_scheme(), AddressScheme::NativeSegwit);
        assert!(addr.canonical().starts_with("bc1q"));
    }

    #[test]
    fn test_btc_address_parse_valid_taproot() {
        let raw = RawBtcAddress::new(
            "bc1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqzk5jj0".to_string(),
        );
        let addr = BtcAddress::parse(&raw, Network::Mainnet).expect("should parse taproot address");
        assert_eq!(addr.address_scheme(), AddressScheme::Taproot);
        assert!(addr.canonical().starts_with("bc1p"));
    }

    #[test]
    fn test_btc_address_parse_trims_whitespace() {
        let raw = RawBtcAddress::new("  1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa  ".to_string());
        let addr = BtcAddress::parse(&raw, Network::Mainnet).expect("should parse after trimming");
        assert_eq!(addr.address_scheme(), AddressScheme::Legacy);
    }

    #[test]
    fn test_btc_address_parse_uppercase_bech32_normalizes() {
        let raw = RawBtcAddress::new("BC1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4".to_string());
        let addr =
            BtcAddress::parse(&raw, Network::Mainnet).expect("should parse uppercase bech32");
        assert_eq!(addr.address_scheme(), AddressScheme::NativeSegwit);
        assert!(
            addr.canonical().starts_with("bc1q"),
            "canonical form should be lowercase"
        );
    }

    #[test]
    fn test_btc_address_parse_empty_rejected() {
        let raw = RawBtcAddress::new("".to_string());
        let result = BtcAddress::parse(&raw, Network::Mainnet);
        assert!(matches!(result, Err(BtcAddressError::Empty)));
    }

    #[test]
    fn test_btc_address_parse_whitespace_only_rejected() {
        let raw = RawBtcAddress::new("   ".to_string());
        let result = BtcAddress::parse(&raw, Network::Mainnet);
        assert!(matches!(result, Err(BtcAddressError::Empty)));
    }

    #[test]
    fn test_btc_address_parse_wrong_network_rejected() {
        let raw = RawBtcAddress::new("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string());
        let result = BtcAddress::parse(&raw, Network::Testnet);
        assert!(matches!(result, Err(BtcAddressError::WrongNetwork(_))));
    }

    #[test]
    fn test_btc_address_parse_invalid_rejected() {
        let raw = RawBtcAddress::new("notabitcoinaddress".to_string());
        let result = BtcAddress::parse(&raw, Network::Mainnet);
        assert!(matches!(result, Err(BtcAddressError::ParseFailed(_))));
    }

    #[test]
    fn test_btc_address_normalized_is_canonical() {
        let raw = RawBtcAddress::new("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string());
        let addr = BtcAddress::parse(&raw, Network::Mainnet).expect("should parse");
        assert_eq!(addr.normalized(), addr.canonical());
    }
}
