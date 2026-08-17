mod api_error;
mod auth;
mod exports;
mod pairing;
mod payments;
mod prices;
mod retention;
#[cfg(feature = "server")]
mod session_context;
#[cfg(feature = "server")]
mod session_token;
mod settings;
mod sync_control;
mod transactions;
mod updates;
mod wallets;

#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
pub(crate) mod public_api;

#[cfg(feature = "server")]
const TRUST_PROXY_HEADERS_ENV: &str = "BITGARTH_TRUST_PROXY_HEADERS";

#[cfg(feature = "server")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ProxyHeaderTrust {
    Trusted,
    ForwardedProtoOnly,
    #[default]
    Untrusted,
}

#[cfg(feature = "server")]
impl ProxyHeaderTrust {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(Self::Trusted),
            "proto" => Some(Self::ForwardedProtoOnly),
            "0" | "false" | "no" | "off" => Some(Self::Untrusted),
            _ => None,
        }
    }

    pub(crate) fn from_env() -> Self {
        let default = Self::default();
        let raw = match std::env::var(TRUST_PROXY_HEADERS_ENV) {
            Ok(raw) => raw,
            Err(_) => return default,
        };

        match Self::parse(&raw) {
            Some(policy) => policy,
            None => {
                dioxus::logger::tracing::warn!(
                    env_var = TRUST_PROXY_HEADERS_ENV,
                    value = %raw,
                    fallback = ?default,
                    "backend: invalid proxy-header trust policy, using default"
                );
                default
            }
        }
    }

    pub(crate) const fn allows_forwarded_for(self) -> bool {
        matches!(self, Self::Trusted)
    }

    pub(crate) const fn allows_forwarded_proto(self) -> bool {
        matches!(self, Self::Trusted | Self::ForwardedProtoOnly)
    }
}

pub(crate) use api_error::ApiErrorEnvelope;
pub(crate) use auth::{AuthError, auth_entry, change_password, login, logout, me, register};
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
pub(crate) use exports::download_hledger;
pub(crate) use exports::{
    ConfirmPremiumTransferRequest, DescribeWalletDataImportRequest, ExportError,
    ExportWalletDataRequest, ImportResultView, ImportWalletDataRequest,
    PremiumTransferImportStatusView, PremiumTransferResultView, PremiumTransferStatusView,
    WalletDataExportCounts, WalletDataExportDownloadView, WalletDataExportSummary,
    WalletDataImportDescription, confirm_premium_transfer, describe_wallet_data_import,
    export_wallet_data, get_wallet_data_export_options, import_wallet_data,
};
pub(crate) use pairing::{
    ApprovePairingRequest, DenyPairingRequest, PairedClientView, PairingReviewResponse,
    RevokePairedClientRequest, approve_pairing, deny_pairing, list_paired_clients, review_pairing,
    revoke_paired_client,
};
pub(crate) use payments::{
    cancel_premium_order, get_payment_catalog, get_payment_state_local, poll_premium_order,
    reconcile_payment_history, refresh_payment_state, refresh_premium_status, start_premium_order,
    start_premium_top_up,
};
pub(crate) use prices::{
    DeletePriceOverrideInput, PriceSourceView, ResolvedPriceView, UpsertPriceOverrideInput,
    delete_price_override, list_resolved_prices_for_holdings_report,
    list_resolved_prices_for_report, upsert_price_override,
};
#[cfg(all(test, feature = "server"))]
pub(crate) use prices::{PriceOverrideView, list_price_overrides};
#[cfg(any(test, feature = "server"))]
pub(crate) use wallets::CurrentAssetValueView;
#[cfg(any(test, feature = "server"))]
pub(crate) use wallets::HoldingsReportWalletRow;
#[cfg(feature = "server")]
pub(crate) use wallets::WalletAggregateBalanceView;
pub(crate) use wallets::{
    AccountBalanceStateView, AccountCreationStateView, AccountLimitNoticeView,
    AccountReferenceKind, AccountStateView, AccountTransactionCountsView, AccountView,
    BalanceAmountView, CustomAccountView, FiatAmountView, HoldingsReportResponse,
    ManualAssetAccountView, ManualSyncDisabledReason, ManualSyncMode, ManualSyncSlotEffect,
    NativeAccountManualSyncView, NativeAccountSyncSlotView, NativeAccountView,
    NativeBalanceStateView, ValidateXpubResponse, WalletBalanceView, WalletError,
    WalletReportAccountRow, WalletReportBalanceStateView, WalletReportResponse,
    WalletValueSummaryView, WalletView, WalletsValueSummaryView, add_bitcoin_address,
    add_ethereum_address, add_manual_asset_account, add_xpub,
    delete_account as delete_wallet_account, delete_wallet, get_account_addresses,
    get_holdings_report, get_wallet_report, get_wallets, manual_asset_catalog_total,
    manual_asset_discovery_detail, manual_asset_discovery_price, move_wallet_account,
    search_manual_asset_instances, update_account_label, update_wallet_label, validate_xpub,
};
#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
pub(crate) use wallets::{get_wallet_by_fingerprint, link_trezor_wallet};
// SettingsError is needed for server function serialization even though client code discards errors
pub(crate) use retention::{HostedRetentionStatus, hosted_retention_status};
pub(crate) use settings::SettingsError;
pub(crate) use settings::{
    get_hledger_export_settings, get_settings, save_coingecko_api_key, save_currency,
    save_date_time_format, save_etherscan_api_key, save_etherscan_base_url,
    save_hledger_account_prefix, save_mempool_base_url, save_number_format, save_timezone,
    set_price_fetching_enabled,
};
pub(crate) use sync_control::{get_account_sync_control_state, run_account_sync_control};
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
pub(crate) use transactions::transactions_sync_events_sse;
pub(crate) use transactions::{
    add_manual_asset_balance_assertion, delete_manual_asset_balance_assertion,
    get_account_sync_snapshots, get_account_transactions, get_sync_state, trigger_sync,
    update_manual_asset_balance_assertion,
};
pub(crate) use updates::{
    UpdateStatus, refresh_update_status, set_update_check_enabled, update_status,
};

