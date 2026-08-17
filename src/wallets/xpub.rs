use super::primitives::AddressScheme;
use serde::{Deserialize, Serialize};
use std::fmt;

pub(crate) const XPUB_MAINNET_VERSION: [u8; 4] = [0x04, 0x88, 0xB2, 0x1E];
pub(crate) const YPUB_MAINNET_VERSION: [u8; 4] = [0x04, 0x9D, 0x7C, 0xB2];
pub(crate) const ZPUB_MAINNET_VERSION: [u8; 4] = [0x04, 0xB2, 0x47, 0x46];

/// Detect the address scheme from the prefix of an extended public key string.
/// Returns `None` if the prefix is not recognized (e.g., Taproot or invalid input).
pub(crate) fn detect_address_scheme_from_prefix(input: &str) -> Option<AddressScheme> {
    let trimmed = input.trim();
    if trimmed.starts_with("xpub") {
        Some(AddressScheme::Legacy)
    } else if trimmed.starts_with("ypub") {
        Some(AddressScheme::NestedSegwit)
    } else if trimmed.starts_with("zpub") {
        Some(AddressScheme::NativeSegwit)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawMasterFingerprint(String);

impl RawMasterFingerprint {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn validate(self) -> Result<ValidatedMasterFingerprint, MasterFingerprintError> {
        ValidatedMasterFingerprint::parse(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct ValidatedMasterFingerprint(String);

impl ValidatedMasterFingerprint {
    pub(crate) fn parse(input: &str) -> Result<Self, MasterFingerprintError> {
        let normalized = input.trim().to_lowercase();
        if normalized.len() != 8 {
            return Err(MasterFingerprintError::InvalidLength {
                expected: 8,
                actual: normalized.len(),
            });
        }

        if !normalized.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(MasterFingerprintError::InvalidHexCharacter);
        }

        Ok(Self(normalized))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ValidatedMasterFingerprint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        ValidatedMasterFingerprint::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MasterFingerprintError {
    InvalidLength { expected: usize, actual: usize },
    InvalidHexCharacter,
}

impl fmt::Display for MasterFingerprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MasterFingerprintError::InvalidLength { expected, actual } => {
                write!(f, "Invalid length: expected {expected}, got {actual}")
            }
            MasterFingerprintError::InvalidHexCharacter => {
                write!(f, "Invalid hex character in master fingerprint")
            }
        }
    }
}

impl std::error::Error for MasterFingerprintError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawExtendedPubkey(String);

impl RawExtendedPubkey {
    #[cfg(any(target_arch = "wasm32", feature = "desktop", test))]
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct NormalizedExtendedPubkey(String);

impl NormalizedExtendedPubkey {
    pub(crate) fn parse(input: &str) -> Result<Self, ExtendedPubkeyError> {
        let validated = validate_extended_pubkey_format(input)?;
        let normalized = normalize_extended_pubkey_to_xpub(&validated)?;
        Ok(Self(normalized))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedExtendedPubkey {
    address_scheme: AddressScheme,
    value: String,
    normalized: NormalizedExtendedPubkey,
}

impl ValidatedExtendedPubkey {
    pub(crate) fn parse(
        address_scheme: AddressScheme,
        input: &str,
    ) -> Result<Self, ExtendedPubkeyError> {
        let validated_key = validate_extended_pubkey_format(input)?;
        let normalized = NormalizedExtendedPubkey::parse(&validated_key)?;

        // Validate scheme is one of the 3 supported Bitcoin HD schemes
        match address_scheme {
            AddressScheme::Legacy | AddressScheme::NestedSegwit | AddressScheme::NativeSegwit => {}
            other => return Err(ExtendedPubkeyError::UnsupportedAddressScheme(other)),
        }

        Ok(Self {
            address_scheme,
            value: validated_key,
            normalized,
        })
    }

    pub(crate) fn address_scheme(&self) -> AddressScheme {
        self.address_scheme
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }

    pub(crate) fn normalized_as_str(&self) -> &str {
        self.normalized.as_str()
    }
}

/// Validate the format of an extended public key without coupling to a specific
/// address scheme. Returns the trimmed key string if valid.
pub(crate) fn validate_extended_pubkey_format(input: &str) -> Result<String, ExtendedPubkeyError> {
    let trimmed = input.trim().to_string();

    if !(trimmed.starts_with("xpub") || trimmed.starts_with("ypub") || trimmed.starts_with("zpub"))
    {
        return Err(ExtendedPubkeyError::UnrecognizedPrefix {
            actual: trimmed.chars().take(4).collect(),
        });
    }

    if trimmed.len() != 111 {
        return Err(ExtendedPubkeyError::InvalidLength {
            expected: 111,
            actual: trimmed.len(),
        });
    }

    if !trimmed
        .chars()
        .all(|c| "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz".contains(c))
    {
        return Err(ExtendedPubkeyError::InvalidBase58Character);
    }

    bs58::decode(&trimmed)
        .with_check(None)
        .into_vec()
        .map_err(|_| ExtendedPubkeyError::InvalidChecksum)?;

    Ok(trimmed)
}

fn normalize_extended_pubkey_to_xpub(input: &str) -> Result<String, ExtendedPubkeyError> {
    let mut decoded = bs58::decode(input)
        .with_check(None)
        .into_vec()
        .map_err(|_| ExtendedPubkeyError::InvalidChecksum)?;

    if decoded.len() < 4 {
        return Err(ExtendedPubkeyError::InvalidChecksum);
    }

    let version = [decoded[0], decoded[1], decoded[2], decoded[3]];
    if version != XPUB_MAINNET_VERSION
        && version != YPUB_MAINNET_VERSION
        && version != ZPUB_MAINNET_VERSION
    {
        return Err(ExtendedPubkeyError::UnsupportedVersionBytes(version));
    }

    decoded[0..4].copy_from_slice(&XPUB_MAINNET_VERSION);
    Ok(bs58::encode(decoded).with_check().into_string())
}

impl<'de> Deserialize<'de> for ValidatedExtendedPubkey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            address_scheme: AddressScheme,
            value: String,
        }

        let helper = Helper::deserialize(deserializer)?;
        ValidatedExtendedPubkey::parse(helper.address_scheme, &helper.value)
            .map_err(serde::de::Error::custom)
    }
}

impl Serialize for ValidatedExtendedPubkey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ValidatedExtendedPubkey", 2)?;
        state.serialize_field("address_scheme", &self.address_scheme)?;
        state.serialize_field("value", &self.value)?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExtendedPubkeyError {
    UnrecognizedPrefix { actual: String },
    InvalidLength { expected: usize, actual: usize },
    InvalidBase58Character,
    InvalidChecksum,
    UnsupportedVersionBytes([u8; 4]),
    UnsupportedAddressScheme(AddressScheme),
}

impl fmt::Display for ExtendedPubkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtendedPubkeyError::UnrecognizedPrefix { actual } => {
                write!(
                    f,
                    "Unrecognized key prefix '{actual}'. Expected xpub, ypub, or zpub."
                )
            }
            ExtendedPubkeyError::InvalidLength { expected, actual } => {
                write!(f, "Invalid length: expected {expected}, got {actual}")
            }
            ExtendedPubkeyError::InvalidBase58Character => {
                write!(f, "Invalid base58 character")
            }
            ExtendedPubkeyError::InvalidChecksum => {
                write!(f, "Invalid checksum")
            }
            ExtendedPubkeyError::UnsupportedVersionBytes(version) => {
                write!(
                    f,
                    "Unsupported version bytes: {:02x}{:02x}{:02x}{:02x}",
                    version[0], version[1], version[2], version[3]
                )
            }
            ExtendedPubkeyError::UnsupportedAddressScheme(address_scheme) => {
                write!(f, "Unsupported address scheme: {}", address_scheme.as_str())
            }
        }
    }
}

