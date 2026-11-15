//! Typed views of `/api/v1` responses, limited to what the CLI renders.
//! `--json` prints the server's full response instead, so these stay small.
//! Enum-like fields are kept as strings so newer server values still render.

use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct Me {
    pub(crate) username: Option<String>,
    pub(crate) client: MeClient,
    pub(crate) server_version: String,
}

#[derive(Deserialize)]
pub(crate) struct MeClient {
    pub(crate) name: String,
    pub(crate) permission: String,
    pub(crate) expires_at: Option<String>,
}

/// `/api/v1/wallet-balances`, the only view legacy `balances_read` keys get.
#[derive(Deserialize)]
pub(crate) struct LegacyBalances {
    pub(crate) wallets: Vec<LegacyWallet>,
}

#[derive(Deserialize)]
pub(crate) struct LegacyWallet {
    pub(crate) name: String,
    pub(crate) balances: Vec<LegacyBalance>,
}

#[derive(Deserialize)]
pub(crate) struct LegacyBalance {
    pub(crate) unit: String,
    pub(crate) amount: Option<String>,
    pub(crate) status: String,
    pub(crate) reasons: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct Wallets {
    pub(crate) wallets: Vec<Wallet>,
    #[serde(default)]
    pub(crate) value_summary: Option<ValueSummary>,
    pub(crate) account_limit: AccountLimit,
}

#[derive(Deserialize)]
pub(crate) struct AccountLimit {
    pub(crate) summary: String,
    #[serde(default)]
    pub(crate) upgrade_call_to_action: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ValueSummary {
    pub(crate) priced_total: String,
    pub(crate) currency: String,
    pub(crate) priced_asset_count: u32,
    pub(crate) total_asset_count: u32,
}

#[derive(Deserialize)]
pub(crate) struct Wallet {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) accounts: Vec<Account>,
    #[serde(default)]
    pub(crate) value_summary: Option<ValueSummary>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Account {
    Native(Box<NativeAccount>),
    Custom(CustomAccount),
    Manual(ManualAccount),
}

#[derive(Deserialize)]
pub(crate) struct NativeAccount {
    pub(crate) account_id: String,
    pub(crate) native_account_id: String,
    pub(crate) account_state: String,
    pub(crate) asset: String,
    pub(crate) scheme: String,
    pub(crate) label: String,
    pub(crate) account_reference_kind: String,
    pub(crate) account_reference: String,
    pub(crate) balance: NativeBalance,
    pub(crate) transaction_counts: TransactionCounts,
    pub(crate) account_mode: String,
}

#[derive(Deserialize)]
pub(crate) struct NativeBalance {
    pub(crate) unit_code: String,
    pub(crate) balance_reliability: Reliability,
    pub(crate) balance_state: BalanceState,
    #[serde(default)]
    pub(crate) current_value: Option<CurrentValue>,
}

#[derive(Deserialize)]
pub(crate) struct Reliability {
    pub(crate) kind: String,
}

#[derive(Deserialize)]
pub(crate) struct TransactionCounts {
    pub(crate) total: u32,
}

#[derive(Deserialize)]
pub(crate) struct CustomAccount {
    pub(crate) account_id: String,
    pub(crate) label: String,
    pub(crate) unit_code: String,
    pub(crate) balance_state: BalanceState,
    #[serde(default)]
    pub(crate) current_value: Option<CurrentValue>,
}

#[derive(Deserialize)]
pub(crate) struct ManualAccount {
    pub(crate) account_id: String,
    pub(crate) account_state: String,
    pub(crate) label: String,
    pub(crate) asset_instance_id: AssetInstanceId,
    pub(crate) unit_code: String,
    pub(crate) asset_name: String,
    pub(crate) balance_state: BalanceState,
    #[serde(default)]
    pub(crate) current_value: Option<CurrentValue>,
}

#[derive(Deserialize, Clone)]
pub(crate) struct AssetInstanceId {
    pub(crate) asset_id: String,
    pub(crate) network_id: String,
}

impl AssetInstanceId {
    pub(crate) fn display(&self) -> String {
        format!("{}@{}", self.asset_id, self.network_id)
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum BalanceState {
    Known { amount: Amount },
    Unknown,
}

impl BalanceState {
    pub(crate) fn formatted(&self) -> Option<&str> {
        match self {
            Self::Known { amount } => Some(&amount.formatted_value),
            Self::Unknown => None,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct Amount {
    pub(crate) formatted_value: String,
}

#[derive(Deserialize)]
pub(crate) struct CurrentValue {
    pub(crate) price: String,
    pub(crate) converted_value: String,
    pub(crate) currency: String,
}

/// The fields every account kind shares, for listing and lookup.
pub(crate) struct AccountSummary<'a> {
    pub(crate) id: &'a str,
    pub(crate) label: &'a str,
    pub(crate) kind: String,
    pub(crate) unit: &'a str,
    pub(crate) balance: Option<&'a str>,
    pub(crate) provisional: bool,
    pub(crate) value: Option<&'a CurrentValue>,
    pub(crate) inactive: bool,
}

impl Account {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Native(account) => &account.account_id,
            Self::Custom(account) => &account.account_id,
            Self::Manual(account) => &account.account_id,
        }
    }

    pub(crate) fn label(&self) -> &str {
        match self {
            Self::Native(account) => &account.label,
            Self::Custom(account) => &account.label,
            Self::Manual(account) => &account.label,
        }
    }

    pub(crate) fn summary(&self) -> AccountSummary<'_> {
        match self {
            Self::Native(account) => AccountSummary {
                id: &account.account_id,
                label: &account.label,
                kind: match account.account_reference_kind.as_str() {
                    "extended_pubkey" => format!("{} xpub", title_case(&account.asset)),
                    _ => format!("{} address", title_case(&account.asset)),
                },
                unit: &account.balance.unit_code,
                balance: account.balance.balance_state.formatted(),
                provisional: account.balance.balance_reliability.kind != "final",
                value: account.balance.current_value.as_ref(),
                inactive: account.account_state == "inactive",
            },
            Self::Custom(account) => AccountSummary {
                id: &account.account_id,
                label: &account.label,
                kind: "Custom".to_owned(),
                unit: &account.unit_code,
                balance: account.balance_state.formatted(),
                provisional: false,
                value: account.current_value.as_ref(),
                inactive: false,
            },
            Self::Manual(account) => AccountSummary {
                id: &account.account_id,
                label: &account.label,
                kind: "Manual".to_owned(),
                unit: &account.unit_code,
                balance: account.balance_state.formatted(),
                provisional: false,
                value: account.current_value.as_ref(),
                inactive: account.account_state == "inactive",
            },
        }
    }
}

pub(crate) fn title_case(value: &str) -> String {
    value
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ============ Settings ============

#[derive(Deserialize)]
pub(crate) struct Settings {
    pub(crate) currency: Option<String>,
    pub(crate) timezone: Option<String>,
    pub(crate) number_format: Option<String>,
    pub(crate) date_format: Option<String>,
    pub(crate) price_fetching: bool,
    pub(crate) mempool_url: Option<String>,
    pub(crate) effective_mempool_url: Option<String>,
    pub(crate) etherscan_url: Option<String>,
    pub(crate) effective_etherscan_url: Option<String>,
    pub(crate) hledger_prefix: Option<String>,
    pub(crate) default_hledger_prefix: String,
    pub(crate) secrets: Secrets,
}

#[derive(Deserialize)]
pub(crate) struct Secrets {
    pub(crate) etherscan_api_key: bool,
    pub(crate) coingecko_api_key: bool,
}

// ============ Sync ============

#[derive(Deserialize)]
pub(crate) struct SyncStatus {
    pub(crate) overall: OverallSync,
    pub(crate) accounts: Vec<AccountSync>,
}

#[derive(Deserialize)]
pub(crate) struct OverallSync {
    pub(crate) is_running: bool,
    pub(crate) last_completed_at: Option<String>,
    pub(crate) last_result: Option<String>,
    pub(crate) last_error: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct AccountSync {
    pub(crate) account_id: String,
    pub(crate) addresses_total: u64,
    pub(crate) addresses_synced: u64,
    pub(crate) addresses_failed: u64,
    pub(crate) addresses_in_progress: u64,
    pub(crate) last_success_at: Option<String>,
    pub(crate) last_result: Option<String>,
    pub(crate) last_error: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SyncStarted {
    pub(crate) outcome: String,
    pub(crate) sync_run_id: String,
}

// ============ Assets ============

#[derive(Deserialize)]
pub(crate) struct AssetSearch {
    pub(crate) results: Vec<AssetSearchRow>,
    #[serde(default)]
    pub(crate) total_match_count: usize,
}

#[derive(Deserialize)]
pub(crate) struct AssetSearchRow {
    pub(crate) asset_instance_id: Option<AssetInstanceId>,
    pub(crate) coingecko_id: Option<String>,
    pub(crate) unit_code: String,
    pub(crate) asset_name: String,
    pub(crate) network_name: String,
}

// ============ Account creation ============

#[derive(Deserialize)]
pub(crate) struct Created {
    pub(crate) wallet_id: String,
    pub(crate) account_id: String,
    pub(crate) account_state: String,
    #[serde(default)]
    pub(crate) account_limit_notice: Option<LimitNotice>,
}

#[derive(Deserialize)]
pub(crate) struct LimitNotice {
    pub(crate) message: String,
}

#[derive(Deserialize)]
pub(crate) struct XpubInspection {
    pub(crate) schemes: Vec<XpubScheme>,
    pub(crate) suggested_scheme: String,
    #[serde(default)]
    pub(crate) already_linked: Option<LinkedWallet>,
}

#[derive(Deserialize)]
pub(crate) struct XpubScheme {
    pub(crate) address_scheme: String,
    pub(crate) first_address: String,
    pub(crate) has_activity: Option<bool>,
    pub(crate) already_linked: bool,
}

#[derive(Deserialize)]
pub(crate) struct LinkedWallet {
    pub(crate) wallet_label: String,
}

// ============ Account history ============

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum History {
    Native(Box<NativeHistory>),
    Custom(Box<ManualHistory>),
}

#[derive(Deserialize)]
pub(crate) struct NativeHistory {
    pub(crate) wallet_label: String,
    pub(crate) unit_code: String,
    pub(crate) address_scheme: String,
    pub(crate) network: String,
    pub(crate) account_mode: String,
    pub(crate) pending: TransactionTable,
    pub(crate) confirmed: TransactionTable,
}

#[derive(Deserialize)]
pub(crate) struct TransactionTable {
    pub(crate) total: u32,
    pub(crate) end: u32,
    pub(crate) rows: Vec<TransactionRow>,
}

#[derive(Deserialize)]
pub(crate) struct TransactionRow {
    pub(crate) tx_hash: String,
    pub(crate) status: String,
    pub(crate) direction: String,
    pub(crate) occurred_at: String,
    pub(crate) value: Amount,
    pub(crate) fee: Option<Amount>,
    pub(crate) closing_balance: Option<Amount>,
}

#[derive(Deserialize)]
pub(crate) struct ManualHistory {
    pub(crate) wallet_label: String,
    pub(crate) unit_code: String,
    pub(crate) asset_name: Option<String>,
    pub(crate) assertions: AssertionTable,
}

#[derive(Deserialize)]
pub(crate) struct AssertionTable {
    pub(crate) page: u32,
    pub(crate) total: u32,
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) rows: Vec<AssertionRow>,
}

#[derive(Deserialize)]
pub(crate) struct AssertionRow {
    pub(crate) assertion_id: String,
    pub(crate) asserted_on: String,
    pub(crate) asserted_balance: Amount,
    pub(crate) note: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct Addresses {
    pub(crate) page: u32,
    pub(crate) page_size: u32,
    pub(crate) total: u32,
    pub(crate) rows: Vec<AddressRow>,
}

#[derive(Deserialize)]
pub(crate) struct AddressRow {
    pub(crate) address: String,
    pub(crate) transaction_count: u64,
    pub(crate) balance: NativeBalance,
    pub(crate) derivation_path: String,
}

// ============ Reports ============

#[derive(Deserialize)]
pub(crate) struct HoldingsReport {
    pub(crate) resolved_from: String,
    pub(crate) resolved_to: String,
    pub(crate) access: ReportAccess,
    pub(crate) wallets: Vec<HoldingsRow>,
}

#[derive(Deserialize)]
pub(crate) struct ReportAccess {
    pub(crate) range_clamped: bool,
    pub(crate) effective_from: String,
}

#[derive(Deserialize)]
pub(crate) struct HoldingsRow {
    pub(crate) wallet_label: String,
    pub(crate) opening_fiat: Option<Amount>,
    pub(crate) closing_fiat: Option<Amount>,
    pub(crate) change_fiat: Option<Amount>,
    pub(crate) change_percent: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct WalletReport {
    pub(crate) wallet_label: String,
    pub(crate) resolved_from: String,
    pub(crate) resolved_to: String,
    pub(crate) access: ReportAccess,
    pub(crate) accounts: Vec<WalletReportRow>,
}

#[derive(Deserialize)]
pub(crate) struct WalletReportRow {
    pub(crate) account_label: String,
    pub(crate) unit_code: String,
    pub(crate) opening_balance: Option<Amount>,
    pub(crate) closing_balance: Option<Amount>,
}

// ============ Prices ============

#[derive(Deserialize)]
pub(crate) struct PriceHistory {
    pub(crate) prices_enabled: bool,
    pub(crate) assets: Vec<HeldAsset>,
    pub(crate) selected: Option<String>,
    pub(crate) currency: String,
    pub(crate) resolved_from: String,
    pub(crate) resolved_to: String,
    pub(crate) entries: Vec<PriceEntry>,
    pub(crate) page: u32,
    pub(crate) total_entries: u32,
}

#[derive(Deserialize)]
pub(crate) struct HeldAsset {
    pub(crate) key: String,
    pub(crate) name: String,
}

#[derive(Deserialize)]
pub(crate) enum PriceEntry {
    Daily {
        date_utc: String,
        price: String,
    },
    YourPrice {
        at: String,
        price: String,
        note: Option<String>,
    },
    Gap {
        from: String,
        to: String,
    },
}
