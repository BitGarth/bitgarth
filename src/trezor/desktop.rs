//! Desktop (native) implementation using Trezor Bridge HTTP API.

use super::bridge::BridgeClient;
use super::proto::{GetPublicKeyRequest, PublicKeyResponse};
use super::types::{
    AccountPubkeyResult, BridgeStatus, LogEntry, MasterFingerprintResult, TrezorDevice,
    TrezorDevicePath, TrezorError, TrezorErrorKind, TrezorSession,
};
use crate::models::UserId;
use crate::wallets::{AddressScheme, RawAccountIndex, RawExtendedPubkey, RawMasterFingerprint};
use chrono::Utc;
use std::{future::Future, pin::Pin, sync::Mutex};

/// Thread-safe log storage for debug panel.
static LOGS: Mutex<Vec<LogEntry>> = Mutex::new(Vec::new());

/// Currently selected device for operations.
static SELECTED_DEVICE: Mutex<Option<TrezorDevicePath>> = Mutex::new(None);

type BridgeFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

const XPUB_MAINNET_VERSION: [u8; 4] = [0x04, 0x88, 0xB2, 0x1E];
const YPUB_MAINNET_VERSION: [u8; 4] = [0x04, 0x9D, 0x7C, 0xB2];
const ZPUB_MAINNET_VERSION: [u8; 4] = [0x04, 0xB2, 0x47, 0x46];

trait BridgeAdapter {
    fn enumerate_devices(&self) -> BridgeFuture<'_, Result<Vec<TrezorDevice>, TrezorError>>;

    fn acquire_session<'a>(
        &'a self,
        path: &'a TrezorDevicePath,
        previous: Option<&'a str>,
    ) -> BridgeFuture<'a, Result<TrezorSession, TrezorError>>;

    fn initialize_device_session<'a>(
        &'a self,
        session: &'a TrezorSession,
    ) -> BridgeFuture<'a, Result<(), TrezorError>>;

    fn request_public_key<'a>(
        &'a self,
        session: &'a TrezorSession,
        request: &'a GetPublicKeyRequest,
    ) -> BridgeFuture<'a, Result<PublicKeyResponse, TrezorError>>;

    fn release_session<'a>(
        &'a self,
        session: &'a TrezorSession,
    ) -> BridgeFuture<'a, Result<(), TrezorError>>;
}

impl BridgeAdapter for BridgeClient {
    fn enumerate_devices(&self) -> BridgeFuture<'_, Result<Vec<TrezorDevice>, TrezorError>> {
        Box::pin(self.enumerate())
    }

    fn acquire_session<'a>(
        &'a self,
        path: &'a TrezorDevicePath,
        previous: Option<&'a str>,
    ) -> BridgeFuture<'a, Result<TrezorSession, TrezorError>> {
        Box::pin(self.acquire(path, previous))
    }

    fn initialize_device_session<'a>(
        &'a self,
        session: &'a TrezorSession,
    ) -> BridgeFuture<'a, Result<(), TrezorError>> {
        Box::pin(self.initialize_session(session))
    }

    fn request_public_key<'a>(
        &'a self,
        session: &'a TrezorSession,
        request: &'a GetPublicKeyRequest,
    ) -> BridgeFuture<'a, Result<PublicKeyResponse, TrezorError>> {
        Box::pin(self.get_public_key(session, request))
    }

    fn release_session<'a>(
        &'a self,
        session: &'a TrezorSession,
    ) -> BridgeFuture<'a, Result<(), TrezorError>> {
        Box::pin(self.release(session))
    }
}

fn format_device_label(device: &TrezorDevice) -> Option<String> {
    if let Some(device_id) = device.device_id.as_ref() {
        return Some(format!("Trezor {}", device_id.as_str()));
    }

    let name = device.product_name.as_ref().or(device.product.as_ref());
    let serial = device.serial_number.as_ref();

    match (name, serial) {
        (Some(name), Some(serial)) => Some(format!("{name} ({serial})")),
        (Some(name), None) => Some(name.clone()),
        (None, Some(serial)) => Some(format!("Trezor ({serial})")),
        (None, None) => None,
    }
}

