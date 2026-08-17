use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::balance_reliability::BalanceReliability;
use crate::report_access::ReportAccessView;
use crate::transactions::{AccountTransactionDirection, AddressBalanceSummary};
use crate::wallets::WalletAccountId;

use crate::backend::ApiErrorEnvelope;

// ============ API Response Types (UI-Focused) ============

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WalletsResponse {
    pub(crate) wallets: Vec<WalletView>,
    #[serde(default)]
    pub(crate) value_summary: Option<WalletsValueSummaryView>,
    pub(crate) account_limit: AccountLimitView,
    pub(crate) sync_capacity: SyncedAccountCapacityView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WalletView {
    pub(crate) id: crate::wallets::WalletId,
    pub(crate) label: String,
    pub(crate) master_fingerprint: Option<String>,
    pub(crate) logical_account_count: u32,
    pub(crate) has_accessors: bool,
    pub(crate) balances: Vec<WalletAggregateBalanceView>,
    pub(crate) accounts: Vec<AccountView>,
    #[serde(default)]
    pub(crate) value_summary: Option<WalletValueSummaryView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WalletAggregateBalanceView {
    pub(crate) asset_id: String,
    pub(crate) network_id: String,
    pub(crate) unit_code: String,
    pub(crate) symbol: Option<String>,
    pub(crate) balance_reliability: BalanceReliability,
    pub(crate) balance_state: AccountBalanceStateView,
    #[serde(default)]
    pub(crate) current_value: Option<CurrentAssetValueView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CurrentAssetValueView {
    pub(crate) price: String,
    pub(crate) converted_value: String,
    pub(crate) currency: crate::models::CurrencyCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WalletValueSummaryView {
    pub(crate) priced_total: String,
    pub(crate) currency: crate::models::CurrencyCode,
    pub(crate) priced_asset_count: u32,
    pub(crate) total_asset_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WalletsValueSummaryView {
    pub(crate) priced_total: String,
    pub(crate) currency: crate::models::CurrencyCode,
    pub(crate) priced_asset_count: u32,
    pub(crate) total_asset_count: u32,
    pub(crate) priced_wallet_count: u32,
    pub(crate) total_wallet_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountStateView {
    Active,
    Inactive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccountLimitNoticeView {
    pub(crate) message: String,
    pub(crate) active_account_limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccountCreationStateView {
    pub(crate) account_id: WalletAccountId,
    pub(crate) account_state: AccountStateView,
    #[serde(default)]
    pub(crate) account_limit_notice: Option<AccountLimitNoticeView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccountLimitView {
    pub(crate) active_count: u16,
    pub(crate) inactive_count: u16,
    pub(crate) active_limit: u16,
    pub(crate) hard_cap: u16,
    pub(crate) summary: String,
    #[serde(default)]
    pub(crate) upgrade_call_to_action: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WalletBalanceContextView {
    pub(crate) network: crate::wallets::Network,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BalanceAmountView {
    pub(crate) raw_value: String,
    pub(crate) formatted_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FiatAmountView {
    pub(crate) raw_value: String,
    pub(crate) formatted_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WalletBalanceView {
    pub(crate) asset_id: crate::wallets::SyncedAssetId,
    pub(crate) context: WalletBalanceContextView,
    pub(crate) unit_code: String,
    pub(crate) symbol: Option<String>,
    pub(crate) balance_reliability: BalanceReliability,
    pub(crate) balance_state: AccountBalanceStateView,
    #[serde(default)]
    pub(crate) current_value: Option<CurrentAssetValueView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountReferenceKind {
    ExtendedPubkey,
    SingleAddress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) struct AccountTransactionCountsView {
    pub(crate) pending: u32,
    pub(crate) confirmed: u32,
    pub(crate) dropped: u32,
    pub(crate) failed: u32,
    pub(crate) total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeAccountSyncSlotView {
    pub(crate) selected: bool,
    pub(crate) active: bool,
    pub(crate) can_select: bool,
    pub(crate) limit: u16,
    pub(crate) selected_at: Option<String>,
    pub(crate) selected_under_tier: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManualSyncMode {
    TransactionHistory,
    BalanceRefresh,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManualSyncSlotEffect {
    AlreadySelected,
    WillSelectAvailableSlot,
    NoCapacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManualSyncDisabledReason {
    SyncUnavailableOnPlan,
    AccountInactive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeAccountManualSyncView {
    pub(crate) mode: ManualSyncMode,
    pub(crate) slot_effect: ManualSyncSlotEffect,
    pub(crate) disabled_reason: Option<ManualSyncDisabledReason>,
    pub(crate) used_slots: u16,
    pub(crate) slot_limit: u16,
    pub(crate) next_tier_display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SyncedAccountCapacityView {
    pub(crate) used_slots: u16,
    pub(crate) slot_limit: u16,
    pub(crate) available_slots: u16,
    pub(crate) summary: String,
    pub(crate) next_tier_display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct NativeAccountView {
    pub(crate) account_id: WalletAccountId,
    pub(crate) native_account_id: crate::wallets::DigitalAssetAccountId,
    pub(crate) account_number: u32,
    pub(crate) account_state: AccountStateView,
    pub(crate) asset: crate::wallets::SyncedAssetId,
    pub(crate) scheme: crate::wallets::AddressScheme,
    pub(crate) label: String,
    pub(crate) derivation_path: Option<String>,
    pub(crate) account_reference_kind: AccountReferenceKind,
    pub(crate) account_reference: String,
    pub(crate) balance: WalletBalanceView,
    pub(crate) transaction_counts: AccountTransactionCountsView,
    pub(crate) has_derived_addresses: bool,
    pub(crate) sync_slot: NativeAccountSyncSlotView,
    pub(crate) manual_sync: NativeAccountManualSyncView,
    #[serde(default, skip_serializing)]
    pub(crate) addresses: AddressesView,
    #[serde(default, skip_serializing)]
    pub(crate) transactions: Vec<AccountTransactionView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AccountBalanceStateView {
    Known { amount: BalanceAmountView },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "amount", rename_all = "snake_case")]
pub(crate) enum NativeBalanceStateView {
    Known(BalanceAmountView),
    CanonicalZero,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CustomAccountView {
    pub(crate) account_id: WalletAccountId,
    pub(crate) label: String,
    pub(crate) unit_code: String,
    pub(crate) decimal_precision: u8,
    pub(crate) symbol: Option<String>,
    pub(crate) balance_state: AccountBalanceStateView,
    #[serde(default)]
    pub(crate) current_value: Option<CurrentAssetValueView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ManualAssetAccountView {
    pub(crate) account_id: WalletAccountId,
    pub(crate) account_state: AccountStateView,
    pub(crate) label: String,
    pub(crate) asset_instance_id: crate::asset_views::ManualAssetInstanceIdView,
    pub(crate) unit_code: String,
    pub(crate) asset_name: String,
    pub(crate) network_name: String,
    pub(crate) decimal_precision: u8,
    pub(crate) symbol: Option<String>,
    pub(crate) balance_state: AccountBalanceStateView,
    #[serde(default)]
    pub(crate) current_value: Option<CurrentAssetValueView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AccountView {
    Native(Box<NativeAccountView>),
    Custom(CustomAccountView),
    Manual(ManualAssetAccountView),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub(crate) struct AddressesView {
    pub(crate) receive: Vec<AddressView>,
    pub(crate) change: Vec<AddressView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddressView {
    pub(crate) address: String,
    pub(crate) derivation_index: u32,
    pub(crate) balance: Option<AddressBalanceSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AccountTransactionView {
    pub(crate) tx_hash: String,
    pub(crate) status: crate::transactions::ChainTransactionStatus,
    pub(crate) direction: AccountTransactionDirection,
    pub(crate) transfer_kind: Option<String>,
    pub(crate) value: BalanceAmountView,
    pub(crate) fee: Option<BalanceAmountView>,
    pub(crate) from_address: Option<String>,
    pub(crate) to_address: Option<String>,
    pub(crate) block_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WalletReportResponse {
    pub(crate) wallet_label: String,
    pub(crate) resolved_from: NaiveDate,
    pub(crate) resolved_to: NaiveDate,
    pub(crate) default_this_year_from: NaiveDate,
    pub(crate) default_this_year_to: NaiveDate,
    pub(crate) access: ReportAccessView,
    pub(crate) accounts: Vec<WalletReportAccountRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct HoldingsReportResponse {
    pub(crate) resolved_from: NaiveDate,
    pub(crate) resolved_to: NaiveDate,
    pub(crate) default_this_year_from: NaiveDate,
    pub(crate) default_this_year_to: NaiveDate,
    pub(crate) access: ReportAccessView,
    pub(crate) wallets: Vec<HoldingsReportWalletRow>,
    pub(crate) price_requirements: Vec<(
        crate::services::price_overrides::PriceSubject,
        crate::services::price_overrides::BoundaryKind,
    )>,
    pub(crate) subject_labels: Vec<(crate::services::price_overrides::PriceSubject, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct HoldingsReportWalletRow {
    pub(crate) wallet_id: crate::wallets::WalletId,
    pub(crate) wallet_label: String,
    pub(crate) opening_fiat: Option<FiatAmountView>,
    pub(crate) closing_fiat: Option<FiatAmountView>,
    pub(crate) change_fiat: Option<FiatAmountView>,
    pub(crate) change_percent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "balance", rename_all = "snake_case")]
pub(crate) enum WalletReportBalanceStateView {
    CanonicalZero,
    NeedsPrice(BalanceAmountView),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WalletReportAccountRow {
    pub(crate) account_id: crate::wallets::WalletAccountId,
    pub(crate) account_label: String,
    pub(crate) catalog_asset_key: Option<crate::asset_views::CatalogAssetKey>,
    pub(crate) asset_display_name: Option<String>,
    pub(crate) unit_code: String,
    pub(crate) symbol: Option<String>,
    #[serde(default)]
    pub(crate) bitcoin_history_coverage:
        Option<crate::balance_reliability::BitcoinHistoryCoverageView>,
    pub(crate) opening_balance_state: WalletReportBalanceStateView,
    pub(crate) opening_balance: Option<BalanceAmountView>,
    pub(crate) opening_balance_date: Option<NaiveDate>,
    pub(crate) closing_balance_state: WalletReportBalanceStateView,
    pub(crate) closing_balance: Option<BalanceAmountView>,
    pub(crate) closing_balance_date: Option<NaiveDate>,
}

// ============ Xpub Validation Response Types ============
// These types are used by the UI components in AddXpubFlow (constructed via
// serde deserialization of server function responses on the client side).

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ValidateXpubResponse {
    pub(crate) schemes: Vec<SchemeValidationResult>,
    pub(crate) suggested_scheme: crate::wallets::AddressScheme,
    pub(crate) existing_wallet: Option<ExistingNormalizedKeyWallet>,
    pub(crate) already_linked: Option<AlreadyLinkedWallet>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SchemeValidationResult {
    pub(crate) address_scheme: crate::wallets::AddressScheme,
    pub(crate) scheme_note: String,
    pub(crate) first_address: String,
    pub(crate) has_activity: Option<bool>,
    pub(crate) activity_check_error: Option<String>,
    pub(crate) already_linked: bool,
    pub(crate) linked_wallet_label: Option<String>,
    pub(crate) linked_account_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AlreadyLinkedWallet {
    pub(crate) wallet_id: crate::wallets::WalletId,
    pub(crate) wallet_label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ExistingNormalizedKeyWallet {
    pub(crate) wallet_id: crate::wallets::WalletId,
    pub(crate) wallet_label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AddXpubResponse {
    pub(crate) wallet_id: crate::wallets::WalletId,
    pub(crate) account_id: crate::wallets::DigitalAssetAccountId,
    pub(crate) account_state: AccountStateView,
    #[serde(default)]
    pub(crate) account_limit_notice: Option<AccountLimitNoticeView>,
}

pub(crate) type WalletError = ApiErrorEnvelope;