impl std::error::Error for ExtendedPubkeyError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct TrezorDeviceId(String);

impl TrezorDeviceId {
    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    pub(crate) fn new(value: String) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(Self(trimmed.to_string()))
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct TrezorDeviceLabel(String);

impl TrezorDeviceLabel {
    #[cfg(any(target_arch = "wasm32", feature = "desktop"))]
    pub(crate) fn new(value: String) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(Self(trimmed.to_string()))
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    // Deterministic xpub fixtures generated from seed=[account_be_bytes, 0...] path=m/84'/0'/{account}'
    // These replace the bitcoin crate dependency for no-server test builds.
    fn test_account_xpub(account: u32) -> String {
        match account {
            0 => "xpub6C7dm6fpZENX4meEzE4DLTSb4nvYMPiZvJKMnbhGoDTfBMTMsY7eBxmaQq9RpSSKTdFyb5MoE1encwjP99mSHwjJf8JVoo572k9ireBAxyq".to_string(),
            2 => "xpub6CPCqKiAkerFSqn3dJSsfzeBX5ZTGS4dufSDVEPTFnhiHg2HgcaSY5T3uLR3Z2QCxzgaawVB3N2HH2cKoLccAi2rVuTEwNxt7LJfaiApAo6".to_string(),
            _ => panic!("no static fixture for account {account}; add one or use account 0 or 2"),
        }
    }