/// Log a message to the debug log.
fn log(level: &str, message: &str, data: Option<String>) {
    // Clone data for tracing before moving into entry
    let data_for_tracing = data.clone();

    let entry = LogEntry {
        timestamp: Utc::now().to_rfc3339(),
        level: level.to_string(),
        message: message.to_string(),
        data,
    };

    if let Ok(mut logs) = LOGS.lock() {
        logs.push(entry);
    }

    // Also log to tracing for development
    match level {
        "error" => tracing::error!("[Trezor] {}: {:?}", message, data_for_tracing),
        "warn" => tracing::warn!("[Trezor] {}: {:?}", message, data_for_tracing),
        _ => tracing::info!("[Trezor] {}: {:?}", message, data_for_tracing),
    }
}

/// Set the selected device for subsequent operations.
pub(crate) fn set_selected_device(device: Option<TrezorDevicePath>) {
    if let Ok(mut selected) = SELECTED_DEVICE.lock() {
        *selected = device;
    }
}

/// Get the currently selected device.
pub(crate) fn get_selected_device() -> Option<TrezorDevicePath> {
    SELECTED_DEVICE.lock().ok()?.clone()
}

/// Check if Trezor Bridge is running.
pub(crate) async fn is_bridge_running(user_id: UserId) -> bool {
    match BridgeClient::new(user_id) {
        Ok(client) => {
            let status = client.check_status().await;
            tracing::debug!(
                "BridgeClient created, checked Trezor Bridge status={:?}",
                status
            );
            status == BridgeStatus::Running
        }
        Err(err) => {
            tracing::error!(
                kind = ?err.kind,
                detail = ?err.detail,
                err = %err,
                "BridgeClient could not be created");
            false
        }
    }
}

/// Enumerate connected Trezor devices.
pub(crate) async fn enumerate_devices(user_id: UserId) -> Result<Vec<TrezorDevice>, TrezorError> {
    log("info", "Enumerating Trezor devices", None);

    let client = BridgeClient::new(user_id)?;
    let devices = client.enumerate().await?;

    log(
        "info",
        &format!("Found {} device(s)", devices.len()),
        Some(
            serde_json::to_string(&devices.iter().map(|d| d.path.as_str()).collect::<Vec<_>>())
                .unwrap_or_default(),
        ),
    );

    Ok(devices)
}

fn select_device_for_operation<'a>(
    devices: &'a [TrezorDevice],
    selected_device: Option<&TrezorDevicePath>,
) -> Result<&'a TrezorDevice, TrezorError> {
    if devices.is_empty() {
        return Err(TrezorError::no_devices());
    }

    match selected_device {
        Some(path) => devices
            .iter()
            .find(|device| device.path == *path)
            .ok_or_else(TrezorError::device_disconnected),
        None => Ok(&devices[0]),
    }
}

async fn get_master_fingerprint_with_adapter(
    adapter: &impl BridgeAdapter,
    selected_device: Option<TrezorDevicePath>,
) -> Result<MasterFingerprintResult, TrezorError> {
    let devices = adapter.enumerate_devices().await?;
    let device = match select_device_for_operation(&devices, selected_device.as_ref()) {
        Ok(device) => device,
        Err(err) => {
            match err.kind {
                TrezorErrorKind::NoDevices => {
                    log("error", "No Trezor devices found", None);
                }
                TrezorErrorKind::DeviceDisconnected => {
                    if let Some(path) = selected_device.as_ref() {
                        log("error", "Selected device not found", Some(path.to_string()));
                    }
                }
                _ => {}
            }
            return Err(err);
        }
    };

    log(
        "info",
        "Acquiring device session",
        Some(device.path.to_string()),
    );

    let session = adapter
        .acquire_session(&device.path, device.session.as_deref())
        .await?;

    if let Err(err) = adapter.initialize_device_session(&session).await {
        let _ = adapter.release_session(&session).await;
        return Err(err);
    }

    let request = GetPublicKeyRequest::for_fingerprint();
    let response = match adapter.request_public_key(&session, &request).await {
        Ok(response) => response,
        Err(err) => {
            let _ = adapter.release_session(&session).await;
            return Err(err);
        }
    };

    if let Err(err) = adapter.release_session(&session).await {
        log("warn", "Failed to release session", Some(err.to_string()));
    }

    let fingerprint = match response.root_fingerprint {
        Some(value) => format!("{value:08x}"),
        None => {
            log("error", "No fingerprint in response", None);
            return Err(TrezorError::protocol_error(
                "Device did not return fingerprint".to_string(),
            ));
        }
    };

    log("info", "Got master fingerprint", Some(fingerprint.clone()));

    Ok(MasterFingerprintResult {
        success: true,
        fingerprint: Some(RawMasterFingerprint::new(fingerprint)),
        device_id: device.device_id.clone(),
        device_label: format_device_label(device).and_then(crate::wallets::TrezorDeviceLabel::new),
        error: None,
    })
}

