use super::GENERATED_HEADER;
use super::format::{
    AggregatedAccountTransaction, BalanceAssertionSource, CommodityDirective,
    CustomBalanceAssertionRenderRow, HledgerFormatError, NativeTransactionRenderContext,
    build_custom_balance_assertion_transaction, build_hledger_transaction,
    format_directives_journal, format_hledger_commodity,
};
use super::label::{
    HledgerAccountSegments, normalize_label_for_hledger, resolve_segment_collisions,
};
use super::queries::{
    ExportAccountBoundaryMode, ExportAccountRow, ExportAccountTransactionLedgerRow,
    ExportManualAssetBalanceAssertionRow, ExportNativeApiBalanceAssertionRow,
    load_all_accounts_for_export, load_all_confirmed_account_transaction_ledger_rows_for_export,
    load_all_manual_asset_balance_assertion_rows_for_export,
    load_all_native_api_balance_assertion_rows_for_export,
    load_incomplete_bitcoin_account_ids_for_export,
};
use crate::amounts::{UnsignedAmount, format_unsigned_amount_fixed};
use crate::db::with_db;
use crate::hledger_owner::hledger_owner_segments_from_username;
#[cfg(test)]
use crate::hledger_owner::{normalize_owner_directory_segment, normalize_owner_posting_segment};
use crate::models::{HledgerAccountPrefix, UserId};
#[cfg(all(test, feature = "db-tests"))]
use crate::project_paths::ensure_dir_exists;
use crate::wallets::{
    Network, SyncedAssetId, WalletAccountId, display_account_label, display_wallet_label,
};
use chrono::{Datelike, NaiveDate};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
use std::io::Write;
#[cfg(all(test, feature = "db-tests"))]
use std::path::{Path, PathBuf};
#[cfg(all(test, feature = "db-tests"))]
use ulid::Ulid;
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
use zip::CompressionMethod;
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
use zip::write::{SimpleFileOptions, ZipWriter};

#[cfg(all(test, feature = "db-tests"))]
const HLEDGER_ROOT_DIR_NAME: &str = "hledger";
#[cfg(all(test, feature = "db-tests"))]
const TMP_ROOT_PREFIX: &str = "hledger.__tmp__";
#[cfg(all(test, feature = "db-tests"))]
const BACKUP_ROOT_PREFIX: &str = "hledger.__old__";
const EMPTY_ACCOUNT_COMMENT: &str = "; No transactions or balance assertions exported.";
const OPENING_BALANCE_PREFIX: &str = "equity:Opening Balances";
const CLOSING_BALANCE_PREFIX: &str = "equity:Closing Balances";

fn generated_file_contents(lines: impl IntoIterator<Item = String>) -> String {
    let mut output = Vec::new();
    output.push(GENERATED_HEADER.to_string());
    output.push(String::new());
    output.extend(lines);
    output.push(String::new());
    output.join("\n")
}

/// A target the hledger export pipeline writes journal entries into.
///
/// Implementations are responsible for materializing each `(relative, contents)`
/// pair somewhere durable. `relative` is always slash-separated and never
/// absolute, empty, or traversal-capable.
pub(crate) trait JournalSink {
    fn write_relative(&mut self, relative: &str, contents: &str) -> Result<(), ExportEngineError>;
}

fn validate_hledger_relative_path(relative: &str) -> Result<(), ExportEngineError> {
    if relative.is_empty() {
        return Err(ExportEngineError::Invariant(
            "Invalid hledger export relative path: path is empty".to_string(),
        ));
    }

    if std::path::Path::new(relative).is_absolute() {
        return Err(ExportEngineError::Invariant(format!(
            "Invalid hledger export relative path {relative:?}: absolute paths are not allowed"
        )));
    }

    for component in relative.split('/') {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(ExportEngineError::Invariant(format!(
                "Invalid hledger export relative path {relative:?}: component {component:?} is not allowed"
            )));
        }
    }

    Ok(())
}

/// Materializes journal entries onto the local filesystem under `root`.
#[cfg(all(test, feature = "db-tests"))]
struct FsSink<'a> {
    root: &'a Path,
}

#[cfg(all(test, feature = "db-tests"))]
impl JournalSink for FsSink<'_> {
    fn write_relative(&mut self, relative: &str, contents: &str) -> Result<(), ExportEngineError> {
        validate_hledger_relative_path(relative)?;

        let mut path = self.root.to_path_buf();
        for component in relative.split('/') {
            path.push(component);
        }
        let parent = path.parent().ok_or_else(|| {
            ExportEngineError::Invariant(format!(
                "Invalid hledger export path without parent: {path:?}"
            ))
        })?;
        ensure_dir_exists(parent).map_err(|err| {
            ExportEngineError::Io(format!(
                "Failed to create hledger export directory at {parent:?}: {err}"
            ))
        })?;
        std::fs::write(&path, contents).map_err(|err| {
            ExportEngineError::Io(format!(
                "Failed to write hledger journal at {path:?}: {err}"
            ))
        })
    }
}

/// Materializes journal entries into a ZIP archive.
///
/// The underlying `zip` crate requires the inner writer to implement `Seek`,
/// so callers buffer into something seekable (typically `Cursor<Vec<u8>>`)
/// and stream the resulting bytes to the HTTP client after `finish` returns.
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
struct ZipSink<W: Write + std::io::Seek> {
    writer: ZipWriter<W>,
    password: Option<String>,
}

#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
impl<W: Write + std::io::Seek> ZipSink<W> {
    fn new(writer: W, password: Option<String>) -> Self {
        Self {
            writer: ZipWriter::new(writer),
            password,
        }
    }

    fn finish(self) -> Result<W, ExportEngineError> {
        let writer = self.writer;
        writer.finish().map_err(|err| {
            ExportEngineError::Io(format!("Failed to finish hledger ZIP archive: {err}"))
        })
    }
}

#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
impl<W: Write + std::io::Seek> JournalSink for ZipSink<W> {
    fn write_relative(&mut self, relative: &str, contents: &str) -> Result<(), ExportEngineError> {
        validate_hledger_relative_path(relative)?;

        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let result = match &self.password {
            Some(password) => self.writer.start_file(
                relative,
                options.with_aes_encryption(zip::AesMode::Aes256, password.as_str()),
            ),
            None => self.writer.start_file(relative, options),
        };
        result.map_err(|err| {
            ExportEngineError::Io(format!(
                "Failed to start hledger ZIP entry {relative}: {err}"
            ))
        })?;
        self.writer.write_all(contents.as_bytes()).map_err(|err| {
            ExportEngineError::Io(format!(
                "Failed to write hledger ZIP entry {relative}: {err}"
            ))
        })?;
        Ok(())
    }
}

const HLEDGER_TEXT_EXTENSION: &str = "j.txt";
const ACCOUNT_JOURNAL_DIR_NAME: &str = "journal";

fn rel_directives() -> String {
    format!("directives.{HLEDGER_TEXT_EXTENSION}")
}

fn rel_owner_dir(owner: &str) -> String {
    owner.to_string()
}

fn rel_wallet_dir(owner: &str, wallet: &str) -> String {
    format!("{}/{}", rel_owner_dir(owner), wallet)
}

fn rel_account_dir(owner: &str, wallet: &str, account: &str) -> String {
    format!("{}/{account}", rel_wallet_dir(owner, wallet))
}

fn rel_account_year_journal(owner: &str, wallet: &str, account: &str, year: &str) -> String {
    format!(
        "{}/{ACCOUNT_JOURNAL_DIR_NAME}/{year}/{year}.{HLEDGER_TEXT_EXTENSION}",
        rel_account_dir(owner, wallet, account)
    )
}

fn rel_account_year_opening_journal(
    owner: &str,
    wallet: &str,
    account: &str,
    year: &str,
) -> String {
    format!(
        "{}/{year}-opening.{HLEDGER_TEXT_EXTENSION}",
        rel_account_dir(owner, wallet, account)
    )
}

fn rel_account_year_closing_journal(
    owner: &str,
    wallet: &str,
    account: &str,
    year: &str,
) -> String {
    format!(
        "{}/{year}-closing.{HLEDGER_TEXT_EXTENSION}",
        rel_account_dir(owner, wallet, account)
    )
}

fn rel_account_year_include_journal(
    owner: &str,
    wallet: &str,
    account: &str,
    year: &str,
) -> String {
    format!(
        "{}/{year}-include.{HLEDGER_TEXT_EXTENSION}",
        rel_account_dir(owner, wallet, account)
    )
}

fn rel_account_all_years_journal(owner: &str, wallet: &str, account: &str) -> String {
    format!(
        "{}/all-years.{HLEDGER_TEXT_EXTENSION}",
        rel_account_dir(owner, wallet, account)
    )
}

fn rel_year_include(dir: &str, year: &str) -> String {
    format!("{dir}/{year}-include.{HLEDGER_TEXT_EXTENSION}")
}

fn rel_all_years(dir: &str) -> String {
    format!("{dir}/all-years.{HLEDGER_TEXT_EXTENSION}")
}

fn rel_root_year_include(year: &str) -> String {
    format!("{year}-include.{HLEDGER_TEXT_EXTENSION}")
}

fn rel_root_all_years() -> String {
    format!("all-years.{HLEDGER_TEXT_EXTENSION}")
}