    fn convert_extended_pubkey_version_for_test(
        input: &str,
        target_version: [u8; 4],
    ) -> Result<String, String> {
        let mut data = bs58::decode(input)
            .with_check(None)
            .into_vec()
            .map_err(|e| format!("failed to decode base58check: {e}"))?;

        if data.len() < 4 {
            return Err("decoded data too short for version bytes".to_string());
        }

        data[0..4].copy_from_slice(&target_version);
        Ok(bs58::encode(data).with_check().into_string())
    }

    fn is_duplicate_extended_pubkey_scheme_for_test(
        existing_normalized: &NormalizedExtendedPubkey,
        existing_scheme: AddressScheme,
        candidate_normalized: &NormalizedExtendedPubkey,
        candidate_scheme: AddressScheme,
    ) -> bool {
        existing_normalized == candidate_normalized && existing_scheme == candidate_scheme
    }

    #[test]
    fn test_master_fingerprint_validation() {
        assert!(ValidatedMasterFingerprint::parse("a1b2c3d4").is_ok());
        assert!(ValidatedMasterFingerprint::parse("A1B2C3D4").is_ok());
        assert!(ValidatedMasterFingerprint::parse("a1b2c3").is_err());
        assert!(ValidatedMasterFingerprint::parse("a1b2c3d4e5").is_err());
        assert!(ValidatedMasterFingerprint::parse("g1b2c3d4").is_err());
    }

    #[test]
    fn test_detect_address_scheme_from_prefix() {
        assert_eq!(
            detect_address_scheme_from_prefix("xpub6ABC..."),
            Some(AddressScheme::Legacy)
        );
        assert_eq!(
            detect_address_scheme_from_prefix("ypub6DEF..."),
            Some(AddressScheme::NestedSegwit)
        );
        assert_eq!(
            detect_address_scheme_from_prefix("zpub6GHI..."),
            Some(AddressScheme::NativeSegwit)
        );
        assert_eq!(detect_address_scheme_from_prefix("tpub6ABC..."), None);
        assert_eq!(detect_address_scheme_from_prefix("invalid"), None);
        assert_eq!(detect_address_scheme_from_prefix(""), None);
        assert_eq!(
            detect_address_scheme_from_prefix("  zpub6GHI...  "),
            Some(AddressScheme::NativeSegwit)
        );
    }

    #[test]
    fn test_validate_extended_pubkey_format_valid_xpub() {
        let xpub = test_account_xpub(0);
        let result = validate_extended_pubkey_format(&xpub);
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with("xpub"));
    }

    #[test]
    fn test_validate_extended_pubkey_format_trims_whitespace() {
        let xpub = format!("  {}  ", test_account_xpub(0));
        let result = validate_extended_pubkey_format(&xpub);
        assert!(result.is_ok());
        let trimmed = result.unwrap();
        assert!(!trimmed.starts_with(' '));
        assert!(!trimmed.ends_with(' '));
    }

