use dioxus::fullstack::{AsStatusCode, StatusCode};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "server")]
use crate::wallets::{
    AccessorKind, AccountKind, AddressScheme, AddressSourceType, IdentitySource, KeyRole,
    KeySource, Network, SyncedAssetId,
};

#[cfg(feature = "server")]
use super::session_context::{require_initialized_session, require_session_token};
#[cfg(feature = "server")]
use crate::amounts::{UnsignedAmount, format_unsigned_amount_fixed};
#[cfg(feature = "server")]
use crate::asset_views::ManualAssetInstanceIdView;
#[cfg(feature = "server")]
use crate::db::{
    AccountSyncSlotRecord, ExportAccountBoundaryMode, ExportAccountRow,
    ExportManualAssetBalanceAssertionRow, ImportDuplicateSkipView, ImportGlobalDuplicateSkipView,
    ImportNativeAccountView, WalletDataImportDbError, WalletDataImportResult,
    WalletDataImportSettings, extract_import_settings, has_api_key,
    import_wallet_data as import_wallet_data_db, list_all_api_keys, list_wallets,
    load_account_sync_slots, load_all_accounts_for_export,
    load_all_manual_asset_balance_assertion_rows_for_export, load_settings, save_api_key,
    save_currency, save_date_time_format, save_etherscan_api_key, save_etherscan_base_url,
    save_hledger_account_prefix, save_language, save_mempool_base_url, save_number_format,
    save_session_duration, save_timezone, with_db, with_user_db,
};
#[cfg(feature = "server")]
#[cfg(feature = "server")]
use crate::i18n::Locale;
#[cfg(feature = "server")]
use crate::models::{
    ApiKeyProvider, CurrencyCode, DateTimeFormat, EtherscanBaseUrl, HledgerAccountPrefix,
    MempoolBaseUrl, NumberFormat, RawEtherscanApiKey, SessionDuration, SessionToken, SimpleApiKey,
    UserSettings,
};
#[cfg(feature = "server")]
use crate::payments::types::{
    PaymentAmount, PaymentOrderId, PaymentOrderStatus, PaymentSecret, ProductTier,
    SubscriptionSubjectId, TokenId,
};
#[cfg(feature = "server")]
use crate::sync_control::sync_control_mode;
#[cfg(feature = "server")]
use crate::tasks::automatic_sync::should_enqueue_automatic_add_sync;
#[cfg(feature = "server")]
use crate::tasks::automatic_sync::{AutomaticSyncAddTarget, automatic_add_sync_scope};
#[cfg(feature = "server")]
use crate::tasks::{
    JobId, JobKey, TriggerEnqueueResult, TriggerParams, TriggerRequest, TriggerSource,
    UserTransactionMonitorParams, enqueue_trigger, ensure_started,
};
#[cfg(feature = "server")]
use crate::transactions::{TransactionSyncRunId, TransactionSyncScope};
#[cfg(feature = "server")]
use crate::wallets::{WalletAccountId, WalletId, WalletWithDetails};
#[cfg(feature = "server")]
use axum_extra::extract::cookie::CookieJar;
#[cfg(feature = "server")]
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
#[cfg(feature = "server")]
use chrono::{DateTime, NaiveDate, Utc};
#[cfg(feature = "server")]
use dioxus::logger::tracing;
#[cfg(feature = "server")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "server")]
use std::io::{Cursor, Read, Write};
#[cfg(feature = "server")]
use std::str::FromStr;
#[cfg(feature = "server")]
use zeroize::Zeroizing;
#[cfg(feature = "server")]
use zip::{CompressionMethod, ZipArchive, ZipWriter, result::ZipError};

#[cfg(feature = "server")]
const MAX_IMPORT_PAYLOAD_BYTES: usize = 10 * 1024 * 1024;
#[cfg(feature = "server")]
const MAX_IMPORT_PAYLOAD_BASE64_BYTES: usize = MAX_IMPORT_PAYLOAD_BYTES.div_ceil(3) * 4 + 4;
#[cfg(feature = "server")]
const WALLET_DATA_INNER_ENTRY_NAME: &str = "wallet-data.json";
#[cfg(feature = "server")]
const BAD_WALLET_DATA_JSON_MESSAGE: &str =
    "The selected file is not a valid BitGarth wallet data export.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportVersion(u16);

#[cfg(feature = "server")]
impl WalletDataExportVersion {
    pub(crate) const V3: Self = Self(3);
    pub(crate) const V4: Self = Self(4);
    pub(crate) const V5: Self = Self(5);
}

#[cfg(feature = "server")]
const SUPPORTED_WALLET_DATA_EXPORT_VERSIONS: [WalletDataExportVersion; 3] = [
    WalletDataExportVersion::V3,
    WalletDataExportVersion::V4,
    WalletDataExportVersion::V5,
];

