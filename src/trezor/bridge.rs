//! Trezor Bridge HTTP client for desktop communication.
//!
//! Trezor Bridge runs on localhost:21325 and provides HTTP API for device communication.
//! This is the standard way to communicate with Trezor devices on desktop platforms.

use super::proto::{
    ButtonAckMessage, ButtonRequestMessage, FailureResponse, GetPublicKeyRequest, MessageType,
    PublicKeyResponse, decode_message, encode_message,
};
use super::types::{
    BridgeStatus, TrezorDevice, TrezorDevicePath, TrezorError, TrezorErrorDetail, TrezorSession,
};
use crate::models::UserId;
use crate::traces::client::{IntegrationLabel, TracedAsyncClient};
use serde::Deserialize;
use std::time::Duration;

const BRIDGE_URL: &str = "http://127.0.0.1:21325";
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP client for Trezor Bridge API.
pub(super) struct BridgeClient {
    client: TracedAsyncClient,
}

/// Device info from Bridge enumerate response.
#[derive(Debug, Deserialize)]
struct BridgeDevice {
    id: Option<String>,
    path: String,
    session: Option<String>,
    #[serde(rename = "productId", alias = "product")]
    product_id: Option<i32>,
    #[serde(rename = "vendorId", alias = "vendor")]
    vendor_id: Option<i32>,
    #[serde(rename = "productName")]
    product_name: Option<String>,
    #[serde(rename = "manufacturerName")]
    manufacturer_name: Option<String>,
    #[serde(rename = "serialNumber")]
    serial_number: Option<String>,
}

/// Acquire response from Bridge.
#[derive(Debug, Deserialize)]
struct AcquireResponse {
    session: String,
}

/// Error response from Bridge.
#[derive(Debug, Deserialize)]
struct BridgeError {
    error: String,
}

impl BridgeClient {
    /// Create a new Bridge client.
    ///
    /// HTTP tracing (when enabled via `BGTRACES=fs`) will write trace files
    /// to that user's traces directory.
    pub(super) fn new(user_id: UserId) -> Result<Self, TrezorError> {
        let client = TracedAsyncClient::builder(IntegrationLabel::new("trezor-bridge"), user_id)
            .configure(|b| b.timeout(BRIDGE_TIMEOUT))
            .build()
            .map_err(|e| TrezorError::internal(e.to_string()))?;

        Ok(Self { client })
    }

    /// Check if Trezor Bridge is running.
    pub(super) async fn check_status(&self) -> BridgeStatus {
        match self.enumerate_raw().await {
            Ok(_) => BridgeStatus::Running,
            Err(e) => {
                tracing::debug!("BridgeClient enumerate_raw error={:?}", e);
                BridgeStatus::NotRunning
            }
        }
    }

    /// Enumerate connected Trezor devices.
    pub(super) async fn enumerate(&self) -> Result<Vec<TrezorDevice>, TrezorError> {
        let devices = self.enumerate_raw().await?;

        Ok(devices
            .into_iter()
            .map(|d| TrezorDevice {
                path: TrezorDevicePath::new(d.path),
                device_id: d.id.and_then(crate::wallets::TrezorDeviceId::new),
                session: d.session,
                product_name: d
                    .product_name
                    .clone()
                    .or_else(|| d.product_id.map(product_name)),
                manufacturer_name: d
                    .manufacturer_name
                    .clone()
                    .or_else(|| d.vendor_id.map(vendor_name)),
                serial_number: d.serial_number,
                product: d.product_name.or_else(|| d.product_id.map(product_name)),
                vendor: d.manufacturer_name.or_else(|| d.vendor_id.map(vendor_name)),
            })
            .collect())
    }