    #[test]
    fn test_validate_extended_pubkey_format_rejects_bad_prefix() {
        let result = validate_extended_pubkey_format("tpub6ABC");
        assert!(matches!(
            result,
            Err(ExtendedPubkeyError::UnrecognizedPrefix { .. })
        ));
    }

    #[test]
    fn test_validated_extended_pubkey_accepts_xpub_with_native_segwit() {
        let xpub = test_account_xpub(0);
        let result = ValidatedExtendedPubkey::parse(AddressScheme::NativeSegwit, &xpub);
        assert!(result.is_ok());
        let validated = result.unwrap();
        assert_eq!(validated.address_scheme(), AddressScheme::NativeSegwit);
        // Key stored as-is — no version byte conversion
        assert!(validated.as_str().starts_with("xpub"));
    }

    #[test]
    fn test_validated_extended_pubkey_accepts_xpub_with_nested_segwit() {
        let xpub = test_account_xpub(0);
        let result = ValidatedExtendedPubkey::parse(AddressScheme::NestedSegwit, &xpub);
        assert!(result.is_ok());
        let validated = result.unwrap();
        assert_eq!(validated.address_scheme(), AddressScheme::NestedSegwit);
        assert!(validated.as_str().starts_with("xpub"));
    }

    #[test]
    fn test_validated_extended_pubkey_accepts_xpub_with_legacy() {
        let xpub = test_account_xpub(0);
        let result = ValidatedExtendedPubkey::parse(AddressScheme::Legacy, &xpub);
        assert!(result.is_ok());
        let validated = result.unwrap();
        assert_eq!(validated.address_scheme(), AddressScheme::Legacy);
        assert!(validated.as_str().starts_with("xpub"));
    }

    #[test]
    fn test_validated_extended_pubkey_rejects_taproot_scheme() {
        let xpub = test_account_xpub(0);
        let result = ValidatedExtendedPubkey::parse(AddressScheme::Taproot, &xpub);
        assert!(matches!(
            result,
            Err(ExtendedPubkeyError::UnsupportedAddressScheme(
                AddressScheme::Taproot
            ))
        ));
    }

    #[test]
    fn test_validated_extended_pubkey_rejects_standard_scheme() {
        let xpub = test_account_xpub(0);
        let result = ValidatedExtendedPubkey::parse(AddressScheme::Standard, &xpub);
        assert!(matches!(
            result,
            Err(ExtendedPubkeyError::UnsupportedAddressScheme(
                AddressScheme::Standard
            ))
        ));
    }

    #[test]
    fn test_normalized_extended_pubkey_equivalence_across_prefix_variants() {
        let xpub = test_account_xpub(0);
        let ypub = convert_extended_pubkey_version_for_test(&xpub, YPUB_MAINNET_VERSION)
            .expect("xpub should convert to ypub for test fixture");
        let zpub = convert_extended_pubkey_version_for_test(&xpub, ZPUB_MAINNET_VERSION)
            .expect("xpub should convert to zpub for test fixture");

        let normalized_xpub = NormalizedExtendedPubkey::parse(&xpub)
            .expect("xpub should normalize")
            .as_str()
            .to_string();
        let normalized_ypub = NormalizedExtendedPubkey::parse(&ypub)
            .expect("ypub should normalize")
            .as_str()
            .to_string();
        let normalized_zpub = NormalizedExtendedPubkey::parse(&zpub)
            .expect("zpub should normalize")
            .as_str()
            .to_string();

        assert_eq!(normalized_xpub, normalized_ypub);
        assert_eq!(normalized_xpub, normalized_zpub);
        assert!(normalized_xpub.starts_with("xpub"));
    }

    #[test]
    fn test_duplicate_extended_pubkey_scheme_predicate() {
        let normalized = NormalizedExtendedPubkey::parse(&test_account_xpub(2))
            .expect("test xpub should normalize");

        assert!(is_duplicate_extended_pubkey_scheme_for_test(
            &normalized,
            AddressScheme::Legacy,
            &normalized,
            AddressScheme::Legacy,
        ));

        assert!(!is_duplicate_extended_pubkey_scheme_for_test(
            &normalized,
            AddressScheme::Legacy,
            &normalized,
            AddressScheme::NativeSegwit,
        ));
    }
}
