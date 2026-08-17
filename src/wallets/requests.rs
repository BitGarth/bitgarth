use super::bitcoin::RawBtcAddress;
use super::labels::RawLabel;
use super::manual_assets::ManualAssetAccountTransactionsResponse;
use super::primitives::{
    AddressScheme, DigitalAssetAccountId, DigitalAssetAddressId, Network, RawAccountIndex,
    TransactionSortDirection, WalletAccountId, WalletId,
};
use super::xpub::{RawExtendedPubkey, RawMasterFingerprint, TrezorDeviceId, TrezorDeviceLabel};
use crate::asset_views::ManualAssetInstanceIdView;
use crate::backend::{AccountCreationStateView, AccountLimitNoticeView, AccountStateView};
use crate::models::CurrencyCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::balance_reliability::BalanceReliability;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TrezorAccountLinkRequest {
    pub account_index: RawAccountIndex,
    pub address_scheme: AddressScheme,
    pub extended_pubkey: RawExtendedPubkey,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LinkTrezorRequest {
    pub master_fingerprint: RawMasterFingerprint,
    pub wallet_label: RawLabel,
    pub device_id: Option<TrezorDeviceId>,
    pub device_label: Option<TrezorDeviceLabel>,
    pub accounts: Vec<TrezorAccountLinkRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum LinkTrezorOutcome {
    NewWallet,
    ExistingWallet,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LinkTrezorResponse {
    pub wallet_id: WalletId,
    pub created_account_ids: Vec<DigitalAssetAccountId>,
    #[serde(default)]
    pub created_accounts: Vec<AccountCreationStateView>,
    pub skipped_account_indexes: Vec<super::primitives::AccountIndex>,
    pub outcome: LinkTrezorOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct UpdateWalletLabelRequest {
    pub wallet_id: WalletId,
    pub label: RawLabel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct UpdateAccountLabelRequest {
    pub account_id: WalletAccountId,
    pub label: RawLabel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct DeleteAccountsChoice(bool);

impl DeleteAccountsChoice {
    pub(crate) fn new(value: bool) -> Self {
        Self(value)
    }

    pub(crate) fn value(&self) -> bool {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DeleteWalletRequest {
    pub wallet_id: WalletId,
    pub delete_accounts: DeleteAccountsChoice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DeleteAccountRequest {
    pub account_id: WalletAccountId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum MoveDestination {
    ExistingWallet { wallet_id: WalletId },
    NewWallet { label: RawLabel },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MoveAccountRequest {
    pub account_id: WalletAccountId,
    pub destination: MoveDestination,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MoveAccountResponse {
    pub destination_wallet_id: WalletId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SelectAccountSyncSlotRequest {
    pub account_id: DigitalAssetAccountId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddManualAssetAccountRequest {
    pub wallet_id: Option<WalletId>,
    pub wallet_label: Option<RawLabel>,
    pub asset: Option<AddManualAssetAccountAssetRequest>,
    #[serde(default)]
    pub asset_instance_id: Option<ManualAssetInstanceIdView>,
    /// Optional user-provided account name. When omitted, the account is
    /// auto-named ("{UNIT} Account {n}").
    #[serde(default)]
    pub account_label: Option<RawLabel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub(crate) enum AddManualAssetAccountAssetRequest {
    BitGarthCatalog {
        asset_instance_id: ManualAssetInstanceIdView,
    },
    CoinGeckoDiscovery {
        snapshot: CoinGeckoManualAssetSnapshotRequest,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CoinGeckoManualAssetSnapshotRequest {
    pub asset_id: String,
    pub network_id: String,
    pub decimal_precision: i64,
    pub unit_code: String,
    pub symbol: Option<String>,
    pub asset_name: String,
    pub network_name: String,
    pub coingecko_id: String,
    pub coingecko_platform_id: Option<String>,
    pub provider_platform_asset_ref: Option<String>,
    pub precision_source: CoinGeckoManualAssetPrecisionSourceRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CoinGeckoManualAssetPrecisionSourceRequest {
    CoingeckoPlatform,
    UserOverride,
    UserDefault,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddManualAssetAccountResponse {
    pub wallet_id: WalletId,
    pub account_id: WalletAccountId,
    pub account_state: AccountStateView,
    #[serde(default)]
    pub account_limit_notice: Option<AccountLimitNoticeView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SearchManualAssetInstancesRequest {
    pub query: String,
    #[serde(default)]
    pub allow_coingecko_catalog_refresh: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManualAssetSearchSource {
    BitGarthCatalog,
    CoinGeckoCatalog,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ManualAssetInstanceSearchRow {
    pub source: ManualAssetSearchSource,
    pub asset_instance_id: Option<ManualAssetInstanceIdView>,
    pub coingecko_id: Option<String>,
    pub unit_code: String,
    pub asset_name: String,
    pub network_name: String,
    pub decimal_precision: Option<u8>,
    pub platform_count: Option<usize>,
    pub platform_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SearchManualAssetInstancesResponse {
    pub results: Vec<ManualAssetInstanceSearchRow>,
    /// True total of matches across both pools, ignoring the display cap.
    #[serde(default)]
    pub total_match_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ManualAssetCatalogTotalResponse {
    /// Deduped, local-only count of searchable manual assets.
    pub total: usize,
    /// True when the local CoinGecko catalog table holds no active rows, i.e.
    /// no user on this device has synced it yet. Drives the "enable CoinGecko"
    /// search hint. False when the prices db could not be opened (unknown).
    #[serde(default)]
    pub coingecko_catalog_empty: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ManualAssetDiscoveryDetailRequest {
    pub coingecko_id: String,
    pub allow_remote_lookup: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ManualAssetDiscoveryDetailResponse {
    pub coingecko_id: String,
    pub name: String,
    pub symbol: String,
    pub suggested_unit_code: Option<String>,
    pub default_decimal_precision: u8,
    pub platforms: Vec<ManualAssetDiscoveryPlatformRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ManualAssetDiscoveryPlatformRow {
    pub provider_platform_id: String,
    pub contract_address: Option<String>,
    pub suggested_decimal_precision: Option<u8>,
    pub network_id: String,
    pub network_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ManualAssetDiscoveryPriceRequest {
    pub asset_id: String,
    pub coingecko_id: String,
    pub quote_currency: CurrencyCode,
    pub allow_remote_lookup: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ManualAssetDiscoveryPriceResponse {
    pub price: Option<String>,
    pub quote_currency: CurrencyCode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GetAccountAddressesRequest {
    pub account_id: DigitalAssetAccountId,
    pub address_scheme: AddressScheme,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AccountAddressSyncStatusResponse {
    pub status: crate::transactions::AccountAddressSyncStatus,
    pub last_completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<crate::transactions::SyncErrorMessage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AccountAddressRowResponse {
    pub address: String,
    pub sync: AccountAddressSyncStatusResponse,
    pub transaction_count: crate::transactions::TransactionCount,
    pub reported_transaction_count: Option<crate::transactions::TransactionCount>,
    pub balance: crate::backend::WalletBalanceView,
    pub derivation_path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GetAccountAddressesResponse {
    pub page: u32,
    pub page_size: u32,
    pub total: u32,
    pub rows: Vec<AccountAddressRowResponse>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GetAccountTransactionsRequest {
    pub account_id: WalletAccountId,
    pub pending_page: Option<u32>,
    pub confirmed_page: Option<u32>,
    pub sort: Option<String>,
    pub filters: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RawTransactionFilters {
    pub status: Option<Vec<String>>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AccountTransactionRowResponse {
    pub tx_hash: String,
    pub status: crate::transactions::ChainTransactionStatus,
    pub direction: crate::transactions::AccountTransactionDirection,
    pub occurred_at: String,
    pub from_addresses: Vec<String>,
    pub to_addresses: Vec<String>,
    pub value: crate::backend::BalanceAmountView,
    pub fee: Option<crate::backend::BalanceAmountView>,
    pub closing_balance: Option<crate::backend::BalanceAmountView>,
    pub balance_reliability: BalanceReliability,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AccountTransactionTableResponse {
    pub page: u32,
    pub page_size: u32,
    pub total: u32,
    pub start: u32,
    pub end: u32,
    pub rows: Vec<AccountTransactionRowResponse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransactionsEmptyHint {
    FreePlanNoHistory,
    FreePlanBalanceUnavailable,
    PaidPlanNoSyncedTransactions,
    HistorySyncPending { expected_transactions: Option<u32> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tier", rename_all = "snake_case")]
pub(crate) enum TransactionHistoryCoverageNoticeView {
    Free {
        approximate_unsynced_count: u32,
    },
    Paid {
        approximate_unsynced_count: u32,
        confirmed_synced_count: u32,
        max_transactions_per_account: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GetAccountTransactionsResponse {
    pub account_id: WalletAccountId,
    pub wallet_id: WalletId,
    pub wallet_label: String,
    pub account_label: Option<String>,
    pub sync_control_enabled: bool,
    pub native_account_id: DigitalAssetAccountId,
    pub account_reference_kind: crate::backend::AccountReferenceKind,
    pub account_reference: String,
    pub address_scheme: AddressScheme,
    pub asset: super::primitives::SyncedAssetId,
    pub network: Network,
    pub unit_code: String,
    pub symbol: Option<String>,
    #[serde(default)]
    pub bitcoin_history_coverage: Option<crate::balance_reliability::BitcoinHistoryCoverageView>,
    pub sync_slot: Box<crate::backend::NativeAccountSyncSlotView>,
    pub manual_sync: Box<crate::backend::NativeAccountManualSyncView>,
    pub etherscan_history_status: Option<crate::transactions::EtherscanHistoryStatus>,
    pub is_free_tier: bool,
    pub current_balance_state: Box<crate::backend::NativeBalanceStateView>,
    pub current_balance_checked_at: Option<Box<str>>,
    pub transaction_history_coverage_notice: Option<TransactionHistoryCoverageNoticeView>,
    pub opening_balance_state: crate::backend::NativeBalanceStateView,
    pub opening_balance_reliability: BalanceReliability,
    pub opening_balance_date: Option<String>,
    pub closing_balance_state: crate::backend::NativeBalanceStateView,
    pub closing_balance_reliability: BalanceReliability,
    pub closing_balance_date: Option<String>,
    pub sort: TransactionSortDirection,
    pub active_status_filter: Vec<crate::transactions::ChainTransactionStatus>,
    pub active_from_date: Option<String>,
    pub active_to_date: Option<String>,
    pub confirmed_empty_hint: Option<TransactionsEmptyHint>,
    pub pending: AccountTransactionTableResponse,
    pub confirmed: AccountTransactionTableResponse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum WalletAccountHistoryResponse {
    Native(GetAccountTransactionsResponse),
    Custom(ManualAssetAccountTransactionsResponse),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GetWalletByFingerprintRequest {
    pub master_fingerprint: RawMasterFingerprint,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddEthAddressRequest {
    pub address: crate::ethereum::RawEthAddress,
    pub network: Network,
    pub wallet_id: Option<WalletId>,
    pub wallet_label: Option<RawLabel>,
    /// Optional user-provided account name. When omitted, the account is
    /// auto-named ("Ethereum Account {n}").
    #[serde(default)]
    pub account_label: Option<RawLabel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddEthAddressResponse {
    pub wallet_id: WalletId,
    pub account_id: DigitalAssetAccountId,
    pub address_id: DigitalAssetAddressId,
    pub account_state: AccountStateView,
    #[serde(default)]
    pub account_limit_notice: Option<AccountLimitNoticeView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddBtcAddressRequest {
    pub address: RawBtcAddress,
    pub network: Network,
    pub wallet_id: Option<WalletId>,
    pub wallet_label: Option<RawLabel>,
    /// Optional user-provided account name. When omitted, the account is
    /// auto-named ("Bitcoin Account {n}").
    #[serde(default)]
    pub account_label: Option<RawLabel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddBtcAddressResponse {
    pub wallet_id: WalletId,
    pub account_id: DigitalAssetAccountId,
    pub address_id: DigitalAssetAddressId,
    pub account_state: AccountStateView,
    #[serde(default)]
    pub account_limit_notice: Option<AccountLimitNoticeView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ValidateXpubRequest {
    pub extended_pubkey: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddXpubRequest {
    pub extended_pubkey: String,
    pub address_scheme: AddressScheme,
    pub wallet_id: Option<WalletId>,
    pub wallet_label: Option<RawLabel>,
    /// Optional user-provided account name. When omitted, the account is
    /// auto-named ("Bitcoin Account {n}").
    #[serde(default)]
    pub account_label: Option<RawLabel>,
}