    /// Raw enumerate call.
    async fn enumerate_raw(&self) -> Result<Vec<BridgeDevice>, TrezorError> {
        let response = self
            .client
            .post(format!("{BRIDGE_URL}/enumerate"))
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    TrezorError::bridge_not_running()
                } else {
                    TrezorError::internal(format!("Bridge request failed: {e}"))
                }
            })?;

        let status = response.status();
        let response_url = response.url().to_string();
        let text = response.text().map_err(|e| {
            TrezorError::internal(format!(
                "Failed to read Bridge response from {response_url}: {e}"
            ))
        })?;

        if !status.is_success() {
            return Err(parse_bridge_error(&text));
        }

        serde_json::from_str(&text).map_err(|e| {
            TrezorError::protocol_error(format!("Failed to parse enumerate response: {e}"))
        })
    }

    /// Acquire a device session.
    pub(super) async fn acquire(
        &self,
        path: &TrezorDevicePath,
        previous: Option<&str>,
    ) -> Result<TrezorSession, TrezorError> {
        let previous_part = previous.unwrap_or("null");
        tracing::debug!(
            path = %path,
            previous = %previous_part,
            "Acquiring device session"
        );
        let url = format!("{BRIDGE_URL}/acquire/{}/{previous_part}", path.as_str());

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| TrezorError::internal(format!("Acquire request failed: {e}")))?;

        let status = response.status();
        let response_url = response.url().to_string();
        let text = response.text().map_err(|e| {
            TrezorError::internal(format!(
                "Failed to read acquire response from {response_url}: {e}"
            ))
        })?;

        if !status.is_success() {
            return Err(parse_bridge_error(&text));
        }

        let acquire: AcquireResponse = serde_json::from_str(&text).map_err(|e| {
            TrezorError::protocol_error(format!("Failed to parse acquire response: {e}"))
        })?;

        tracing::debug!(session_id = %acquire.session, "Session acquired successfully");
        Ok(TrezorSession::new(acquire.session, path.clone()))
    }

    /// Release a device session.
    pub(super) async fn release(&self, session: &TrezorSession) -> Result<(), TrezorError> {
        let url = format!("{BRIDGE_URL}/release/{}", session.session_id);

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| TrezorError::internal(format!("Release request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().unwrap_or_default();
            return Err(parse_bridge_error(&text));
        }

        Ok(())
    }

    /// Send Initialize message to the device to reset its state.
    /// Must be called after acquire and before any other commands.
    /// The device responds with Features, which we acknowledge but don't parse.
    ///
    /// Retries once on failure — the bridge can return 400 when the device has
    /// stale state from a previous session, but the message may still reach the
    /// device and clear that state.
    pub(super) async fn initialize_session(
        &self,
        session: &TrezorSession,
    ) -> Result<(), TrezorError> {
        tracing::debug!("Sending Initialize to device");
        let message = encode_message(MessageType::Initialize, &[]);

        let response_bytes = match self.call(session, &message).await {
            Ok(bytes) => bytes,
            Err(first_err) => {
                tracing::warn!("First Initialize failed, retrying: {first_err}");
                self.call(session, &message).await?
            }
        };

        let (msg_type, resp_payload) = decode_message(&response_bytes)?;

        match msg_type {
            MessageType::Features => {
                tracing::debug!("Device responded with Features — initialized");
                Ok(())
            }
            MessageType::Failure => {
                let failure = FailureResponse::decode(&resp_payload)?;
                Err(failure.to_error())
            }
            other => Err(TrezorError::protocol_error(format!(
                "Unexpected response to Initialize: {other:?}"
            ))),
        }
    }

    /// Send a message to the device and receive response.
    /// Handles hex encoding for Bridge API.
    pub(super) async fn call(
        &self,
        session: &TrezorSession,
        message: &[u8],
    ) -> Result<Vec<u8>, TrezorError> {
        let url = format!("{BRIDGE_URL}/call/{}", session.session_id);
        let hex_message = hex::encode(message);
        tracing::debug!(
            session = %session.session_id,
            hex_len = hex_message.len(),
            raw_len = message.len(),
            hex_prefix = %&hex_message[..hex_message.len().min(40)],
            "Sending call to Bridge"
        );

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "text/plain")
            .body(hex_message)
            .send()
            .await
            .map_err(|e| TrezorError::internal(format!("Call request failed: {e}")))?;

        let status = response.status();
        let response_url = response.url().to_string();
        let text = response.text().map_err(|e| {
            TrezorError::internal(format!(
                "Failed to read call response from {response_url}: {e}"
            ))
        })?;

        if !status.is_success() {
            tracing::debug!(
                status = %status,
                body = %text,
                "Bridge call returned error"
            );
            return Err(parse_bridge_error(&text));
        }

        hex::decode(text.trim()).map_err(|e| {
            TrezorError::protocol_error(format!("Invalid hex response from Bridge: {e}"))
        })
    }

    /// Get public key from device, handling button confirmation flow.
    pub(super) async fn get_public_key(
        &self,
        session: &TrezorSession,
        request: &GetPublicKeyRequest,
    ) -> Result<PublicKeyResponse, TrezorError> {
        let encoded_payload = request.encode();
        let message = encode_message(MessageType::GetPublicKey, &encoded_payload);

        let mut response_bytes = self.call(session, &message).await?;
        let mut stale_features_retried = false;

        // Loop to handle ButtonRequest/ButtonAck flow
        loop {
            let (msg_type, resp_payload) = decode_message(&response_bytes)?;

            match msg_type {
                MessageType::PublicKey => {
                    return PublicKeyResponse::decode(&resp_payload);
                }
                MessageType::Failure => {
                    let failure = FailureResponse::decode(&resp_payload)?;
                    return Err(failure.to_error());
                }
                MessageType::Features if !stale_features_retried => {
                    // Stale Features response from a desynchronized Initialize.
                    // Re-send the GetPublicKey request.
                    tracing::warn!("Received stale Features response, re-sending GetPublicKey");
                    stale_features_retried = true;
                    response_bytes = self.call(session, &message).await?;
                }
                MessageType::ButtonRequest => {
                    // Device is waiting for user confirmation
                    let _button_req = ButtonRequestMessage::decode(&resp_payload)?;

                    // Send ButtonAck
                    let ack = encode_message(MessageType::ButtonAck, &ButtonAckMessage::encode());
                    response_bytes = self.call(session, &ack).await?;
                }
                MessageType::PinMatrixRequest => {
                    // Safe 3/5 should not send this (they have on-device PIN)
                    // For older models, we would need to handle PIN entry differently
                    return Err(TrezorError::pin_required());
                }
                MessageType::PassphraseRequest => {
                    // Passphrase entry is typically done on-device or in Trezor Suite
                    return Err(TrezorError::passphrase_required());
                }
                other => {
                    return Err(TrezorError::protocol_error(format!(
                        "Unexpected message type: {other:?}"
                    )));
                }
            }
        }
    }
}