/// Get master fingerprint from the Trezor device.
pub(crate) async fn get_master_fingerprint(
    user_id: UserId,
) -> Result<MasterFingerprintResult, TrezorError> {
    log("info", "Getting master fingerprint", None);

    let client = BridgeClient::new(user_id)?;
    get_master_fingerprint_with_adapter(&client, get_selected_device()).await
}

fn account_derivation_path(
    account_index: RawAccountIndex,
    address_scheme: AddressScheme,
) -> String {
    let purpose = match address_scheme {
        AddressScheme::Legacy => 44,
        AddressScheme::NestedSegwit => 49,
        AddressScheme::NativeSegwit => 84,
        AddressScheme::Taproot => 86,
        AddressScheme::Standard => 44,
    };
    format!("m/{purpose}'/0'/{}'", account_index.as_u32())
}

fn target_version_for_address_scheme(
    address_scheme: AddressScheme,
) -> Result<[u8; 4], TrezorError> {
    match address_scheme {
        AddressScheme::Legacy => Ok(XPUB_MAINNET_VERSION),
        AddressScheme::NestedSegwit => Ok(YPUB_MAINNET_VERSION),
        AddressScheme::NativeSegwit => Ok(ZPUB_MAINNET_VERSION),
        AddressScheme::Taproot => Err(TrezorError::protocol_error(
            "Taproot extended pubkeys are not supported in this flow".to_string(),
        )),
        AddressScheme::Standard => Err(TrezorError::protocol_error(
            "Standard address scheme is not applicable to Trezor".to_string(),
        )),
    }
}

fn convert_to_selected_pubkey_prefix(
    value: &str,
    address_scheme: AddressScheme,
) -> Result<String, TrezorError> {
    let target_version = target_version_for_address_scheme(address_scheme)?;

    if value.starts_with("xpub") || value.starts_with("ypub") || value.starts_with("zpub") {
        let mut data = bs58::decode(value)
            .with_check(None)
            .into_vec()
            .map_err(|e| {
                TrezorError::protocol_error(format!("Invalid extended pubkey encoding: {e}"))
            })?;

        if data.len() < 4 {
            return Err(TrezorError::protocol_error(
                "Extended pubkey payload too short".to_string(),
            ));
        }

        data[0..4].copy_from_slice(&target_version);
        return Ok(bs58::encode(data).with_check().into_string());
    }

    Err(TrezorError::protocol_error(format!(
        "Unknown extended pubkey format: {}",
        &value[..4.min(value.len())]
    )))
}

fn build_account_pubkey_result(
    account_index: RawAccountIndex,
    address_scheme: AddressScheme,
    response: PublicKeyResponse,
) -> Result<AccountPubkeyResult, TrezorError> {
    let extended_pubkey = convert_to_selected_pubkey_prefix(&response.xpub, address_scheme)?;
    let fingerprint = response
        .root_fingerprint
        .map(|fp| RawMasterFingerprint::new(format!("{fp:08x}")));
    let index = account_index.as_u32();
    let path = account_derivation_path(account_index, address_scheme);

    log(
        "info",
        &format!("Got account public key for account {index}"),
        Some(truncate_xpub(&extended_pubkey)),
    );

    Ok(AccountPubkeyResult {
        success: true,
        account_index,
        address_scheme,
        extended_pubkey: Some(RawExtendedPubkey::new(extended_pubkey)),
        path: Some(path),
        fingerprint,
        error: None,
    })
}

