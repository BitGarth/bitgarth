//! Web (WASM) implementation using trezor-connect JavaScript SDK.

use super::types::{
    AccountPubkeyResult, MasterFingerprintResult, TrezorError, TrezorErrorDetail, TrezorErrorKind,
    TrezorInitError, TrezorInitResult,
};
use crate::models::UserId;
use crate::wallets::{AddressScheme, RawAccountIndex};
use serde_wasm_bindgen as swb;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = TrezorBridge)]
    async fn init() -> JsValue;

    #[wasm_bindgen(js_namespace = TrezorBridge)]
    async fn getMasterFingerprint() -> JsValue;

    #[wasm_bindgen(js_namespace = TrezorBridge)]
    async fn getMultipleAccountPubkeys(account_indexes: JsValue, address_scheme: String)
    -> JsValue;

}

/// Initialize Trezor Connect (web only).
pub(crate) async fn initialize_trezor(_user_id: UserId) -> Result<(), TrezorError> {
    let result: TrezorInitResult = swb::from_value(init().await).map_err(|e| {
        TrezorError::with_detail(
            TrezorErrorKind::ConnectInitParseFailed,
            TrezorErrorDetail::new(format!("init parse error: {e}")),
        )
    })?;

    if result.success {
        return Ok(());
    }

    let error = match result.error {
        Some(TrezorInitError::Detailed(payload)) => {
            TrezorError::with_detail(TrezorErrorKind::ConnectInitFailed, payload.to_detail())
        }
        Some(TrezorInitError::Message(message)) => TrezorError::with_detail(
            TrezorErrorKind::ConnectInitFailed,
            TrezorErrorDetail::new(format!("message={}", message.as_str())),
        ),
        None => TrezorError::new(TrezorErrorKind::ConnectInitFailed),
    };

    Err(error)
}

/// Get master fingerprint from connected Trezor (web only).
pub(crate) async fn get_master_fingerprint(
    _user_id: UserId,
) -> Result<MasterFingerprintResult, TrezorError> {
    let result: MasterFingerprintResult =
        swb::from_value(getMasterFingerprint().await).map_err(|e| {
            TrezorError::with_detail(
                TrezorErrorKind::ConnectFingerprintParseFailed,
                TrezorErrorDetail::new(format!("fingerprint parse error: {e}")),
            )
        })?;

    if result.success {
        Ok(result)
    } else {
        Err(result
            .error
            .map(|payload| {
                TrezorError::with_detail(
                    TrezorErrorKind::ConnectFingerprintFailed,
                    payload.to_detail(),
                )
            })
            .unwrap_or_else(|| TrezorError::new(TrezorErrorKind::ConnectFingerprintFailed)))
    }
}

/// Get account extended public keys for multiple accounts (web only).
pub(crate) async fn get_account_pubkeys(
    _user_id: UserId,
    account_indexes: Vec<RawAccountIndex>,
    address_scheme: AddressScheme,
) -> Result<Vec<AccountPubkeyResult>, TrezorError> {
    let js_indexes = swb::to_value(&account_indexes).map_err(|e| {
        TrezorError::with_detail(
            TrezorErrorKind::ConnectAccountIndexesSerializeFailed,
            TrezorErrorDetail::new(format!("account index serialize error: {e}")),
        )
    })?;

    let results: Vec<AccountPubkeyResult> = swb::from_value(
        getMultipleAccountPubkeys(js_indexes, address_scheme.as_str().to_string()).await,
    )
    .map_err(|e| {
        TrezorError::with_detail(
            TrezorErrorKind::ConnectZpubParseFailed,
            TrezorErrorDetail::new(format!("account pubkey parse error: {e}")),
        )
    })?;

    for result in &results {
        if !result.success {
            return Err(result
                .error
                .clone()
                .map(|payload| {
                    TrezorError::with_detail(
                        TrezorErrorKind::ConnectZpubFailed,
                        payload.to_detail(),
                    )
                })
                .unwrap_or_else(|| TrezorError::new(TrezorErrorKind::ConnectZpubFailed)));
        }
    }

    Ok(results)
}