fn rel_root_entry_journal() -> String {
    format!("bitgarth.{HLEDGER_TEXT_EXTENSION}")
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AccountYearRef {
    owner: String,
    wallet: String,
    account: String,
    year: String,
}

#[derive(Debug, Default)]
struct IncludeIndex {
    wallet_year_accounts: BTreeMap<(String, String, String), BTreeSet<String>>,
    owner_year_wallets: BTreeMap<(String, String), BTreeSet<String>>,
    root_year_owners: BTreeMap<String, BTreeSet<String>>,
}

impl IncludeIndex {
    fn record_account_year(&mut self, entry: AccountYearRef) {
        self.wallet_year_accounts
            .entry((
                entry.owner.clone(),
                entry.wallet.clone(),
                entry.year.clone(),
            ))
            .or_default()
            .insert(entry.account.clone());
        self.owner_year_wallets
            .entry((entry.owner.clone(), entry.year.clone()))
            .or_default()
            .insert(entry.wallet.clone());
        self.root_year_owners
            .entry(entry.year)
            .or_default()
            .insert(entry.owner);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(all(test, feature = "db-tests"))]
pub(crate) struct ExportResult {
    pub(crate) export_dir: PathBuf,
    pub(crate) accounts_exported: u32,
    pub(crate) transactions_exported: u32,
    pub(crate) balance_assertions_exported: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExportEngineError {
    Query(String),
    Format(String),
    Io(String),
    Invariant(String),
}

impl std::fmt::Display for ExportEngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportEngineError::Query(message) => write!(f, "{message}"),
            ExportEngineError::Format(message) => write!(f, "{message}"),
            ExportEngineError::Io(message) => write!(f, "{message}"),
            ExportEngineError::Invariant(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ExportEngineError {}

impl From<crate::db::DbError> for ExportEngineError {
    fn from(value: crate::db::DbError) -> Self {
        ExportEngineError::Query(value.to_string())
    }
}

impl From<HledgerFormatError> for ExportEngineError {
    fn from(value: HledgerFormatError) -> Self {
        ExportEngineError::Format(value.to_string())
    }
}

#[derive(Debug, Clone)]
struct PendingResolvedAccount {
    account_id: WalletAccountId,
    commodity: super::queries::ExportCommodity,
    boundary_mode: ExportAccountBoundaryMode,
    native_asset_id: Option<SyncedAssetId>,
    native_network: Option<Network>,
}

#[derive(Debug, Clone)]
struct ResolvedAccount {
    account_id: WalletAccountId,
    commodity: super::queries::ExportCommodity,
    boundary_mode: ExportAccountBoundaryMode,
    wallet_segment: String,
    account_segment: String,
    hledger_account_name: String,
    native_render_context: Option<NativeTransactionRenderContext>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HledgerAccountNaming {
    account_prefix: String,
}

impl HledgerAccountNaming {
    pub(crate) fn new(
        hledger_owner_posting_segment: &str,
        hledger_account_prefix: Option<&HledgerAccountPrefix>,
    ) -> Self {
        let account_prefix = hledger_account_prefix
            .map(|prefix| prefix.as_str().to_string())
            .unwrap_or_else(|| format!("assets:{hledger_owner_posting_segment}"));
        Self { account_prefix }
    }

    fn account_name(&self, wallet_segment: &str, account_segment: &str) -> String {
        format!("{}:{wallet_segment}:{account_segment}", self.account_prefix)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExportAccountTransaction {
    Native(AggregatedAccountTransaction),
    ManualAssertion(CustomBalanceAssertionRenderRow),
}

#[derive(Debug)]
enum LedgerRowMappingError {
    Invalid { message: String },
}

#[cfg(all(test, feature = "db-tests"))]
fn export_all_accounts_to_dir(
    user_id: UserId,
    final_hledger_dir: &Path,
    hledger_owner_directory_segment: &str,
    hledger_owner_posting_segment: &str,
) -> Result<ExportResult, ExportEngineError> {
    let account_rows = load_all_accounts_for_export(user_id)?;
    let history_cap = export_history_cap(user_id)?;
    let incomplete_bitcoin_accounts =
        load_incomplete_bitcoin_account_ids_for_export(user_id, &account_rows, history_cap)?;
    let naming = HledgerAccountNaming::new(hledger_owner_posting_segment, None);
    let resolved_accounts = resolve_accounts(account_rows, &naming)?;
    let transactions_by_account =
        load_transactions_by_account(user_id, &incomplete_bitcoin_accounts)?;

    let temp_hledger_dir = create_temp_export_dir(final_hledger_dir)?;
    let mut sink = FsSink {
        root: &temp_hledger_dir,
    };
    let write_result = write_snapshot(
        &mut sink,
        &resolved_accounts,
        &transactions_by_account,
        hledger_owner_directory_segment,
        hledger_owner_posting_segment,
        &incomplete_bitcoin_accounts,
    );

    let write_counts = match write_result {
        Ok(value) => value,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&temp_hledger_dir);
            return Err(err);
        }
    };

    if let Err(err) = replace_export_root_atomically(&temp_hledger_dir, final_hledger_dir) {
        let _ = std::fs::remove_dir_all(&temp_hledger_dir);
        return Err(err);
    }

    let accounts_exported = u32::try_from(resolved_accounts.len()).map_err(|_| {
        ExportEngineError::Invariant("Too many accounts to export into u32 count".to_string())
    })?;

    Ok(ExportResult {
        export_dir: final_hledger_dir.to_path_buf(),
        accounts_exported,
        transactions_exported: write_counts.transactions_exported,
        balance_assertions_exported: write_counts.balance_assertions_exported,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
pub(crate) struct ZipExportResult {
    pub(crate) accounts_exported: u32,
    pub(crate) transactions_exported: u32,
    pub(crate) balance_assertions_exported: u32,
}

/// Builds a hledger export for `user_id` into `writer` as a ZIP archive.
///
/// Optionally encrypts each ZIP entry with AES-256 if `password` is `Some`.
/// The underlying `zip` crate requires the inner writer to implement `Seek`,
/// so production callers should pass a `Cursor<Vec<u8>>` and chunk the
/// resulting bytes onto the network themselves.
#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
pub(crate) fn export_all_accounts_to_zip<W: Write + std::io::Seek>(
    user_id: UserId,
    hledger_owner_directory_segment: &str,
    hledger_owner_posting_segment: &str,
    hledger_account_prefix: Option<&HledgerAccountPrefix>,
    writer: W,
    password: Option<String>,
) -> Result<(W, ZipExportResult), ExportEngineError> {
    let account_rows = load_all_accounts_for_export(user_id)?;
    let history_cap = export_history_cap(user_id)?;
    let incomplete_bitcoin_accounts =
        load_incomplete_bitcoin_account_ids_for_export(user_id, &account_rows, history_cap)?;
    let naming = HledgerAccountNaming::new(hledger_owner_posting_segment, hledger_account_prefix);
    let resolved_accounts = resolve_accounts(account_rows, &naming)?;
    let transactions_by_account =
        load_transactions_by_account(user_id, &incomplete_bitcoin_accounts)?;

    let mut sink = ZipSink::new(writer, password);
    let write_counts = write_snapshot(
        &mut sink,
        &resolved_accounts,
        &transactions_by_account,
        hledger_owner_directory_segment,
        hledger_owner_posting_segment,
        &incomplete_bitcoin_accounts,
    )?;
    let writer = sink.finish()?;

    let accounts_exported = u32::try_from(resolved_accounts.len()).map_err(|_| {
        ExportEngineError::Invariant("Too many accounts to export into u32 count".to_string())
    })?;

    Ok((
        writer,
        ZipExportResult {
            accounts_exported,
            transactions_exported: write_counts.transactions_exported,
            balance_assertions_exported: write_counts.balance_assertions_exported,
        },
    ))
}

fn resolve_accounts(
    account_rows: Vec<ExportAccountRow>,
    naming: &HledgerAccountNaming,
) -> Result<Vec<ResolvedAccount>, ExportEngineError> {
    let mut pending = Vec::new();
    let mut collision_segments = Vec::new();
    for row in account_rows {
        let wallet_label = display_wallet_label(&row.wallet_label);
        let account_label = display_account_label(&row.account_label);

        let wallet_segment = normalize_label_for_hledger(&wallet_label);
        let account_segment = normalize_label_for_hledger(&account_label);
        pending.push(PendingResolvedAccount {
            account_id: row.account_id,
            commodity: row.commodity.clone(),
            boundary_mode: row.boundary_mode,
            native_asset_id: row.native_asset_id,
            native_network: row.native_network,
        });
        collision_segments.push(HledgerAccountSegments {
            account_id: row.account_id,
            wallet_segment,
            account_segment,
        });
    }

    let resolved_segments = resolve_segment_collisions(collision_segments);
    let segment_map: HashMap<WalletAccountId, (String, String)> = resolved_segments
        .into_iter()
        .map(|segment| {
            (
                segment.account_id,
                (segment.wallet_segment, segment.account_segment),
            )
        })
        .collect();

    pending
        .into_iter()
        .map(|account| {
            let (wallet_segment, account_segment) = segment_map
                .get(&account.account_id)
                .cloned()
                .ok_or_else(|| {
                ExportEngineError::Invariant(format!(
                    "Missing resolved segment for account {}",
                    account.account_id
                ))
            })?;
            Ok(ResolvedAccount {
                account_id: account.account_id,
                commodity: account.commodity,
                boundary_mode: account.boundary_mode,
                hledger_account_name: naming.account_name(&wallet_segment, &account_segment),
                native_render_context: native_render_context(
                    account.native_asset_id,
                    account.native_network,
                )?,
                wallet_segment,
                account_segment,
            })
        })
        .collect()
}

fn native_network_display_name(network: Network) -> &'static str {
    match network {
        Network::Mainnet => "Mainnet",
        Network::Testnet => "Testnet",
        Network::Signet => "Signet",
        Network::Regtest => "Regtest",
    }
}

fn native_render_context(
    asset_id: Option<SyncedAssetId>,
    network: Option<Network>,
) -> Result<Option<NativeTransactionRenderContext>, ExportEngineError> {
    match (asset_id, network) {
        (Some(asset_id), Some(network)) => {
            let asset_display_name = asset_id.display_name().to_string();
            let network_display_name = native_network_display_name(network);
            Ok(Some(NativeTransactionRenderContext {
                network_fee_account: format!(
                    "expenses:Fees:{asset_display_name}:Network:{network_display_name}"
                ),
                asset_display_name,
            }))
        }
        (None, None) => Ok(None),
        _ => Err(ExportEngineError::Invariant(
            "native hledger render context requires both asset and network".to_string(),
        )),
    }
}

fn export_history_cap(
    user_id: UserId,
) -> Result<crate::transactions::TransactionCount, ExportEngineError> {
    let entitlements =
        crate::payments::entitlements::load_feature_entitlements(user_id, chrono::Utc::now())?;
    Ok(crate::transactions::TransactionCount::from_u32(
        entitlements.historical_backfill_transactions_per_account,
    ))
}

fn load_transactions_by_account(
    user_id: UserId,
    incomplete_bitcoin_accounts: &std::collections::HashSet<WalletAccountId>,
) -> Result<HashMap<WalletAccountId, Vec<ExportAccountTransaction>>, ExportEngineError> {
    let mut transactions_by_account: HashMap<WalletAccountId, Vec<ExportAccountTransaction>> =
        HashMap::new();
    let ledger_rows = load_all_confirmed_account_transaction_ledger_rows_for_export(user_id)?;

    for row in ledger_rows {
        let account_id = row.account_id;
        match map_account_transaction_ledger_row(row) {
            Ok(aggregated) => {
                transactions_by_account
                    .entry(account_id)
                    .or_default()
                    .push(ExportAccountTransaction::Native(aggregated));
            }
            Err(LedgerRowMappingError::Invalid { message }) => {
                return Err(ExportEngineError::Format(message));
            }
        }
    }

    let manual_assertions = load_all_manual_asset_balance_assertion_rows_for_export(user_id)?;
    for (account_id, assertions) in map_custom_balance_assertion_rows(manual_assertions) {
        transactions_by_account
            .entry(account_id)
            .or_default()
            .extend(
                assertions
                    .into_iter()
                    .map(ExportAccountTransaction::ManualAssertion),
            );
    }
    let api_assertions = load_all_native_api_balance_assertion_rows_for_export(user_id)?;
    for (account_id, assertions) in map_native_api_balance_assertion_rows(api_assertions) {
        transactions_by_account
            .entry(account_id)
            .or_default()
            .extend(
                assertions
                    .into_iter()
                    .map(ExportAccountTransaction::ManualAssertion),
            );
    }

    for (account_id, transactions) in transactions_by_account.iter_mut() {
        prepare_account_transactions_for_export(
            *account_id,
            transactions,
            incomplete_bitcoin_accounts.contains(account_id),
        )?;
    }

    Ok(transactions_by_account)
}

fn prepare_account_transactions_for_export(
    account_id: WalletAccountId,
    transactions: &mut [ExportAccountTransaction],
    allow_unasserted_native_window: bool,
) -> Result<(), ExportEngineError> {
    transactions.sort_by(compare_export_transactions);
    if !allow_unasserted_native_window
        || transactions.iter().any(|transaction| {
            matches!(
                transaction,
                ExportAccountTransaction::Native(native) if native.closing_balance.is_some()
            )
        })
    {
        verify_native_ledger_chain(account_id, transactions)?;
    }
    Ok(())
}

fn map_custom_balance_assertion_rows(
    rows: Vec<ExportManualAssetBalanceAssertionRow>,
) -> HashMap<WalletAccountId, Vec<CustomBalanceAssertionRenderRow>> {
    let mut assertions_by_account: HashMap<WalletAccountId, Vec<CustomBalanceAssertionRenderRow>> =
        HashMap::new();

    for row in rows {
        assertions_by_account
            .entry(row.account_id)
            .or_default()
            .push(CustomBalanceAssertionRenderRow {
                assertion_id: row.assertion_id.to_string(),
                asserted_on: row.asserted_on,
                asserted_balance: row.asserted_balance,
                note: row.note,
                source: BalanceAssertionSource::Manual,
            });
    }

    assertions_by_account
}

fn map_native_api_balance_assertion_rows(
    rows: Vec<ExportNativeApiBalanceAssertionRow>,
) -> HashMap<WalletAccountId, Vec<CustomBalanceAssertionRenderRow>> {
    let mut assertions_by_account: HashMap<WalletAccountId, Vec<CustomBalanceAssertionRenderRow>> =
        HashMap::new();

    for row in rows {
        assertions_by_account
            .entry(row.account_id)
            .or_default()
            .push(CustomBalanceAssertionRenderRow {
                assertion_id: row.assertion_id,
                asserted_on: row.asserted_on,
                asserted_balance: row.asserted_balance,
                note: Some("provider balance sync".to_string()),
                source: BalanceAssertionSource::Api,
            });
    }

    assertions_by_account
}

fn compare_export_transactions(
    left: &ExportAccountTransaction,
    right: &ExportAccountTransaction,
) -> std::cmp::Ordering {
    match (left, right) {
        (ExportAccountTransaction::Native(left), ExportAccountTransaction::Native(right)) => left
            .occurred_at
            .cmp(&right.occurred_at)
            .then(
                left.block_height
                    .unwrap_or(i64::MAX)
                    .cmp(&right.block_height.unwrap_or(i64::MAX)),
            )
            .then(
                left.nonce
                    .unwrap_or(i64::MAX)
                    .cmp(&right.nonce.unwrap_or(i64::MAX)),
            )
            .then(
                left.min_transfer_index
                    .unwrap_or(i64::MAX)
                    .cmp(&right.min_transfer_index.unwrap_or(i64::MAX)),
            )
            .then(left.tx_hash.cmp(&right.tx_hash)),
        (
            ExportAccountTransaction::ManualAssertion(left),
            ExportAccountTransaction::ManualAssertion(right),
        ) => left
            .asserted_on
            .cmp(&right.asserted_on)
            .then(left.assertion_id.cmp(&right.assertion_id)),
        (ExportAccountTransaction::Native(_), ExportAccountTransaction::ManualAssertion(_)) => {
            std::cmp::Ordering::Less
        }
        (ExportAccountTransaction::ManualAssertion(_), ExportAccountTransaction::Native(_)) => {
            std::cmp::Ordering::Greater
        }
    }
}

fn verify_native_ledger_chain(
    account_id: WalletAccountId,
    transactions: &[ExportAccountTransaction],
) -> Result<(), ExportEngineError> {
    let mut previous_closing: Option<i128> = None;
    for tx in transactions {
        let ExportAccountTransaction::Native(native) = tx else {
            continue;
        };
        let closing = native
            .closing_balance
            .map(|value| i128::try_from(value.value()))
            .transpose()
            .map_err(|_| {
                ExportEngineError::Invariant(format!(
                    "ledger chain: closing balance out of range for account {account_id} tx {}",
                    native.tx_hash
                ))
            })?
            .ok_or_else(|| {
                ExportEngineError::Invariant(format!(
                    "ledger chain: confirmed row missing closing balance for account {account_id} tx {}",
                    native.tx_hash
                ))
            })?;
        if closing < 0 {
            return Err(ExportEngineError::Invariant(format!(
                "ledger chain: negative closing balance for account {account_id} tx {}",
                native.tx_hash
            )));
        }
        match previous_closing {
            Some(prev) => {
                let expected = prev.checked_add(native.balance_delta).ok_or_else(|| {
                    ExportEngineError::Invariant(format!(
                        "ledger chain: balance overflow for account {account_id} tx {}",
                        native.tx_hash
                    ))
                })?;
                if expected != closing {
                    return Err(ExportEngineError::Invariant(format!(
                        "ledger chain desync for account {account_id} tx {}: expected {expected}, got {closing}",
                        native.tx_hash
                    )));
                }
            }
            None => {
                let opening = closing.checked_sub(native.balance_delta).ok_or_else(|| {
                    ExportEngineError::Invariant(format!(
                        "ledger chain: implied opening overflow for account {account_id} tx {}",
                        native.tx_hash
                    ))
                })?;
                if opening < 0 {
                    return Err(ExportEngineError::Invariant(format!(
                        "ledger chain: negative implied opening for account {account_id} tx {}",
                        native.tx_hash
                    )));
                }
            }
        }
        previous_closing = Some(closing);
    }
    Ok(())
}

fn map_account_transaction_ledger_row(
    row: ExportAccountTransactionLedgerRow,
) -> Result<AggregatedAccountTransaction, LedgerRowMappingError> {
    if row.tx_hash.trim().is_empty() {
        return Err(LedgerRowMappingError::Invalid {
            message: "Invalid export ledger row: empty tx_hash".to_string(),
        });
    }

    let fee = row.fee.unwrap_or(UnsignedAmount::zero());

    Ok(AggregatedAccountTransaction {
        account_id: row.account_id,
        tx_hash: row.tx_hash,
        direction: row.direction,
        balance_delta: row.balance_delta,
        fee,
        occurred_at: row.occurred_at,
        block_height: row.block_height,
        nonce: row.nonce,
        min_transfer_index: row.min_transfer_index,
        closing_balance: row.closing_balance,
    })
}

#[cfg(all(test, feature = "db-tests"))]
fn create_temp_export_dir(final_hledger_dir: &Path) -> Result<PathBuf, ExportEngineError> {
    let parent = final_hledger_dir.parent().ok_or_else(|| {
        ExportEngineError::Invariant(format!(
            "Invalid hledger export path without parent: {:?}",
            final_hledger_dir
        ))
    })?;
    ensure_dir_exists(parent).map_err(|err| {
        ExportEngineError::Io(format!(
            "Failed to ensure export parent directory exists at {:?}: {err}",
            parent
        ))
    })?;

    let temp_hledger_dir = parent.join(format!("{TMP_ROOT_PREFIX}{}", Ulid::new()));
    std::fs::create_dir_all(&temp_hledger_dir).map_err(|err| {
        ExportEngineError::Io(format!(
            "Failed to create temporary hledger export directory at {:?}: {err}",
            temp_hledger_dir
        ))
    })?;
    Ok(temp_hledger_dir)
}

#[derive(Debug, Clone, Copy)]
struct SnapshotWriteCounts {
    transactions_exported: u32,
    balance_assertions_exported: u32,
}

fn write_snapshot(
    sink: &mut dyn JournalSink,
    accounts: &[ResolvedAccount],
    transactions_by_account: &HashMap<WalletAccountId, Vec<ExportAccountTransaction>>,
    hledger_owner_directory_segment: &str,
    hledger_owner_posting_segment: &str,
    incomplete_bitcoin_accounts: &std::collections::HashSet<WalletAccountId>,
) -> Result<SnapshotWriteCounts, ExportEngineError> {
    let directives = accounts
        .iter()
        .map(|account| CommodityDirective {
            unit_code: account.commodity.unit_code.clone(),
            decimal_precision: account.commodity.decimal_precision,
        })
        .collect::<Vec<_>>();
    let directives_contents = format_directives_journal(&directives);
    sink.write_relative(&rel_directives(), &directives_contents)?;

    let mut transactions_exported = 0_u32;
    let mut balance_assertions_exported = 0_u32;
    let mut include_index = IncludeIndex::default();
    for account in accounts {
        let account_transactions = transactions_by_account
            .get(&account.account_id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let account_tx_count = count_export_rows_by_kind(
            account_transactions,
            ExportAccountTransactionKind::Native,
            account.account_id,
        )?;
        transactions_exported = transactions_exported
            .checked_add(account_tx_count)
            .ok_or_else(|| {
                ExportEngineError::Invariant("Transaction export count overflowed u32".to_string())
            })?;
        let account_assertion_count = count_export_rows_by_kind(
            account_transactions,
            ExportAccountTransactionKind::BalanceAssertion,
            account.account_id,
        )?;
        balance_assertions_exported = balance_assertions_exported
            .checked_add(account_assertion_count)
            .ok_or_else(|| {
                ExportEngineError::Invariant(
                    "Balance assertion export count overflowed u32".to_string(),
                )
            })?;

        let transaction_buckets = partition_account_transactions_by_year(account_transactions);

        if transaction_buckets.is_empty() {
            write_empty_account_journals(sink, account, hledger_owner_directory_segment)?;
        } else {
            let latest_account_year = transaction_buckets
                .last_key_value()
                .map(|(year, _)| year.clone())
                .ok_or_else(|| {
                    ExportEngineError::Invariant(
                        "Cannot determine latest year from non-empty export buckets".to_string(),
                    )
                })?;
            let mut years = Vec::new();
            match account.boundary_mode {
                ExportAccountBoundaryMode::Native => {
                    let supports_native_balance_assertions =
                        !incomplete_bitcoin_accounts.contains(&account.account_id);
                    let mut previous_closing_balance: Option<UnsignedAmount> = None;
                    for (year, transactions) in transaction_buckets {
                        let native_transactions = native_transactions_in_bucket(&transactions);
                        let native_boundaries = if native_transactions.is_empty()
                            || !supports_native_balance_assertions
                        {
                            None
                        } else {
                            Some(native_year_opening_and_closing_balances(
                                account.account_id,
                                &native_transactions,
                                previous_closing_balance,
                            )?)
                        };
                        if let Some((opening_balance, _)) = native_boundaries {
                            write_account_year_opening_journal(
                                sink,
                                account,
                                &year,
                                &opening_balance,
                                hledger_owner_directory_segment,
                                hledger_owner_posting_segment,
                            )?;
                        }
                        write_account_year_journal(
                            sink,
                            account,
                            &year,
                            &transactions,
                            hledger_owner_directory_segment,
                            hledger_owner_posting_segment,
                        )?;
                        let include_closing =
                            native_boundaries.is_some() && year < latest_account_year;
                        if let Some((_, closing_balance)) = native_boundaries {
                            if include_closing {
                                write_account_year_closing_journal(
                                    sink,
                                    account,
                                    &year,
                                    &closing_balance,
                                    hledger_owner_directory_segment,
                                    hledger_owner_posting_segment,
                                )?;
                            }
                            previous_closing_balance = Some(closing_balance);
                        }
                        write_account_year_include_journal(
                            sink,
                            account,
                            &year,
                            native_boundaries.is_some(),
                            include_closing,
                            hledger_owner_directory_segment,
                        )?;
                        include_index.record_account_year(AccountYearRef {
                            owner: hledger_owner_directory_segment.to_string(),
                            wallet: account.wallet_segment.clone(),
                            account: account.account_segment.clone(),
                            year: year.clone(),
                        });
                        years.push(year);
                    }
                }
                ExportAccountBoundaryMode::ManualAsset => {
                    for (year, transactions) in transaction_buckets {
                        let opening_balance =
                            custom_year_opening_balance(account_transactions, &year)?;
                        let closing_balance =
                            custom_year_closing_balance(account_transactions, &year)?;
                        if let Some(opening_balance) = opening_balance {
                            write_account_year_opening_journal(
                                sink,
                                account,
                                &year,
                                &opening_balance,
                                hledger_owner_directory_segment,
                                hledger_owner_posting_segment,
                            )?;
                        }
                        write_account_year_journal(
                            sink,
                            account,
                            &year,
                            &transactions,
                            hledger_owner_directory_segment,
                            hledger_owner_posting_segment,
                        )?;
                        let include_closing =
                            closing_balance.is_some() && year < latest_account_year;
                        if let Some(closing_balance) = closing_balance.filter(|_| include_closing) {
                            write_account_year_closing_journal(
                                sink,
                                account,
                                &year,
                                &closing_balance,
                                hledger_owner_directory_segment,
                                hledger_owner_posting_segment,
                            )?;
                        }
                        write_account_year_include_journal(
                            sink,
                            account,
                            &year,
                            opening_balance.is_some(),
                            include_closing,
                            hledger_owner_directory_segment,
                        )?;
                        include_index.record_account_year(AccountYearRef {
                            owner: hledger_owner_directory_segment.to_string(),
                            wallet: account.wallet_segment.clone(),
                            account: account.account_segment.clone(),
                            year: year.clone(),
                        });
                        years.push(year);
                    }
                }
            }
            write_account_all_years_journal(
                sink,
                account,
                &years,
                hledger_owner_directory_segment,
            )?;
        }
    }

    write_aggregate_include_indexes(sink, &include_index)?;
    write_root_entry_journal(sink)?;

    Ok(SnapshotWriteCounts {
        transactions_exported,
        balance_assertions_exported,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportAccountTransactionKind {
    Native,
    BalanceAssertion,
}

fn count_export_rows_by_kind(
    transactions: &[ExportAccountTransaction],
    kind: ExportAccountTransactionKind,
    account_id: WalletAccountId,
) -> Result<u32, ExportEngineError> {
    let count = transactions
        .iter()
        .filter(|transaction| {
            matches!(
                (kind, transaction),
                (
                    ExportAccountTransactionKind::Native,
                    ExportAccountTransaction::Native(_)
                ) | (
                    ExportAccountTransactionKind::BalanceAssertion,
                    ExportAccountTransaction::ManualAssertion(_)
                )
            )
        })
        .count();
    u32::try_from(count).map_err(|_| {
        ExportEngineError::Invariant(format!(
            "Too many hledger export rows to count for account {account_id}"
        ))
    })
}

fn partition_account_transactions_by_year(
    transactions: &[ExportAccountTransaction],
) -> BTreeMap<String, Vec<ExportAccountTransaction>> {
    let mut by_year: BTreeMap<String, Vec<ExportAccountTransaction>> = BTreeMap::new();

    for transaction in transactions {
        let year = export_transaction_year(transaction);
        by_year.entry(year).or_default().push(transaction.clone());
    }

    by_year
}

fn transaction_year(transaction: &AggregatedAccountTransaction) -> String {
    transaction.occurred_at.year().to_string()
}

fn export_transaction_year(transaction: &ExportAccountTransaction) -> String {
    match transaction {
        ExportAccountTransaction::Native(transaction) => transaction_year(transaction),
        ExportAccountTransaction::ManualAssertion(assertion) => {
            assertion.asserted_on.year().to_string()
        }
    }
}

fn native_year_opening_and_closing_balances(
    account_id: WalletAccountId,
    transactions: &[&AggregatedAccountTransaction],
    previous_closing_balance: Option<UnsignedAmount>,
) -> Result<(UnsignedAmount, UnsignedAmount), ExportEngineError> {
    let opening_balance = match previous_closing_balance {
        Some(balance) => balance,
        None => {
            let first = transactions.first().ok_or_else(|| {
                ExportEngineError::Invariant(
                    "ledger chain: cannot compute opening balance for empty native year"
                        .to_string(),
                )
            })?;
            implied_native_opening_balance(account_id, first)?
        }
    };
    let closing_balance = transactions
        .last()
        .and_then(|transaction| transaction.closing_balance)
        .ok_or_else(|| {
            ExportEngineError::Invariant(
                format!("ledger chain: cannot compute closing balance for account {account_id} from empty year or missing closing_balance")
            )
        })?;

    Ok((opening_balance, closing_balance))
}

fn implied_native_opening_balance(
    account_id: WalletAccountId,
    transaction: &AggregatedAccountTransaction,
) -> Result<UnsignedAmount, ExportEngineError> {
    let closing = transaction
        .closing_balance
        .map(|value| i128::try_from(value.value()))
        .transpose()
        .map_err(|_| {
            ExportEngineError::Invariant(format!(
                "ledger chain: closing balance out of range for account {account_id} tx {}",
                transaction.tx_hash
            ))
        })?
        .ok_or_else(|| {
            ExportEngineError::Invariant(format!(
                "ledger chain: confirmed row missing closing balance for account {account_id} tx {}",
                transaction.tx_hash
            ))
        })?;
    let opening = closing
        .checked_sub(transaction.balance_delta)
        .ok_or_else(|| {
            ExportEngineError::Invariant(format!(
                "ledger chain: implied opening overflow for account {account_id} tx {}",
                transaction.tx_hash
            ))
        })?;
    let opening = u128::try_from(opening).map_err(|_| {
        ExportEngineError::Invariant(format!(
            "ledger chain: negative implied opening for account {account_id} tx {}",
            transaction.tx_hash
        ))
    })?;
    Ok(UnsignedAmount::from_u128(opening))
}

fn native_transactions_in_bucket(
    transactions: &[ExportAccountTransaction],
) -> Vec<&AggregatedAccountTransaction> {
    transactions
        .iter()
        .filter_map(|transaction| match transaction {
            ExportAccountTransaction::Native(transaction) => Some(transaction),
            ExportAccountTransaction::ManualAssertion(_) => None,
        })
        .collect()
}

fn custom_year_opening_balance(
    transactions: &[ExportAccountTransaction],
    year: &str,
) -> Result<Option<UnsignedAmount>, ExportEngineError> {
    let year = parse_export_year(year)?;
    let boundary = NaiveDate::from_ymd_opt(year, 1, 1).ok_or_else(|| {
        ExportEngineError::Invariant(format!(
            "Invalid custom export opening year boundary: {year}"
        ))
    })?;
    custom_balance_on_or_before(transactions, boundary)
}

fn custom_year_closing_balance(
    transactions: &[ExportAccountTransaction],
    year: &str,
) -> Result<Option<UnsignedAmount>, ExportEngineError> {
    let year = parse_export_year(year)?;
    let boundary = NaiveDate::from_ymd_opt(year, 12, 31).ok_or_else(|| {
        ExportEngineError::Invariant(format!(
            "Invalid custom export closing year boundary: {year}"
        ))
    })?;
    custom_balance_on_or_before(transactions, boundary)
}

fn parse_export_year(year: &str) -> Result<i32, ExportEngineError> {
    year.parse::<i32>()
        .map_err(|err| ExportEngineError::Invariant(format!("Invalid export year {year}: {err}")))
}

fn custom_balance_on_or_before(
    transactions: &[ExportAccountTransaction],
    boundary: NaiveDate,
) -> Result<Option<UnsignedAmount>, ExportEngineError> {
    for transaction in transactions.iter().rev() {
        match transaction {
            ExportAccountTransaction::ManualAssertion(assertion) => {
                if assertion.asserted_on <= boundary {
                    return Ok(Some(assertion.asserted_balance));
                }
            }
            ExportAccountTransaction::Native(_) => {
                return Err(ExportEngineError::Invariant(
                    "Custom balance resolution encountered native transaction".to_string(),
                ));
            }
        }
    }

    Ok(None)
}

pub(crate) fn resolve_user_hledger_owner_segments(
    user_id: UserId,
) -> Result<(String, String), ExportEngineError> {
    let username = load_username_for_user(user_id)?;
    Ok(hledger_owner_segments_from_username(&username))
}

fn load_username_for_user(user_id: UserId) -> Result<String, ExportEngineError> {
    let username = with_db(|conn| {
        match conn.query_row(
            "SELECT username FROM users WHERE user_id = ?1",
            [user_id.to_string()],
            |row| row.get::<_, String>(0),
        ) {
            Ok(username) => Ok(Some(username)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(crate::db::DbError::from_rusqlite_error(
                "Failed to load username for hledger export",
                error,
            )),
        }
    })
    .map_err(|err| {
        ExportEngineError::Query(format!("Failed to load username for user {user_id}: {err}"))
    })?;

    username.ok_or_else(|| {
        ExportEngineError::Invariant(format!(
            "No username found for user {user_id} while exporting hledger"
        ))
    })
}

fn equity_opening_balance_account_name(
    owner_segment: &str,
    wallet_segment: &str,
    account_segment: &str,
) -> String {
    format!("{OPENING_BALANCE_PREFIX}:{owner_segment}:{wallet_segment}:{account_segment}")
}

fn equity_closing_balance_account_name(
    owner_segment: &str,
    wallet_segment: &str,
    account_segment: &str,
) -> String {
    format!("{CLOSING_BALANCE_PREFIX}:{owner_segment}:{wallet_segment}:{account_segment}")
}

fn balance_assertions_equity_account_name(
    owner_segment: &str,
    wallet_segment: &str,
    account_segment: &str,
) -> String {
    format!("equity:Balance Assertions:{owner_segment}:{wallet_segment}:{account_segment}")
}

fn write_account_year_journal(
    sink: &mut dyn JournalSink,
    account: &ResolvedAccount,
    year: &str,
    transactions: &[ExportAccountTransaction],
    hledger_owner_directory_segment: &str,
    hledger_owner_posting_segment: &str,
) -> Result<(), ExportEngineError> {
    let relative = rel_account_year_journal(
        hledger_owner_directory_segment,
        &account.wallet_segment,
        &account.account_segment,
        year,
    );
    let hledger_account_name = account.hledger_account_name.clone();
    let journal_contents = build_account_journal(
        &hledger_account_name,
        &balance_assertions_equity_account_name(
            hledger_owner_posting_segment,
            &account.wallet_segment,
            &account.account_segment,
        ),
        &account.commodity.unit_code,
        account.commodity.decimal_precision,
        account.native_render_context.as_ref(),
        transactions,
    )?;
    let journal_contents =
        generated_file_contents(journal_contents.lines().map(ToString::to_string));
    sink.write_relative(&relative, &journal_contents)
}

fn write_account_year_opening_journal(
    sink: &mut dyn JournalSink,
    account: &ResolvedAccount,
    year: &str,
    opening_balance: &UnsignedAmount,
    hledger_owner_directory_segment: &str,
    hledger_owner_posting_segment: &str,
) -> Result<(), ExportEngineError> {
    let relative = rel_account_year_opening_journal(
        hledger_owner_directory_segment,
        &account.wallet_segment,
        &account.account_segment,
        year,
    );
    let hledger_account_name = account.hledger_account_name.clone();
    let opening_journal_contents = build_opening_closing_journal(&OpeningClosingJournalParams {
        transaction_date: &format!("{}-01-01", year),
        description: "Opening balance",
        hledger_account_name: &hledger_account_name,
        equity_account_name: &equity_opening_balance_account_name(
            hledger_owner_posting_segment,
            &account.wallet_segment,
            &account.account_segment,
        ),
        balance: opening_balance,
        hledger_amount_is_negative: false,
        equity_amount_is_negative: true,
        unit_code: &account.commodity.unit_code,
        decimal_precision: account.commodity.decimal_precision,
    });
    let opening_journal_contents =
        generated_file_contents(opening_journal_contents.lines().map(ToString::to_string));
    sink.write_relative(&relative, &opening_journal_contents)
}

fn write_account_year_closing_journal(
    sink: &mut dyn JournalSink,
    account: &ResolvedAccount,
    year: &str,
    closing_balance: &UnsignedAmount,
    hledger_owner_directory_segment: &str,
    hledger_owner_posting_segment: &str,
) -> Result<(), ExportEngineError> {
    let relative = rel_account_year_closing_journal(
        hledger_owner_directory_segment,
        &account.wallet_segment,
        &account.account_segment,
        year,
    );
    let hledger_account_name = account.hledger_account_name.clone();
    let closing_journal_contents = build_opening_closing_journal(&OpeningClosingJournalParams {
        transaction_date: &format!("{}-12-31", year),
        description: "Closing balance",
        hledger_account_name: &hledger_account_name,
        equity_account_name: &equity_closing_balance_account_name(
            hledger_owner_posting_segment,
            &account.wallet_segment,
            &account.account_segment,
        ),
        balance: closing_balance,
        hledger_amount_is_negative: true,
        equity_amount_is_negative: false,
        unit_code: &account.commodity.unit_code,
        decimal_precision: account.commodity.decimal_precision,
    });
    let closing_journal_contents =
        generated_file_contents(closing_journal_contents.lines().map(ToString::to_string));
    sink.write_relative(&relative, &closing_journal_contents)
}

fn write_account_year_include_journal(
    sink: &mut dyn JournalSink,
    account: &ResolvedAccount,
    year: &str,
    include_opening: bool,
    include_closing: bool,
    hledger_owner_directory_segment: &str,
) -> Result<(), ExportEngineError> {
    let relative = rel_account_year_include_journal(
        hledger_owner_directory_segment,
        &account.wallet_segment,
        &account.account_segment,
        year,
    );

    let mut include_lines = Vec::new();
    if include_opening {
        include_lines.push(format!("include {year}-opening.{HLEDGER_TEXT_EXTENSION}"));
    }
    include_lines.push(format!(
        "include {ACCOUNT_JOURNAL_DIR_NAME}/{year}/{year}.{HLEDGER_TEXT_EXTENSION}"
    ));
    if include_closing {
        include_lines.push(format!("include {year}-closing.{HLEDGER_TEXT_EXTENSION}"));
    }
    let include_contents = generated_file_contents(include_lines);
    sink.write_relative(&relative, &include_contents)
}

fn write_account_all_years_journal(
    sink: &mut dyn JournalSink,
    account: &ResolvedAccount,
    years: &[String],
    hledger_owner_directory_segment: &str,
) -> Result<(), ExportEngineError> {
    let relative = rel_account_all_years_journal(
        hledger_owner_directory_segment,
        &account.wallet_segment,
        &account.account_segment,
    );

    let mut lines = Vec::new();
    if years.is_empty() {
        lines.push(EMPTY_ACCOUNT_COMMENT.to_string());
    } else {
        lines.extend(
            years
                .iter()
                .map(|year| format!("include {year}-include.{HLEDGER_TEXT_EXTENSION}")),
        );
    }

    let all_years_contents = generated_file_contents(lines);
    sink.write_relative(&relative, &all_years_contents)
}

fn write_aggregate_include_indexes(
    sink: &mut dyn JournalSink,
    include_index: &IncludeIndex,
) -> Result<(), ExportEngineError> {
    let mut wallet_years = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for ((owner, wallet, year), accounts) in &include_index.wallet_year_accounts {
        let wallet_dir = rel_wallet_dir(owner, wallet);
        let lines = accounts
            .iter()
            .map(|account| format!("include {account}/{year}-include.{HLEDGER_TEXT_EXTENSION}"));
        let contents = generated_file_contents(lines);
        sink.write_relative(&rel_year_include(&wallet_dir, year), &contents)?;

        wallet_years
            .entry((owner.clone(), wallet.clone()))
            .or_default()
            .insert(year.clone());
    }
    for ((owner, wallet), years) in wallet_years {
        let wallet_dir = rel_wallet_dir(&owner, &wallet);
        let lines = years
            .iter()
            .map(|year| format!("include {year}-include.{HLEDGER_TEXT_EXTENSION}"));
        let contents = generated_file_contents(lines);
        sink.write_relative(&rel_all_years(&wallet_dir), &contents)?;
    }

    let mut owner_years = BTreeMap::<String, BTreeSet<String>>::new();
    for ((owner, year), wallets) in &include_index.owner_year_wallets {
        let lines = wallets
            .iter()
            .map(|wallet| format!("include {wallet}/{year}-include.{HLEDGER_TEXT_EXTENSION}"));
        let contents = generated_file_contents(lines);
        sink.write_relative(&rel_year_include(owner, year), &contents)?;

        owner_years
            .entry(owner.clone())
            .or_default()
            .insert(year.clone());
    }
    for (owner, years) in owner_years {
        let lines = years
            .iter()
            .map(|year| format!("include {year}-include.{HLEDGER_TEXT_EXTENSION}"));
        let contents = generated_file_contents(lines);
        sink.write_relative(&rel_all_years(&owner), &contents)?;
    }

    for (year, owners) in &include_index.root_year_owners {
        let lines = owners
            .iter()
            .map(|owner| format!("include {owner}/{year}-include.{HLEDGER_TEXT_EXTENSION}"));
        let contents = generated_file_contents(lines);
        sink.write_relative(&rel_root_year_include(year), &contents)?;
    }

    let lines = include_index
        .root_year_owners
        .keys()
        .map(|year| format!("include {year}-include.{HLEDGER_TEXT_EXTENSION}"));
    let contents = generated_file_contents(lines);
    sink.write_relative(&rel_root_all_years(), &contents)
}

fn write_root_entry_journal(sink: &mut dyn JournalSink) -> Result<(), ExportEngineError> {
    let contents = generated_file_contents([
        format!("include {}", rel_directives()),
        format!("include {}", rel_root_all_years()),
    ]);
    sink.write_relative(&rel_root_entry_journal(), &contents)
}

fn write_empty_account_journals(
    sink: &mut dyn JournalSink,
    account: &ResolvedAccount,
    hledger_owner_directory_segment: &str,
) -> Result<(), ExportEngineError> {
    write_account_all_years_journal(sink, account, &[], hledger_owner_directory_segment)
}

struct OpeningClosingJournalParams<'a> {
    transaction_date: &'a str,
    description: &'a str,
    hledger_account_name: &'a str,
    equity_account_name: &'a str,
    balance: &'a UnsignedAmount,
    hledger_amount_is_negative: bool,
    equity_amount_is_negative: bool,
    unit_code: &'a str,
    decimal_precision: u8,
}

fn build_opening_closing_journal(params: &OpeningClosingJournalParams<'_>) -> String {
    let formatted_unit_code = format_hledger_commodity(params.unit_code);
    let mut lines = Vec::new();
    lines.push(format!(
        "{} * {}",
        params.transaction_date, params.description
    ));
    lines.push(format!(
        "    {}    {} {}",
        params.hledger_account_name,
        format_posting_amount(
            params.balance,
            params.hledger_amount_is_negative,
            params.decimal_precision
        ),
        formatted_unit_code
    ));
    lines.push(format!(
        "    {}    {} {}",
        params.equity_account_name,
        format_posting_amount(
            params.balance,
            params.equity_amount_is_negative,
            params.decimal_precision
        ),
        formatted_unit_code
    ));
    lines.push(String::new());
    lines.join("\n")
}

fn format_posting_amount(
    amount: &UnsignedAmount,
    is_negative: bool,
    decimal_precision: u8,
) -> String {
    let formatted = format_unsigned_amount_fixed(*amount, decimal_precision);
    if is_negative && amount.value() != 0 {
        return format!("-{formatted}");
    }
    formatted
}

fn build_account_journal(
    hledger_account_name: &str,
    balance_assertions_equity_account_name: &str,
    unit_code: &str,
    decimal_precision: u8,
    native_render_context: Option<&NativeTransactionRenderContext>,
    transactions: &[ExportAccountTransaction],
) -> Result<String, ExportEngineError> {
    let mut lines = Vec::new();
    if transactions.is_empty() {
        lines.push(EMPTY_ACCOUNT_COMMENT.to_string());
        lines.push(String::new());
        return Ok(lines.join("\n"));
    }

    for (index, transaction) in transactions.iter().enumerate() {
        lines.push(match transaction {
            ExportAccountTransaction::Native(transaction) => {
                let render_context = native_render_context.ok_or_else(|| {
                    ExportEngineError::Invariant(
                        "native hledger transaction missing render context".to_string(),
                    )
                })?;
                build_hledger_transaction(
                    hledger_account_name,
                    unit_code,
                    decimal_precision,
                    render_context,
                    transaction,
                )?
            }
            ExportAccountTransaction::ManualAssertion(assertion) => {
                build_custom_balance_assertion_transaction(
                    hledger_account_name,
                    balance_assertions_equity_account_name,
                    unit_code,
                    decimal_precision,
                    assertion,
                )
            }
        });
        if index + 1 < transactions.len() {
            lines.push(String::new());
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

#[cfg(all(test, feature = "db-tests"))]
fn replace_export_root_atomically(
    temp_hledger_dir: &Path,
    final_hledger_dir: &Path,
) -> Result<(), ExportEngineError> {
    let parent = final_hledger_dir.parent().ok_or_else(|| {
        ExportEngineError::Invariant(format!(
            "Invalid final hledger export path without parent: {:?}",
            final_hledger_dir
        ))
    })?;
    let final_name = final_hledger_dir
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or(HLEDGER_ROOT_DIR_NAME);

    if final_hledger_dir.exists() {
        let backup_hledger_dir = parent.join(format!("{BACKUP_ROOT_PREFIX}{}", Ulid::new()));
        std::fs::rename(final_hledger_dir, &backup_hledger_dir).map_err(|err| {
            ExportEngineError::Io(format!(
                "Failed to move current export root from {:?} to {:?}: {err}",
                final_hledger_dir, backup_hledger_dir
            ))
        })?;

        match std::fs::rename(temp_hledger_dir, final_hledger_dir) {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&backup_hledger_dir);
                Ok(())
            }
            Err(publish_err) => {
                let restore_result = std::fs::rename(&backup_hledger_dir, final_hledger_dir);
                match restore_result {
                    Ok(()) => Err(ExportEngineError::Io(format!(
                        "Failed to publish updated {final_name} export root: {publish_err}"
                    ))),
                    Err(restore_err) => Err(ExportEngineError::Io(format!(
                        "Failed to publish updated {final_name} export root: {publish_err}; failed to restore previous export root: {restore_err}"
                    ))),
                }
            }
        }
    } else {
        std::fs::rename(temp_hledger_dir, final_hledger_dir).map_err(|err| {
            ExportEngineError::Io(format!(
                "Failed to publish new {final_name} export root from {:?} to {:?}: {err}",
                temp_hledger_dir, final_hledger_dir
            ))
        })
    }
}

#[cfg(test)]
mod test_helpers {
    use super::*;
    use crate::transactions::AccountTransactionDirection;
    use chrono::TimeZone;

    pub(super) fn fixed_time(day: u32, hour: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 2, day, hour, 0, 0)
            .single()
            .expect("valid fixed timestamp")
    }

    pub(super) fn fixed_time_in_year(
        year: i32,
        day: u32,
        hour: u32,
    ) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(year, 2, day, hour, 0, 0)
            .single()
            .expect("valid fixed timestamp")
    }

    pub(super) fn same_dt() -> chrono::DateTime<chrono::Utc> {
        fixed_time(1, 0)
    }

    #[derive(Clone, Copy)]
    pub(super) struct NativeTxOrder {
        pub(super) occurred_at: chrono::DateTime<chrono::Utc>,
        pub(super) block_height: Option<i64>,
        pub(super) nonce: Option<i64>,
        pub(super) min_transfer_index: Option<i64>,
    }

    pub(super) fn native_tx_order(
        block_height: Option<i64>,
        nonce: Option<i64>,
        min_transfer_index: Option<i64>,
    ) -> NativeTxOrder {
        NativeTxOrder {
            occurred_at: same_dt(),
            block_height,
            nonce,
            min_transfer_index,
        }
    }

    pub(super) fn native_tx(
        account_id: WalletAccountId,
        tx_hash: &str,
        balance_delta: i128,
        closing_balance: u128,
        block_height: Option<i64>,
        nonce: Option<i64>,
    ) -> ExportAccountTransaction {
        native_tx_at(
            account_id,
            tx_hash,
            balance_delta,
            Some(UnsignedAmount::from_u128(closing_balance)),
            native_tx_order(block_height, nonce, None),
        )
    }

    pub(super) fn native_tx_at(
        account_id: WalletAccountId,
        tx_hash: &str,
        balance_delta: i128,
        closing_balance: Option<UnsignedAmount>,
        order: NativeTxOrder,
    ) -> ExportAccountTransaction {
        let direction = if balance_delta < 0 {
            AccountTransactionDirection::Outgoing
        } else if balance_delta > 0 {
            AccountTransactionDirection::Incoming
        } else {
            AccountTransactionDirection::SelfTransfer
        };

        ExportAccountTransaction::Native(AggregatedAccountTransaction {
            account_id,
            tx_hash: tx_hash.to_string(),
            direction,
            balance_delta,
            fee: UnsignedAmount::zero(),
            occurred_at: order.occurred_at,
            block_height: order.block_height,
            nonce: order.nonce,
            min_transfer_index: order.min_transfer_index,
            closing_balance,
        })
    }

    pub(super) fn assert_ledger_chain_error(
        account_id: WalletAccountId,
        txs: &[ExportAccountTransaction],
        expected: &str,
    ) {
        let err = verify_native_ledger_chain(account_id, txs).unwrap_err();
        let message = format!("{err}");
        assert!(
            message.contains("ledger chain"),
            "{message:?} should mention ledger chain"
        );
        assert!(
            message.contains(expected),
            "{message:?} should contain {expected:?}"
        );
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod pure_tests {
    use super::test_helpers::*;
    use super::*;
    use crate::transactions::AccountTransactionDirection;

    #[test]
    fn native_render_context_maps_native_network_fee_accounts_and_rejects_partial_context() {
        let bitcoin_mainnet =
            native_render_context(Some(SyncedAssetId::Bitcoin), Some(Network::Mainnet))
                .expect("bitcoin mainnet context should render")
                .expect("bitcoin mainnet should have native render context");
        assert_eq!(bitcoin_mainnet.asset_display_name, "Bitcoin");
        assert_eq!(
            bitcoin_mainnet.network_fee_account,
            "expenses:Fees:Bitcoin:Network:Mainnet"
        );

        let ethereum_testnet =
            native_render_context(Some(SyncedAssetId::Ethereum), Some(Network::Testnet))
                .expect("ethereum testnet context should render")
                .expect("ethereum testnet should have native render context");
        assert_eq!(ethereum_testnet.asset_display_name, "Ethereum");
        assert_eq!(
            ethereum_testnet.network_fee_account,
            "expenses:Fees:Ethereum:Network:Testnet"
        );

        let bitcoin_signet =
            native_render_context(Some(SyncedAssetId::Bitcoin), Some(Network::Signet))
                .expect("bitcoin signet context should render")
                .expect("bitcoin signet should have native render context");
        assert_eq!(
            bitcoin_signet.network_fee_account,
            "expenses:Fees:Bitcoin:Network:Signet"
        );

        let bitcoin_regtest =
            native_render_context(Some(SyncedAssetId::Bitcoin), Some(Network::Regtest))
                .expect("bitcoin regtest context should render")
                .expect("bitcoin regtest should have native render context");
        assert_eq!(
            bitcoin_regtest.network_fee_account,
            "expenses:Fees:Bitcoin:Network:Regtest"
        );

        for result in [
            native_render_context(Some(SyncedAssetId::Bitcoin), None),
            native_render_context(None, Some(Network::Mainnet)),
        ] {
            let message = match result {
                Err(ExportEngineError::Invariant(message)) => message,
                other => panic!("expected invariant error, got {other:?}"),
            };
            assert!(
                message.contains("native hledger render context requires both asset and network")
            );
        }

        assert_eq!(native_render_context(None, None), Ok(None));
    }

    #[test]
    fn aggregate_include_paths_use_same_directory_pattern() {
        assert_eq!(rel_root_year_include("2026"), "2026-include.j.txt");
        assert_eq!(rel_root_all_years(), "all-years.j.txt");
        assert_eq!(rel_root_entry_journal(), "bitgarth.j.txt");
        assert_eq!(rel_year_include("aaa", "2026"), "aaa/2026-include.j.txt");
        assert_eq!(rel_all_years("aaa/Random"), "aaa/Random/all-years.j.txt");
    }

    #[test]
    fn hledger_account_naming_defaults_to_owner_asset_prefix() {
        let naming = HledgerAccountNaming::new("Alice", None);

        assert_eq!(
            naming.account_name("Main Wallet", "Ethereum Account"),
            "assets:Alice:Main Wallet:Ethereum Account"
        );
    }

    #[test]
    fn hledger_account_naming_uses_custom_account_prefix() {
        let prefix =
            HledgerAccountPrefix::parse("assets:My Wallet").expect("test prefix should parse");
        let naming = HledgerAccountNaming::new("Alice", Some(&prefix));

        assert_eq!(
            naming.account_name("Main Wallet", "Ethereum Account"),
            "assets:My Wallet:Main Wallet:Ethereum Account"
        );
    }

    #[test]
    fn validate_hledger_relative_path_rejects_unsafe_components() {
        for relative in [
            "../x.j.txt",
            "./x.j.txt",
            "a/../x.j.txt",
            "a//x.j.txt",
            "/x.j.txt",
            "",
        ] {
            assert!(
                matches!(
                    validate_hledger_relative_path(relative),
                    Err(ExportEngineError::Invariant(_))
                ),
                "{relative:?} should be rejected"
            );
        }

        for relative in ["aaa/Wallet/Account/2026-include.j.txt", "all-years.j.txt"] {
            validate_hledger_relative_path(relative)
                .unwrap_or_else(|err| panic!("{relative:?} should be accepted: {err}"));
        }
    }

    #[test]
    fn guard_accepts_consistent_chain() {
        let account_id = WalletAccountId::new();
        let txs = vec![
            native_tx(account_id, "a", 600, 600, Some(1), None),
            native_tx(account_id, "b", -100, 500, Some(2), None),
        ];
        assert!(verify_native_ledger_chain(account_id, &txs).is_ok());
    }

    #[test]
    fn guard_rejects_desynced_closing() {
        let account_id = WalletAccountId::new();
        let txs = vec![
            native_tx(account_id, "a", 600, 600, Some(1), None),
            native_tx(account_id, "b", -100, 499, Some(2), None),
        ];
        assert_ledger_chain_error(account_id, &txs, "desync");
    }

    #[test]
    fn guard_rejects_missing_closing_balance() {
        let account_id = WalletAccountId::new();
        let txs = vec![native_tx_at(
            account_id,
            "a",
            100,
            None,
            native_tx_order(Some(1), None, None),
        )];
        assert_ledger_chain_error(account_id, &txs, "missing closing balance");
    }

    #[test]
    fn hledger_preparation_keeps_proven_transaction_and_provider_assertions() {
        let account_id = WalletAccountId::new();
        let mut transactions = vec![
            native_tx(account_id, "a", 100, 100, Some(1), None),
            native_tx(account_id, "b", -50, 50, Some(2), None),
            ExportAccountTransaction::ManualAssertion(CustomBalanceAssertionRenderRow {
                assertion_id: "api-balance".to_string(),
                asserted_on: NaiveDate::from_ymd_opt(2026, 2, 1).expect("valid date"),
                asserted_balance: UnsignedAmount::from_u128(50),
                note: Some("provider balance sync".to_string()),
                source: BalanceAssertionSource::Api,
            }),
        ];

        prepare_account_transactions_for_export(account_id, &mut transactions, false)
            .expect("proven transaction window should validate");

        assert_eq!(transactions.len(), 3);
        assert!(transactions.iter().any(|transaction| matches!(
            transaction,
            ExportAccountTransaction::ManualAssertion(assertion)
                if assertion.source == BalanceAssertionSource::Api
        )));
        assert!(
            transactions
                .iter()
                .filter_map(|transaction| match transaction {
                    ExportAccountTransaction::Native(native) => Some(native),
                    ExportAccountTransaction::ManualAssertion(_) => None,
                })
                .all(|native| native.closing_balance.is_some())
        );
    }

    #[test]
    fn hledger_preparation_allows_fully_unasserted_incomplete_bitcoin_window() {
        let account_id = WalletAccountId::new();
        let mut transactions = vec![
            native_tx_at(
                account_id,
                "a",
                100,
                None,
                native_tx_order(Some(1), None, None),
            ),
            native_tx_at(
                account_id,
                "b",
                -50,
                None,
                native_tx_order(Some(2), None, None),
            ),
            ExportAccountTransaction::ManualAssertion(CustomBalanceAssertionRenderRow {
                assertion_id: "api-balance".to_string(),
                asserted_on: NaiveDate::from_ymd_opt(2026, 2, 1).expect("valid date"),
                asserted_balance: UnsignedAmount::from_u128(50),
                note: Some("provider balance sync".to_string()),
                source: BalanceAssertionSource::Api,
            }),
        ];

        prepare_account_transactions_for_export(account_id, &mut transactions, true)
            .expect("fully unasserted incomplete Bitcoin window should export");

        assert_eq!(transactions.len(), 3);
        assert!(transactions.iter().any(|transaction| matches!(
            transaction,
            ExportAccountTransaction::ManualAssertion(assertion)
                if assertion.source == BalanceAssertionSource::Api
        )));
    }

    #[test]
    fn hledger_preparation_rejects_fully_unasserted_ethereum_window() {
        let account_id = WalletAccountId::new();
        let mut transactions = vec![native_tx_at(
            account_id,
            "a",
            100,
            None,
            native_tx_order(Some(1), None, None),
        )];

        let error = prepare_account_transactions_for_export(account_id, &mut transactions, false)
            .expect_err("fully unasserted Ethereum window must fail");
        assert!(error.to_string().contains("missing closing balance"));
    }

    #[test]
    fn hledger_preparation_rejects_mixed_transaction_assertions() {
        let account_id = WalletAccountId::new();
        let mut transactions = vec![
            native_tx(account_id, "a", 100, 100, Some(1), None),
            native_tx_at(
                account_id,
                "b",
                -50,
                None,
                native_tx_order(Some(2), None, None),
            ),
        ];

        let error = prepare_account_transactions_for_export(account_id, &mut transactions, true)
            .expect_err("mixed asserted and unasserted rows must fail");
        assert!(error.to_string().contains("missing closing balance"));
    }

    #[test]
    fn guard_rejects_negative_implied_opening() {
        let account_id = WalletAccountId::new();
        let txs = vec![native_tx(account_id, "a", 100, 50, Some(1), None)];
        assert_ledger_chain_error(account_id, &txs, "negative implied opening");
    }

    #[test]
    fn guard_rejects_closing_balance_out_of_i128_range() {
        let account_id = WalletAccountId::new();
        let txs = vec![native_tx_at(
            account_id,
            "a",
            0,
            Some(UnsignedAmount::from_u128(u128::MAX)),
            native_tx_order(Some(1), None, None),
        )];
        assert_ledger_chain_error(account_id, &txs, "out of range");
    }

    #[test]
    fn guard_rejects_balance_add_overflow() {
        let account_id = WalletAccountId::new();
        let max_i128_balance = UnsignedAmount::from_u128(i128::MAX as u128);
        let txs = vec![
            native_tx_at(
                account_id,
                "a",
                0,
                Some(max_i128_balance),
                native_tx_order(Some(1), None, None),
            ),
            native_tx_at(
                account_id,
                "b",
                1,
                Some(max_i128_balance),
                native_tx_order(Some(2), None, None),
            ),
        ];
        assert_ledger_chain_error(account_id, &txs, "balance overflow");
    }

    #[test]
    fn guard_rejects_first_row_implied_opening_sub_overflow() {
        let account_id = WalletAccountId::new();
        let txs = vec![native_tx_at(
            account_id,
            "a",
            i128::MIN,
            Some(UnsignedAmount::zero()),
            native_tx_order(Some(1), None, None),
        )];
        assert_ledger_chain_error(account_id, &txs, "implied opening overflow");
    }

    #[test]
    fn guard_orders_same_timestamp_by_block_height() {
        let account_id = WalletAccountId::new();
        let mut txs = vec![
            native_tx_at(
                account_id,
                "b",
                -100,
                Some(UnsignedAmount::from_u128(500)),
                native_tx_order(Some(2), None, None),
            ),
            native_tx_at(
                account_id,
                "a",
                600,
                Some(UnsignedAmount::from_u128(600)),
                native_tx_order(Some(1), None, None),
            ),
        ];
        txs.sort_by(compare_export_transactions);
        assert!(verify_native_ledger_chain(account_id, &txs).is_ok());
    }

    #[test]
    fn native_ordering_uses_full_rebuild_tie_break_chain() {
        let account_id = WalletAccountId::new();
        let mut txs = [
            native_tx_at(
                account_id,
                "z",
                0,
                Some(UnsignedAmount::zero()),
                NativeTxOrder {
                    occurred_at: fixed_time_in_year(2025, 1, 23),
                    block_height: None,
                    nonce: Some(0),
                    min_transfer_index: Some(0),
                },
            ),
            native_tx_at(
                account_id,
                "f",
                0,
                Some(UnsignedAmount::zero()),
                native_tx_order(None, Some(0), Some(0)),
            ),
            native_tx_at(
                account_id,
                "d",
                0,
                Some(UnsignedAmount::zero()),
                native_tx_order(Some(1), Some(2), Some(1)),
            ),
            native_tx_at(
                account_id,
                "b",
                0,
                Some(UnsignedAmount::zero()),
                native_tx_order(Some(1), Some(1), Some(1)),
            ),
            native_tx_at(
                account_id,
                "e",
                0,
                Some(UnsignedAmount::zero()),
                native_tx_order(Some(2), Some(1), Some(1)),
            ),
            native_tx_at(
                account_id,
                "c",
                0,
                Some(UnsignedAmount::zero()),
                native_tx_order(Some(1), Some(1), Some(2)),
            ),
            native_tx_at(
                account_id,
                "a",
                0,
                Some(UnsignedAmount::zero()),
                native_tx_order(Some(1), Some(1), Some(1)),
            ),
            native_tx_at(
                account_id,
                "0",
                0,
                Some(UnsignedAmount::zero()),
                NativeTxOrder {
                    occurred_at: fixed_time(1, 2),
                    block_height: Some(0),
                    nonce: Some(0),
                    min_transfer_index: Some(0),
                },
            ),
        ];

        txs.sort_by(compare_export_transactions);
        let tx_hashes = txs
            .iter()
            .map(|tx| match tx {
                ExportAccountTransaction::Native(native) => native.tx_hash.as_str(),
                ExportAccountTransaction::ManualAssertion(_) => unreachable!("only native txs"),
            })
            .collect::<Vec<_>>();
        assert_eq!(tx_hashes, vec!["z", "a", "b", "c", "d", "e", "f", "0"]);
    }

    #[test]
    fn map_account_transaction_ledger_row_keeps_zero_delta_rows() {
        let row = ExportAccountTransactionLedgerRow {
            account_id: WalletAccountId::new(),
            tx_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            direction: AccountTransactionDirection::SelfTransfer,
            fee: None,
            balance_delta: 0,
            occurred_at: fixed_time(24, 1),
            block_height: None,
            nonce: None,
            min_transfer_index: None,
            closing_balance: Some(UnsignedAmount::zero()),
        };

        let mapped = map_account_transaction_ledger_row(row).expect("zero delta rows map");
        assert_eq!(mapped.balance_delta, 0);
    }

    #[test]
    fn transaction_year_uses_ledger_occurred_at() {
        let row_2025 = ExportAccountTransactionLedgerRow {
            account_id: WalletAccountId::new(),
            tx_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            direction: AccountTransactionDirection::Outgoing,
            fee: Some(UnsignedAmount::zero()),
            balance_delta: -100,
            occurred_at: fixed_time_in_year(2025, 3, 1),
            block_height: Some(1),
            nonce: Some(2),
            min_transfer_index: Some(3),
            closing_balance: Some(UnsignedAmount::from_u128(900)),
        };
        let row_2027 = ExportAccountTransactionLedgerRow {
            account_id: WalletAccountId::new(),
            tx_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            direction: AccountTransactionDirection::Incoming,
            fee: None,
            balance_delta: 200,
            occurred_at: fixed_time_in_year(2027, 4, 1),
            block_height: Some(1),
            nonce: Some(2),
            min_transfer_index: Some(3),
            closing_balance: Some(UnsignedAmount::from_u128(200)),
        };

        let mapped_2025 =
            map_account_transaction_ledger_row(row_2025).expect("mapped with occurred_at");
        let mapped_2027 =
            map_account_transaction_ledger_row(row_2027).expect("mapped with occurred_at");

        assert_eq!(transaction_year(&mapped_2025), "2025".to_string());
        assert_eq!(transaction_year(&mapped_2027), "2027".to_string());
    }

    #[test]
    fn normalize_owner_directory_segment_removes_unsafe_characters_and_spaces() {
        assert_eq!(
            normalize_owner_directory_segment("rustic-detective"),
            "rustic-detective"
        );
        assert_eq!(
            normalize_owner_directory_segment("Rustic Detect ive"),
            "RusticDetective"
        );
        assert_eq!(normalize_owner_directory_segment("ab&cd@2026"), "abcd2026");
    }

    #[test]
    fn normalize_owner_directory_segment_replaces_dot_components() {
        assert_eq!(normalize_owner_directory_segment("."), "me");
        assert_eq!(normalize_owner_directory_segment(".."), "me");
    }

    #[test]
    fn normalize_owner_posting_segment_is_title_case_word_boundaries() {
        assert_eq!(
            normalize_owner_posting_segment("rustic-detective"),
            "RusticDetective"
        );
        assert_eq!(
            normalize_owner_posting_segment("rustic_detective.one"),
            "RusticDetectiveOne"
        );
        assert_eq!(normalize_owner_posting_segment("john..doe"), "JohnDoe");
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::test_helpers::*;
    use super::*;
    use crate::db::{
        ProviderTransferKey, SyncAccountTransactionRecord, SyncAccountTransferRecord,
        SyncTransactionInputRecord, SyncTransactionOutputRecord, SyncTransactionRecord,
        acquire_test_runtime, add_bitcoin_address, add_ethereum_address, get_user_db_dek,
        initialize_user_db_for_test, rebuild_account_transaction_ledger,
        reconcile_account_transactions, reconcile_address_transactions,
        update_wallet_account_label, with_db,
    };
    use crate::ethereum::{EthAddress, RawEthAddress, TransferKind};
    use crate::project_paths::{
        hledger_directives_path, hledger_owner_account_all_years_journal_path,
        hledger_owner_account_year_closing_journal_path,
        hledger_owner_account_year_include_journal_path, hledger_owner_account_year_journal_path,
        hledger_owner_account_year_opening_journal_path, user_database_path_from_project_dir,
    };
    use crate::transactions::{ChainTransactionStatus, TrackedAddress, TxHash};
    use crate::wallets::{
        ACCOUNT_LABEL_MAX_LENGTH, IdentitySource, Label, ManualAssetDisplayScale, Network,
        RawBtcAddress, RawManualAssetAssertionNote, SyncedAssetId,
        ValidatedManualAssetAssertionNote, ValidatedManualAssetBalanceLiteral,
        ValidatedManualAssetUnitCode, WALLET_LABEL_MAX_LENGTH, WalletId,
    };
    use rusqlite::{OpenFlags, params};
    use std::path::PathBuf;

    struct TempExportRoot {
        root: PathBuf,
    }

    fn publish_complete_bitcoin_ledger(
        user_id: crate::models::UserId,
        account_id: crate::wallets::DigitalAssetAccountId,
        address_id: crate::wallets::DigitalAssetAddressId,
        confirmed_tx_count: u32,
        complete_height: i64,
        observed_at: chrono::DateTime<chrono::Utc>,
    ) {
        let run_id = crate::transactions::TransactionSyncRunId::new();
        crate::db::mark_account_integration_sync_started(
            user_id,
            account_id,
            crate::transactions::SyncIntegrationId::Mempool,
            observed_at,
        )
        .expect("account integration sync should start");
        crate::db::mark_address_sync_started(user_id, address_id, run_id, observed_at)
            .expect("address sync state should persist");
        crate::db::mark_address_sync_completed_success(
            user_id,
            &crate::db::AddressSyncSuccess {
                address_id,
                run_id,
                started_at: observed_at,
                completed_at: observed_at,
                last_tip_height: crate::transactions::ChainTipHeight::try_new(complete_height)
                    .expect("height should parse"),
                new_tx_count: crate::transactions::TransactionCount::from_u32(confirmed_tx_count),
                updated_tx_count: crate::transactions::TransactionCount::zero(),
                api_confirmed_balance: None,
            },
        )
        .expect("address sync state should complete");
        let complete = crate::db::publish_bitcoin_account_completion(
            user_id,
            crate::db::BitcoinAccountCompletionPublication {
                account_id,
                final_address_proof: Some(crate::db::BitcoinAddressProofPublication {
                    address_id,
                    proof: crate::db::MempoolHistoryProof {
                        confirmed_tx_count: crate::transactions::TransactionCount::from_u32(
                            confirmed_tx_count,
                        ),
                        complete_height: crate::transactions::ChainTipHeight::try_new(
                            complete_height,
                        )
                        .expect("height should parse"),
                    },
                    scan_start_run_id: None,
                }),
                completed_hd_discovery: None,
                observed_at,
            },
        )
        .expect("Bitcoin ledger and proof should publish");
        assert!(complete, "Bitcoin export fixture should be complete");
        crate::db::refresh_account_integration_sync_state(
            user_id,
            account_id,
            crate::transactions::SyncIntegrationId::Mempool,
            observed_at,
        )
        .expect("account integration success should persist");
        assert!(matches!(
            crate::db::balance_reliability::load_effective_bitcoin_history_coverage(
                user_id,
                account_id,
                crate::transactions::TransactionCount::zero(),
            )
            .expect("effective Bitcoin coverage should load"),
            Some(crate::db::BitcoinAccountHistoryCoverage::Complete { .. })
        ));
    }

    impl TempExportRoot {
        fn new() -> Self {
            Self {
                root: std::env::temp_dir().join(format!("bitgarth-hledger-export-{}", Ulid::new())),
            }
        }

        fn hledger_dir(&self, user_id: UserId) -> PathBuf {
            self.root
                .join("users")
                .join(user_id.to_string())
                .join("hledger")
        }
    }

    impl Drop for TempExportRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn owner_segments_from_username(username: &str) -> (String, String) {
        let directory_segment = normalize_owner_directory_segment(username);
        let posting_segment = normalize_owner_posting_segment(&directory_segment);
        (directory_segment, posting_segment)
    }

    fn assert_generated_header(contents: &str) {
        let mut lines = contents.lines();
        assert_eq!(lines.next(), Some("; Generated by https://bitgarth.app/"));
        assert_eq!(lines.next(), Some(""));
    }

    fn assert_hledger_parses(hledger_dir: &std::path::Path) {
        let output = std::process::Command::new("hledger")
            .current_dir(hledger_dir)
            .args(["-f", "bitgarth.j.txt", "check"])
            .output()
            .expect("hledger must be installed for export parser tests");
        assert!(
            output.status.success(),
            "hledger rejected generated export:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn hledger_transaction_block_by_hash<'a>(journal: &'a str, tx_hash: &str) -> &'a str {
        let marker = format!("    ; Transaction {tx_hash}");
        let marker_start = journal.find(&marker).expect("transaction marker exists");
        let block_start = journal[..marker_start]
            .rfind("\n\n")
            .map_or(0, |separator| separator + 2);
        let block_end = journal[marker_start..]
            .find("\n\n")
            .map_or(journal.len(), |separator| marker_start + separator);

        &journal[block_start..block_end]
    }

    #[test]
    fn native_year_boundaries_use_implied_first_opening() {
        let account_id = WalletAccountId::new();
        let txs = vec![native_tx(account_id, "a", 50, 150, Some(1), None)];
        let refs = native_transactions_in_bucket(&txs);
        let (opening, closing) =
            native_year_opening_and_closing_balances(account_id, &refs, None).expect("boundaries");

        assert_eq!(opening, UnsignedAmount::from_u128(100));
        assert_eq!(closing, UnsignedAmount::from_u128(150));
    }

    fn insert_test_user(user_id: UserId, username: &str, timestamp: chrono::DateTime<chrono::Utc>) {
        let normalized_timestamp = timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
        with_db(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO users (user_id, username, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                (
                    user_id.to_string(),
                    username,
                    &normalized_timestamp,
                    &normalized_timestamp,
                ),
            )
            .map_err(|err| {
                crate::db::DbError::from_rusqlite_error(
                    "Failed to insert test user for hledger export",
                    err,
                )
            })?;
            Ok::<(), crate::db::DbError>(())
        })
        .expect("test user should be inserted");
    }

    fn wallet_label(value: &str) -> Label {
        Label::parse_with_limit(value, WALLET_LABEL_MAX_LENGTH).expect("valid wallet label")
    }

    fn account_label(value: &str) -> Label {
        Label::parse_with_limit(value, ACCOUNT_LABEL_MAX_LENGTH).expect("valid account label")
    }

    fn open_test_user_db(
        runtime: &crate::db::TestRuntimeGuard,
        user_id: UserId,
    ) -> rusqlite::Connection {
        let db_path =
            user_database_path_from_project_dir(runtime.runtime_context().project_dir(), user_id);
        let connection =
            rusqlite::Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
                .expect("test user db should open");
        if let Some(dek) = get_user_db_dek(&user_id).expect("test user db should resolve DEK") {
            let sqlcipher_compatibility = crate::db::encryption::read_envelope(user_id)
                .expect("test user db should read envelope")
                .sqlcipher_compatibility()
                .expect("test user db should expose SQLCipher compatibility");
            connection
                .execute_batch(&format!("PRAGMA key = \"x'{}'\"", dek.as_hex()))
                .expect("test user db should set SQLCipher key");
            connection
                .pragma_update(
                    None,
                    "cipher_compatibility",
                    sqlcipher_compatibility.as_u32().to_string(),
                )
                .expect("test user db should set SQLCipher compatibility");
        }
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .expect("foreign keys should enable");
        connection
    }

    fn insert_wallet_fixture(
        connection: &rusqlite::Connection,
        wallet_id: WalletId,
        label: &Label,
        now: chrono::DateTime<chrono::Utc>,
    ) {
        let timestamp = now.to_rfc3339();
        connection
            .execute(
                "INSERT INTO wallets
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    wallet_id.to_string(),
                    label.as_str(),
                    label.key().as_str(),
                    Option::<String>::None,
                    IdentitySource::UserProvided.as_str(),
                    Option::<String>::None,
                    timestamp,
                    timestamp,
                ],
            )
            .expect("wallet fixture should insert");
    }

    fn create_manual_account(
        user_id: UserId,
        wallet_id: WalletId,
        unit_code: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> crate::wallets::AddManualAssetAccountResponse {
        let account_id = WalletAccountId::new();
        let validated_unit = ValidatedManualAssetUnitCode::parse(unit_code)
            .expect("test custom unit code should validate");
        let label_text = format!("{} Account 1", validated_unit.as_str());
        let label = Label::parse_with_limit(&label_text, ACCOUNT_LABEL_MAX_LENGTH)
            .expect("test custom account label should parse");
        let timestamp = now.to_rfc3339();
        with_user_db_mut_for_test(user_id, |conn| {
            conn.execute(
                "INSERT INTO manual_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    account_id.to_string(),
                    wallet_id.to_string(),
                    label.as_str(),
                    label.key().as_str(),
                    unit_code.to_ascii_lowercase(),
                    "test-mainnet",
                    2_i64,
                    validated_unit.as_str(),
                    unit_code,
                    "Test Network",
                    unit_code.to_ascii_lowercase(),
                    "bitgarth_catalog",
                    "bitgarth_catalog",
                    &timestamp,
                    &timestamp,
                ],
            )
            .expect("manual account fixture insert should succeed");
        });
        crate::wallets::AddManualAssetAccountResponse {
            wallet_id,
            account_id,
            account_state: crate::backend::AccountStateView::Active,
            account_limit_notice: None,
        }
    }

    fn add_manual_assertion(
        user_id: UserId,
        account_id: WalletAccountId,
        asserted_on: NaiveDate,
        balance: &str,
        note: Option<&str>,
        now: chrono::DateTime<chrono::Utc>,
    ) {
        let literal = ValidatedManualAssetBalanceLiteral::parse(balance)
            .expect("custom balance literal should validate");
        let note_value = ValidatedManualAssetAssertionNote::parse_optional(
            note.map(|value| RawManualAssetAssertionNote::new(value.to_string())),
        )
        .expect("custom assertion note should validate");
        let assertion_id = crate::wallets::ManualAssetBalanceAssertionId::new();
        let timestamp = now.to_rfc3339();
        let asserted_on_text = asserted_on.format("%Y-%m-%d").to_string();
        let note_text = note_value.as_ref().map(|n| n.as_str().to_string());
        let stored_scale = ManualAssetDisplayScale::from_u8(2);
        let balance_amount = literal
            .parse_at_scale(stored_scale)
            .expect("manual assertion balance parse")
            .amount();
        let (hi, lo) = split_amount_for_assertion(balance_amount);

        with_user_db_mut_for_test(user_id, |conn| {
            conn.execute(
                "INSERT INTO manual_asset_balance_assertions
                 (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo,
                  entered_balance_text, note, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    assertion_id.to_string(),
                    account_id.to_string(),
                    &asserted_on_text,
                    hi,
                    lo,
                    balance,
                    note_text.as_deref(),
                    &timestamp,
                    &timestamp,
                ],
            )
            .expect("manual assertion insert");
        });
    }

    fn with_user_db_mut_for_test<F>(user_id: UserId, f: F)
    where
        F: FnOnce(&mut rusqlite::Connection),
    {
        crate::db::with_user_db_mut(user_id, |conn| {
            f(conn);
            Ok::<(), crate::db::DbError>(())
        })
        .expect("test user_db_mut should succeed");
    }

    fn split_amount_for_assertion(amount: UnsignedAmount) -> (i64, i64) {
        let value = amount.value();
        let divisor: u128 = crate::amounts::GLOBAL_SPLIT_DIVISOR as u128;
        let hi = i64::try_from(value / divisor).expect("hi fits");
        let lo = i64::try_from(value % divisor).expect("lo fits");
        (hi, lo)
    }

    fn mark_address_api_balance_success(
        connection: &rusqlite::Connection,
        address_id: impl std::fmt::Display,
        balance: UnsignedAmount,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) {
        let timestamp = timestamp.to_rfc3339();
        let hi = i64::try_from(balance.value() / crate::amounts::GLOBAL_SPLIT_DIVISOR as u128)
            .expect("test balance high part should fit i64");
        let lo = i64::try_from(balance.value() % crate::amounts::GLOBAL_SPLIT_DIVISOR as u128)
            .expect("test balance low part should fit i64");
        connection
            .execute(
                "INSERT INTO transaction_sync_state
                 (id, scope, address_id, last_run_id, last_started_at, last_completed_at, last_result, last_error, last_tip_height, new_tx_count, updated_tx_count, api_confirmed_balance_hi, api_confirmed_balance_lo, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    Ulid::new().to_string(),
                    "address",
                    address_id.to_string(),
                    Ulid::new().to_string(),
                    &timestamp,
                    &timestamp,
                    "success",
                    Option::<String>::None,
                    Option::<i64>::None,
                    0_i64,
                    0_i64,
                    hi,
                    lo,
                    &timestamp,
                    &timestamp,
                ],
            )
            .expect("API balance sync state should insert");
    }

    #[test]
    fn export_writes_journal_for_account_without_confirmed_transactions() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let temp_root = TempExportRoot::new();

        let now = fixed_time(20, 10);
        let username = "rustic-detective";
        let (owner_directory_segment, owner_posting_segment) =
            owner_segments_from_username(username);
        insert_test_user(user_id, username, now);
        let eth_raw = RawEthAddress::new("0x52908400098527886E0F7030069857D2E4169EE7".to_string());
        let eth_address = EthAddress::parse(&eth_raw).expect("valid ETH address");
        let response = add_ethereum_address(
            user_id,
            &eth_address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Main Wallet")),
            now,
        )
        .expect("ethereum account should insert");
        update_wallet_account_label(
            user_id,
            response.account_id,
            account_label("Ethereum Account 1"),
            now,
        )
        .expect("account label should update");

        let hledger_dir = temp_root.hledger_dir(user_id);
        let result = export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &owner_posting_segment,
        )
        .expect("export should succeed");
        assert_eq!(result.accounts_exported, 1);
        assert_eq!(result.transactions_exported, 0);
        assert_eq!(result.balance_assertions_exported, 0);
        assert_eq!(result.export_dir, hledger_dir);

        let wallet_segment = normalize_label_for_hledger("Main Wallet");
        let account_segment = normalize_label_for_hledger("Ethereum Account 1");
        let all_years = hledger_owner_account_all_years_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
        );
        let all_years_contents =
            std::fs::read_to_string(&all_years).expect("account journal should exist");
        assert_generated_header(&all_years_contents);
        assert!(all_years_contents.contains(EMPTY_ACCOUNT_COMMENT));

        let directives_contents = std::fs::read_to_string(hledger_directives_path(&hledger_dir))
            .expect("directives journal should exist");
        assert_generated_header(&directives_contents);
        assert!(directives_contents.contains("commodity 0.000000000000000000 ETH"));
    }

    #[test]
    fn export_native_api_balance_assertion_without_transactions_writes_assertion_only_journal() {
        let runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let temp_root = TempExportRoot::new();

        let now = fixed_time(20, 10);
        let username = "rustic-detective";
        let (owner_directory_segment, owner_posting_segment) =
            owner_segments_from_username(username);
        insert_test_user(user_id, username, now);
        let eth_raw = RawEthAddress::new("0x52908400098527886E0F7030069857D2E4169EE7".to_string());
        let eth_address = EthAddress::parse(&eth_raw).expect("valid ETH address");
        let response = add_ethereum_address(
            user_id,
            &eth_address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Main Wallet")),
            now,
        )
        .expect("ethereum account should insert");
        update_wallet_account_label(
            user_id,
            response.account_id,
            account_label("Ethereum Account 1"),
            now,
        )
        .expect("account label should update");

        let connection = open_test_user_db(&runtime, user_id);
        mark_address_api_balance_success(
            &connection,
            response.address_id,
            UnsignedAmount::from_u128(2_441_190_093_160_u128),
            now,
        );
        drop(connection);

        let hledger_dir = temp_root.hledger_dir(user_id);
        let result = export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &owner_posting_segment,
        )
        .expect("export should succeed");
        assert_eq!(result.accounts_exported, 1);
        assert_eq!(result.transactions_exported, 0);
        assert_eq!(result.balance_assertions_exported, 1);

        let wallet_segment = normalize_label_for_hledger("Main Wallet");
        let account_segment = normalize_label_for_hledger("Ethereum Account 1");
        let journal = std::fs::read_to_string(hledger_owner_account_year_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
            "2026",
        ))
        .expect("assertion journal should exist");
        assert_generated_header(&journal);
        assert!(journal.contains("2026-02-20 * API Balance Assertion: provider balance sync"));
        assert!(journal.contains("= 0.000002441190093160 ETH"));

        assert!(
            !hledger_owner_account_year_opening_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2026",
            )
            .exists()
        );
        assert!(
            !hledger_owner_account_year_closing_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2026",
            )
            .exists()
        );
        let include = std::fs::read_to_string(hledger_owner_account_year_include_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
            "2026",
        ))
        .expect("include journal should exist");
        assert_generated_header(&include);
        assert!(!include.contains("2026-opening.j.txt"));
        assert!(include.contains("include journal/2026/2026.j.txt"));
        assert!(!include.contains("2026-closing.j.txt"));
    }

    #[test]
    fn hledger_unasserted_ethereum_window_still_fails_preparation() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = fixed_time(20, 10);
        let username = "rustic-detective";
        insert_test_user(user_id, username, now);

        let eth_raw = RawEthAddress::new("0x52908400098527886E0F7030069857D2E4169EE7".to_string());
        let eth_address = EthAddress::parse(&eth_raw).expect("valid ETH address");
        let response = add_ethereum_address(
            user_id,
            &eth_address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Main Wallet")),
            now,
        )
        .expect("ethereum account should insert");
        let external_address = TrackedAddress::parse("0x1111111111111111111111111111111111111111")
            .expect("valid external tracked address");
        let owned_address =
            TrackedAddress::parse(&eth_address.checksummed()).expect("valid owned tracked address");
        reconcile_account_transactions(
            user_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            &[SyncAccountTransactionRecord {
                tx_hash: TxHash::parse(
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                )
                .expect("valid tx hash"),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(200),
                block_hash: Some("eth-block-200".to_string()),
                block_time: Some(now),
                fee_amount: Some(UnsignedAmount::zero()),
                nonce: Some(1),
                transfers: vec![SyncAccountTransferRecord {
                    provider_transfer_key: ProviderTransferKey::normal(),
                    transfer_index: 0,
                    transfer_kind: TransferKind::Normal,
                    from_address: Some(external_address),
                    to_address: Some(owned_address),
                    value_amount: UnsignedAmount::from_u128(1_000_000_000_000_000_000),
                }],
            }],
            now,
        )
        .expect("ethereum transaction should reconcile");
        rebuild_account_transaction_ledger(user_id, response.account_id, now)
            .expect("ethereum ledger should rebuild");
        with_user_db_mut_for_test(user_id, |connection| {
            let updated = connection
                .execute(
                    "UPDATE account_transaction_ledger
                     SET closing_balance_hi = NULL,
                         closing_balance_lo = NULL
                     WHERE account_id = ?1",
                    params![response.account_id.to_string()],
                )
                .expect("ethereum closing balance should clear");
            assert_eq!(updated, 1);
        });

        let account_rows =
            load_all_accounts_for_export(user_id).expect("export accounts should load");
        let incomplete_bitcoin_accounts = load_incomplete_bitcoin_account_ids_for_export(
            user_id,
            &account_rows,
            export_history_cap(user_id).expect("export history cap should load"),
        )
        .expect("incomplete Bitcoin accounts should load");
        let error = load_transactions_by_account(user_id, &incomplete_bitcoin_accounts)
            .expect_err("unasserted Ethereum window must fail preparation");
        assert!(error.to_string().contains("missing closing balance"));
    }

    #[test]
    fn hledger_balance_only_bitcoin_exports_latest_provider_assertion() {
        let runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let temp_root = TempExportRoot::new();

        let now = fixed_time(20, 10);
        let username = "rustic-detective";
        let (owner_directory_segment, owner_posting_segment) =
            owner_segments_from_username(username);
        insert_test_user(user_id, username, now);

        let btc_address = crate::wallets::BtcAddress::parse(
            &RawBtcAddress::new("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string()),
            Network::Mainnet,
        )
        .expect("valid BTC address");
        let response = add_bitcoin_address(
            user_id,
            &btc_address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Balance Wallet")),
            now,
        )
        .expect("bitcoin account should insert");
        update_wallet_account_label(
            user_id,
            response.account_id,
            account_label("Bitcoin Balance Only"),
            now,
        )
        .expect("bitcoin account label should update");

        let connection = open_test_user_db(&runtime, user_id);
        mark_address_api_balance_success(
            &connection,
            response.address_id,
            UnsignedAmount::from_u128(12_835_640),
            now,
        );
        drop(connection);

        let hledger_dir = temp_root.hledger_dir(user_id);
        let result = export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &owner_posting_segment,
        )
        .expect("balance-only Bitcoin export should succeed");
        assert_eq!(result.transactions_exported, 0);
        assert_eq!(result.balance_assertions_exported, 1);

        let wallet_segment = normalize_label_for_hledger("Balance Wallet");
        let account_segment = normalize_label_for_hledger("Bitcoin Balance Only");
        let journal = std::fs::read_to_string(hledger_owner_account_year_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
            "2026",
        ))
        .expect("balance assertion journal should exist");
        assert!(journal.contains("2026-02-20 * API Balance Assertion: provider balance sync"));
        assert!(journal.contains("= 0.12835640 BTC"));
        assert!(!journal.contains("; Transaction "));
        assert!(
            !hledger_owner_account_year_opening_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2026",
            )
            .exists()
        );
        assert!(
            !hledger_owner_account_year_closing_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2026",
            )
            .exists()
        );
        assert_hledger_parses(&hledger_dir);
    }

    #[test]
    fn hledger_limited_bitcoin_exports_proven_window_and_provider_assertion() {
        let runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let temp_root = TempExportRoot::new();
        let now = fixed_time(22, 10);
        let username = "rustic-detective";
        let (owner_directory_segment, owner_posting_segment) =
            owner_segments_from_username(username);
        insert_test_user(user_id, username, now);

        let btc_address_raw = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";
        let btc_address = crate::wallets::BtcAddress::parse(
            &RawBtcAddress::new(btc_address_raw.to_string()),
            Network::Mainnet,
        )
        .expect("valid BTC address");
        let response = add_bitcoin_address(
            user_id,
            &btc_address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Limited Wallet")),
            now,
        )
        .expect("bitcoin account should insert");
        update_wallet_account_label(
            user_id,
            response.account_id,
            account_label("Limited Bitcoin"),
            now,
        )
        .expect("bitcoin account label should update");

        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[SyncTransactionRecord {
                tx_hash: TxHash::parse(
                    "abababababababababababababababababababababababababababababababab",
                )
                .expect("valid tx hash"),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(100),
                block_hash: Some("limited-block-100".to_string()),
                block_time: Some(now),
                fee_amount: Some(0),
                inputs: Vec::new(),
                outputs: vec![SyncTransactionOutputRecord {
                    output_index: 0,
                    raw_address: Some(
                        TrackedAddress::parse(btc_address_raw).expect("valid tracked BTC address"),
                    ),
                    script_pubkey_hex: "00".to_string(),
                    value_amount: 100_000,
                }],
            }],
            now,
        )
        .expect("bitcoin transaction should reconcile");
        let connection = open_test_user_db(&runtime, user_id);
        mark_address_api_balance_success(
            &connection,
            response.address_id,
            UnsignedAmount::from_u128(100_000),
            now,
        );
        drop(connection);
        rebuild_account_transaction_ledger(user_id, response.account_id, now)
            .expect("incomplete ledger should rebuild");

        let hledger_dir = temp_root.hledger_dir(user_id);
        let result = export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &owner_posting_segment,
        )
        .expect("incomplete Bitcoin export should succeed");
        assert_eq!(result.transactions_exported, 1);
        assert_eq!(result.balance_assertions_exported, 1);

        let wallet_segment = normalize_label_for_hledger("Limited Wallet");
        let account_segment = normalize_label_for_hledger("Limited Bitcoin");
        let journal = std::fs::read_to_string(hledger_owner_account_year_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
            "2026",
        ))
        .expect("limited Bitcoin transaction journal should exist");
        assert!(journal.contains(
            "    ; Transaction abababababababababababababababababababababababababababababababab"
        ));
        assert!(journal.contains("2026-02-22 * API Balance Assertion: provider balance sync"));
        assert_eq!(journal.matches("= 0.00100000 BTC").count(), 2);
        assert!(
            !hledger_owner_account_year_opening_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2026",
            )
            .exists()
        );
        assert!(
            !hledger_owner_account_year_closing_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2026",
            )
            .exists()
        );
        assert_hledger_parses(&hledger_dir);
    }

    #[test]
    fn export_resolves_duplicate_normalized_segments_deterministically() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let temp_root = TempExportRoot::new();

        let now = fixed_time(21, 9);
        let username = "rustic-detective";
        let (owner_directory_segment, _owner_posting_segment) =
            owner_segments_from_username(username);
        insert_test_user(user_id, username, now);
        let btc_one = crate::wallets::BtcAddress::parse(
            &RawBtcAddress::new("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string()),
            Network::Mainnet,
        )
        .expect("valid BTC address");
        let btc_two = crate::wallets::BtcAddress::parse(
            &RawBtcAddress::new("1BoatSLRHtKNngkdXEeobR76b53LETtpyT".to_string()),
            Network::Mainnet,
        )
        .expect("valid BTC address");

        let first = add_bitcoin_address(
            user_id,
            &btc_one,
            Network::Mainnet,
            None,
            Some(&wallet_label("Main Wallet!!!")),
            now,
        )
        .expect("first bitcoin account should insert");
        let second = add_bitcoin_address(
            user_id,
            &btc_two,
            Network::Mainnet,
            None,
            Some(&wallet_label("Main/Wallet")),
            now,
        )
        .expect("second bitcoin account should insert");

        update_wallet_account_label(
            user_id,
            first.account_id,
            account_label("Bitcoin Account #1"),
            now,
        )
        .expect("first account label should update");
        update_wallet_account_label(
            user_id,
            second.account_id,
            account_label("Bitcoin.Account.1"),
            now,
        )
        .expect("second account label should update");

        let hledger_dir = temp_root.hledger_dir(user_id);
        let result = export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &normalize_owner_posting_segment(&owner_directory_segment),
        )
        .expect("export should succeed");
        assert_eq!(result.accounts_exported, 2);
        assert_eq!(result.transactions_exported, 0);
        assert_eq!(result.balance_assertions_exported, 0);

        let wallet_segment = normalize_label_for_hledger("Main Wallet");
        let base_account_segment = normalize_label_for_hledger("Bitcoin Account 1");
        let first_suffix = {
            let text = first.account_id.to_string();
            text[text.len().saturating_sub(8)..].to_string()
        };
        let second_suffix = {
            let text = second.account_id.to_string();
            text[text.len().saturating_sub(8)..].to_string()
        };
        let first_account_segment = format!("{base_account_segment}__{first_suffix}");
        let second_account_segment = format!("{base_account_segment}__{second_suffix}");

        let first_path = hledger_owner_account_all_years_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &first_account_segment,
        );
        let second_path = hledger_owner_account_all_years_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &second_account_segment,
        );
        assert!(first_path.exists());
        assert!(second_path.exists());
        assert_ne!(first_path, second_path);
    }

    #[test]
    fn reexport_atomically_replaces_previous_root_and_removes_stale_files() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let temp_root = TempExportRoot::new();

        let now = fixed_time(22, 8);
        let username = "rustic-detective";
        let (owner_directory_segment, owner_posting_segment) =
            owner_segments_from_username(username);
        insert_test_user(user_id, username, now);
        let eth_raw = RawEthAddress::new("0xde0B295669a9FD93d5F28D9Ec85E40f4cb697BAe".to_string());
        let eth_address = EthAddress::parse(&eth_raw).expect("valid ETH address");
        let response = add_ethereum_address(
            user_id,
            &eth_address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Atomic Wallet")),
            now,
        )
        .expect("ethereum account should insert");
        update_wallet_account_label(
            user_id,
            response.account_id,
            account_label("Atomic Account"),
            now,
        )
        .expect("account label should update");

        let hledger_dir = temp_root.hledger_dir(user_id);
        export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &owner_posting_segment,
        )
        .expect("first export should succeed");

        let stale_file = hledger_dir.join("stale").join("old.txt");
        let stale_parent = stale_file.parent().expect("stale parent should exist");
        std::fs::create_dir_all(stale_parent).expect("stale dir should create");
        std::fs::write(&stale_file, "stale").expect("stale file should write");
        assert!(stale_file.exists());

        export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &owner_posting_segment,
        )
        .expect("second export should succeed");
        assert!(!stale_file.exists());

        let wallet_segment = normalize_label_for_hledger("Atomic Wallet");
        let account_segment = normalize_label_for_hledger("Atomic Account");
        let all_years = hledger_owner_account_all_years_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
        );
        assert!(all_years.exists());
        assert!(hledger_directives_path(&hledger_dir).exists());
    }

    #[test]
    fn export_happy_path_writes_btc_and_eth_transaction_snapshots() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let temp_root = TempExportRoot::new();

        let now = fixed_time(23, 11);
        let username = "rustic-detective";
        let (owner_directory_segment, owner_posting_segment) =
            owner_segments_from_username(username);
        insert_test_user(user_id, username, now);

        let btc_address_raw = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";
        let btc_address = crate::wallets::BtcAddress::parse(
            &RawBtcAddress::new(btc_address_raw.to_string()),
            Network::Mainnet,
        )
        .expect("valid BTC address");
        let btc_response = add_bitcoin_address(
            user_id,
            &btc_address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Main Wallet")),
            now,
        )
        .expect("bitcoin account should insert");
        update_wallet_account_label(
            user_id,
            btc_response.account_id,
            account_label("Bitcoin Account"),
            now,
        )
        .expect("bitcoin account label should update");

        let eth_raw = RawEthAddress::new("0x52908400098527886E0F7030069857D2E4169EE7".to_string());
        let eth_address = EthAddress::parse(&eth_raw).expect("valid ETH address");
        let eth_checksummed = eth_address.checksummed();
        let eth_response = add_ethereum_address(
            user_id,
            &eth_address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Hardware Wallet")),
            now,
        )
        .expect("ethereum account should insert");
        update_wallet_account_label(
            user_id,
            eth_response.account_id,
            account_label("Ethereum Account"),
            now,
        )
        .expect("ethereum account label should update");

        let btc_receive_hash =
            TxHash::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .expect("valid tx hash");
        let btc_spend_hash =
            TxHash::parse("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd")
                .expect("valid tx hash");
        let btc_cospend_receive_hash =
            TxHash::parse("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
                .expect("valid tx hash");
        let btc_cospend_hash =
            TxHash::parse("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
                .expect("valid tx hash");
        let btc_cospend_external_prev_hash =
            TxHash::parse("9999999999999999999999999999999999999999999999999999999999999999")
                .expect("valid tx hash");
        let btc_tracked_address =
            TrackedAddress::parse(btc_address_raw).expect("valid tracked BTC address");
        let btc_receive = SyncTransactionRecord {
            tx_hash: btc_receive_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(100),
            block_hash: Some("btc-block-100".to_string()),
            block_time: Some(fixed_time(23, 12)),
            fee_amount: Some(0_i64),
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(btc_tracked_address.clone()),
                script_pubkey_hex: "0014751e76e8199196d454941c45d1b3a323f1433bd6".to_string(),
                value_amount: 10_000_000_i64,
            }],
        };
        let btc_spend = SyncTransactionRecord {
            tx_hash: btc_spend_hash,
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(101),
            block_hash: Some("btc-block-101".to_string()),
            block_time: Some(fixed_time(23, 13)),
            fee_amount: Some(3_172_i64),
            inputs: vec![SyncTransactionInputRecord {
                input_index: 0,
                prev_tx_hash: btc_receive_hash,
                prev_output_index: 0,
                prev_address: Some(btc_tracked_address.clone()),
                value_amount: Some(10_000_000_i64),
            }],
            outputs: Vec::new(),
        };
        let btc_cospend_receive = SyncTransactionRecord {
            tx_hash: btc_cospend_receive_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(102),
            block_hash: Some("btc-block-102".to_string()),
            block_time: Some(fixed_time(23, 14)),
            fee_amount: Some(0_i64),
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(btc_tracked_address.clone()),
                script_pubkey_hex: "0014751e76e8199196d454941c45d1b3a323f1433bd6".to_string(),
                value_amount: 888_i64,
            }],
        };
        let btc_cospend = SyncTransactionRecord {
            tx_hash: btc_cospend_hash,
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(103),
            block_hash: Some("btc-block-103".to_string()),
            block_time: Some(fixed_time(23, 15)),
            fee_amount: Some(30_624_i64),
            inputs: vec![
                SyncTransactionInputRecord {
                    input_index: 0,
                    prev_tx_hash: btc_cospend_receive_hash,
                    prev_output_index: 0,
                    prev_address: Some(btc_tracked_address),
                    value_amount: Some(888_i64),
                },
                SyncTransactionInputRecord {
                    input_index: 1,
                    prev_tx_hash: btc_cospend_external_prev_hash,
                    prev_output_index: 0,
                    prev_address: None,
                    value_amount: Some(6_523_671_306_814_i64),
                },
            ],
            outputs: Vec::new(),
        };
        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[btc_receive, btc_spend, btc_cospend_receive, btc_cospend],
            fixed_time(23, 15),
        )
        .expect("bitcoin reconcile should succeed");

        let eth_incoming = SyncAccountTransactionRecord {
            tx_hash: TxHash::parse(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )
            .expect("valid tx hash"),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(200),
            block_hash: Some("eth-block-200".to_string()),
            block_time: Some(fixed_time(23, 13)),
            fee_amount: Some(UnsignedAmount::zero()),
            nonce: Some(1_i64),
            transfers: vec![SyncAccountTransferRecord {
                provider_transfer_key: ProviderTransferKey::normal(),
                transfer_index: 0,
                transfer_kind: TransferKind::Normal,
                from_address: Some(
                    TrackedAddress::parse("0x1111111111111111111111111111111111111111")
                        .expect("valid external tracked address"),
                ),
                to_address: Some(
                    TrackedAddress::parse(&eth_checksummed).expect("valid owned tracked address"),
                ),
                value_amount: UnsignedAmount::from_u128(1_000_000_000_000_000_000_u128),
            }],
        };
        let eth_outgoing = SyncAccountTransactionRecord {
            tx_hash: TxHash::parse(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            )
            .expect("valid tx hash"),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(201),
            block_hash: Some("eth-block-201".to_string()),
            block_time: Some(fixed_time(23, 14)),
            fee_amount: Some(UnsignedAmount::from_u128(10_000_000_000_000_000_u128)),
            nonce: Some(2_i64),
            transfers: vec![SyncAccountTransferRecord {
                provider_transfer_key: ProviderTransferKey::normal(),
                transfer_index: 0,
                transfer_kind: TransferKind::Normal,
                from_address: Some(
                    TrackedAddress::parse(&eth_checksummed).expect("valid owned tracked address"),
                ),
                to_address: Some(
                    TrackedAddress::parse("0x2222222222222222222222222222222222222222")
                        .expect("valid external tracked address"),
                ),
                value_amount: UnsignedAmount::from_u128(500_000_000_000_000_000_u128),
            }],
        };
        reconcile_account_transactions(
            user_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            &[eth_incoming, eth_outgoing],
            fixed_time(23, 14),
        )
        .expect("ethereum reconcile should succeed");
        publish_complete_bitcoin_ledger(
            user_id,
            btc_response.account_id,
            btc_response.address_id,
            4,
            103,
            fixed_time(23, 15),
        );
        rebuild_account_transaction_ledger(user_id, eth_response.account_id, fixed_time(23, 14))
            .expect("ethereum ledger rebuild should succeed");

        let hledger_dir = temp_root.hledger_dir(user_id);
        let result = export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &owner_posting_segment,
        )
        .expect("export should succeed");
        assert_eq!(result.accounts_exported, 2);
        assert_eq!(result.transactions_exported, 6);
        assert_eq!(result.balance_assertions_exported, 0);

        let directives = std::fs::read_to_string(hledger_directives_path(&hledger_dir))
            .expect("directives journal should exist");
        assert_generated_header(&directives);
        assert!(directives.contains("commodity 0.00000000 BTC"));
        assert!(directives.contains("commodity 0.000000000000000000 ETH"));

        let btc_wallet_segment = normalize_label_for_hledger("Main Wallet");
        let btc_account_segment = normalize_label_for_hledger("Bitcoin Account");
        let year = "2026";
        let btc_journal = std::fs::read_to_string(hledger_owner_account_year_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &btc_wallet_segment,
            &btc_account_segment,
            year,
        ))
        .expect("btc journal should exist");
        assert_generated_header(&btc_journal);
        assert!(!btc_journal.contains("include "));
        assert!(btc_journal.contains("2026-02-23 * Received Bitcoin"));
        assert!(btc_journal.contains(
            "    ; Transaction aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        assert!(btc_journal.contains("0.10000000 BTC = 0.10000000 BTC"));
        assert!(btc_journal.contains("2026-02-23 * Sent Bitcoin"));
        assert!(btc_journal.contains(
            "    ; Transaction dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
        ));
        assert!(btc_journal.contains(&format!(
            "assets:{owner_posting_segment}:{btc_wallet_segment}:{btc_account_segment}    -0.10000000 BTC = 0.00000000 BTC\n    expenses:Fees:Bitcoin:Network:Mainnet    0.00003172 BTC\n    expenses:unknown    0.09996828 BTC"
        )));
        assert!(btc_journal.contains(
            "    ; Transaction eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        ));
        assert!(btc_journal.contains("0.00000888 BTC = 0.00000888 BTC"));
        assert!(btc_journal.contains(
            "    ; Transaction ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        ));
        let cospend_section = hledger_transaction_block_by_hash(
            &btc_journal,
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        );
        assert!(cospend_section.contains("-0.00000888 BTC = 0.00000000 BTC"));
        assert!(!cospend_section.contains("-0.00030624 BTC = 0.00000000 BTC"));
        assert!(
            cospend_section.contains("expenses:Fees:Bitcoin:Network:Mainnet    0.00000888 BTC")
        );
        assert!(!cospend_section.contains("expenses:unknown"));
        assert!(!btc_journal.contains("expenses:fees"));
        assert!(btc_journal.contains("BTC"));
        let btc_opening_journal =
            std::fs::read_to_string(hledger_owner_account_year_opening_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &btc_wallet_segment,
                &btc_account_segment,
                year,
            ))
            .expect("btc opening journal should exist");
        assert_generated_header(&btc_opening_journal);
        assert!(btc_opening_journal.contains("Opening balance"));
        assert!(btc_opening_journal.contains(&format!(
            "assets:{owner_posting_segment}:{btc_wallet_segment}:{btc_account_segment}"
        )));
        assert!(btc_opening_journal.contains(" 0.00000000 BTC"));
        assert!(
            !hledger_owner_account_year_closing_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &btc_wallet_segment,
                &btc_account_segment,
                year,
            )
            .exists()
        );
        let btc_include = std::fs::read_to_string(hledger_owner_account_year_include_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &btc_wallet_segment,
            &btc_account_segment,
            year,
        ))
        .expect("btc include journal should exist");
        assert_generated_header(&btc_include);
        assert!(btc_include.contains("include 2026-opening.j.txt"));
        assert!(btc_include.contains("include journal/2026/2026.j.txt"));
        assert!(!btc_include.contains("include 2026-closing.j.txt"));
        let btc_all_years = std::fs::read_to_string(hledger_owner_account_all_years_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &btc_wallet_segment,
            &btc_account_segment,
        ))
        .expect("btc all-years should exist");
        assert_generated_header(&btc_all_years);
        assert!(btc_all_years.contains("include 2026-include.j.txt"));

        let eth_wallet_segment = normalize_label_for_hledger("Hardware Wallet");
        let eth_account_segment = normalize_label_for_hledger("Ethereum Account");
        let year = "2026";
        let eth_journal = std::fs::read_to_string(hledger_owner_account_year_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &eth_wallet_segment,
            &eth_account_segment,
            year,
        ))
        .expect("eth journal should exist");
        assert_generated_header(&eth_journal);
        assert!(!eth_journal.contains("include "));
        assert!(eth_journal.contains("2026-02-23 * Received Ethereum"));
        assert!(eth_journal.contains(
            "    ; Transaction bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        ));
        assert!(eth_journal.contains("1.000000000000000000 ETH = 1.000000000000000000 ETH"));
        assert!(eth_journal.contains("2026-02-23 * Sent Ethereum"));
        let received_marker = eth_journal
            .find("2026-02-23 * Received Ethereum")
            .expect("received ethereum transaction exists");
        let sent_marker = eth_journal
            .find("2026-02-23 * Sent Ethereum")
            .expect("sent ethereum transaction exists");
        let received_section = &eth_journal[received_marker..sent_marker];
        assert!(!received_section.contains("expenses:Fees:Ethereum:Network:Mainnet"));
        assert!(eth_journal.contains(
            "    ; Transaction cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        ));
        assert!(eth_journal.contains("-0.510000000000000000 ETH = 0.490000000000000000 ETH"));
        assert!(
            eth_journal
                .contains("expenses:Fees:Ethereum:Network:Mainnet    0.010000000000000000 ETH")
        );
        assert!(!eth_journal.contains("expenses:fees"));
        assert!(eth_journal.contains("ETH"));
        let eth_opening_journal =
            std::fs::read_to_string(hledger_owner_account_year_opening_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &eth_wallet_segment,
                &eth_account_segment,
                year,
            ))
            .expect("eth opening journal should exist");
        assert_generated_header(&eth_opening_journal);
        assert!(eth_opening_journal.contains("Opening balance"));
        assert!(eth_opening_journal.contains(&format!(
            "assets:{owner_posting_segment}:{eth_wallet_segment}:{eth_account_segment}"
        )));
        assert!(eth_opening_journal.contains(" 0.000000000000000000 ETH"));
        assert!(
            !hledger_owner_account_year_closing_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &eth_wallet_segment,
                &eth_account_segment,
                year,
            )
            .exists()
        );
        let eth_include = std::fs::read_to_string(hledger_owner_account_year_include_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &eth_wallet_segment,
            &eth_account_segment,
            year,
        ))
        .expect("eth include journal should exist");
        assert_generated_header(&eth_include);
        assert!(eth_include.contains("include 2026-opening.j.txt"));
        assert!(eth_include.contains("include journal/2026/2026.j.txt"));
        assert!(!eth_include.contains("include 2026-closing.j.txt"));

        let owner_all_years = std::fs::read_to_string(
            hledger_dir
                .join(&owner_directory_segment)
                .join("all-years.j.txt"),
        )
        .expect("owner all-years should exist");
        assert_generated_header(&owner_all_years);
        assert!(owner_all_years.contains("include 2026-include.j.txt"));

        let btc_wallet_year_include = std::fs::read_to_string(
            hledger_dir
                .join(&owner_directory_segment)
                .join(&btc_wallet_segment)
                .join("2026-include.j.txt"),
        )
        .expect("btc wallet yearly include should exist");
        assert_generated_header(&btc_wallet_year_include);
        assert!(btc_wallet_year_include.contains("include BitcoinAccount/2026-include.j.txt"));
        assert!(!btc_wallet_year_include.contains("EthereumAccount/2026-include.j.txt"));

        let eth_wallet_year_include = std::fs::read_to_string(
            hledger_dir
                .join(&owner_directory_segment)
                .join(&eth_wallet_segment)
                .join("2026-include.j.txt"),
        )
        .expect("eth wallet yearly include should exist");
        assert_generated_header(&eth_wallet_year_include);
        assert!(eth_wallet_year_include.contains("include EthereumAccount/2026-include.j.txt"));

        let root_year_include = std::fs::read_to_string(hledger_dir.join("2026-include.j.txt"))
            .expect("root yearly include should exist");
        assert_generated_header(&root_year_include);
        assert!(root_year_include.contains(&format!(
            "include {owner_directory_segment}/2026-include.j.txt"
        )));

        let root_all_years = std::fs::read_to_string(hledger_dir.join("all-years.j.txt"))
            .expect("root all-years should exist");
        assert_generated_header(&root_all_years);
        assert!(root_all_years.contains("include 2026-include.j.txt"));

        let root_entry =
            std::fs::read_to_string(hledger_dir.join("bitgarth.j.txt")).expect("root entry exists");
        assert_eq!(
            root_entry,
            "; Generated by https://bitgarth.app/\n\ninclude directives.j.txt\ninclude all-years.j.txt\n"
        );
        assert_hledger_parses(&hledger_dir);
    }

    #[test]
    fn export_manual_assertions_write_synthetic_transactions_and_conditional_boundaries() {
        let runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let temp_root = TempExportRoot::new();

        let now = fixed_time(24, 11);
        let username = "rustic-detective";
        let (owner_directory_segment, owner_posting_segment) =
            owner_segments_from_username(username);
        insert_test_user(user_id, username, now);

        let wallet_id = WalletId::new();
        let wallet = wallet_label("Manual Wallet");
        let connection = open_test_user_db(&runtime, user_id);
        insert_wallet_fixture(&connection, wallet_id, &wallet, now);
        drop(connection);

        let account = create_manual_account(user_id, wallet_id, "ADA", now);
        add_manual_assertion(
            user_id,
            account.account_id,
            NaiveDate::from_ymd_opt(2026, 2, 10).expect("valid date"),
            "1.25",
            Some(" corrected;\nmanual snapshot "),
            now,
        );
        add_manual_assertion(
            user_id,
            account.account_id,
            NaiveDate::from_ymd_opt(2026, 2, 20).expect("valid date"),
            "2.00",
            None,
            now,
        );
        add_manual_assertion(
            user_id,
            account.account_id,
            NaiveDate::from_ymd_opt(2027, 3, 1).expect("valid date"),
            "0",
            None,
            now,
        );

        let hledger_dir = temp_root.hledger_dir(user_id);
        let result = export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &owner_posting_segment,
        )
        .expect("manual export should succeed");
        assert_eq!(result.accounts_exported, 1);
        assert_eq!(result.transactions_exported, 0);
        assert_eq!(result.balance_assertions_exported, 3);

        let directives = std::fs::read_to_string(hledger_directives_path(&hledger_dir))
            .expect("directives journal should exist");
        assert_generated_header(&directives);
        assert!(directives.contains("commodity 0.00 ADA"));

        let wallet_segment = normalize_label_for_hledger("Manual Wallet");
        let account_segment = normalize_label_for_hledger("ADA Account 1");

        let journal_2026 = std::fs::read_to_string(hledger_owner_account_year_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
            "2026",
        ))
        .expect("2026 journal should exist");
        assert_generated_header(&journal_2026);
        assert!(
            journal_2026.contains("2026-02-10 * Balance Assertion: corrected, manual snapshot")
        );
        assert!(journal_2026.contains("= 1.25 ADA"));
        assert!(journal_2026.contains("= 2.00 ADA"));

        let opening_2026 = hledger_owner_account_year_opening_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
            "2026",
        );
        assert!(!opening_2026.exists());

        let closing_2026 =
            std::fs::read_to_string(hledger_owner_account_year_closing_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2026",
            ))
            .expect("2026 closing journal should exist");
        assert_generated_header(&closing_2026);
        assert!(closing_2026.contains("Closing balance"));
        assert!(closing_2026.contains("-2.00 ADA"));

        let include_2026 =
            std::fs::read_to_string(hledger_owner_account_year_include_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2026",
            ))
            .expect("2026 include journal should exist");
        assert_generated_header(&include_2026);
        assert!(!include_2026.contains("include 2026-opening.j.txt"));
        assert!(include_2026.contains("include journal/2026/2026.j.txt"));
        assert!(include_2026.contains("include 2026-closing.j.txt"));

        let opening_2027 =
            std::fs::read_to_string(hledger_owner_account_year_opening_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2027",
            ))
            .expect("2027 opening journal should exist");
        assert_generated_header(&opening_2027);
        assert!(opening_2027.contains("Opening balance"));
        assert!(opening_2027.contains(" 2.00 ADA"));

        let journal_2027 = std::fs::read_to_string(hledger_owner_account_year_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
            "2027",
        ))
        .expect("2027 journal should exist");
        assert_generated_header(&journal_2027);
        assert!(journal_2027.contains("= 0.00 ADA"));
        assert!(
            journal_2027
                .contains("equity:Balance Assertions:RusticDetective:ManualWallet:ADAAccount1")
        );

        let include_2027 =
            std::fs::read_to_string(hledger_owner_account_year_include_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2027",
            ))
            .expect("2027 include journal should exist");
        assert_generated_header(&include_2027);
        assert!(include_2027.contains("include 2027-opening.j.txt"));
        assert!(include_2027.contains("include journal/2027/2027.j.txt"));
        assert!(!include_2027.contains("include 2027-closing.j.txt"));
        assert!(
            !hledger_owner_account_year_closing_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2027",
            )
            .exists()
        );
    }

    #[test]
    fn export_quotes_digit_containing_manual_asset_unit_codes_everywhere() {
        let runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let temp_root = TempExportRoot::new();

        let now = fixed_time(24, 12);
        let username = "rustic-detective";
        let (owner_directory_segment, owner_posting_segment) =
            owner_segments_from_username(username);
        insert_test_user(user_id, username, now);

        let wallet_id = WalletId::new();
        let wallet = wallet_label("Manual Wallet");
        let connection = open_test_user_db(&runtime, user_id);
        insert_wallet_fixture(&connection, wallet_id, &wallet, now);
        drop(connection);

        let account = create_manual_account(user_id, wallet_id, "SP500", now);
        add_manual_assertion(
            user_id,
            account.account_id,
            NaiveDate::from_ymd_opt(2026, 2, 10).expect("valid date"),
            "1.25",
            None,
            now,
        );
        add_manual_assertion(
            user_id,
            account.account_id,
            NaiveDate::from_ymd_opt(2027, 3, 1).expect("valid date"),
            "2.00",
            None,
            now,
        );

        let hledger_dir = temp_root.hledger_dir(user_id);
        export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &owner_posting_segment,
        )
        .expect("quoted manual export should succeed");

        let directives = std::fs::read_to_string(hledger_directives_path(&hledger_dir))
            .expect("directives journal should exist");
        assert_generated_header(&directives);
        assert!(directives.contains("commodity 0.00 \"SP500\""));

        let wallet_segment = normalize_label_for_hledger("Manual Wallet");
        let account_segment = normalize_label_for_hledger("SP500 Account 1");

        let journal_2026 = std::fs::read_to_string(hledger_owner_account_year_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
            "2026",
        ))
        .expect("2026 journal should exist");
        assert_generated_header(&journal_2026);
        assert!(journal_2026.contains("= 1.25 \"SP500\""));

        let opening_2027 =
            std::fs::read_to_string(hledger_owner_account_year_opening_journal_path(
                &hledger_dir,
                &owner_directory_segment,
                &wallet_segment,
                &account_segment,
                "2027",
            ))
            .expect("2027 opening journal should exist");
        assert_generated_header(&opening_2027);
        assert!(opening_2027.contains("1.25 \"SP500\""));
    }

    #[test]
    fn reexport_manual_assertions_includes_middle_insert() {
        let runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let temp_root = TempExportRoot::new();

        let now = fixed_time(25, 11);
        let username = "rustic-detective";
        let (owner_directory_segment, owner_posting_segment) =
            owner_segments_from_username(username);
        insert_test_user(user_id, username, now);

        let wallet_id = WalletId::new();
        let wallet = wallet_label("Manual Wallet");
        let connection = open_test_user_db(&runtime, user_id);
        insert_wallet_fixture(&connection, wallet_id, &wallet, now);
        drop(connection);

        let account = create_manual_account(user_id, wallet_id, "ADA", now);
        add_manual_assertion(
            user_id,
            account.account_id,
            NaiveDate::from_ymd_opt(2026, 2, 10).expect("valid date"),
            "1.0",
            None,
            now,
        );
        add_manual_assertion(
            user_id,
            account.account_id,
            NaiveDate::from_ymd_opt(2026, 2, 20).expect("valid date"),
            "3.0",
            None,
            now,
        );

        let hledger_dir = temp_root.hledger_dir(user_id);
        export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &owner_posting_segment,
        )
        .expect("first manual export should succeed");

        let wallet_segment = normalize_label_for_hledger("Manual Wallet");
        let account_segment = normalize_label_for_hledger("ADA Account 1");
        let journal_path = hledger_owner_account_year_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
            "2026",
        );
        let first_journal =
            std::fs::read_to_string(&journal_path).expect("first 2026 journal should exist");
        assert_generated_header(&first_journal);
        assert!(first_journal.contains("= 3.00 ADA"));

        add_manual_assertion(
            user_id,
            account.account_id,
            NaiveDate::from_ymd_opt(2026, 2, 15).expect("valid date"),
            "1.5",
            None,
            now,
        );

        let result = export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &owner_posting_segment,
        )
        .expect("second manual export should succeed");
        assert_eq!(result.transactions_exported, 0);
        assert_eq!(result.balance_assertions_exported, 3);

        let second_journal =
            std::fs::read_to_string(&journal_path).expect("second 2026 journal should exist");
        assert_generated_header(&second_journal);
        assert!(second_journal.contains("= 1.50 ADA"));
        assert!(second_journal.contains("= 3.00 ADA"));
    }

    #[test]
    fn export_keeps_zero_delta_rows_as_header_only_transactions() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let temp_root = TempExportRoot::new();

        let now = fixed_time(25, 9);
        let username = "rustic-detective";
        let (owner_directory_segment, _owner_posting_segment) =
            owner_segments_from_username(username);
        insert_test_user(user_id, username, now);
        let btc_raw = RawBtcAddress::new("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string());
        let btc_address = crate::wallets::BtcAddress::parse(&btc_raw, Network::Mainnet)
            .expect("valid BTC address");
        let response = add_bitcoin_address(
            user_id,
            &btc_address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Main Wallet")),
            now,
        )
        .expect("bitcoin account should insert");
        update_wallet_account_label(
            user_id,
            response.account_id,
            account_label("Bitcoin Account"),
            now,
        )
        .expect("account label should update");

        let tx = SyncTransactionRecord {
            tx_hash: TxHash::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .expect("valid tx hash"),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(500),
            block_hash: Some("btc-block-500".to_string()),
            block_time: Some(fixed_time(25, 10)),
            fee_amount: Some(0_i64),
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(
                    TrackedAddress::parse("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa")
                        .expect("valid address"),
                ),
                script_pubkey_hex: "0014751e76e8199196d454941c45d1b3a323f1433bd6".to_string(),
                value_amount: 0_i64,
            }],
        };
        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[tx],
            fixed_time(25, 10),
        )
        .expect("bitcoin reconcile should succeed");
        publish_complete_bitcoin_ledger(
            user_id,
            response.account_id,
            response.address_id,
            1,
            500,
            fixed_time(25, 10),
        );

        let hledger_dir = temp_root.hledger_dir(user_id);
        let result = export_all_accounts_to_dir(
            user_id,
            &hledger_dir,
            &owner_directory_segment,
            &_owner_posting_segment,
        )
        .expect("export should succeed with zero-delta rows");
        assert_eq!(result.transactions_exported, 1);
        assert_eq!(result.balance_assertions_exported, 0);
        assert_eq!(result.accounts_exported, 1);

        let wallet_segment = normalize_label_for_hledger("Main Wallet");
        let account_segment = normalize_label_for_hledger("Bitcoin Account");
        let account_journal = hledger_owner_account_year_journal_path(
            &hledger_dir,
            &owner_directory_segment,
            &wallet_segment,
            &account_segment,
            "2026",
        );
        let journal_contents =
            std::fs::read_to_string(account_journal).expect("account journal should exist");
        assert_generated_header(&journal_contents);
        assert!(journal_contents.contains("Received Bitcoin"));
        assert!(journal_contents.contains(
            "; Transaction aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        assert!(!journal_contents.contains(EMPTY_ACCOUNT_COMMENT));
        assert!(!journal_contents.contains("expenses:unknown"));
        assert!(!journal_contents.contains("income:unknown"));
    }
}