async fn get_account_pubkeys_with_adapter(
    adapter: &impl BridgeAdapter,
    account_indexes: Vec<RawAccountIndex>,
    address_scheme: AddressScheme,
    selected_device: Option<TrezorDevicePath>,
) -> Result<Vec<AccountPubkeyResult>, TrezorError> {
    let devices = adapter.enumerate_devices().await?;
    let device = select_device_for_operation(&devices, selected_device.as_ref())?;

    let session = adapter
        .acquire_session(&device.path, device.session.as_deref())
        .await?;

    if let Err(err) = adapter.initialize_device_session(&session).await {
        let _ = adapter.release_session(&session).await;
        return Err(err);
    }

    let mut results = Vec::new();
    for account_index in account_indexes {
        let index = account_index.as_u32();
        log(
            "info",
            &format!(
                "Getting account public key for account {index} ({})",
                address_scheme.as_str()
            ),
            None,
        );

        let request = GetPublicKeyRequest::for_address_scheme(index, address_scheme);
        let response = match adapter.request_public_key(&session, &request).await {
            Ok(response) => response,
            Err(err) => {
                log(
                    "error",
                    &format!("Failed to get account public key for account {index}"),
                    Some(err.to_string()),
                );
                let _ = adapter.release_session(&session).await;
                return Err(err);
            }
        };

        results.push(build_account_pubkey_result(
            account_index,
            address_scheme,
            response,
        )?);
    }

    if let Err(err) = adapter.release_session(&session).await {
        log("warn", "Failed to release session", Some(err.to_string()));
    }

    Ok(results)
}

/// Get account extended public keys for multiple accounts.
pub(crate) async fn get_account_pubkeys(
    user_id: UserId,
    account_indexes: Vec<RawAccountIndex>,
    address_scheme: AddressScheme,
) -> Result<Vec<AccountPubkeyResult>, TrezorError> {
    log(
        "info",
        "Getting account public keys for accounts",
        Some(format!(
            "scheme={} indexes={:?}",
            address_scheme.as_str(),
            account_indexes
                .iter()
                .map(|i| i.as_u32())
                .collect::<Vec<_>>()
        )),
    );

    let client = BridgeClient::new(user_id)?;
    get_account_pubkeys_with_adapter(
        &client,
        account_indexes,
        address_scheme,
        get_selected_device(),
    )
    .await
}

