use crate::wallets::{AddressScheme, RawAccountIndex, RawExtendedPubkey, RawMasterFingerprint};
use serde::{Deserialize, Serialize};

/// Error returned by Trezor operations.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct TrezorError {
    pub kind: TrezorErrorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<TrezorErrorDetail>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TrezorErrorKind {
    BridgeNotRunning,
    NoDevices,
    DeviceDisconnected,
    UserCancelled,
    PinRequired,
    PassphraseRequired,
    ProtocolError,
    InternalError,
    SessionExpired,
    SessionConflict,
    BridgeRejected,
    BridgeError,
    DeviceError,
    MissingFingerprint,
    MissingMasterFingerprint,
    InvalidFingerprint,
    NoAccountsSelected,
    MissingZpubData,
    WrongDeviceConnected,
    ConnectInitParseFailed,
    ConnectInitFailed,
    ConnectFingerprintParseFailed,
    ConnectFingerprintFailed,
    ConnectAccountIndexesSerializeFailed,
    ConnectZpubParseFailed,
    ConnectZpubFailed,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(transparent)]
pub(crate) struct TrezorErrorDetail(String);

impl TrezorErrorDetail {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for TrezorErrorDetail {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for TrezorErrorDetail {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl TrezorError {
    pub(crate) fn new(kind: TrezorErrorKind) -> Self {
        Self { kind, detail: None }
    }

    pub(crate) fn with_detail(kind: TrezorErrorKind, detail: TrezorErrorDetail) -> Self {
        Self {
            kind,
            detail: Some(detail),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn bridge_not_running() -> Self {
        Self::new(TrezorErrorKind::BridgeNotRunning)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn no_devices() -> Self {
        Self::new(TrezorErrorKind::NoDevices)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn device_disconnected() -> Self {
        Self::new(TrezorErrorKind::DeviceDisconnected)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn user_cancelled() -> Self {
        Self::new(TrezorErrorKind::UserCancelled)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn pin_required() -> Self {
        Self::new(TrezorErrorKind::PinRequired)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn passphrase_required() -> Self {
        Self::new(TrezorErrorKind::PassphraseRequired)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn protocol_error(details: impl Into<TrezorErrorDetail>) -> Self {
        Self::with_detail(TrezorErrorKind::ProtocolError, details.into())
    }

    pub(crate) fn internal(details: impl Into<TrezorErrorDetail>) -> Self {
        Self::with_detail(TrezorErrorKind::InternalError, details.into())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn session_expired() -> Self {
        Self::new(TrezorErrorKind::SessionExpired)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn session_conflict() -> Self {
        Self::new(TrezorErrorKind::SessionConflict)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn bridge_rejected() -> Self {
        Self::new(TrezorErrorKind::BridgeRejected)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn bridge_error(details: impl Into<TrezorErrorDetail>) -> Self {
        Self::with_detail(TrezorErrorKind::BridgeError, details.into())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn device_error(details: Option<TrezorErrorDetail>) -> Self {
        match details {
            Some(detail) => Self::with_detail(TrezorErrorKind::DeviceError, detail),
            None => Self::new(TrezorErrorKind::DeviceError),
        }
    }

    pub(crate) fn missing_fingerprint() -> Self {
        Self::new(TrezorErrorKind::MissingFingerprint)
    }

    pub(crate) fn missing_master_fingerprint() -> Self {
        Self::new(TrezorErrorKind::MissingMasterFingerprint)
    }

    pub(crate) fn invalid_fingerprint(details: impl Into<TrezorErrorDetail>) -> Self {
        Self::with_detail(TrezorErrorKind::InvalidFingerprint, details.into())
    }

    pub(crate) fn no_accounts_selected() -> Self {
        Self::new(TrezorErrorKind::NoAccountsSelected)
    }

    pub(crate) fn missing_zpub_data() -> Self {
        Self::new(TrezorErrorKind::MissingZpubData)
    }
}

impl std::fmt::Display for TrezorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "{:?}: {}", self.kind, detail.as_str()),
            None => write!(f, "{:?}", self.kind),
        }
    }
}

impl std::error::Error for TrezorError {}

#[cfg(target_arch = "wasm32")]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(transparent)]
pub(crate) struct RawExternalErrorMessage(String);

#[cfg(target_arch = "wasm32")]
impl RawExternalErrorMessage {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(transparent)]
pub(crate) struct RawExternalErrorTroubleshooting(String);

#[cfg(target_arch = "wasm32")]
impl RawExternalErrorTroubleshooting {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TrezorErrorPayload {
    #[cfg(target_arch = "wasm32")]
    pub message: RawExternalErrorMessage,
    #[cfg(target_arch = "wasm32")]
    pub troubleshooting: RawExternalErrorTroubleshooting,
}

#[cfg(target_arch = "wasm32")]
impl TrezorErrorPayload {
    pub(crate) fn to_detail(&self) -> TrezorErrorDetail {
        TrezorErrorDetail::new(format!(
            "message={}; troubleshooting={}",
            self.message.as_str(),
            self.troubleshooting.as_str()
        ))
    }
}

/// Result from Trezor Connect initialization.
#[cfg(target_arch = "wasm32")]
#[derive(Debug, Deserialize)]
pub(crate) struct TrezorInitResult {
    pub success: bool,
    pub error: Option<TrezorInitError>,
}

/// Error variants for initialization.
#[cfg(target_arch = "wasm32")]
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum TrezorInitError {
    Message(RawExternalErrorMessage),
    Detailed(TrezorErrorPayload),
}

/// Result from getting master fingerprint.
#[derive(Debug, Deserialize)]
pub(crate) struct MasterFingerprintResult {
    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(
            dead_code,
            reason = "Desktop path does not read web-only success metadata"
        )
    )]
    pub success: bool,
    pub fingerprint: Option<RawMasterFingerprint>,
    #[serde(rename = "deviceId")]
    pub device_id: Option<crate::wallets::TrezorDeviceId>,
    #[serde(rename = "deviceLabel")]
    pub device_label: Option<crate::wallets::TrezorDeviceLabel>,
    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(
            dead_code,
            reason = "Desktop path does not read web-only error metadata"
        )
    )]
    pub error: Option<TrezorErrorPayload>,
}

/// Result from getting an account extended pubkey.
#[derive(Debug, Deserialize, Clone)]
pub(crate) struct AccountPubkeyResult {
    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(
            dead_code,
            reason = "Desktop path does not read web-only success metadata"
        )
    )]
    pub success: bool,
    #[serde(rename = "accountIndex")]
    pub account_index: RawAccountIndex,
    #[serde(rename = "addressScheme")]
    pub address_scheme: AddressScheme,
    #[serde(rename = "extendedPubkey", alias = "zpub")]
    pub extended_pubkey: Option<RawExtendedPubkey>,
    #[cfg_attr(
        not(all(test, not(bitgarth_db_unit_only))),
        expect(
            dead_code,
            reason = "Desktop path does not read returned derivation path metadata"
        )
    )]
    pub path: Option<String>,
    #[cfg_attr(
        not(all(test, not(bitgarth_db_unit_only))),
        expect(
            dead_code,
            reason = "Desktop path does not read returned fingerprint metadata"
        )
    )]
    pub fingerprint: Option<RawMasterFingerprint>,
    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(
            dead_code,
            reason = "Desktop path does not read web-only error metadata"
        )
    )]
    pub error: Option<TrezorErrorPayload>,
}

/// Debug log entry.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub data: Option<String>,
}