#[cfg(feature = "server")]
const _: () = {
    let _ = std::mem::size_of::<DescribeWalletDataImportRequest>();
    let _ = std::mem::size_of::<FiatAmountView>();
    let _ = std::mem::size_of::<HoldingsReportResponse>();
    let _ = std::mem::size_of::<HoldingsReportWalletRow>();
    let _ = std::mem::size_of::<WalletDataImportDescription>();
    let _ = std::mem::size_of::<UpdateStatus>();
    let _ = describe_wallet_data_import;
    let _ = get_holdings_report;
    let _ = get_hledger_export_settings;
    let _ = manual_asset_catalog_total;
    let _ = manual_asset_discovery_detail;
    let _ = manual_asset_discovery_price;
    let _ = list_resolved_prices_for_holdings_report;
    let _ = refresh_update_status;
    let _ = save_hledger_account_prefix;
    let _ = set_update_check_enabled;
    let _ = update_status;
};

#[cfg(test)]
const _: () = {
    let _ = std::mem::size_of::<FiatAmountView>();
    let _ = std::mem::size_of::<HoldingsReportResponse>();
    let _ = std::mem::size_of::<HoldingsReportWalletRow>();
};

use dioxus::prelude::*;

#[cfg(feature = "server")]
use dioxus::logger::tracing;

#[cfg(feature = "server")]
use crate::db::DbInitError;
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use axum::body::{Body, to_bytes};
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use axum::extract::Request;
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use axum::http::StatusCode;
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use axum::middleware::Next;
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
use axum::response::Response;

#[cfg(feature = "server")]
impl From<DbInitError> for ServerFnError {
    fn from(e: DbInitError) -> Self {
        ServerFnError::new(e.to_string())
    }
}

#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
fn is_server_fn_input_deserialize_error(status: StatusCode, body: &str) -> bool {
    if status != StatusCode::INTERNAL_SERVER_ERROR {
        return false;
    }

    body.contains("error deserializing server function arguments")
        || body.contains("error deserializing server function results")
        || body.contains("missing argument")
}

/// Normalize malformed server-function inputs to `400 Bad Request`.
///
/// Dioxus currently reports some argument decode failures as `500`.
/// This middleware preserves the original response body but corrects
/// transport-level malformed-input responses to `400`.
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
pub(crate) async fn normalize_server_fn_bad_request(req: Request, next: Next) -> Response {
    let response = next.run(req).await;
    let status = response.status();

    if status != StatusCode::INTERNAL_SERVER_ERROR {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let body_bytes = match to_bytes(body, MAX_ERROR_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return Response::from_parts(parts, Body::empty()),
    };
    let body_text = String::from_utf8_lossy(&body_bytes);

    if is_server_fn_input_deserialize_error(status, &body_text) {
        parts.status = StatusCode::BAD_REQUEST;
    }

    Response::from_parts(parts, Body::from(body_bytes))
}

/// Returns the server's running build identifier as plaintext, with
/// aggressive no-cache headers so the client always sees the live value.
///
/// Deliberately a raw Axum handler (not a Dioxus server function): the
/// drift check must survive version skew in the server-fn wire format,
/// which is the very thing it guards against.
#[cfg(all(feature = "server", any(not(feature = "desktop"), test)))]
pub(crate) async fn current_build() -> impl axum::response::IntoResponse {
    use axum::http::header;
    (
        [
            (header::CACHE_CONTROL, "no-store, max-age=0"),
            (header::PRAGMA, "no-cache"),
            (header::EXPIRES, "0"),
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
        ],
        crate::version::version().to_string(),
    )
}

#[get("/health")]
pub(crate) async fn health() -> Result<(), ServerFnError> {
    tracing::debug!("backend: health");
    Ok(())
}

#[get("/_app/instance-notice")]
pub(crate) async fn instance_notice_html() -> Result<Option<String>, ServerFnError> {
    #[cfg(all(feature = "server", not(feature = "desktop")))]
    {
        Ok(crate::instance_notice::cached_html())
    }
    #[cfg(any(not(feature = "server"), feature = "desktop"))]
    {
        Ok(None)
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn deserialize_error_marker_maps_to_bad_request() {
        assert!(is_server_fn_input_deserialize_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "error deserializing server function arguments: expected value",
        ));
    }

    #[test]
    fn deserialize_results_marker_maps_to_bad_request() {
        assert!(is_server_fn_input_deserialize_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "error deserializing server function results: EOF while parsing an object",
        ));
    }

    #[test]
    fn missing_argument_marker_maps_to_bad_request() {
        assert!(is_server_fn_input_deserialize_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "missing argument username",
        ));
    }

    #[test]
    fn non_deserialize_internal_error_stays_internal() {
        assert!(!is_server_fn_input_deserialize_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database is unavailable",
        ));
    }

    #[test]
    fn non_internal_status_never_maps() {
        assert!(!is_server_fn_input_deserialize_error(
            StatusCode::BAD_REQUEST,
            "error deserializing server function arguments",
        ));
    }
}