/// Truncate xpub for logging (don't log full key).
fn truncate_xpub(xpub: &str) -> String {
    if xpub.len() > 20 {
        format!("{}...{}", &xpub[..12], &xpub[xpub.len() - 4..])
    } else {
        xpub.to_string()
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::wallets::TrezorDeviceId;
    use bitcoin::Network;
    use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv, Xpub};
    use bitcoin::secp256k1::Secp256k1;
    use std::collections::VecDeque;

    struct MockBridge {
        devices: Vec<TrezorDevice>,
        acquire_result: Result<TrezorSession, TrezorError>,
        initialize_result: Result<(), TrezorError>,
        public_key_results: Mutex<VecDeque<Result<PublicKeyResponse, TrezorError>>>,
        release_result: Result<(), TrezorError>,
        acquired_paths: Mutex<Vec<String>>,
        release_calls: Mutex<usize>,
    }

    impl MockBridge {
        fn new(
            devices: Vec<TrezorDevice>,
            public_key_results: Vec<Result<PublicKeyResponse, TrezorError>>,
        ) -> Self {
            Self {
                devices,
                acquire_result: Ok(TrezorSession::new(
                    "session-1".to_string(),
                    TrezorDevicePath::new("device-1".to_string()),
                )),
                initialize_result: Ok(()),
                public_key_results: Mutex::new(VecDeque::from(public_key_results)),
                release_result: Ok(()),
                acquired_paths: Mutex::new(Vec::new()),
                release_calls: Mutex::new(0),
            }
        }

        fn with_acquire_result(mut self, value: Result<TrezorSession, TrezorError>) -> Self {
            self.acquire_result = value;
            self
        }

        fn with_initialize_result(mut self, value: Result<(), TrezorError>) -> Self {
            self.initialize_result = value;
            self
        }

        fn acquire_paths(&self) -> Vec<String> {
            self.acquired_paths
                .lock()
                .map(|paths| paths.clone())
                .unwrap_or_default()
        }

        fn release_count(&self) -> usize {
            self.release_calls
                .lock()
                .map(|calls| *calls)
                .unwrap_or_default()
        }
    }

    impl BridgeAdapter for MockBridge {
        fn enumerate_devices(&self) -> BridgeFuture<'_, Result<Vec<TrezorDevice>, TrezorError>> {
            let devices = self.devices.clone();
            Box::pin(async move { Ok(devices) })
        }

        fn acquire_session<'a>(
            &'a self,
            path: &'a TrezorDevicePath,
            _previous: Option<&'a str>,
        ) -> BridgeFuture<'a, Result<TrezorSession, TrezorError>> {
            Box::pin(async move {
                if let Ok(mut paths) = self.acquired_paths.lock() {
                    paths.push(path.to_string());
                }
                self.acquire_result.clone()
            })
        }

        fn initialize_device_session<'a>(
            &'a self,
            _session: &'a TrezorSession,
        ) -> BridgeFuture<'a, Result<(), TrezorError>> {
            let value = self.initialize_result.clone();
            Box::pin(async move { value })
        }

        fn request_public_key<'a>(
            &'a self,
            _session: &'a TrezorSession,
            _request: &'a GetPublicKeyRequest,
        ) -> BridgeFuture<'a, Result<PublicKeyResponse, TrezorError>> {
            Box::pin(async move {
                let mut responses = self.public_key_results.lock().map_err(|_| {
                    TrezorError::internal("Mock public key queue lock poisoned".to_string())
                })?;

                match responses.pop_front() {
                    Some(result) => result,
                    None => Err(TrezorError::internal(
                        "Missing mock public key response".to_string(),
                    )),
                }
            })
        }

        fn release_session<'a>(
            &'a self,
            _session: &'a TrezorSession,
        ) -> BridgeFuture<'a, Result<(), TrezorError>> {
            let value = self.release_result.clone();
            Box::pin(async move {
                if let Ok(mut release_calls) = self.release_calls.lock() {
                    *release_calls += 1;
                }
                value
            })
        }
    }

    fn make_device(path: &str, device_id: &str) -> TrezorDevice {
        let parsed_id = match TrezorDeviceId::new(device_id.to_string()) {
            Some(value) => value,
            None => panic!("device id should be non-empty"),
        };

        TrezorDevice {
            path: TrezorDevicePath::new(path.to_string()),
            device_id: Some(parsed_id),
            session: None,
            product: Some("Trezor".to_string()),
            vendor: Some("SatoshiLabs".to_string()),
            product_name: Some("Trezor Safe 3".to_string()),
            manufacturer_name: Some("SatoshiLabs".to_string()),
            serial_number: Some("SERIAL123".to_string()),
        }
    }

    fn test_account_xpub(account: u32) -> String {
        let secp = Secp256k1::new();

        let mut seed = [0_u8; 32];
        seed[0..4].copy_from_slice(&account.to_be_bytes());
        let master = Xpriv::new_master(Network::Bitcoin, &seed)
            .expect("deterministic test seed should produce a valid Xpriv");

        let path = DerivationPath::from(vec![
            ChildNumber::Hardened { index: 84 },
            ChildNumber::Hardened { index: 0 },
            ChildNumber::Hardened { index: account },
        ]);

        let account_xpriv = master
            .derive_priv(&secp, &path)
            .expect("deterministic account derivation should succeed");

        Xpub::from_priv(&secp, &account_xpriv).to_string()
    }

    #[test]
    fn test_convert_to_selected_pubkey_prefix_passthrough() {
        let source_xpub = test_account_xpub(0);
        let zpub = convert_to_selected_pubkey_prefix(&source_xpub, AddressScheme::NativeSegwit)
            .expect("xpub should convert to zpub");

        let converted = convert_to_selected_pubkey_prefix(&zpub, AddressScheme::NativeSegwit)
            .expect("zpub should pass through unchanged");

        assert_eq!(converted, zpub);
    }

    #[test]
    fn test_truncate_xpub() {
        let long = "zpubDEADBEEFABCDEF1234567890EXAMPLE";
        let truncated = truncate_xpub(long);
        assert!(truncated.contains("..."));
        assert!(truncated.len() < long.len());
    }

    #[test]
    fn test_format_device_label_prefers_device_id() {
        let device_id = TrezorDeviceId::new("C67C7D32D598B9DF40567CEF".to_string())
            .expect("device id should be valid");
        let device = TrezorDevice {
            path: TrezorDevicePath::new("2".to_string()),
            device_id: Some(device_id),
            session: None,
            product: Some("Trezor".to_string()),
            vendor: Some("SatoshiLabs".to_string()),
            product_name: Some("Trezor Safe 3".to_string()),
            manufacturer_name: Some("SatoshiLabs".to_string()),
            serial_number: Some("SERIAL123".to_string()),
        };

        assert_eq!(
            format_device_label(&device),
            Some("Trezor C67C7D32D598B9DF40567CEF".to_string())
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_get_master_fingerprint_contract_uses_selected_device_and_releases_session() {
        let selected_path = TrezorDevicePath::new("device-2".to_string());
        let mock = MockBridge::new(
            vec![
                make_device("device-1", "DEVICE_ONE"),
                make_device("device-2", "DEVICE_TWO"),
            ],
            vec![Ok(PublicKeyResponse {
                xpub: "zpubContractFingerprint".to_string(),
                root_fingerprint: Some(0xA1B2C3D4),
            })],
        )
        .with_acquire_result(Ok(TrezorSession::new(
            "session-2".to_string(),
            selected_path.clone(),
        )));

        let result = get_master_fingerprint_with_adapter(&mock, Some(selected_path))
            .await
            .expect("master fingerprint contract test should succeed");

        assert_eq!(
            result.fingerprint.as_ref().map(|value| {
                value
                    .clone()
                    .validate()
                    .expect("fingerprint should validate")
                    .as_str()
                    .to_string()
            }),
            Some("a1b2c3d4".to_string())
        );
        assert_eq!(
            result.device_id.as_ref().map(|value| value.as_str()),
            Some("DEVICE_TWO")
        );
        assert_eq!(
            result.device_label.as_ref().map(|value| value.as_str()),
            Some("Trezor DEVICE_TWO")
        );
        assert_eq!(mock.acquire_paths(), vec!["device-2".to_string()]);
        assert_eq!(mock.release_count(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_get_master_fingerprint_contract_releases_session_on_public_key_error() {
        let mock = MockBridge::new(
            vec![make_device("device-1", "DEVICE_ONE")],
            vec![Err(TrezorError::protocol_error("fingerprint failed"))],
        );

        let err = get_master_fingerprint_with_adapter(&mock, None)
            .await
            .expect_err("master fingerprint contract test should fail");

        assert_eq!(err.kind, TrezorErrorKind::ProtocolError);
        assert_eq!(mock.release_count(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_get_account_pubkeys_contract_returns_expected_payload() {
        let account_indexes = vec![RawAccountIndex::new(0), RawAccountIndex::new(2)];
        let account_0_xpub = test_account_xpub(0);
        let account_2_xpub = test_account_xpub(2);
        let expected_0_zpub =
            convert_to_selected_pubkey_prefix(&account_0_xpub, AddressScheme::NativeSegwit)
                .expect("valid xpub should convert to zpub");
        let expected_2_zpub =
            convert_to_selected_pubkey_prefix(&account_2_xpub, AddressScheme::NativeSegwit)
                .expect("valid xpub should convert to zpub");
        let mock = MockBridge::new(
            vec![make_device("device-1", "DEVICE_ONE")],
            vec![
                Ok(PublicKeyResponse {
                    xpub: account_0_xpub.clone(),
                    root_fingerprint: Some(0x11223344),
                }),
                Ok(PublicKeyResponse {
                    xpub: account_2_xpub.clone(),
                    root_fingerprint: None,
                }),
            ],
        );

        let results = get_account_pubkeys_with_adapter(
            &mock,
            account_indexes.clone(),
            AddressScheme::NativeSegwit,
            Some(TrezorDevicePath::new("device-1".to_string())),
        )
        .await
        .expect("account pubkey contract test should succeed");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].account_index, account_indexes[0]);
        assert_eq!(
            results[0]
                .extended_pubkey
                .as_ref()
                .map(|value| value.as_str()),
            Some(expected_0_zpub.as_str())
        );
        assert_eq!(results[0].path.as_deref(), Some("m/84'/0'/0'"));
        assert_eq!(
            results[0].fingerprint.as_ref().map(|value| {
                value
                    .clone()
                    .validate()
                    .expect("fingerprint should validate")
                    .as_str()
                    .to_string()
            }),
            Some("11223344".to_string())
        );
        assert_eq!(results[1].account_index, account_indexes[1]);
        assert_eq!(
            results[1]
                .extended_pubkey
                .as_ref()
                .map(|value| value.as_str()),
            Some(expected_2_zpub.as_str())
        );
        assert_eq!(results[1].path.as_deref(), Some("m/84'/0'/2'"));
        assert_eq!(
            results[1].fingerprint.as_ref().map(|value| {
                value
                    .clone()
                    .validate()
                    .expect("fingerprint should validate")
                    .as_str()
                    .to_string()
            }),
            None
        );
        assert_eq!(mock.release_count(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_get_account_pubkeys_contract_releases_session_on_partial_failure() {
        let account_indexes = vec![RawAccountIndex::new(0), RawAccountIndex::new(1)];
        let account_0_xpub = test_account_xpub(0);
        let mock = MockBridge::new(
            vec![make_device("device-1", "DEVICE_ONE")],
            vec![
                Ok(PublicKeyResponse {
                    xpub: account_0_xpub,
                    root_fingerprint: Some(0x11223344),
                }),
                Err(TrezorError::device_disconnected()),
            ],
        );

        let err = get_account_pubkeys_with_adapter(
            &mock,
            account_indexes,
            AddressScheme::NativeSegwit,
            None,
        )
        .await
        .expect_err("account pubkey contract test should fail");

        assert_eq!(err.kind, TrezorErrorKind::DeviceDisconnected);
        assert_eq!(mock.release_count(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_get_account_pubkeys_contract_fails_when_selected_device_missing() {
        let mock = MockBridge::new(
            vec![make_device("device-1", "DEVICE_ONE")],
            vec![Ok(PublicKeyResponse {
                xpub: "zpubContractAccount0".to_string(),
                root_fingerprint: Some(0x11223344),
            })],
        );

        let err = get_account_pubkeys_with_adapter(
            &mock,
            vec![RawAccountIndex::new(0)],
            AddressScheme::NativeSegwit,
            Some(TrezorDevicePath::new("device-missing".to_string())),
        )
        .await
        .expect_err("account pubkey contract test should fail when selected device is missing");

        assert_eq!(err.kind, TrezorErrorKind::DeviceDisconnected);
        assert!(mock.acquire_paths().is_empty());
        assert_eq!(mock.release_count(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_get_account_pubkeys_contract_releases_session_on_initialize_error() {
        let mock = MockBridge::new(vec![make_device("device-1", "DEVICE_ONE")], Vec::new())
            .with_initialize_result(Err(TrezorError::session_conflict()));

        let err = get_account_pubkeys_with_adapter(
            &mock,
            vec![RawAccountIndex::new(0)],
            AddressScheme::NativeSegwit,
            None,
        )
        .await
        .expect_err("account pubkey contract test should fail");

        assert_eq!(err.kind, TrezorErrorKind::SessionConflict);
        assert_eq!(mock.release_count(), 1);
    }
}