/// Parse Bridge error response.
fn parse_bridge_error(text: &str) -> TrezorError {
    if let Ok(err) = serde_json::from_str::<BridgeError>(text) {
        match err.error.as_str() {
            "device disconnected during action" => TrezorError::device_disconnected(),
            "session not found" => TrezorError::session_expired(),
            "wrong previous session" | "Invalid session" => TrezorError::session_conflict(),
            "" => TrezorError::bridge_rejected(),
            other => {
                TrezorError::bridge_error(TrezorErrorDetail::new(format!("bridge error: {other}")))
            }
        }
    } else {
        TrezorError::bridge_error(TrezorErrorDetail::new(format!("bridge error: {text}")))
    }
}

/// Convert Trezor product ID to name.
fn product_name(product_id: i32) -> String {
    match product_id {
        0x0001 => "Trezor One".to_string(),
        0x0002 => "Trezor Model T".to_string(),
        0x0003 => "Trezor Safe 3".to_string(),
        0x0004 => "Trezor Safe 5".to_string(),
        other => format!("Trezor (product {other})"),
    }
}

/// Convert vendor ID to name.
fn vendor_name(vendor_id: i32) -> String {
    match vendor_id {
        0x534c => "SatoshiLabs".to_string(),
        other => format!("Vendor {other}"),
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn test_product_names() {
        assert_eq!(product_name(0x0001), "Trezor One");
        assert_eq!(product_name(0x0002), "Trezor Model T");
        assert_eq!(product_name(0x0003), "Trezor Safe 3");
        assert_eq!(product_name(0x0004), "Trezor Safe 5");
    }
}