#[cfg(feature = "server")]
fn current_wallet_data_export_version() -> WalletDataExportVersion {
    SUPPORTED_WALLET_DATA_EXPORT_VERSIONS[2]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WalletDataExportDownloadView {
    pub(crate) file_name: String,
    pub(crate) zip_base64: String,
    pub(crate) summary: WalletDataExportSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExport {
    pub(crate) version: WalletDataExportVersion,
    pub(crate) exported_at: chrono::DateTime<chrono::Utc>,
    pub(crate) bitgarth_version: String,
    pub(crate) wallets: Vec<WalletDataExportWallet>,
    pub(crate) settings: Option<WalletDataExportSettings>,
    #[serde(default)]
    pub(crate) api_keys: Vec<WalletDataExportApiKey>,
    #[serde(
        rename = "subscription_transfer",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) premium_transfer: Option<WalletDataExportPremiumTransfer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportApiKey {
    pub(crate) provider: String,
    pub(crate) api_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportWallet {
    pub(crate) label: String,
    pub(crate) master_fingerprint: Option<String>,
    pub(crate) identity_source: IdentitySource,
    pub(crate) verified_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) accessors: Vec<WalletDataExportAccessor>,
    pub(crate) digital_asset_accounts: Vec<WalletDataExportDigitalAssetAccount>,
    #[serde(default)]
    pub(crate) manual_asset_accounts: Vec<WalletDataExportManualAssetAccount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportAccessor {
    pub(crate) accessor_kind: AccessorKind,
    pub(crate) accessor_label: Option<String>,
    pub(crate) device_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportDigitalAssetAccount {
    pub(crate) label: String,
    pub(crate) asset_id: SyncedAssetId,
    pub(crate) network: Network,
    pub(crate) account_kind: AccountKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sync_slot: Option<WalletDataExportSyncSlot>,
    pub(crate) hd_keys: Vec<WalletDataExportHdKey>,
    pub(crate) addresses: Vec<WalletDataExportAddress>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportSyncSlot {
    pub(crate) selected_at: chrono::DateTime<chrono::Utc>,
    pub(crate) selected_under_tier: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportHdKey {
    pub(crate) key_role: KeyRole,
    pub(crate) extended_pubkey: String,
    pub(crate) derivation_purpose: u32,
    pub(crate) derivation_coin_type: u32,
    pub(crate) derivation_account: u32,
    pub(crate) address_scheme: AddressScheme,
    pub(crate) key_source: KeySource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportAddress {
    pub(crate) address: String,
    pub(crate) address_scheme: AddressScheme,
    pub(crate) source_type: AddressSourceType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportManualAssetAccount {
    pub(crate) label: String,
    pub(crate) asset_instance_id: ManualAssetInstanceIdView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) created_at: Option<DateTime<Utc>>,
    pub(crate) unit_code: String,
    pub(crate) decimal_precision: u8,
    pub(crate) symbol: Option<String>,
    pub(crate) asset_name: String,
    pub(crate) network_name: String,
    pub(crate) coingecko_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) asset_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) precision_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) coingecko_platform_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider_platform_asset_ref: Option<String>,
    pub(crate) balance_assertions: Vec<WalletDataExportBalanceAssertion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportBalanceAssertion {
    pub(crate) asserted_on: chrono::NaiveDate,
    pub(crate) balance_amount: String,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WalletDataExportSummary {
    pub(crate) wallets: u32,
    pub(crate) native_accounts: u32,
    pub(crate) addresses: u32,
    pub(crate) custom_accounts: u32,
    pub(crate) balance_assertions: u32,
    pub(crate) api_keys: u32,
    pub(crate) settings_exported: bool,
    pub(crate) premium_transfer_exported: bool,
    pub(crate) encrypted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportSettings {
    pub(crate) language: Option<String>,
    pub(crate) date_time_format: Option<String>,
    pub(crate) number_format: Option<String>,
    pub(crate) currency: Option<String>,
    pub(crate) timezone: Option<String>,
    pub(crate) session_duration: Option<String>,
    pub(crate) mempool_base_url: Option<String>,
    pub(crate) etherscan_base_url: Option<String>,
    pub(crate) hledger_account_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportPremiumTransfer {
    pub(crate) exported_at: chrono::DateTime<chrono::Utc>,
    pub(crate) management_secret: String,
    pub(crate) active_token: Option<String>,
    pub(crate) token_id: Option<String>,
    pub(crate) subscription_subject_id: Option<String>,
    pub(crate) subscription_valid_until: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) token_issued_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) orders: Vec<WalletDataExportPremiumOrder>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(feature = "server")]
pub(crate) struct WalletDataExportPremiumOrder {
    pub(crate) order_id: String,
    pub(crate) product_tier: String,
    pub(crate) order_amount_minor_units: u64,
    pub(crate) order_currency: String,
    #[serde(rename = "order_display_scale")]
    pub(crate) order_decimal_precision: u8,
    pub(crate) status: String,
    pub(crate) paid_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ExportWalletDataRequest {
    pub(crate) include_premium_transfer: bool,
    pub(crate) encrypted: bool,
    pub(crate) password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WalletDataExportOptionsView {
    pub(crate) premium_transfer_available: bool,
    pub(crate) counts: WalletDataExportCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WalletDataExportCounts {
    pub(crate) wallets: u32,
    pub(crate) native_accounts: u32,
    pub(crate) addresses: u32,
    pub(crate) custom_accounts: u32,
    pub(crate) balance_assertions: u32,
    pub(crate) api_keys: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ImportWalletDataRequest {
    pub(crate) file_name: String,
    pub(crate) payload_base64: String,
    pub(crate) password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DescribeWalletDataImportRequest {
    pub(crate) file_name: String,
    pub(crate) payload_base64: String,
    pub(crate) password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WalletDataImportDescription {
    pub(crate) file_version: u16,
    pub(crate) has_subscription_transfer: bool,
    pub(crate) api_keys_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConfirmPremiumTransferRequest {
    pub(crate) pending_transfer_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PremiumTransferResultView {
    pub(crate) status: PremiumTransferStatusView,
    pub(crate) paid_through: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) offline_access_until: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PremiumTransferStatusView {
    Active,
    RetryableFailure,
    NonRetryableFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ImportAccountCreatedView {
    pub(crate) wallet_label: String,
    pub(crate) account_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ImportAccountMatchedView {
    pub(crate) wallet_label: String,
    pub(crate) account_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DuplicateSkipView {
    pub(crate) identifier_kind: String,
    pub(crate) identifier: String,
    pub(crate) wallet_label: String,
    pub(crate) account_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GlobalDuplicateSkipView {
    pub(crate) identifier_kind: String,
    pub(crate) identifier: String,
    pub(crate) existing_wallet_label: String,
    pub(crate) existing_account_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ImportResultView {
    pub(crate) wallets_created: Vec<String>,
    pub(crate) wallets_matched: Vec<String>,
    pub(crate) native_accounts_created: Vec<ImportAccountCreatedView>,
    pub(crate) native_accounts_matched: Vec<ImportAccountMatchedView>,
    pub(crate) duplicate_skips: Vec<DuplicateSkipView>,
    pub(crate) global_duplicate_skips: Vec<GlobalDuplicateSkipView>,
    pub(crate) assertions_created: u32,
    pub(crate) assertions_skipped: u32,
    pub(crate) validation_warnings: Vec<String>,
    pub(crate) sync_triggered: bool,
    pub(crate) sync_scope: String,
    pub(crate) settings_imported: bool,
    pub(crate) api_keys_imported: u32,
    pub(crate) api_keys_skipped_already_present: u32,
    pub(crate) premium_transfer_status: PremiumTransferImportStatusView,
    pub(crate) pending_premium_transfer_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PremiumTransferImportStatusView {
    NotPresent,
    PendingConfirmation,
    InvalidMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ExportError {
    BadRequest(String),
    Validation(String),
    PasswordRequired(String),
    EncryptedZipAuthFailed(String),
    Unauthorized(String),
    Internal(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::BadRequest(message) => write!(f, "Bad request: {message}"),
            ExportError::Validation(message) => write!(f, "Validation failed: {message}"),
            ExportError::PasswordRequired(message) => write!(f, "Password required: {message}"),
            ExportError::EncryptedZipAuthFailed(message) => {
                write!(f, "Encrypted ZIP authentication failed: {message}")
            }
            ExportError::Unauthorized(message) => write!(f, "Unauthorized: {message}"),
            ExportError::Internal(message) => write!(f, "Internal error: {message}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl AsStatusCode for ExportError {
    fn as_status_code(&self) -> StatusCode {
        match self {
            ExportError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ExportError::Validation(_)
            | ExportError::PasswordRequired(_)
            | ExportError::EncryptedZipAuthFailed(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ExportError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ExportError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<ServerFnError> for ExportError {
    fn from(value: ServerFnError) -> Self {
        match value {
            ServerFnError::Args(message)
            | ServerFnError::MissingArg(message)
            | ServerFnError::Deserialization(message)
            | ServerFnError::Serialization(message) => ExportError::BadRequest(message),
            ServerFnError::ServerError {
                message, code: 400, ..
            } => ExportError::BadRequest(message),
            ServerFnError::ServerError {
                message, code: 422, ..
            } => ExportError::Validation(message),
            other => ExportError::Internal(other.to_string()),
        }
    }
}

#[cfg(feature = "server")]
fn session_token_from_cookie(cookies: &CookieJar) -> Result<SessionToken, ExportError> {
    require_session_token("exports", cookies, ExportError::Unauthorized)
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WalletDataManualAssetAccountExportSource {
    account_id: WalletAccountId,
    wallet_id: WalletId,
    label: String,
    asset_instance_id: ManualAssetInstanceIdView,
    created_at: DateTime<Utc>,
    unit_code: String,
    decimal_precision: u8,
    symbol: Option<String>,
    asset_name: String,
    network_name: String,
    coingecko_id: String,
    asset_source: Option<String>,
    precision_source: Option<String>,
    coingecko_platform_id: Option<String>,
    provider_platform_asset_ref: Option<String>,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WalletDataManualAssertionExportSource {
    account_id: WalletAccountId,
    asserted_on: NaiveDate,
    asserted_balance: UnsignedAmount,
    note: Option<String>,
}

#[cfg(feature = "server")]
struct WalletDataExportBuildInput<'a> {
    exported_at: DateTime<Utc>,
    bitgarth_version: &'a str,
    username: &'a str,
    wallets: Vec<WalletWithDetails>,
    manual_asset_accounts: Vec<WalletDataManualAssetAccountExportSource>,
    manual_assertions: Vec<WalletDataManualAssertionExportSource>,
    sync_slots: Vec<AccountSyncSlotRecord>,
    user_settings: Option<UserSettings>,
    api_keys: Vec<(ApiKeyProvider, SimpleApiKey)>,
    premium_transfer: Option<WalletDataExportPremiumTransfer>,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WalletDataExportPayloadView {
    file_name: String,
    payload: WalletDataExport,
    summary: WalletDataExportSummary,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WalletDataZipPayload {
    payload_json: String,
    inner_file_name: String,
    encrypted: bool,
}

#[cfg(feature = "server")]
fn parse_manual_asset_accounts_for_wallet_data(
    export_account_rows: &[ExportAccountRow],
) -> Result<Vec<WalletDataManualAssetAccountExportSource>, ExportError> {
    let mut result = Vec::new();
    for row in export_account_rows {
        if row.boundary_mode != ExportAccountBoundaryMode::ManualAsset {
            continue;
        }
        let asset_instance_id = row.manual_asset_instance_id.clone().ok_or_else(|| {
            ExportError::Internal(format!(
                "Manual asset export row {} missing asset_instance_id",
                row.account_id
            ))
        })?;
        result.push(WalletDataManualAssetAccountExportSource {
            account_id: row.account_id,
            wallet_id: row.wallet_id,
            label: row.account_label.as_str().to_string(),
            asset_instance_id,
            created_at: row.created_at,
            unit_code: row.commodity.unit_code.clone(),
            decimal_precision: row.commodity.decimal_precision,
            symbol: row.manual_symbol.clone(),
            asset_name: row.manual_asset_name.clone().ok_or_else(|| {
                ExportError::Internal(format!(
                    "Manual asset export row {} missing asset_name",
                    row.account_id
                ))
            })?,
            network_name: row.manual_network_name.clone().ok_or_else(|| {
                ExportError::Internal(format!(
                    "Manual asset export row {} missing network_name",
                    row.account_id
                ))
            })?,
            coingecko_id: row.manual_coingecko_id.clone().ok_or_else(|| {
                ExportError::Internal(format!(
                    "Manual asset export row {} missing coingecko_id",
                    row.account_id
                ))
            })?,
            asset_source: row.manual_asset_source.clone(),
            precision_source: row.manual_precision_source.clone(),
            coingecko_platform_id: row.manual_coingecko_platform_id.clone(),
            provider_platform_asset_ref: row.manual_provider_platform_asset_ref.clone(),
        });
    }
    Ok(result)
}

#[cfg(feature = "server")]
fn parse_manual_assertions_for_wallet_data(
    assertion_rows: Vec<ExportManualAssetBalanceAssertionRow>,
) -> Vec<WalletDataManualAssertionExportSource> {
    assertion_rows
        .into_iter()
        .map(|row| WalletDataManualAssertionExportSource {
            account_id: row.account_id,
            asserted_on: row.asserted_on,
            asserted_balance: row.asserted_balance,
            note: row.note,
        })
        .collect()
}

#[cfg(feature = "server")]
fn count_to_u32(value: usize, label: &'static str) -> Result<u32, ExportError> {
    u32::try_from(value)
        .map_err(|_| ExportError::Internal(format!("Export {label} count exceeds u32")))
}

#[cfg(feature = "server")]
fn build_wallet_data_export_counts(
    wallets: &[WalletWithDetails],
    manual_asset_accounts: &[WalletDataManualAssetAccountExportSource],
    manual_assertions: &[WalletDataManualAssertionExportSource],
    api_key_count: usize,
) -> Result<WalletDataExportCounts, ExportError> {
    let exportable_account_ids = manual_asset_accounts
        .iter()
        .map(|account| account.account_id)
        .collect::<HashSet<_>>();
    let native_accounts_count = wallets
        .iter()
        .map(|wallet| wallet.accounts.len())
        .sum::<usize>();
    let addresses_count = wallets
        .iter()
        .flat_map(|wallet| wallet.accounts.iter())
        .flat_map(|account| account.addresses.iter())
        .filter(|address| {
            !matches!(
                address.source_type,
                AddressSourceType::Derived | AddressSourceType::Observed
            )
        })
        .count();
    let balance_assertions_count = manual_assertions
        .iter()
        .filter(|assertion| exportable_account_ids.contains(&assertion.account_id))
        .count();
    let total_custom_accounts = manual_asset_accounts.len();

    Ok(WalletDataExportCounts {
        wallets: count_to_u32(wallets.len(), "wallet")?,
        native_accounts: count_to_u32(native_accounts_count, "native account")?,
        addresses: count_to_u32(addresses_count, "address")?,
        custom_accounts: count_to_u32(total_custom_accounts, "custom account")?,
        balance_assertions: count_to_u32(balance_assertions_count, "balance assertion")?,
        api_keys: count_to_u32(api_key_count, "api key")?,
    })
}

#[cfg(feature = "server")]
fn count_wallet_data_export_rows(
    user_id: crate::models::UserId,
    sql: &'static str,
) -> Result<u32, ExportError> {
    let count = with_user_db(user_id, |conn| {
        conn.query_row(sql, [], |row| row.get::<_, i64>(0)).map_err(|err| {
            crate::db::DbError::from_rusqlite_error(
                "Failed to count wallet-data export rows",
                err,
            )
        })
    })
    .map_err(|err| {
        tracing::error!(user_id = %user_id, error = %err, "exports: failed to count wallet-data export rows");
        ExportError::Internal(format!("Failed to count wallet-data export rows: {err}"))
    })?;
    u32::try_from(count)
        .map_err(|_| ExportError::Internal("Wallet-data export row count exceeds u32".to_string()))
}

#[cfg(feature = "server")]
fn count_wallet_data_export_api_keys(user_id: crate::models::UserId) -> Result<u32, ExportError> {
    let blank_known_provider_rows = count_wallet_data_export_rows(
        user_id,
        "SELECT COUNT(*)
         FROM api_keys
         WHERE provider IN ('etherscan', 'coingecko')
           AND TRIM(api_key) = ''",
    )?;
    if blank_known_provider_rows > 0 {
        tracing::error!(
            user_id = %user_id,
            blank_known_provider_rows,
            "exports: failed to count API keys because stored API-key metadata is invalid"
        );
        return Err(ExportError::Internal(
            "Failed to count API keys for wallet-data export options".to_string(),
        ));
    }

    count_wallet_data_export_rows(
        user_id,
        "SELECT COUNT(*)
         FROM api_keys
         WHERE provider IN ('etherscan', 'coingecko')
           AND TRIM(api_key) <> ''",
    )
}

#[cfg(feature = "server")]
fn load_wallet_data_export_counts(
    user_id: crate::models::UserId,
) -> Result<WalletDataExportCounts, ExportError> {
    let manual_asset_accounts =
        count_wallet_data_export_rows(user_id, "SELECT COUNT(*) FROM manual_asset_accounts")?;
    let manual_balance_assertions = count_wallet_data_export_rows(
        user_id,
        "SELECT COUNT(*)
         FROM manual_asset_balance_assertions b
         JOIN manual_asset_accounts a ON a.id = b.account_id",
    )?;

    Ok(WalletDataExportCounts {
        wallets: count_wallet_data_export_rows(user_id, "SELECT COUNT(*) FROM wallets")?,
        native_accounts: count_wallet_data_export_rows(
            user_id,
            "SELECT COUNT(*) FROM digital_asset_accounts",
        )?,
        addresses: count_wallet_data_export_rows(
            user_id,
            "SELECT COUNT(*)
             FROM digital_asset_addresses
             WHERE source_type NOT IN ('derived', 'observed')",
        )?,
        custom_accounts: manual_asset_accounts,
        balance_assertions: manual_balance_assertions,
        api_keys: count_wallet_data_export_api_keys(user_id)?,
    })
}

#[cfg(feature = "server")]
fn wallet_data_file_name(username: &str, exported_at: DateTime<Utc>) -> String {
    format!(
        "bitgarth-walletdata-{}-{}.zip",
        username,
        exported_at.format("%Y%m%d")
    )
}

#[cfg(feature = "server")]
fn zip_io_error(message: impl Into<String>, err: impl std::fmt::Display) -> ExportError {
    ExportError::Internal(format!("{}: {err}", message.into()))
}

#[cfg(feature = "server")]
fn wrap_in_wallet_data_zip(
    payload_json: &str,
    password: Option<&str>,
) -> Result<Vec<u8>, ExportError> {
    let cursor = Cursor::new(Vec::new());
    let mut archive = ZipWriter::new(cursor);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let options = if let Some(password) = password {
        options.with_aes_encryption(zip::AesMode::Aes256, password)
    } else {
        options
    };

    archive
        .start_file(WALLET_DATA_INNER_ENTRY_NAME, options)
        .map_err(|err| zip_io_error("Failed to start wallet-data ZIP entry", err))?;
    archive
        .write_all(payload_json.as_bytes())
        .map_err(|err| zip_io_error("Failed to write wallet-data ZIP entry", err))?;
    let cursor = archive
        .finish()
        .map_err(|err| zip_io_error("Failed to finish wallet-data ZIP", err))?;
    Ok(cursor.into_inner())
}

#[cfg(feature = "server")]
fn read_zip_entry_to_string<R: Read>(
    entry: &mut R,
    encrypted: bool,
) -> Result<String, ExportError> {
    let mut limited = entry.take((MAX_IMPORT_PAYLOAD_BYTES + 1) as u64);
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes).map_err(|err| {
        if encrypted {
            ExportError::EncryptedZipAuthFailed(
                "Wrong password or damaged encrypted file. Check the password and try again."
                    .to_string(),
            )
        } else {
            ExportError::Validation(format!("Failed to read wallet-data ZIP entry: {err}"))
        }
    })?;
    if bytes.len() > MAX_IMPORT_PAYLOAD_BYTES {
        return Err(ExportError::Validation(format!(
            "Wallet-data import file is too large ({} bytes). Maximum allowed is {} bytes.",
            bytes.len(),
            MAX_IMPORT_PAYLOAD_BYTES
        )));
    }
    String::from_utf8(bytes).map_err(|_| {
        ExportError::Validation(
            "The selected file does not contain a valid wallet-data export.".to_string(),
        )
    })
}

#[cfg(feature = "server")]
fn zip_archive_error(err: ZipError) -> ExportError {
    match err {
        ZipError::InvalidPassword => ExportError::EncryptedZipAuthFailed(
            "Wrong password or damaged encrypted file. Check the password and try again."
                .to_string(),
        ),
        ZipError::UnsupportedArchive(message) if message == ZipError::PASSWORD_REQUIRED => {
            ExportError::PasswordRequired(
                "Enter the password used when this file was exported.".to_string(),
            )
        }
        other => ExportError::Validation(format!(
            "The selected file is not a valid ZIP archive. Re-export and try again. ({other})"
        )),
    }
}

#[cfg(feature = "server")]
fn unwrap_wallet_data_zip(
    zip_bytes: &[u8],
    password: Option<&str>,
) -> Result<WalletDataZipPayload, ExportError> {
    let cursor = Cursor::new(zip_bytes);
    let mut archive = ZipArchive::new(cursor).map_err(zip_archive_error)?;
    if archive.len() != 1 {
        return Err(ExportError::Validation(
            "The selected file does not contain a valid wallet-data export.".to_string(),
        ));
    }

    let (entry_name, encrypted) = {
        let entry = archive.by_index_raw(0).map_err(zip_archive_error)?;
        let entry_name = entry.name().to_string();
        if entry.is_dir() || entry.is_symlink() {
            return Err(ExportError::Validation(
                "The selected file does not contain a valid wallet-data export.".to_string(),
            ));
        }
        (entry_name, entry.encrypted())
    };

    if entry_name != WALLET_DATA_INNER_ENTRY_NAME {
        return Err(ExportError::Validation(
            "The selected file does not contain a valid wallet-data export.".to_string(),
        ));
    }

    let payload_json = if encrypted {
        let password = password.filter(|value| !value.is_empty()).ok_or_else(|| {
            ExportError::PasswordRequired(
                "Enter the password used when this file was exported.".to_string(),
            )
        })?;
        let mut entry = archive
            .by_index_decrypt(0, password.as_bytes())
            .map_err(zip_archive_error)?;
        read_zip_entry_to_string(&mut entry, true)?
    } else {
        let mut entry = archive.by_index(0).map_err(zip_archive_error)?;
        read_zip_entry_to_string(&mut entry, false)?
    };

    Ok(WalletDataZipPayload {
        payload_json,
        inner_file_name: entry_name,
        encrypted,
    })
}

#[cfg(feature = "server")]
fn password_is_nonempty(password: Option<&str>) -> bool {
    password.is_some_and(|value| !value.is_empty())
}

#[cfg(feature = "server")]
fn unwrap_wallet_data_payload(
    bytes: &[u8],
    password: Option<&str>,
) -> Result<WalletDataZipPayload, ExportError> {
    if bytes.starts_with(b"PK\x03\x04") {
        return unwrap_wallet_data_zip(bytes, password);
    }

    if bytes.len() > MAX_IMPORT_PAYLOAD_BYTES {
        return Err(ExportError::Validation(format!(
            "Wallet-data import file is too large ({} bytes). Maximum allowed is {} bytes.",
            bytes.len(),
            MAX_IMPORT_PAYLOAD_BYTES
        )));
    }

    let first_non_whitespace = bytes.iter().find(|byte| !byte.is_ascii_whitespace());
    if first_non_whitespace == Some(&b'{') {
        if password_is_nonempty(password) {
            return Err(ExportError::Validation(
                "Cannot decrypt a raw JSON file. Upload the original ZIP if it was encrypted."
                    .to_string(),
            ));
        }
        let payload_json = String::from_utf8(bytes.to_vec()).map_err(|_| {
            ExportError::Validation(
                "The selected file does not contain a valid wallet-data export.".to_string(),
            )
        })?;
        return Ok(WalletDataZipPayload {
            payload_json,
            inner_file_name: WALLET_DATA_INNER_ENTRY_NAME.to_string(),
            encrypted: false,
        });
    }

    Err(ExportError::Validation(
        "Upload a wallet-data .zip or .json file.".to_string(),
    ))
}

#[cfg(feature = "server")]
fn translate_v3_to_v4_payload(value: &mut serde_json::Value) -> Result<(), ExportError> {
    let object = value.as_object_mut().ok_or_else(|| {
        ExportError::Validation("Wallet data export payload must be a JSON object.".to_string())
    })?;
    if object.get("version").and_then(serde_json::Value::as_u64) != Some(3) {
        return Ok(());
    }

    let legacy_etherscan_api_key = object
        .get_mut("settings")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|settings| settings.remove("etherscan_api_key"))
        .and_then(|value| match value {
            serde_json::Value::String(raw) => {
                let trimmed = raw.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }
            _ => None,
        });

    if let Some(premium_transfer) = object.remove("premium_transfer") {
        object.insert("subscription_transfer".to_string(), premium_transfer);
    }

    let api_keys_value = object
        .entry("api_keys".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let Some(api_keys) = api_keys_value.as_array_mut() else {
        return Err(ExportError::Validation(
            "Wallet data export api_keys field must be an array.".to_string(),
        ));
    };
    if let Some(api_key) = legacy_etherscan_api_key {
        api_keys.push(serde_json::json!({
            "provider": "etherscan",
            "api_key": api_key,
        }));
    }

    object.insert("version".to_string(), serde_json::json!(4));
    Ok(())
}

#[cfg(feature = "server")]
fn parse_wallet_data_json_value(payload_json: &str) -> Result<serde_json::Value, ExportError> {
    serde_json::from_str(payload_json)
        .map_err(|_| ExportError::BadRequest(BAD_WALLET_DATA_JSON_MESSAGE.to_string()))
}

#[cfg(feature = "server")]
fn translated_wallet_data_json(payload_json: &str) -> Result<String, ExportError> {
    let mut value = parse_wallet_data_json_value(payload_json)?;
    translate_v3_to_v4_payload(&mut value)?;
    serde_json::to_string(&value).map_err(|err| {
        ExportError::Internal(format!(
            "Failed to serialize translated wallet-data import: {err}"
        ))
    })
}

#[cfg(feature = "server")]
fn describe_wallet_data_value(
    value: &serde_json::Value,
) -> Result<WalletDataImportDescription, ExportError> {
    let object = value.as_object().ok_or_else(|| {
        ExportError::Validation("Wallet data export payload must be a JSON object.".to_string())
    })?;
    let file_version = object
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u16::try_from(version).ok())
        .ok_or_else(|| {
            ExportError::Validation(
                "Wallet data export is missing a valid numeric version field.".to_string(),
            )
        })?;
    let api_keys_count = match object.get("api_keys") {
        Some(serde_json::Value::Array(api_keys)) => count_to_u32(api_keys.len(), "api key")?,
        Some(_) => {
            return Err(ExportError::Validation(
                "Wallet data export api_keys field must be an array.".to_string(),
            ));
        }
        None => 0,
    };

    Ok(WalletDataImportDescription {
        file_version,
        has_subscription_transfer: object.contains_key("subscription_transfer"),
        api_keys_count,
    })
}

#[cfg(feature = "server")]
fn validate_wallet_data_import_file_name(file_name: &str) -> Result<(), ExportError> {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".zip") || lower.ends_with(".json") {
        return Ok(());
    }

    Err(ExportError::Validation(
        "Upload a wallet-data .zip or .json file.".to_string(),
    ))
}

#[cfg(feature = "server")]
fn decode_wallet_data_zip_base64(zip_base64: &str) -> Result<Vec<u8>, ExportError> {
    if zip_base64.len() > MAX_IMPORT_PAYLOAD_BASE64_BYTES {
        return Err(ExportError::Validation(format!(
            "Wallet-data import file is too large. Maximum allowed is {} bytes.",
            MAX_IMPORT_PAYLOAD_BYTES
        )));
    }
    let zip_bytes = BASE64.decode(zip_base64).map_err(|_| {
        ExportError::Validation("Wallet-data import file is not valid base64.".to_string())
    })?;
    if zip_bytes.len() > MAX_IMPORT_PAYLOAD_BYTES {
        return Err(ExportError::Validation(format!(
            "Wallet-data import file is too large ({} bytes). Maximum allowed is {} bytes.",
            zip_bytes.len(),
            MAX_IMPORT_PAYLOAD_BYTES
        )));
    }
    Ok(zip_bytes)
}

#[cfg(feature = "server")]
fn build_wallet_data_export_payload_view(
    input: WalletDataExportBuildInput<'_>,
) -> Result<WalletDataExportPayloadView, ExportError> {
    let export_counts = build_wallet_data_export_counts(
        &input.wallets,
        &input.manual_asset_accounts,
        &input.manual_assertions,
        input.api_keys.len(),
    )?;

    let mut manual_account_scale_by_id = HashMap::<WalletAccountId, u8>::new();
    let mut manual_asset_accounts_by_wallet =
        HashMap::<WalletId, Vec<WalletDataManualAssetAccountExportSource>>::new();
    for manual_account in input.manual_asset_accounts {
        manual_account_scale_by_id
            .insert(manual_account.account_id, manual_account.decimal_precision);
        manual_asset_accounts_by_wallet
            .entry(manual_account.wallet_id)
            .or_default()
            .push(manual_account);
    }
    let sync_slots_by_account = input
        .sync_slots
        .into_iter()
        .map(|slot| (slot.account_id, slot))
        .collect::<HashMap<_, _>>();

    let mut assertions_by_account =
        HashMap::<WalletAccountId, Vec<WalletDataExportBalanceAssertion>>::new();
    for assertion in input.manual_assertions {
        let decimal_precision = manual_account_scale_by_id
            .get(&assertion.account_id)
            .copied()
            .ok_or_else(|| {
                ExportError::Internal(format!(
                    "Manual balance assertion references missing manual account {}",
                    assertion.account_id
                ))
            })?;
        let balance_amount =
            format_unsigned_amount_fixed(assertion.asserted_balance, decimal_precision);

        assertions_by_account
            .entry(assertion.account_id)
            .or_default()
            .push(WalletDataExportBalanceAssertion {
                asserted_on: assertion.asserted_on,
                balance_amount,
                note: assertion.note,
            });
    }

    let mut wallets_payload = Vec::with_capacity(input.wallets.len());
    for wallet in input.wallets {
        let accessors = wallet
            .accessors
            .into_iter()
            .map(|accessor| WalletDataExportAccessor {
                accessor_kind: accessor.accessor_kind,
                accessor_label: accessor
                    .accessor_label
                    .map(|value| value.as_str().to_string()),
                device_model: accessor.device_model,
            })
            .collect::<Vec<_>>();

        let digital_asset_accounts = wallet
            .accounts
            .into_iter()
            .map(|account| {
                let hd_keys = account
                    .hd_keys
                    .into_iter()
                    .map(|key| WalletDataExportHdKey {
                        key_role: key.key_role,
                        extended_pubkey: key.extended_pubkey.as_str().to_string(),
                        derivation_purpose: key.derivation_path.purpose.value(),
                        derivation_coin_type: key.derivation_path.coin_type.value(),
                        derivation_account: key.derivation_path.account.as_u32(),
                        address_scheme: key.address_scheme,
                        key_source: key.key_source,
                    })
                    .collect::<Vec<_>>();

                let addresses = account
                    .addresses
                    .into_iter()
                    .filter(|address| {
                        !matches!(
                            address.source_type,
                            AddressSourceType::Derived | AddressSourceType::Observed
                        )
                    })
                    .map(|address| WalletDataExportAddress {
                        address: address.address,
                        address_scheme: address.address_scheme,
                        source_type: address.source_type,
                    })
                    .collect::<Vec<_>>();

                WalletDataExportDigitalAssetAccount {
                    label: account.label.as_str().to_string(),
                    asset_id: account.asset_id,
                    network: account.network,
                    account_kind: account.account_kind,
                    created_at: Some(account.created_at),
                    sync_slot: sync_slots_by_account.get(&account.id).map(|slot| {
                        WalletDataExportSyncSlot {
                            selected_at: slot.selected_at,
                            selected_under_tier: slot.selected_under_tier.as_str().to_string(),
                        }
                    }),
                    hd_keys,
                    addresses,
                }
            })
            .collect::<Vec<_>>();

        let wallet_manual_asset_accounts = manual_asset_accounts_by_wallet
            .remove(&wallet.wallet.id)
            .unwrap_or_default()
            .into_iter()
            .map(|manual_account| WalletDataExportManualAssetAccount {
                label: manual_account.label,
                asset_instance_id: manual_account.asset_instance_id,
                created_at: Some(manual_account.created_at),
                unit_code: manual_account.unit_code,
                decimal_precision: manual_account.decimal_precision,
                symbol: manual_account.symbol,
                asset_name: manual_account.asset_name,
                network_name: manual_account.network_name,
                coingecko_id: manual_account.coingecko_id,
                asset_source: manual_account.asset_source,
                precision_source: manual_account.precision_source,
                coingecko_platform_id: manual_account.coingecko_platform_id,
                provider_platform_asset_ref: manual_account.provider_platform_asset_ref,
                balance_assertions: assertions_by_account
                    .remove(&manual_account.account_id)
                    .unwrap_or_default(),
            })
            .collect::<Vec<_>>();

        wallets_payload.push(WalletDataExportWallet {
            label: wallet.wallet.label.as_str().to_string(),
            master_fingerprint: wallet
                .wallet
                .master_fingerprint
                .map(|value| value.as_str().to_string()),
            identity_source: wallet.wallet.identity_source,
            verified_at: wallet.wallet.verified_at,
            accessors,
            digital_asset_accounts,
            manual_asset_accounts: wallet_manual_asset_accounts,
        });
    }

    if !manual_asset_accounts_by_wallet.is_empty() {
        return Err(ExportError::Internal(
            "Manual asset account export rows reference unknown wallet ids".to_string(),
        ));
    }

    if !assertions_by_account.is_empty() {
        return Err(ExportError::Internal(
            "Manual balance assertions reference unknown manual account ids".to_string(),
        ));
    }

    let export_settings = input.user_settings.map(|s| WalletDataExportSettings {
        language: s.language.map(|l| l.code().to_string()),
        date_time_format: s.date_time_format.map(|f| f.code().to_string()),
        number_format: s.number_format.map(|f| f.code().to_string()),
        currency: s.currency.map(|c| c.code().to_string()),
        timezone: s.timezone.map(|t| t.name()),
        session_duration: s.session_duration.map(|d| d.code()),
        mempool_base_url: s.mempool_base_url.map(|u| u.as_str().to_string()),
        etherscan_base_url: s.etherscan_base_url.map(|u| u.as_str().to_string()),
        hledger_account_prefix: s.hledger_account_prefix.map(|p| p.as_str().to_string()),
    });

    let api_keys = input
        .api_keys
        .into_iter()
        .map(|(provider, api_key)| WalletDataExportApiKey {
            provider: provider.as_storage_key().to_string(),
            api_key: api_key.as_str().to_string(),
        })
        .collect::<Vec<_>>();

    let summary = WalletDataExportSummary {
        wallets: export_counts.wallets,
        native_accounts: export_counts.native_accounts,
        addresses: export_counts.addresses,
        custom_accounts: export_counts.custom_accounts,
        balance_assertions: export_counts.balance_assertions,
        api_keys: export_counts.api_keys,
        settings_exported: export_settings.is_some(),
        premium_transfer_exported: input.premium_transfer.is_some(),
        encrypted: false,
    };

    let payload = WalletDataExport {
        version: current_wallet_data_export_version(),
        exported_at: input.exported_at,
        bitgarth_version: input.bitgarth_version.to_string(),
        wallets: wallets_payload,
        settings: export_settings,
        api_keys,
        premium_transfer: input.premium_transfer,
    };

    Ok(WalletDataExportPayloadView {
        file_name: wallet_data_file_name(input.username, input.exported_at),
        payload,
        summary,
    })
}

#[cfg(feature = "server")]
fn premium_order_status_string(status: PaymentOrderStatus) -> String {
    status.as_str().to_string()
}

#[cfg(feature = "server")]
fn build_premium_transfer_export(
    user_id: crate::models::UserId,
    exported_at: DateTime<Utc>,
) -> Result<WalletDataExportPremiumTransfer, ExportError> {
    let subject = crate::db::payments::load_payment_subject(user_id)
        .map_err(|err| {
            tracing::error!(user_id = %user_id, error = %err, "exports: failed to load payment subject for wallet-data export");
            ExportError::Internal(format!("Failed to load payment subject for export: {err}"))
        })?
        .ok_or_else(|| {
            ExportError::Validation("Premium transfer data is not available for this user yet.".to_string())
        })?;
    let management_secret = subject.management_secret.ok_or_else(|| {
        ExportError::Validation(
            "Premium transfer data is not available for this user yet.".to_string(),
        )
    })?;
    let orders = crate::db::payments::load_all_payment_order_history(user_id)
        .map_err(|err| {
            tracing::error!(user_id = %user_id, error = %err, "exports: failed to load payment orders for wallet-data export");
            ExportError::Internal(format!("Failed to load payment orders for export: {err}"))
        })?
        .into_iter()
        .map(|order| WalletDataExportPremiumOrder {
            order_id: order.order_id.to_storage_value(),
            product_tier: order.product_tier.as_str().to_string(),
            order_amount_minor_units: order.amount.minor_units,
            order_currency: order.amount.currency,
            order_decimal_precision: order.amount.decimal_precision,
            status: premium_order_status_string(order.status),
            paid_at: order.paid_at,
        })
        .collect();

    // TODO: refactor export flow (Task 10)
    let history = crate::db::payments::load_active_token_history(user_id)
        .map_err(|err| ExportError::Internal(err.to_string()))?;

    Ok(WalletDataExportPremiumTransfer {
        exported_at,
        management_secret: management_secret.as_str().to_string(),
        active_token: history.as_ref().map(|h| h.active_token.clone()),
        token_id: history.as_ref().map(|h| h.token_id.to_storage_value()),
        subscription_subject_id: history
            .as_ref()
            .map(|h| h.subscription_subject_id.to_storage_value()),
        subscription_valid_until: history.as_ref().map(|h| h.subscription_valid_until),
        token_expires_at: history.as_ref().map(|h| h.token_expires_at),
        token_issued_at: history.as_ref().map(|h| h.token_issued_at),
        orders,
    })
}

#[cfg(feature = "server")]
fn map_import_error(error: WalletDataImportDbError) -> ExportError {
    match error {
        WalletDataImportDbError::BadRequest(message) => ExportError::BadRequest(message),
        WalletDataImportDbError::Validation(message) => ExportError::Validation(message),
        WalletDataImportDbError::Internal(message) => {
            tracing::error!(error = %message, "exports: wallet-data import failed");
            ExportError::Internal(message)
        }
    }
}

#[cfg(feature = "server")]
fn to_import_account_created_view(value: ImportNativeAccountView) -> ImportAccountCreatedView {
    ImportAccountCreatedView {
        wallet_label: value.wallet_label,
        account_label: value.account_label,
    }
}

#[cfg(feature = "server")]
fn to_import_account_matched_view(value: ImportNativeAccountView) -> ImportAccountMatchedView {
    ImportAccountMatchedView {
        wallet_label: value.wallet_label,
        account_label: value.account_label,
    }
}

#[cfg(feature = "server")]
fn to_duplicate_skip_view(value: ImportDuplicateSkipView) -> DuplicateSkipView {
    DuplicateSkipView {
        identifier_kind: value.identifier_kind,
        identifier: value.identifier,
        wallet_label: value.wallet_label,
        account_label: value.account_label,
    }
}

#[cfg(feature = "server")]
fn to_global_duplicate_skip_view(value: ImportGlobalDuplicateSkipView) -> GlobalDuplicateSkipView {
    GlobalDuplicateSkipView {
        identifier_kind: value.identifier_kind,
        identifier: value.identifier,
        existing_wallet_label: value.existing_wallet_label,
        existing_account_label: value.existing_account_label,
    }
}

#[cfg(feature = "server")]
fn import_result_view(
    result: WalletDataImportResult,
    sync_triggered: bool,
    settings_imported: bool,
    api_keys_imported: u32,
    api_keys_skipped_already_present: u32,
    premium_transfer_status: PremiumTransferImportStatusView,
    pending_premium_transfer_id: Option<String>,
) -> ImportResultView {
    ImportResultView {
        wallets_created: result.wallets_created,
        wallets_matched: result.wallets_matched,
        native_accounts_created: result
            .native_accounts_created
            .into_iter()
            .map(to_import_account_created_view)
            .collect(),
        native_accounts_matched: result
            .native_accounts_matched
            .into_iter()
            .map(to_import_account_matched_view)
            .collect(),
        duplicate_skips: result
            .duplicate_skips
            .into_iter()
            .map(to_duplicate_skip_view)
            .collect(),
        global_duplicate_skips: result
            .global_duplicate_skips
            .into_iter()
            .map(to_global_duplicate_skip_view)
            .collect(),
        assertions_created: result.assertions_created,
        assertions_skipped: result.assertions_skipped,
        validation_warnings: result.validation_warnings,
        sync_triggered,
        sync_scope: "user".to_string(),
        settings_imported,
        api_keys_imported,
        api_keys_skipped_already_present,
        premium_transfer_status,
        pending_premium_transfer_id,
    }
}

#[cfg(feature = "server")]
#[derive(Debug, Deserialize)]
struct RawImportedApiKey {
    provider: String,
    api_key: String,
}

#[cfg(feature = "server")]
struct ImportedApiKeys {
    valid: Vec<(ApiKeyProvider, SimpleApiKey)>,
    skipped_invalid: u32,
}

#[cfg(feature = "server")]
fn extract_imported_api_keys(payload_json: &str) -> Result<ImportedApiKeys, ExportError> {
    let value = parse_wallet_data_json_value(payload_json)?;
    let Some(rows) = value.get("api_keys").and_then(serde_json::Value::as_array) else {
        return Ok(ImportedApiKeys {
            valid: Vec::new(),
            skipped_invalid: 0,
        });
    };

    let mut valid = Vec::new();
    let mut skipped_invalid = 0_u32;
    for row in rows {
        let parsed = serde_json::from_value::<RawImportedApiKey>(row.clone());
        let Ok(parsed) = parsed else {
            // Newer or edited backups may contain rows this build cannot use.
            skipped_invalid = skipped_invalid.saturating_add(1);
            continue;
        };
        let Some(provider) = ApiKeyProvider::from_storage_key(parsed.provider.trim()) else {
            // Newer or edited backups may contain rows this build cannot use.
            skipped_invalid = skipped_invalid.saturating_add(1);
            continue;
        };
        let Some(api_key) = SimpleApiKey::new(parsed.api_key) else {
            // Newer or edited backups may contain rows this build cannot use.
            skipped_invalid = skipped_invalid.saturating_add(1);
            continue;
        };
        valid.push((provider, api_key));
    }

    Ok(ImportedApiKeys {
        valid,
        skipped_invalid,
    })
}

#[cfg(feature = "server")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawImportedPremiumTransfer {
    #[serde(rename = "exported_at")]
    _exported_at: Option<DateTime<Utc>>,
    management_secret: String,
    active_token: Option<String>,
    token_id: Option<String>,
    subscription_subject_id: Option<String>,
    subscription_valid_until: Option<DateTime<Utc>>,
    token_expires_at: Option<DateTime<Utc>>,
    token_issued_at: Option<DateTime<Utc>>,
    #[serde(default)]
    orders: Vec<RawImportedPremiumOrder>,
}

#[cfg(feature = "server")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawImportedPremiumOrder {
    order_id: String,
    product_tier: ProductTier,
    order_amount_minor_units: u64,
    order_currency: String,
    #[serde(rename = "order_display_scale")]
    order_decimal_precision: u8,
    status: PaymentOrderStatus,
    paid_at: Option<DateTime<Utc>>,
}

#[cfg(feature = "server")]
struct ExtractedPendingPremiumTransfer {
    pending_transfer: crate::db::payments::NewPendingPremiumTransfer,
    orders: Vec<crate::db::payments::NewPaymentOrderHistoryRecord>,
}

#[cfg(feature = "server")]
fn parse_imported_premium_order(
    order: &RawImportedPremiumOrder,
) -> Result<crate::db::payments::NewPaymentOrderHistoryRecord, ()> {
    let order_id = PaymentOrderId::from_str(&order.order_id).map_err(|_| ())?;
    let currency = order.order_currency.trim();
    if order.order_amount_minor_units == 0
        || currency.is_empty()
        || currency.chars().any(char::is_control)
        || order.order_decimal_precision > 18
    {
        return Err(());
    }

    Ok(crate::db::payments::NewPaymentOrderHistoryRecord {
        order_id,
        product_tier: order.product_tier,
        amount: PaymentAmount {
            minor_units: order.order_amount_minor_units,
            currency: currency.to_string(),
            currency_symbol: None,
            decimal_precision: order.order_decimal_precision,
        },
        status: order.status,
        paid_at: order.paid_at,
    })
}

#[cfg(feature = "server")]
fn non_empty_optional(value: Option<String>) -> Option<String> {
    value.filter(|raw| !raw.trim().is_empty())
}

#[cfg(feature = "server")]
fn extract_pending_premium_transfer(
    file_name: &str,
    payload_json: &str,
) -> Result<Option<ExtractedPendingPremiumTransfer>, ()> {
    let value: serde_json::Value = serde_json::from_str(payload_json).map_err(|_| ())?;
    let Some(premium_transfer_value) = value
        .get("subscription_transfer")
        .or_else(|| value.get("premium_transfer"))
    else {
        return Ok(None);
    };
    let raw: RawImportedPremiumTransfer =
        serde_json::from_value(premium_transfer_value.clone()).map_err(|_| ())?;
    let orders = raw
        .orders
        .iter()
        .map(parse_imported_premium_order)
        .collect::<Result<Vec<_>, _>>()?;
    let imported_management_secret =
        PaymentSecret::from_raw(raw.management_secret).map_err(|_| ())?;
    let imported_active_token = non_empty_optional(raw.active_token);
    let imported_token_id = raw
        .token_id
        .map(|value| TokenId::from_str(&value))
        .transpose()
        .map_err(|_| ())?;
    let imported_subscription_subject_id = raw
        .subscription_subject_id
        .map(|value| SubscriptionSubjectId::from_str(&value))
        .transpose()
        .map_err(|_| ())?;

    Ok(Some(ExtractedPendingPremiumTransfer {
        pending_transfer: crate::db::payments::NewPendingPremiumTransfer {
            source_file_name: file_name.to_string(),
            imported_management_secret,
            imported_active_token,
            imported_token_id,
            imported_subscription_subject_id,
            imported_subscription_valid_until: raw.subscription_valid_until,
            imported_token_expires_at: raw.token_expires_at,
            imported_token_issued_at: raw.token_issued_at,
        },
        orders,
    }))
}

#[cfg(feature = "server")]
fn retryable_central_transfer_error(error: &crate::payments::client::CentralClientError) -> bool {
    match error {
        crate::payments::client::CentralClientError::Request(_)
        | crate::payments::client::CentralClientError::ResponseEncoding(_)
        | crate::payments::client::CentralClientError::ResponseJson(_) => true,
        crate::payments::client::CentralClientError::Http { status, .. } => {
            *status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
        }
        crate::payments::client::CentralClientError::Build(_)
        | crate::payments::client::CentralClientError::Url(_)
        | crate::payments::client::CentralClientError::Contract(_) => false,
    }
}

#[cfg(feature = "server")]
fn central_transfer_error_code(error: &crate::payments::client::CentralClientError) -> String {
    match error {
        crate::payments::client::CentralClientError::Http {
            error_code: Some(code),
            ..
        } => code.clone(),
        crate::payments::client::CentralClientError::Http { status, .. } => {
            format!("http_{}", status.as_u16())
        }
        crate::payments::client::CentralClientError::Request(_) => "request_failed".to_string(),
        crate::payments::client::CentralClientError::ResponseEncoding(_) => {
            "response_encoding_failed".to_string()
        }
        crate::payments::client::CentralClientError::ResponseJson(_) => {
            "response_json_failed".to_string()
        }
        crate::payments::client::CentralClientError::Build(_) => "client_build_failed".to_string(),
        crate::payments::client::CentralClientError::Url(_) => "invalid_url".to_string(),
        crate::payments::client::CentralClientError::Contract(_) => {
            "response_contract_failed".to_string()
        }
    }
}

#[cfg(feature = "server")]
fn premium_transfer_failure_view(
    retryable: bool,
    message: impl Into<String>,
) -> PremiumTransferResultView {
    PremiumTransferResultView {
        status: if retryable {
            PremiumTransferStatusView::RetryableFailure
        } else {
            PremiumTransferStatusView::NonRetryableFailure
        },
        paid_through: None,
        offline_access_until: None,
        message: Some(message.into()),
    }
}

#[cfg(feature = "server")]
fn automatic_add_trigger_request(
    user_id: crate::models::UserId,
    target: AutomaticSyncAddTarget,
) -> TriggerRequest {
    TriggerRequest {
        key: JobKey::User {
            job_id: JobId::UserTransactionMonitor,
            user_id,
        },
        source: TriggerSource::AutoAdd,
        params: TriggerParams::UserTransactionMonitor(UserTransactionMonitorParams {
            run_id: TransactionSyncRunId::new(),
            scope: automatic_add_sync_scope(target),
        }),
    }
}

#[cfg(feature = "server")]
async fn enqueue_automatic_add_sync(
    user_id: crate::models::UserId,
    target: AutomaticSyncAddTarget,
) -> bool {
    if !should_enqueue_automatic_add_sync(sync_control_mode()) {
        tracing::info!(
            user_id = %user_id,
            target = ?target,
            "exports: automatic add sync suppressed because sync control is enabled"
        );
        return false;
    }

    if let Err(err) = ensure_started() {
        tracing::warn!(
            user_id = %user_id,
            error = %err,
            "exports: failed to start task manager for automatic add sync"
        );
        return false;
    }

    let request = automatic_add_trigger_request(user_id, target);
    let requested_scope = match request.params {
        TriggerParams::UserTransactionMonitor(params) => params.scope,
        TriggerParams::SessionCleanup(_)
        | TriggerParams::TraceCleanup(_)
        | TriggerParams::InactiveUserCleanup(_)
        | TriggerParams::PriceHistoryReconciliation(_) => TransactionSyncScope::User,
    };

    match enqueue_trigger(request).await {
        TriggerEnqueueResult::AcceptedStarted { run_id } => {
            tracing::info!(
                user_id = %user_id,
                requested_scope = ?requested_scope,
                run_id = ?run_id,
                "exports: automatic add sync started"
            );
            true
        }
        TriggerEnqueueResult::AcceptedQueued { run_id } => {
            tracing::info!(
                user_id = %user_id,
                requested_scope = ?requested_scope,
                run_id = ?run_id,
                "exports: automatic add sync queued"
            );
            true
        }
        TriggerEnqueueResult::RejectedInvalidKey => {
            tracing::warn!(
                user_id = %user_id,
                requested_scope = ?requested_scope,
                "exports: automatic add sync rejected because the task key was invalid"
            );
            false
        }
        TriggerEnqueueResult::RejectedShuttingDown => {
            tracing::warn!(
                user_id = %user_id,
                requested_scope = ?requested_scope,
                "exports: automatic add sync rejected because the task manager is unavailable"
            );
            false
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
pub(crate) struct DownloadHledgerRequest {
    pub(crate) encrypted: bool,
    #[serde(default)]
    pub(crate) password: Option<String>,
}

#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
fn hledger_zip_file_name(owner_directory_segment: &str, exported_at: DateTime<Utc>) -> String {
    format!(
        "bitgarth-hledger-{}-{}.zip",
        owner_directory_segment,
        exported_at.format("%Y%m%d")
    )
}

#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
const HLEDGER_DOWNLOAD_CHANNEL_DEPTH: usize = 16;
#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
const HLEDGER_DOWNLOAD_CHUNK_SIZE: usize = 64 * 1024;

#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
fn export_error_to_response(err: ExportError) -> axum::response::Response {
    use axum::http::HeaderValue;
    use axum::http::header::CONTENT_TYPE;
    use axum::response::IntoResponse;

    let status = err.as_status_code();
    let body = serde_json::json!({
        "error": format!("{:?}", err).split('(').next().unwrap_or("Error"),
        "message": err.to_string(),
    });
    let body_bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    let mut response = (status, body_bytes).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

#[cfg(all(
    feature = "server",
    any(
        all(not(test), not(feature = "desktop")),
        all(test, not(bitgarth_db_unit_only))
    )
))]
pub(crate) async fn download_hledger(
    cookies: CookieJar,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::http::header;
    use axum::response::IntoResponse;

    let request: DownloadHledgerRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(err) => {
            return export_error_to_response(ExportError::BadRequest(format!(
                "Failed to parse hledger download request: {err}"
            )));
        }
    };
    let DownloadHledgerRequest {
        encrypted,
        password,
    } = request;

    let initialized_session = match session_token_from_cookie(&cookies).and_then(|token| {
        require_initialized_session(
            "exports",
            &token,
            ExportError::Unauthorized,
            ExportError::Internal,
        )
    }) {
        Ok(session) => session,
        Err(err) => return export_error_to_response(err),
    };
    let user_id = initialized_session.session.user_id;

    let password = match (encrypted, password) {
        (true, Some(password)) if !password.is_empty() => Some(Zeroizing::new(password)),
        (true, _) => {
            return export_error_to_response(ExportError::Validation(
                "Enter a password before downloading an encrypted hledger archive.".to_string(),
            ));
        }
        (false, Some(password)) if !password.is_empty() => {
            return export_error_to_response(ExportError::Validation(
                "Do not send a password when encryption is disabled.".to_string(),
            ));
        }
        (false, _) => None,
    };

    let (owner_directory_segment, owner_posting_segment) =
        match crate::exports::hledger::export::resolve_user_hledger_owner_segments(user_id) {
            Ok(value) => value,
            Err(err) => {
                tracing::error!(user_id = %user_id, error = %err, "exports: failed to resolve hledger owner segments");
                return export_error_to_response(ExportError::Internal(format!(
                    "Failed to resolve hledger owner segments for download: {err}"
                )));
            }
        };
    let hledger_account_prefix = match load_settings(user_id) {
        Ok(settings) => settings.hledger_account_prefix,
        Err(err) => {
            tracing::error!(user_id = %user_id, error = %err, "exports: failed to load hledger account prefix");
            return export_error_to_response(ExportError::Internal(format!(
                "Failed to load hledger settings for download: {err}"
            )));
        }
    };

    let exported_at = Utc::now();
    let file_name = hledger_zip_file_name(&owner_directory_segment, exported_at);

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(
        HLEDGER_DOWNLOAD_CHANNEL_DEPTH,
    );

    #[cfg(test)]
    let runtime_context = crate::runtime_context::current_runtime_context();
    let password_for_task = password;
    let owner_directory_for_task = owner_directory_segment.clone();
    let owner_posting_for_task = owner_posting_segment.clone();
    let account_prefix_for_task = hledger_account_prefix.clone();
    let tx_for_task = tx.clone();

    tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        let _runtime_context_guard =
            runtime_context.map(crate::runtime_context::push_default_runtime_context);

        let buffer = Cursor::new(Vec::<u8>::new());
        let password_string = password_for_task
            .as_ref()
            .map(|password| String::from(password.as_str()));
        let result = crate::exports::hledger::export::export_all_accounts_to_zip(
            user_id,
            &owner_directory_for_task,
            &owner_posting_for_task,
            account_prefix_for_task.as_ref(),
            buffer,
            password_string,
        );

        match result {
            Ok((cursor, counts)) => {
                tracing::info!(
                    user_id = %user_id,
                    accounts_exported = counts.accounts_exported,
                    transactions_exported = counts.transactions_exported,
                    balance_assertions_exported = counts.balance_assertions_exported,
                    encrypted,
                    "exports: hledger download completed"
                );
                let bytes = cursor.into_inner();
                for chunk in bytes.chunks(HLEDGER_DOWNLOAD_CHUNK_SIZE) {
                    let chunk_bytes = axum::body::Bytes::copy_from_slice(chunk);
                    if tx_for_task.blocking_send(Ok(chunk_bytes)).is_err() {
                        tracing::debug!(user_id = %user_id, "exports: hledger download client disconnected");
                        return;
                    }
                }
            }
            Err(err) => {
                tracing::error!(user_id = %user_id, error = %err, "exports: hledger download failed");
                let _ = tx_for_task.blocking_send(Err(std::io::Error::other(err.to_string())));
            }
        }
    });
    drop(tx);

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let body = axum::body::Body::from_stream(stream);
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/zip"),
    );
    let disposition = format!("attachment; filename=\"{file_name}\"");
    let disposition_value = axum::http::HeaderValue::from_str(&disposition).unwrap_or_else(|_| {
        axum::http::HeaderValue::from_static("attachment; filename=\"bitgarth-hledger.zip\"")
    });
    headers.insert(header::CONTENT_DISPOSITION, disposition_value);
    headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

#[post("/_app/user/exports/wallet-data", cookies: CookieJar)]
pub(crate) async fn export_wallet_data(
    request: ExportWalletDataRequest,
) -> Result<WalletDataExportDownloadView, ExportError> {
    let ExportWalletDataRequest {
        include_premium_transfer,
        encrypted,
        password,
    } = request;
    tracing::debug!(
        include_premium_transfer,
        encrypted,
        "exports: wallet-data export requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session = require_initialized_session(
        "exports",
        &session_token,
        ExportError::Unauthorized,
        ExportError::Internal,
    )?;
    let user_id = initialized_session.session.user_id;

    let wallets = list_wallets(user_id).map_err(|err| {
        tracing::error!(user_id = %user_id, error = %err, "exports: failed to load wallets for wallet-data export");
        ExportError::Internal(format!("Failed to load wallets for wallet-data export: {err}"))
    })?;
    let account_rows = load_all_accounts_for_export(user_id).map_err(|err| {
        tracing::error!(user_id = %user_id, error = %err, "exports: failed to load account rows for wallet-data export");
        ExportError::Internal(format!(
            "Failed to load account rows for wallet-data export: {err}"
        ))
    })?;
    let manual_asset_accounts = parse_manual_asset_accounts_for_wallet_data(&account_rows)?;
    let manual_assertions = parse_manual_assertions_for_wallet_data(
        load_all_manual_asset_balance_assertion_rows_for_export(user_id).map_err(|err| {
            tracing::error!(user_id = %user_id, error = %err, "exports: failed to load manual asset balance assertions for wallet-data export");
            ExportError::Internal(format!(
                "Failed to load manual asset balance assertions for wallet-data export: {err}"
            ))
        })?,
    );
    let sync_slots = load_account_sync_slots(user_id).map_err(|err| {
        tracing::error!(user_id = %user_id, error = %err, "exports: failed to load sync slots for wallet-data export");
        ExportError::Internal(format!("Failed to load sync slots for wallet-data export: {err}"))
    })?;
    let user_settings = load_settings(user_id).map_err(|err| {
        tracing::error!(user_id = %user_id, error = %err, "exports: failed to load settings for wallet-data export");
        ExportError::Internal(format!("Failed to load settings for wallet-data export: {err}"))
    })?;
    let api_keys = list_all_api_keys(user_id).map_err(|err| {
        tracing::error!(user_id = %user_id, error = %err, "exports: failed to load API key metadata for wallet-data export");
        ExportError::Internal("Failed to load API keys for wallet-data export".to_string())
    })?;

    let username: String = with_db(|conn| {
        conn.query_row(
            "SELECT username FROM users WHERE user_id = ?1",
            [user_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .map_err(|err| crate::db::DbError::from_rusqlite_error("Failed to load username for wallet-data export", err))
    }).map_err(|err| {
        tracing::error!(user_id = %user_id, error = %err, "exports: failed to load username for wallet-data export");
        ExportError::Internal(format!("Failed to load username for wallet-data export: {err}"))
    })?;

    let exported_at = Utc::now();
    let password = match (encrypted, password) {
        (true, Some(password)) if !password.is_empty() => Some(Zeroizing::new(password)),
        (true, _) => {
            return Err(ExportError::Validation(
                "Enter a password before exporting encrypted wallet data.".to_string(),
            ));
        }
        (false, Some(password)) if !password.is_empty() => {
            return Err(ExportError::Validation(
                "Do not send a password when encrypted export is disabled.".to_string(),
            ));
        }
        (false, _) => None,
    };

    let premium_transfer = if include_premium_transfer {
        Some(build_premium_transfer_export(user_id, exported_at)?)
    } else {
        None
    };

    let export_payload = build_wallet_data_export_payload_view(WalletDataExportBuildInput {
        exported_at,
        bitgarth_version: crate::version::version(),
        username: &username,
        wallets,
        manual_asset_accounts,
        manual_assertions,
        sync_slots,
        user_settings: Some(user_settings),
        api_keys,
        premium_transfer,
    })?;
    let payload_json = serde_json::to_string_pretty(&export_payload.payload).map_err(|err| {
        tracing::error!(user_id = %user_id, error = %err, "exports: failed to serialize wallet-data payload");
        ExportError::Internal(format!("Failed to serialize wallet-data export payload: {err}"))
    })?;
    let zip_bytes =
        wrap_in_wallet_data_zip(&payload_json, password.as_deref().map(String::as_str))?;
    let zip_base64 = BASE64.encode(zip_bytes);
    let mut summary = export_payload.summary;
    summary.encrypted = encrypted;
    let export_view = WalletDataExportDownloadView {
        file_name: export_payload.file_name,
        zip_base64,
        summary,
    };

    tracing::info!(
        user_id = %user_id,
        file_name = %export_view.file_name,
        wallets_exported = export_view.summary.wallets,
        native_accounts_exported = export_view.summary.native_accounts,
        addresses_exported = export_view.summary.addresses,
        custom_accounts_exported = export_view.summary.custom_accounts,
        balance_assertions_exported = export_view.summary.balance_assertions,
        api_keys_exported = export_view.summary.api_keys,
        premium_transfer_exported = export_view.summary.premium_transfer_exported,
        encrypted,
        "exports: wallet-data export completed"
    );

    Ok(export_view)
}

#[get("/_app/user/exports/wallet-data/options", cookies: CookieJar)]
pub(crate) async fn get_wallet_data_export_options()
-> Result<WalletDataExportOptionsView, ExportError> {
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session = require_initialized_session(
        "exports",
        &session_token,
        ExportError::Unauthorized,
        ExportError::Internal,
    )?;
    let user_id = initialized_session.session.user_id;
    let premium_transfer_available = crate::db::payments::load_payment_subject(user_id)
        .map_err(|err| {
            tracing::error!(user_id = %user_id, error = %err, "exports: failed to load payment subject for wallet-data export options");
            ExportError::Internal(format!("Failed to load payment subject for export options: {err}"))
        })?
        .and_then(|subject| subject.management_secret)
        .is_some();
    let counts = load_wallet_data_export_counts(user_id)?;

    Ok(WalletDataExportOptionsView {
        premium_transfer_available,
        counts,
    })
}

#[cfg(feature = "server")]
fn apply_import_settings(user_id: crate::models::UserId, settings: &WalletDataImportSettings) {
    if let Some(ref lang_str) = settings.language {
        if let Some(locale) = Locale::try_from_code(lang_str) {
            if let Err(err) = save_language(user_id, locale) {
                tracing::warn!(user_id = %user_id, error = %err, "imports: failed to apply imported language setting");
            }
        } else {
            tracing::warn!(user_id = %user_id, language = %lang_str, "imports: skipping unknown imported language");
        }
    }
    if let Some(ref fmt_str) = settings.date_time_format {
        if let Some(fmt) = DateTimeFormat::from_code(fmt_str) {
            if let Err(err) = save_date_time_format(user_id, fmt) {
                tracing::warn!(user_id = %user_id, error = %err, "imports: failed to apply imported date_time_format setting");
            }
        } else {
            tracing::warn!(user_id = %user_id, format = %fmt_str, "imports: skipping unknown imported date_time_format");
        }
    }
    if let Some(ref fmt_str) = settings.number_format {
        if let Some(fmt) = NumberFormat::from_code(fmt_str) {
            if let Err(err) = save_number_format(user_id, fmt) {
                tracing::warn!(user_id = %user_id, error = %err, "imports: failed to apply imported number_format setting");
            }
        } else {
            tracing::warn!(user_id = %user_id, format = %fmt_str, "imports: skipping unknown imported number_format");
        }
    }
    if let Some(ref code_str) = settings.currency {
        if let Some(currency) = CurrencyCode::from_code(code_str) {
            if let Err(err) = save_currency(user_id, currency) {
                tracing::warn!(user_id = %user_id, error = %err, "imports: failed to apply imported currency setting");
            }
        } else {
            tracing::warn!(user_id = %user_id, currency = %code_str, "imports: skipping unknown imported currency");
        }
    }
    if let Some(ref tz_str) = settings.timezone {
        match tz_str.parse() {
            Ok(tz) => {
                if let Err(err) = save_timezone(user_id, crate::models::UserTimezone(tz)) {
                    tracing::warn!(user_id = %user_id, error = %err, "imports: failed to apply imported timezone setting");
                }
            }
            Err(_) => {
                tracing::warn!(user_id = %user_id, timezone = %tz_str, "imports: skipping invalid imported timezone");
            }
        }
    }
    if let Some(ref dur_str) = settings.session_duration {
        if let Some(duration) = SessionDuration::from_code(dur_str) {
            if let Err(err) = save_session_duration(user_id, duration) {
                tracing::warn!(user_id = %user_id, error = %err, "imports: failed to apply imported session_duration setting");
            }
        } else {
            tracing::warn!(user_id = %user_id, duration = %dur_str, "imports: skipping unknown imported session_duration");
        }
    }
    if settings.mempool_base_url.is_some() || settings.etherscan_base_url.is_some() {
        let mempool_opt = settings.mempool_base_url.as_ref().and_then(|url_str| {
            match MempoolBaseUrl::parse(url_str) {
                Ok(url) => Some(url),
                Err(_) => {
                    tracing::warn!(user_id = %user_id, url = %url_str, "imports: skipping invalid imported mempool_base_url");
                    None
                }
            }
        });
        if let Some(ref url) = mempool_opt
            && let Err(err) = save_mempool_base_url(user_id, Some(url))
        {
            tracing::warn!(user_id = %user_id, error = %err, "imports: failed to apply imported mempool_base_url");
        }

        let etherscan_url_opt = settings.etherscan_base_url.as_ref().and_then(|url_str| {
            match EtherscanBaseUrl::parse(url_str) {
                Ok(url) => Some(url),
                Err(_) => {
                    tracing::warn!(user_id = %user_id, url = %url_str, "imports: skipping invalid imported etherscan_base_url");
                    None
                }
            }
        });
        if let Some(ref url) = etherscan_url_opt
            && let Err(err) = save_etherscan_base_url(user_id, Some(url))
        {
            tracing::warn!(user_id = %user_id, error = %err, "imports: failed to apply imported etherscan_base_url");
        }
    }
    if let Some(ref key_str) = settings.etherscan_api_key {
        let key = RawEtherscanApiKey::new(key_str.clone());
        if let Err(err) = save_etherscan_api_key(user_id, Some(&key)) {
            tracing::warn!(user_id = %user_id, error = %err, "imports: failed to apply imported etherscan_api_key");
        }
    }
    if let Some(ref prefix_str) = settings.hledger_account_prefix {
        match HledgerAccountPrefix::parse(prefix_str) {
            Ok(prefix) => {
                if let Err(err) = save_hledger_account_prefix(user_id, Some(&prefix)) {
                    tracing::warn!(user_id = %user_id, error = %err, "imports: failed to apply imported hledger_account_prefix");
                }
            }
            Err(_) => {
                tracing::warn!(user_id = %user_id, prefix = %prefix_str, "imports: skipping invalid imported hledger_account_prefix");
            }
        }
    }
}

#[post("/_app/user/imports/wallet-data/describe", cookies: CookieJar)]
pub(crate) async fn describe_wallet_data_import(
    request: DescribeWalletDataImportRequest,
) -> Result<WalletDataImportDescription, ExportError> {
    tracing::debug!(
        file_name = %request.file_name,
        payload_base64_bytes = request.payload_base64.len(),
        "exports: wallet-data import describe requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session = require_initialized_session(
        "exports",
        &session_token,
        ExportError::Unauthorized,
        ExportError::Internal,
    )?;
    let _user_id = initialized_session.session.user_id;

    validate_wallet_data_import_file_name(&request.file_name)?;

    let password = request.password.map(Zeroizing::new);
    let payload_bytes = decode_wallet_data_zip_base64(&request.payload_base64)?;
    let import_payload =
        unwrap_wallet_data_payload(&payload_bytes, password.as_deref().map(String::as_str))?;
    let mut payload_value = parse_wallet_data_json_value(&import_payload.payload_json)?;
    translate_v3_to_v4_payload(&mut payload_value)?;
    describe_wallet_data_value(&payload_value)
}

#[post("/_app/user/imports/wallet-data", cookies: CookieJar)]
pub(crate) async fn import_wallet_data(
    request: ImportWalletDataRequest,
) -> Result<ImportResultView, ExportError> {
    tracing::debug!(
        file_name = %request.file_name,
        payload_base64_bytes = request.payload_base64.len(),
        "exports: wallet-data import requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session = require_initialized_session(
        "exports",
        &session_token,
        ExportError::Unauthorized,
        ExportError::Internal,
    )?;
    let user_id = initialized_session.session.user_id;

    validate_wallet_data_import_file_name(&request.file_name)?;

    let password = request.password.map(Zeroizing::new);
    let payload_bytes = decode_wallet_data_zip_base64(&request.payload_base64)?;
    let import_payload =
        unwrap_wallet_data_payload(&payload_bytes, password.as_deref().map(String::as_str))?;
    let translated_payload_json = translated_wallet_data_json(&import_payload.payload_json)?;

    let import_settings =
        extract_import_settings(&translated_payload_json).map_err(map_import_error)?;

    let import_started_at = Utc::now();
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, import_started_at)
            .map_err(|err| ExportError::Internal(format!("Failed to load entitlements: {err}")))?;
    let import_result = import_wallet_data_db(
        user_id,
        &translated_payload_json,
        usize::from(entitlements.sync_account_slots_limit),
        import_started_at,
    )
    .map_err(map_import_error)?;

    let (premium_transfer_status, pending_premium_transfer_id) =
        match extract_pending_premium_transfer(
            &import_payload.inner_file_name,
            &translated_payload_json,
        ) {
            Ok(Some(pending_transfer)) => {
                crate::db::payments::upsert_imported_payment_order_history(
                    user_id,
                    &pending_transfer.orders,
                    Utc::now(),
                )
                .map_err(|err| {
                    tracing::error!(user_id = %user_id, error = %err, "imports: failed to persist imported premium order history");
                    ExportError::Internal(format!(
                        "Failed to persist imported Premium order history: {err}"
                    ))
                })?;
                let pending_id = crate::db::payments::insert_pending_premium_transfer(
                    user_id,
                    &pending_transfer.pending_transfer,
                    Utc::now(),
                )
                .map_err(|err| {
                    tracing::error!(user_id = %user_id, error = %err, "imports: failed to persist pending premium transfer");
                    ExportError::Internal(format!(
                        "Failed to persist pending Premium transfer: {err}"
                    ))
                })?;
                (
                    PremiumTransferImportStatusView::PendingConfirmation,
                    Some(pending_id),
                )
            }
            Ok(None) => (PremiumTransferImportStatusView::NotPresent, None),
            Err(()) => {
                tracing::warn!(
                    user_id = %user_id,
                    file_name = %request.file_name,
                    "imports: ignoring invalid premium transfer metadata"
                );
                (PremiumTransferImportStatusView::InvalidMetadata, None)
            }
        };

    let imported_api_keys = extract_imported_api_keys(&translated_payload_json)?;
    let mut api_keys_imported = 0_u32;
    let mut api_keys_skipped_already_present = imported_api_keys.skipped_invalid;
    for (provider, api_key) in imported_api_keys.valid {
        if has_api_key(user_id, provider).map_err(|err| {
            tracing::error!(user_id = %user_id, error = %err, "imports: failed to check api key");
            ExportError::Internal(format!("Failed to check API key: {err}"))
        })? {
            api_keys_skipped_already_present = api_keys_skipped_already_present.saturating_add(1);
            continue;
        }
        save_api_key(user_id, provider, &api_key).map_err(|err| {
            tracing::error!(user_id = %user_id, error = %err, "imports: failed to save api key");
            ExportError::Internal(format!("Failed to save API key: {err}"))
        })?;
        api_keys_imported = api_keys_imported.saturating_add(1);
    }

    let settings_imported = import_settings.is_some();
    if let Some(ref settings) = import_settings {
        apply_import_settings(user_id, settings);
    }

    let sync_triggered =
        enqueue_automatic_add_sync(user_id, AutomaticSyncAddTarget::MultiAccountImport).await;

    tracing::info!(
        user_id = %user_id,
        file_name = %request.file_name,
        wallets_created = import_result.wallets_created.len(),
        wallets_matched = import_result.wallets_matched.len(),
        native_accounts_created = import_result.native_accounts_created.len(),
        native_accounts_matched = import_result.native_accounts_matched.len(),
        duplicate_skips = import_result.duplicate_skips.len(),
        global_duplicate_skips = import_result.global_duplicate_skips.len(),
        assertions_created = import_result.assertions_created,
        assertions_skipped = import_result.assertions_skipped,
        sync_triggered,
        settings_imported,
        api_keys_imported,
        api_keys_skipped_already_present,
        premium_transfer_status = ?premium_transfer_status,
        encrypted = import_payload.encrypted,
        "exports: wallet-data import completed"
    );

    Ok(import_result_view(
        import_result,
        sync_triggered,
        settings_imported,
        api_keys_imported,
        api_keys_skipped_already_present,
        premium_transfer_status,
        pending_premium_transfer_id,
    ))
}

#[post("/_app/user/imports/wallet-data/premium-transfer", cookies: CookieJar)]
pub(crate) async fn confirm_premium_transfer(
    request: ConfirmPremiumTransferRequest,
) -> Result<PremiumTransferResultView, ExportError> {
    tracing::debug!(
        pending_transfer_id = %request.pending_transfer_id,
        "exports: premium transfer confirmation requested"
    );
    let session_token = session_token_from_cookie(&cookies)?;
    let initialized_session = require_initialized_session(
        "exports",
        &session_token,
        ExportError::Unauthorized,
        ExportError::Internal,
    )?;
    let user_id = initialized_session.session.user_id;
    let now = Utc::now();
    let pending = crate::db::payments::load_pending_premium_transfer(
        user_id,
        &request.pending_transfer_id,
    )
    .map_err(|err| {
        tracing::error!(user_id = %user_id, error = %err, "exports: failed to load pending premium transfer");
        ExportError::Internal(format!("Failed to load pending Premium transfer: {err}"))
    })?
    .ok_or_else(|| ExportError::Validation("Pending Premium transfer not found.".to_string()))?;

    if pending.status != "pending_confirmation" && pending.status != "retryable_failure" {
        return Err(ExportError::Validation(
            "Pending Premium transfer cannot be retried.".to_string(),
        ));
    }

    let subject = crate::db::payments::load_or_create_payment_subject(user_id, now).map_err(
        |err| {
            tracing::error!(user_id = %user_id, error = %err, "exports: failed to load payment subject for premium transfer");
            ExportError::Internal(format!("Failed to load payment subject: {err}"))
        },
    )?;
    let new_management_secret = PaymentSecret::generate();
    let client = crate::payments::client::BitGarthCentralClient::new(user_id)
        .map_err(|err| ExportError::Internal(format!("Failed to build Central client: {err}")))?;
    let outcome = match client
        .transfer_subscription(
            &pending.imported_management_secret,
            subject.entitlement_holder_id,
            &new_management_secret,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            let retryable = retryable_central_transfer_error(&error);
            let error_code = central_transfer_error_code(&error);
            let message = if retryable {
                "Premium transfer is pending. BitGarth could not reach the payment service reliably; retry later."
            } else if error.is_upgrade_required() {
                "BitGarth needs an update before this Premium transfer can be completed."
            } else {
                "Premium transfer could not be completed. The transfer secret may already be invalid or the subscription may no longer be transferable."
            };
            crate::db::payments::mark_pending_premium_transfer_failure(
                user_id,
                &pending.id,
                retryable,
                &error_code,
                &error.to_string(),
                Utc::now(),
            )
            .map_err(|err| {
                tracing::error!(user_id = %user_id, error = %err, "exports: failed to mark premium transfer failure");
                ExportError::Internal(format!("Failed to record Premium transfer failure: {err}"))
            })?;
            return Ok(premium_transfer_failure_view(retryable, message));
        }
    };

    let crate::payments::client::CentralTransferOutcome::Active {
        premium_access_token,
        token_id,
        subscription_valid_until,
        token_expires_at,
    } = outcome;
    let verified = match crate::payments::keys::verify_premium_token(
        &premium_access_token,
        subject.entitlement_holder_id,
        Utc::now(),
    ) {
        Ok(verified) => verified,
        Err(error) => {
            crate::db::payments::mark_pending_premium_transfer_failure(
                    user_id,
                    &pending.id,
                    false,
                    "token_verification_failed",
                    &error.to_string(),
                    Utc::now(),
                )
                .map_err(|err| {
                    tracing::error!(user_id = %user_id, error = %err, "exports: failed to mark premium transfer token failure");
                    ExportError::Internal(format!(
                        "Failed to record Premium transfer token failure: {err}"
                    ))
                })?;
            return Ok(premium_transfer_failure_view(
                false,
                "Premium transfer returned an invalid token and was not activated.",
            ));
        }
    };
    if verified.claims.token_id != token_id
        || verified.claims.subscription_valid_until != subscription_valid_until
        || verified.claims.token_expires_at != token_expires_at
    {
        crate::db::payments::mark_pending_premium_transfer_failure(
            user_id,
            &pending.id,
            false,
            "token_metadata_mismatch",
            "Central transfer token metadata did not match signed claims",
            Utc::now(),
        )
        .map_err(|err| {
            tracing::error!(user_id = %user_id, error = %err, "exports: failed to mark premium transfer metadata failure");
            ExportError::Internal(format!(
                "Failed to record Premium transfer metadata failure: {err}"
            ))
        })?;
        return Ok(premium_transfer_failure_view(
            false,
            "Premium transfer returned inconsistent token metadata and was not activated.",
        ));
    }

    crate::db::payments::update_payment_management_secret(user_id, &new_management_secret, Utc::now())
        .map_err(|err| {
            tracing::error!(user_id = %user_id, error = %err, "exports: failed to store transferred management secret");
            ExportError::Internal(format!("Failed to store transferred management secret: {err}"))
        })?;
    crate::db::payments::store_verified_premium_token(user_id, None, &verified, None, Utc::now())
        .map_err(|err| {
            tracing::error!(user_id = %user_id, error = %err, "exports: failed to store transferred premium token");
            ExportError::Internal(format!("Failed to store transferred Premium token: {err}"))
        })?;
    crate::db::payments::mark_pending_premium_transfer_completed(user_id, &pending.id, Utc::now())
        .map_err(|err| {
            tracing::error!(user_id = %user_id, error = %err, "exports: failed to mark premium transfer completed");
            ExportError::Internal(format!("Failed to record completed Premium transfer: {err}"))
        })?;

    Ok(PremiumTransferResultView {
        status: PremiumTransferStatusView::Active,
        paid_through: Some(verified.claims.subscription_valid_until),
        offline_access_until: Some(verified.claims.token_expires_at),
        message: Some(
            "Subscription moved to this local user. The previous user can no longer refresh the subscription."
                .to_string(),
        ),
    })
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::account_model::AccountModel;
    use crate::models::{
        ApiKeyProvider, CurrencyCode, DateTimeFormat, NumberFormat, RawEtherscanApiKey,
        SimpleApiKey, UserSettings,
    };
    use crate::wallets::{
        ACCOUNT_LABEL_MAX_LENGTH, AccessorKind, AccountIndex, AccountKind, AddressScheme,
        AddressSourceType, DerivationCoinType, DerivationPath, DerivationPurpose,
        DigitalAssetAccountId, DigitalAssetAddressId, DigitalAssetAddressRecord, HdKeyId,
        HdKeyRecord, IdentitySource, KeyRole, KeySource, Label, Network, SyncedAssetId,
        ValidatedExtendedPubkey, ValidatedMasterFingerprint, WALLET_LABEL_MAX_LENGTH,
        WalletAccessorId, WalletAccessorSummary, WalletAccountId, WalletId, WalletSummary,
        WalletWithDetails,
    };
    use chrono::TimeZone;

    const SAMPLE_ZPUB: &str = "zpub6qU5MALAB8Bscej9sTEkgSocaxvLzAYYeytsL9fXfv8W4BTykA99FNDNpftwXMGomwc2KatVrbXo4qXsdBC1DiNHCHGapas9enpPBo8y8Y4";

    fn fixed_datetime() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 4, 12, 0, 0)
            .single()
            .expect("fixed test datetime should be valid")
    }

    fn label(value: &str, max_len: usize) -> Label {
        Label::parse_with_limit(value, max_len).expect("label should parse")
    }

    #[test]
    fn wallet_data_export_version_keeps_v3_for_legacy_imports() {
        let version_json =
            serde_json::to_value(WalletDataExportVersion::V3).expect("version should serialize");

        assert_eq!(version_json, serde_json::json!(3));
    }

    fn test_settings_with_etherscan_key(key: &str) -> UserSettings {
        UserSettings {
            language: None,
            date_time_format: None,
            number_format: None,
            currency: None,
            timezone: None,
            session_duration: None,
            mempool_base_url: None,
            etherscan_base_url: None,
            hledger_account_prefix: None,
            etherscan_api_key: Some(RawEtherscanApiKey::new(key.to_string())),
            has_etherscan_api_key: true,
            has_coingecko_api_key: false,
            price_fetching_enabled: false,
        }
    }

    fn sample_wallet() -> WalletWithDetails {
        let wallet_id = WalletId::new();
        let account_id = DigitalAssetAccountId::new();

        WalletWithDetails {
            wallet: WalletSummary {
                id: wallet_id,
                master_fingerprint: Some(
                    ValidatedMasterFingerprint::parse("a1b2c3d4")
                        .expect("fingerprint should parse"),
                ),
                identity_source: IdentitySource::DeviceVerified,
                verified_at: Some(fixed_datetime()),
                label: label("Trezor", WALLET_LABEL_MAX_LENGTH),
                created_at: fixed_datetime(),
                updated_at: fixed_datetime(),
            },
            accessors: vec![WalletAccessorSummary {
                id: WalletAccessorId::new(),
                accessor_kind: AccessorKind::Trezor,
                accessor_label: Some(label("Primary Trezor", WALLET_LABEL_MAX_LENGTH)),
                device_id_hash: None,
                device_model: Some("Model T".to_string()),
                accessor_version: None,
                firmware_version: None,
                created_at: fixed_datetime(),
                updated_at: fixed_datetime(),
            }],
            accounts: vec![crate::wallets::AccountWithHdKeys {
                id: account_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                account_model: AccountModel::Utxo,
                account_kind: AccountKind::HdPubkey,
                label: label("BTC Savings", ACCOUNT_LABEL_MAX_LENGTH),
                hd_keys: vec![HdKeyRecord {
                    id: HdKeyId::new(),
                    key_role: KeyRole::Primary,
                    key_source: KeySource::DeviceVerified,
                    verified_by_accessor_id: None,
                    address_scheme: AddressScheme::NativeSegwit,
                    extended_pubkey: ValidatedExtendedPubkey::parse(
                        AddressScheme::NativeSegwit,
                        SAMPLE_ZPUB,
                    )
                    .expect("zpub should parse"),
                    derivation_path: DerivationPath {
                        purpose: DerivationPurpose::Bip84,
                        coin_type: DerivationCoinType::new(0),
                        account: AccountIndex::new(0).expect("account index should parse"),
                    },
                    created_at: fixed_datetime(),
                    updated_at: fixed_datetime(),
                }],
                addresses: vec![
                    DigitalAssetAddressRecord {
                        id: DigitalAssetAddressId::new(),
                        asset_id: SyncedAssetId::Bitcoin,
                        network: Network::Mainnet,
                        address: "bc1quserprovided".to_string(),
                        address_scheme: AddressScheme::NativeSegwit,
                        derivation_change: None,
                        derivation_index: None,
                        source_type: AddressSourceType::UserProvided,
                        created_at: fixed_datetime(),
                        updated_at: fixed_datetime(),
                    },
                    DigitalAssetAddressRecord {
                        id: DigitalAssetAddressId::new(),
                        asset_id: SyncedAssetId::Bitcoin,
                        network: Network::Mainnet,
                        address: "bc1qimported".to_string(),
                        address_scheme: AddressScheme::NativeSegwit,
                        derivation_change: None,
                        derivation_index: None,
                        source_type: AddressSourceType::Imported,
                        created_at: fixed_datetime(),
                        updated_at: fixed_datetime(),
                    },
                    DigitalAssetAddressRecord {
                        id: DigitalAssetAddressId::new(),
                        asset_id: SyncedAssetId::Bitcoin,
                        network: Network::Mainnet,
                        address: "bc1qderived".to_string(),
                        address_scheme: AddressScheme::NativeSegwit,
                        derivation_change: Some(0),
                        derivation_index: Some(0),
                        source_type: AddressSourceType::Derived,
                        created_at: fixed_datetime(),
                        updated_at: fixed_datetime(),
                    },
                    DigitalAssetAddressRecord {
                        id: DigitalAssetAddressId::new(),
                        asset_id: SyncedAssetId::Bitcoin,
                        network: Network::Mainnet,
                        address: "bc1qobserved".to_string(),
                        address_scheme: AddressScheme::NativeSegwit,
                        derivation_change: None,
                        derivation_index: None,
                        source_type: AddressSourceType::Observed,
                        created_at: fixed_datetime(),
                        updated_at: fixed_datetime(),
                    },
                ],
                created_at: fixed_datetime(),
                updated_at: fixed_datetime(),
            }],
        }
    }

    #[test]
    fn wallet_data_export_builder_filters_addresses_and_builds_summary() {
        let manual_account_id = WalletAccountId::new();
        let wallet = sample_wallet();
        let native_account_id = wallet.accounts[0].id;
        let wallet_id = wallet.wallet.id;

        let manual_asset_accounts = vec![WalletDataManualAssetAccountExportSource {
            account_id: manual_account_id,
            wallet_id,
            label: "Cardano Staking".to_string(),
            asset_instance_id: ManualAssetInstanceIdView {
                asset_id: "cardano".to_string(),
                network_id: "cardano-mainnet".to_string(),
            },
            created_at: fixed_datetime(),
            unit_code: "ADA".to_string(),
            decimal_precision: 6,
            symbol: Some("A".to_string()),
            asset_name: "Cardano".to_string(),
            network_name: "Cardano".to_string(),
            coingecko_id: "cardano".to_string(),
            asset_source: Some("bitgarth_catalog".to_string()),
            precision_source: Some("bitgarth_catalog".to_string()),
            coingecko_platform_id: None,
            provider_platform_asset_ref: None,
        }];
        let manual_assertions = vec![WalletDataManualAssertionExportSource {
            account_id: manual_account_id,
            asserted_on: NaiveDate::from_ymd_opt(2026, 3, 1).expect("date should parse"),
            asserted_balance: UnsignedAmount::from_u128(15_000_000_000),
            note: Some("Post-epoch snapshot".to_string()),
        }];

        let export = build_wallet_data_export_payload_view(WalletDataExportBuildInput {
            exported_at: fixed_datetime(),
            bitgarth_version: "0.1.0-test",
            username: "testuser",
            wallets: vec![wallet],
            manual_asset_accounts,
            manual_assertions,
            sync_slots: vec![AccountSyncSlotRecord {
                account_id: native_account_id,
                selected_at: fixed_datetime(),
                selected_under_tier: crate::payments::types::EntitlementTier::Free,
            }],
            user_settings: None,
            api_keys: Vec::new(),
            premium_transfer: None,
        })
        .expect("wallet data export should build");

        assert_eq!(
            export.file_name,
            "bitgarth-walletdata-testuser-20260404.zip"
        );
        assert_eq!(export.payload.version, WalletDataExportVersion::V5);
        assert_eq!(export.payload.bitgarth_version, "0.1.0-test");
        assert!(export.payload.settings.is_none());
        assert!(export.payload.api_keys.is_empty());
        assert!(export.payload.premium_transfer.is_none());

        assert_eq!(export.summary.wallets, 1);
        assert_eq!(export.summary.native_accounts, 1);
        assert_eq!(export.summary.addresses, 2);
        assert_eq!(export.summary.custom_accounts, 1);
        assert_eq!(export.summary.balance_assertions, 1);
        assert_eq!(export.summary.api_keys, 0);
        assert!(!export.summary.settings_exported);
        assert!(!export.summary.premium_transfer_exported);
        assert!(!export.summary.encrypted);

        let wallet_payload = &export.payload.wallets[0];
        assert_eq!(wallet_payload.label, "Trezor");
        assert_eq!(
            wallet_payload.master_fingerprint.as_deref(),
            Some("a1b2c3d4")
        );
        assert_eq!(
            wallet_payload.identity_source,
            IdentitySource::DeviceVerified
        );

        assert_eq!(wallet_payload.digital_asset_accounts.len(), 1);
        let native_account = &wallet_payload.digital_asset_accounts[0];
        assert_eq!(native_account.label, "BTC Savings");
        assert_eq!(native_account.created_at, Some(fixed_datetime()));
        assert_eq!(
            native_account
                .sync_slot
                .as_ref()
                .map(|slot| slot.selected_under_tier.as_str()),
            Some("free")
        );
        assert_eq!(native_account.addresses.len(), 2);
        assert_eq!(native_account.addresses[0].address, "bc1quserprovided");
        assert_eq!(native_account.addresses[1].address, "bc1qimported");

        assert_eq!(wallet_payload.manual_asset_accounts.len(), 1);
        let manual_account = &wallet_payload.manual_asset_accounts[0];
        assert_eq!(manual_account.label, "Cardano Staking");
        assert_eq!(manual_account.unit_code, "ADA");
        assert_eq!(manual_account.decimal_precision, 6);
        assert_eq!(manual_account.balance_assertions.len(), 1);
        assert_eq!(
            manual_account.balance_assertions[0].balance_amount,
            "15000.000000"
        );
    }

    #[test]
    fn wallet_data_export_builder_includes_settings_when_provided() {
        let wallet = sample_wallet();
        let user_settings = UserSettings {
            language: Some(crate::i18n::Locale::English),
            date_time_format: Some(DateTimeFormat::YearMonthDay24),
            number_format: Some(NumberFormat::DotComma),
            currency: Some(CurrencyCode::from_code("EUR").expect("EUR should parse")),
            timezone: None,
            session_duration: None,
            mempool_base_url: None,
            etherscan_base_url: None,
            hledger_account_prefix: Some(
                HledgerAccountPrefix::parse("assets:My Wallet").expect("test prefix should parse"),
            ),
            etherscan_api_key: None,
            has_etherscan_api_key: false,
            has_coingecko_api_key: false,
            price_fetching_enabled: false,
        };

        let export = build_wallet_data_export_payload_view(WalletDataExportBuildInput {
            exported_at: fixed_datetime(),
            bitgarth_version: "0.1.0-test",
            username: "testuser",
            wallets: vec![wallet],
            manual_asset_accounts: Vec::new(),
            manual_assertions: Vec::new(),
            sync_slots: Vec::new(),
            user_settings: Some(user_settings),
            api_keys: Vec::new(),
            premium_transfer: None,
        })
        .expect("wallet data export should build");

        assert!(export.summary.settings_exported);
        let settings = export.payload.settings.expect("settings should be present");
        assert_eq!(settings.language.as_deref(), Some("en"));
        assert_eq!(settings.date_time_format.as_deref(), Some("ymd_24"));
        assert_eq!(settings.number_format.as_deref(), Some("dot_comma"));
        assert_eq!(settings.currency.as_deref(), Some("EUR"));
        assert_eq!(
            settings.hledger_account_prefix.as_deref(),
            Some("assets:My Wallet")
        );
        assert!(settings.timezone.is_none());
    }

    #[test]
    fn wallet_data_export_v5_native_account_includes_created_at() {
        let wallet = sample_wallet();

        let export = build_wallet_data_export_payload_view(WalletDataExportBuildInput {
            exported_at: fixed_datetime(),
            bitgarth_version: "0.1.0-test",
            username: "testuser",
            wallets: vec![wallet],
            manual_asset_accounts: Vec::new(),
            manual_assertions: Vec::new(),
            sync_slots: Vec::new(),
            user_settings: None,
            api_keys: Vec::new(),
            premium_transfer: None,
        })
        .expect("wallet data export should build");

        let json =
            serde_json::to_value(&export.payload).expect("wallet data export should serialize");

        assert_eq!(export.payload.version, WalletDataExportVersion::V5);
        assert_eq!(
            json.pointer("/wallets/0/digital_asset_accounts/0/created_at"),
            Some(&serde_json::json!("2026-04-04T12:00:00Z"))
        );
    }

    #[test]
    fn wallet_data_export_v5_manual_account_includes_created_at() {
        let wallet = sample_wallet();
        let wallet_id = wallet.wallet.id;
        let manual_account_id = WalletAccountId::new();

        let manual_asset_accounts = vec![WalletDataManualAssetAccountExportSource {
            account_id: manual_account_id,
            wallet_id,
            label: "Cardano Staking".to_string(),
            asset_instance_id: ManualAssetInstanceIdView {
                asset_id: "cardano".to_string(),
                network_id: "cardano-mainnet".to_string(),
            },
            created_at: fixed_datetime(),
            unit_code: "ADA".to_string(),
            decimal_precision: 6,
            symbol: Some("A".to_string()),
            asset_name: "Cardano".to_string(),
            network_name: "Cardano".to_string(),
            coingecko_id: "cardano".to_string(),
            asset_source: Some("bitgarth_catalog".to_string()),
            precision_source: Some("bitgarth_catalog".to_string()),
            coingecko_platform_id: None,
            provider_platform_asset_ref: None,
        }];

        let export = build_wallet_data_export_payload_view(WalletDataExportBuildInput {
            exported_at: fixed_datetime(),
            bitgarth_version: "0.1.0-test",
            username: "testuser",
            wallets: vec![wallet],
            manual_asset_accounts,
            manual_assertions: Vec::new(),
            sync_slots: Vec::new(),
            user_settings: None,
            api_keys: Vec::new(),
            premium_transfer: None,
        })
        .expect("wallet data export should build");

        let json =
            serde_json::to_value(&export.payload).expect("wallet data export should serialize");

        assert_eq!(export.payload.version, WalletDataExportVersion::V5);
        assert_eq!(
            json.pointer("/wallets/0/manual_asset_accounts/0/created_at"),
            Some(&serde_json::json!("2026-04-04T12:00:00Z"))
        );
    }

    #[test]
    fn wallet_data_export_payload_has_v5_version_and_api_keys() {
        let wallet = sample_wallet();
        let api_key = SimpleApiKey::new("etherscan-export-key".to_string()).expect("valid key");

        let export = build_wallet_data_export_payload_view(WalletDataExportBuildInput {
            exported_at: fixed_datetime(),
            bitgarth_version: "0.1.0-test",
            username: "testuser",
            wallets: vec![wallet],
            manual_asset_accounts: Vec::new(),
            manual_assertions: Vec::new(),
            sync_slots: Vec::new(),
            user_settings: None,
            api_keys: vec![(ApiKeyProvider::Etherscan, api_key)],
            premium_transfer: None,
        })
        .expect("wallet data export should build");

        assert_eq!(export.payload.version, WalletDataExportVersion::V5);
        assert_eq!(export.payload.api_keys.len(), 1);
        assert_eq!(export.payload.api_keys[0].provider, "etherscan");
        assert_eq!(export.payload.api_keys[0].api_key, "etherscan-export-key");
        assert_eq!(export.summary.api_keys, 1);
    }

    #[test]
    fn wallet_data_export_omits_etherscan_api_key_from_settings() {
        let wallet = sample_wallet();

        let export = build_wallet_data_export_payload_view(WalletDataExportBuildInput {
            exported_at: fixed_datetime(),
            bitgarth_version: "0.1.0-test",
            username: "testuser",
            wallets: vec![wallet],
            manual_asset_accounts: Vec::new(),
            manual_assertions: Vec::new(),
            sync_slots: Vec::new(),
            user_settings: Some(test_settings_with_etherscan_key("old-settings-key")),
            api_keys: Vec::new(),
            premium_transfer: None,
        })
        .expect("wallet data export should build");

        let json =
            serde_json::to_value(&export.payload).expect("wallet data export should serialize");

        assert!(json.pointer("/settings/etherscan_api_key").is_none());
    }

    #[test]
    fn translate_v3_to_v4_lifts_etherscan_api_key_and_renames_transfer() {
        let mut value = serde_json::json!({
            "version": 3,
            "settings": {
                "language": "en",
                "etherscan_api_key": "  legacy-key  "
            },
            "premium_transfer": {
                "management_secret": "secret"
            }
        });

        translate_v3_to_v4_payload(&mut value).expect("translation should succeed");

        assert_eq!(value["version"], serde_json::json!(4));
        assert!(value.pointer("/settings/etherscan_api_key").is_none());
        assert!(value.get("premium_transfer").is_none());
        assert_eq!(
            value["subscription_transfer"],
            serde_json::json!({ "management_secret": "secret" })
        );
        assert_eq!(
            value["api_keys"],
            serde_json::json!([{ "provider": "etherscan", "api_key": "legacy-key" }])
        );
    }

    #[test]
    fn describe_wallet_data_import_returns_counts_and_flags() {
        let mut value = serde_json::json!({
            "version": 3,
            "settings": {
                "etherscan_api_key": "legacy-key"
            },
            "premium_transfer": {
                "management_secret": "secret"
            }
        });

        translate_v3_to_v4_payload(&mut value).expect("translation should succeed");
        let description =
            describe_wallet_data_value(&value).expect("description should be derived");

        assert_eq!(description.file_version, 4);
        assert!(description.has_subscription_transfer);
        assert_eq!(description.api_keys_count, 1);
    }

    #[test]
    fn get_wallet_data_export_options_returns_correct_counts() {
        let manual_account_id = WalletAccountId::new();
        let mut wallet = sample_wallet();
        let wallet_id = wallet.wallet.id;
        wallet.accounts.push(crate::wallets::AccountWithHdKeys {
            id: DigitalAssetAccountId::new(),
            asset_id: SyncedAssetId::Ethereum,
            network: Network::Mainnet,
            account_model: AccountModel::Account,
            account_kind: AccountKind::SingleAddress,
            label: label("ETH", ACCOUNT_LABEL_MAX_LENGTH),
            hd_keys: Vec::new(),
            addresses: Vec::new(),
            created_at: fixed_datetime(),
            updated_at: fixed_datetime(),
        });
        let manual_asset_accounts = vec![WalletDataManualAssetAccountExportSource {
            account_id: manual_account_id,
            wallet_id,
            label: "Cardano Staking".to_string(),
            asset_instance_id: ManualAssetInstanceIdView {
                asset_id: "cardano".to_string(),
                network_id: "cardano-mainnet".to_string(),
            },
            created_at: fixed_datetime(),
            unit_code: "ADA".to_string(),
            decimal_precision: 6,
            symbol: Some("A".to_string()),
            asset_name: "Cardano".to_string(),
            network_name: "Cardano".to_string(),
            coingecko_id: "cardano".to_string(),
            asset_source: Some("bitgarth_catalog".to_string()),
            precision_source: Some("bitgarth_catalog".to_string()),
            coingecko_platform_id: None,
            provider_platform_asset_ref: None,
        }];
        let manual_assertions = vec![
            WalletDataManualAssertionExportSource {
                account_id: manual_account_id,
                asserted_on: NaiveDate::from_ymd_opt(2026, 3, 1).expect("date should parse"),
                asserted_balance: UnsignedAmount::from_u128(15_000_000_000),
                note: Some("Post-epoch snapshot".to_string()),
            },
            WalletDataManualAssertionExportSource {
                account_id: WalletAccountId::new(),
                asserted_on: NaiveDate::from_ymd_opt(2026, 3, 2).expect("date should parse"),
                asserted_balance: UnsignedAmount::from_u128(1),
                note: None,
            },
        ];

        let counts = build_wallet_data_export_counts(
            &[wallet],
            &manual_asset_accounts,
            &manual_assertions,
            2,
        )
        .expect("counts should build");

        assert_eq!(
            counts,
            WalletDataExportCounts {
                wallets: 1,
                native_accounts: 2,
                addresses: 2,
                custom_accounts: 1,
                balance_assertions: 1,
                api_keys: 2,
            }
        );
    }

    #[test]
    fn wallet_data_export_builder_includes_premium_transfer_when_requested() {
        let wallet = sample_wallet();
        let premium_transfer = WalletDataExportPremiumTransfer {
            exported_at: fixed_datetime(),
            management_secret: "5FuYMBR_MhwubKAJQeNMrUH0JD3PvFuyt3sfFh0ezLw".to_string(),
            active_token: Some("signed-token".to_string()),
            token_id: Some("01JQABCDEF000000000000000F".to_string()),
            subscription_subject_id: Some("01JQABCDEF000000000000000G".to_string()),
            subscription_valid_until: Some(fixed_datetime()),
            token_expires_at: Some(fixed_datetime()),
            token_issued_at: Some(fixed_datetime()),
            orders: vec![WalletDataExportPremiumOrder {
                order_id: "01JQABCDEF000000000000000E".to_string(),
                product_tier: "premium".to_string(),
                order_amount_minor_units: 999,
                order_currency: "USD".to_string(),
                order_decimal_precision: 2,
                status: "paid".to_string(),
                paid_at: Some(fixed_datetime()),
            }],
        };

        let export = build_wallet_data_export_payload_view(WalletDataExportBuildInput {
            exported_at: fixed_datetime(),
            bitgarth_version: "0.1.0-test",
            username: "testuser",
            wallets: vec![wallet],
            manual_asset_accounts: Vec::new(),
            manual_assertions: Vec::new(),
            sync_slots: Vec::new(),
            user_settings: None,
            api_keys: Vec::new(),
            premium_transfer: Some(premium_transfer),
        })
        .expect("wallet data export should build");

        assert!(export.summary.premium_transfer_exported);
        let premium_transfer = export
            .payload
            .premium_transfer
            .expect("premium transfer should be exported");
        assert_eq!(
            premium_transfer.management_secret,
            "5FuYMBR_MhwubKAJQeNMrUH0JD3PvFuyt3sfFh0ezLw"
        );
        assert_eq!(premium_transfer.orders.len(), 1);
        assert_eq!(premium_transfer.orders[0].status, "paid");
    }

    #[test]
    fn wallet_data_export_omits_legacy_and_keeps_supported_manual_rows() {
        let wallet = sample_wallet();
        let wallet_id = wallet.wallet.id;
        let manual_account_id = WalletAccountId::new();

        let manual_asset_accounts = vec![WalletDataManualAssetAccountExportSource {
            account_id: manual_account_id,
            wallet_id,
            label: "Cardano Staking".to_string(),
            asset_instance_id: ManualAssetInstanceIdView {
                asset_id: "cardano".to_string(),
                network_id: "cardano-mainnet".to_string(),
            },
            created_at: fixed_datetime(),
            unit_code: "ADA".to_string(),
            decimal_precision: 6,
            symbol: Some("A".to_string()),
            asset_name: "Cardano".to_string(),
            network_name: "Cardano".to_string(),
            coingecko_id: "cardano".to_string(),
            asset_source: Some("bitgarth_catalog".to_string()),
            precision_source: Some("bitgarth_catalog".to_string()),
            coingecko_platform_id: None,
            provider_platform_asset_ref: None,
        }];
        let manual_assertions = vec![WalletDataManualAssertionExportSource {
            account_id: manual_account_id,
            asserted_on: NaiveDate::from_ymd_opt(2026, 3, 1).expect("date should parse"),
            asserted_balance: UnsignedAmount::from_u128(15_000_000),
            note: Some("Supported snapshot".to_string()),
        }];

        let export = build_wallet_data_export_payload_view(WalletDataExportBuildInput {
            exported_at: fixed_datetime(),
            bitgarth_version: "0.1.0-test",
            username: "testuser",
            wallets: vec![wallet],
            manual_asset_accounts,
            manual_assertions,
            sync_slots: Vec::new(),
            user_settings: None,
            api_keys: Vec::new(),
            premium_transfer: None,
        })
        .expect("wallet data export should build");

        let payload =
            serde_json::to_value(&export.payload).expect("wallet data export should serialize");
        for wallet in payload["wallets"].as_array().expect("wallet array") {
            assert!(wallet.get("legacy_custom_asset_accounts").is_none());
        }
        assert_eq!(export.summary.custom_accounts, 1);
        assert_eq!(export.summary.balance_assertions, 1);

        let wallet_payload = export.payload.wallets.first().expect("wallet");
        assert_eq!(wallet_payload.manual_asset_accounts.len(), 1);
        assert_eq!(
            wallet_payload.manual_asset_accounts[0]
                .asset_instance_id
                .asset_id,
            "cardano"
        );
        assert_eq!(
            wallet_payload.manual_asset_accounts[0]
                .asset_instance_id
                .network_id,
            "cardano-mainnet"
        );
        assert_eq!(wallet_payload.manual_asset_accounts[0].unit_code, "ADA");
        assert_eq!(wallet_payload.manual_asset_accounts[0].decimal_precision, 6);
        assert_eq!(
            wallet_payload.manual_asset_accounts[0].asset_name,
            "Cardano"
        );
        assert_eq!(
            wallet_payload.manual_asset_accounts[0].network_name,
            "Cardano"
        );
        assert_eq!(
            wallet_payload.manual_asset_accounts[0].coingecko_id,
            "cardano"
        );
        assert_eq!(
            wallet_payload.manual_asset_accounts[0]
                .asset_source
                .as_deref(),
            Some("bitgarth_catalog")
        );
        assert_eq!(
            wallet_payload.manual_asset_accounts[0]
                .precision_source
                .as_deref(),
            Some("bitgarth_catalog")
        );
        assert_eq!(
            wallet_payload.manual_asset_accounts[0]
                .balance_assertions
                .len(),
            1
        );
    }

    #[test]
    fn wallet_data_export_builder_errors_when_assertion_account_is_missing() {
        let wallet = sample_wallet();

        let result = build_wallet_data_export_payload_view(WalletDataExportBuildInput {
            exported_at: fixed_datetime(),
            bitgarth_version: "0.1.0-test",
            username: "testuser",
            wallets: vec![wallet],
            manual_asset_accounts: Vec::new(),
            manual_assertions: vec![WalletDataManualAssertionExportSource {
                account_id: WalletAccountId::new(),
                asserted_on: NaiveDate::from_ymd_opt(2026, 3, 1).expect("date should parse"),
                asserted_balance: UnsignedAmount::from_u128(1),
                note: None,
            }],
            sync_slots: Vec::new(),
            user_settings: None,
            api_keys: Vec::new(),
            premium_transfer: None,
        });

        assert!(matches!(
            result,
            Err(ExportError::Internal(message))
            if message.contains("references missing manual account")
        ));
    }

    #[test]
    fn wallet_data_zip_unencrypted_round_trips_payload_json() {
        let zip_bytes = wrap_in_wallet_data_zip(r#"{"version":3,"wallets":[]}"#, None)
            .expect("zip should build");

        let payload = unwrap_wallet_data_zip(&zip_bytes, None).expect("zip should unwrap");

        assert_eq!(payload.payload_json, r#"{"version":3,"wallets":[]}"#);
        assert_eq!(payload.inner_file_name, WALLET_DATA_INNER_ENTRY_NAME);
        assert!(!payload.encrypted);
    }

    #[test]
    fn unwrap_wallet_data_payload_accepts_raw_json() {
        let payload = unwrap_wallet_data_payload(br#"  {"version":4,"wallets":[]}"#, None)
            .expect("raw JSON should unwrap");

        assert_eq!(payload.payload_json, r#"  {"version":4,"wallets":[]}"#);
        assert_eq!(payload.inner_file_name, WALLET_DATA_INNER_ENTRY_NAME);
        assert!(!payload.encrypted);
    }

    #[test]
    fn wallet_data_zip_encrypted_round_trips_payload_json() {
        let zip_bytes =
            wrap_in_wallet_data_zip(r#"{"version":3,"wallets":[]}"#, Some("correct-password"))
                .expect("encrypted zip should build");

        let payload = unwrap_wallet_data_zip(&zip_bytes, Some("correct-password"))
            .expect("zip should unwrap");

        assert_eq!(payload.payload_json, r#"{"version":3,"wallets":[]}"#);
        assert_eq!(payload.inner_file_name, WALLET_DATA_INNER_ENTRY_NAME);
        assert!(payload.encrypted);
    }

    #[test]
    fn wallet_data_zip_encrypted_without_password_requests_password() {
        let zip_bytes =
            wrap_in_wallet_data_zip(r#"{"version":3,"wallets":[]}"#, Some("correct-password"))
                .expect("encrypted zip should build");

        let result = unwrap_wallet_data_zip(&zip_bytes, None);

        assert!(matches!(result, Err(ExportError::PasswordRequired(_))));
    }

    #[test]
    fn wallet_data_zip_wrong_password_returns_auth_failed() {
        let zip_bytes =
            wrap_in_wallet_data_zip(r#"{"version":3,"wallets":[]}"#, Some("correct-password"))
                .expect("encrypted zip should build");

        let result = unwrap_wallet_data_zip(&zip_bytes, Some("wrong-password"));

        assert!(matches!(
            result,
            Err(ExportError::EncryptedZipAuthFailed(_))
        ));
    }

    #[test]
    fn wallet_data_zip_rejects_multiple_entries() {
        let cursor = Cursor::new(Vec::new());
        let mut archive = ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated);
        archive
            .start_file(WALLET_DATA_INNER_ENTRY_NAME, options)
            .expect("first entry should start");
        archive.write_all(b"{}").expect("first entry should write");
        archive
            .start_file("extra.json", options)
            .expect("second entry should start");
        archive.write_all(b"{}").expect("second entry should write");
        let zip_bytes = archive.finish().expect("zip should finish").into_inner();

        let result = unwrap_wallet_data_zip(&zip_bytes, None);

        assert!(matches!(result, Err(ExportError::Validation(_))));
    }

    #[test]
    fn wallet_data_zip_rejects_non_zip_bytes() {
        let result = unwrap_wallet_data_zip(b"not a zip", None);

        assert!(matches!(result, Err(ExportError::Validation(_))));
    }
}