/// Information about a connected Trezor device.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TrezorDevice {
    /// Device path used by Bridge for identification.
    pub path: TrezorDevicePath,
    /// Device ID from Bridge enumerate payload.
    pub device_id: Option<crate::wallets::TrezorDeviceId>,
    /// Previous session ID if device was previously acquired.
    pub session: Option<String>,
    /// Device product name (e.g., "Trezor Safe 3") from Bridge IDs.
    pub product: Option<String>,
    /// Device vendor name.
    pub vendor: Option<String>,
    /// Device product name from Bridge (productName).
    pub product_name: Option<String>,
    /// Device vendor name from Bridge (manufacturerName).
    pub manufacturer_name: Option<String>,
    /// Device serial number from Bridge (serialNumber).
    pub serial_number: Option<String>,
}

/// Unique path identifier for a Trezor device from Bridge.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TrezorDevicePath(String);

#[cfg(not(target_arch = "wasm32"))]
impl TrezorDevicePath {
    pub(crate) fn new(path: String) -> Self {
        Self(path)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TrezorDevicePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Session handle for an acquired Trezor device.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub(crate) struct TrezorSession {
    pub session_id: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl TrezorSession {
    pub(crate) fn new(session_id: String, _device_path: TrezorDevicePath) -> Self {
        Self { session_id }
    }
}

/// Status of Trezor Bridge availability.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BridgeStatus {
    /// Bridge is running and available.
    Running,
    /// Bridge is not running or not installed.
    NotRunning,
}
