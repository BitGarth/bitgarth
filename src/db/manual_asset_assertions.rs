use super::account_transactions::types::{parse_split_amount, split_unsigned_amount};
use super::error::DbError;
use super::user_db::{with_user_db, with_user_db_mut};
use crate::amounts::UnsignedAmount;
use crate::wallets::{
    ACCOUNT_LABEL_MAX_LENGTH, Label, ManualAssetBalanceAssertionId, ManualAssetDisplayScale,
    ManualAssetPrecisionStatus, TransactionSortDirection,
    ValidatedAddManualAssetBalanceAssertionRequest, ValidatedManualAssetAssertionNote,
    ValidatedManualAssetUnitCode, ValidatedUpdateManualAssetBalanceAssertionRequest,
    WALLET_LABEL_MAX_LENGTH, WalletAccountId, WalletId,
};
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualAssetBalanceAssertionRecord {
    pub(crate) assertion_id: ManualAssetBalanceAssertionId,
    pub(crate) asserted_on: NaiveDate,
    pub(crate) balance: UnsignedAmount,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManualAssetBalanceState {
    Known(UnsignedAmount),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualAssetBalanceAssertionPage {
    pub(crate) page: u32,
    pub(crate) page_size: u32,
    pub(crate) total: u32,
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) rows: Vec<ManualAssetBalanceAssertionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualAssetAccountHistoryData {
    pub(crate) account_id: WalletAccountId,
    pub(crate) wallet_id: WalletId,
    pub(crate) wallet_label: Label,
    pub(crate) account_label: Label,
    pub(crate) unit_code: ValidatedManualAssetUnitCode,
    pub(crate) decimal_precision: ManualAssetDisplayScale,
    pub(crate) symbol: Option<String>,
    pub(crate) asset_name: Option<String>,
    pub(crate) network_name: Option<String>,
    pub(crate) precision_status: ManualAssetPrecisionStatus,
    pub(crate) precision_shared_with_other_accounts: bool,
    pub(crate) opening_balance_state: ManualAssetBalanceState,
    pub(crate) opening_balance_date: Option<NaiveDate>,
    pub(crate) closing_balance_state: ManualAssetBalanceState,
    pub(crate) closing_balance_date: Option<NaiveDate>,
    pub(crate) assertions: ManualAssetBalanceAssertionPage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualAssetWalletReportRowData {
    pub(crate) account_id: WalletAccountId,
    pub(crate) account_label: Label,
    pub(crate) asset_id: crate::asset_capabilities::AssetId,
    pub(crate) unit_code: ValidatedManualAssetUnitCode,
    pub(crate) decimal_precision: ManualAssetDisplayScale,
    pub(crate) opening_balance_state: ManualAssetBalanceState,
    pub(crate) opening_balance_date: Option<NaiveDate>,
    pub(crate) closing_balance_state: ManualAssetBalanceState,
    pub(crate) closing_balance_date: Option<NaiveDate>,
}

#[derive(Debug, Clone)]
pub(crate) enum ManualAssetAssertionDbError {
    AccountNotFound,
    AssertionNotFound,
    DuplicateAssertionDate,
    InactiveAccountReadOnly,
    Database(DbError),
}

impl std::fmt::Display for ManualAssetAssertionDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccountNotFound => write!(f, "Manual asset account not found"),
            Self::AssertionNotFound => write!(f, "Balance assertion not found"),
            Self::DuplicateAssertionDate => {
                write!(f, "A balance assertion already exists for that date")
            }
            Self::InactiveAccountReadOnly => {
                write!(f, "Inactive manual asset accounts are read-only")
            }
            Self::Database(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ManualAssetAssertionDbError {}

impl From<DbError> for ManualAssetAssertionDbError {
    fn from(value: DbError) -> Self {
        Self::Database(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManualAssetAccountMeta {
    wallet_id: WalletId,
    wallet_label: Label,
    account_label: Label,
    unit_code: ValidatedManualAssetUnitCode,
    decimal_precision: ManualAssetDisplayScale,
    symbol: Option<String>,
    asset_name: Option<String>,
    network_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManualAssetWalletReportMeta {
    account_id: WalletAccountId,
    account_label: Label,
    asset_id: crate::asset_capabilities::AssetId,
    unit_code: ValidatedManualAssetUnitCode,
    decimal_precision: ManualAssetDisplayScale,
    created_at_date: NaiveDate,
}

struct ManualAssetAccountSnapshotForAssertions {
    asset_id: crate::asset_capabilities::AssetId,
    network_id: crate::asset_capabilities::unsynced::UnsyncedNetworkId,
    unit_code: ValidatedManualAssetUnitCode,
    decimal_precision: ManualAssetDisplayScale,
    symbol: Option<String>,
    asset_name: String,
    network_name: String,
    coingecko_id: crate::asset_capabilities::unsynced::CoingeckoAssetId,
}

struct ManualAssetAccountSnapshotRaw {
    asset_id: String,
    network_id: String,
    decimal_precision: i64,
    unit_code: String,
    symbol: Option<String>,
    asset_name: String,
    network_name: String,
    coingecko_id: String,
}

impl ManualAssetAccountSnapshotForAssertions {
    fn observe_metadata_fields(&self) {
        let _ = (
            self.network_id.as_str(),
            self.symbol.as_deref(),
            self.asset_name.as_str(),
            self.network_name.as_str(),
            self.coingecko_id.as_str(),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SameUnitPrecisionReadMeta {
    precision_status: ManualAssetPrecisionStatus,
    precision_shared_with_other_accounts: bool,
}

fn parse_asserted_on(raw: &str, field_name: &'static str) -> Result<NaiveDate, DbError> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|err| DbError::new(format!("Invalid {field_name} in DB: {err}")))
}

fn page_bounds(
    page: u32,
    page_size: u32,
    total: u32,
    row_count: usize,
) -> Result<(u32, u32), DbError> {
    if total == 0 || row_count == 0 {
        return Ok((0, 0));
    }

    let start_u64 = u64::from(page.saturating_sub(1))
        .saturating_mul(u64::from(page_size))
        .saturating_add(1);
    let row_count_u64 =
        u64::try_from(row_count).map_err(|_| DbError::new("Assertion row count overflow"))?;
    let end_u64 = start_u64
        .saturating_add(row_count_u64.saturating_sub(1))
        .min(u64::from(total));

    let start =
        u32::try_from(start_u64).map_err(|_| DbError::new("Assertion page start overflow"))?;
    let end = u32::try_from(end_u64).map_err(|_| DbError::new("Assertion page end overflow"))?;
    Ok((start, end))
}

fn total_to_u32(total: usize) -> Result<u32, DbError> {
    u32::try_from(total).map_err(|_| DbError::new("Assertion count exceeds supported range"))
}

fn latest_assertion_on_or_before(
    assertions: &[ManualAssetBalanceAssertionRecord],
    date: NaiveDate,
) -> Option<&ManualAssetBalanceAssertionRecord> {
    assertions
        .iter()
        .rev()
        .find(|assertion| assertion.asserted_on <= date)
}

fn resolve_opening_balance_state(
    assertions: &[ManualAssetBalanceAssertionRecord],
    start_date: Option<NaiveDate>,
) -> (ManualAssetBalanceState, Option<NaiveDate>) {
    match start_date {
        Some(start_date) => {
            let balance_state = latest_assertion_on_or_before(assertions, start_date)
                .map(|assertion| ManualAssetBalanceState::Known(assertion.balance))
                .unwrap_or(ManualAssetBalanceState::Unknown);
            (balance_state, Some(start_date))
        }
        None => match assertions.first() {
            Some(assertion) => (
                ManualAssetBalanceState::Known(assertion.balance),
                Some(assertion.asserted_on),
            ),
            None => (ManualAssetBalanceState::Unknown, None),
        },
    }
}

fn resolve_closing_balance_state(
    assertions: &[ManualAssetBalanceAssertionRecord],
    end_date: Option<NaiveDate>,
) -> (ManualAssetBalanceState, Option<NaiveDate>) {
    match end_date {
        Some(end_date) => {
            let balance_state = latest_assertion_on_or_before(assertions, end_date)
                .map(|assertion| ManualAssetBalanceState::Known(assertion.balance))
                .unwrap_or(ManualAssetBalanceState::Unknown);
            (balance_state, Some(end_date))
        }
        None => assertions
            .last()
            .map(|assertion| {
                (
                    ManualAssetBalanceState::Known(assertion.balance),
                    Some(assertion.asserted_on),
                )
            })
            .unwrap_or((ManualAssetBalanceState::Unknown, None)),
    }
}

fn resolve_wallet_report_balance_state(
    assertions: &[ManualAssetBalanceAssertionRecord],
    boundary_date: NaiveDate,
    account_created_date: NaiveDate,
) -> (ManualAssetBalanceState, Option<NaiveDate>) {
    // A balance assertion on or before the boundary is authoritative and
    // carries forward indefinitely, even across years and even when it is
    // back-dated before the account row was created. Check it before applying
    // the pre-creation zero floor.
    if let Some(assertion) = latest_assertion_on_or_before(assertions, boundary_date) {
        return (
            ManualAssetBalanceState::Known(assertion.balance),
            Some(boundary_date),
        );
    }

    if boundary_date < account_created_date {
        return (
            ManualAssetBalanceState::Known(UnsignedAmount::zero()),
            Some(boundary_date),
        );
    }

    (ManualAssetBalanceState::Unknown, Some(boundary_date))
}

fn filter_assertions_by_date_range(
    assertions: &[ManualAssetBalanceAssertionRecord],
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
) -> Vec<ManualAssetBalanceAssertionRecord> {
    assertions
        .iter()
        .filter(|assertion| {
            let on_or_after_start = start_date
                .map(|start| assertion.asserted_on >= start)
                .unwrap_or(true);
            let on_or_before_end = end_date
                .map(|end| assertion.asserted_on <= end)
                .unwrap_or(true);
            on_or_after_start && on_or_before_end
        })
        .cloned()
        .collect()
}

fn paginate_assertions(
    assertions: Vec<ManualAssetBalanceAssertionRecord>,
    page: u32,
    page_size: u32,
) -> Result<ManualAssetBalanceAssertionPage, DbError> {
    let total = total_to_u32(assertions.len())?;
    let offset =
        usize::try_from(u64::from(page.saturating_sub(1)).saturating_mul(u64::from(page_size)))
            .map_err(|_| DbError::new("Assertion page offset overflow"))?;
    let page_size_usize =
        usize::try_from(page_size).map_err(|_| DbError::new("Assertion page size overflow"))?;

    let rows = assertions
        .into_iter()
        .skip(offset)
        .take(page_size_usize)
        .collect::<Vec<_>>();
    let (start, end) = page_bounds(page, page_size, total, rows.len())?;

    Ok(ManualAssetBalanceAssertionPage {
        page,
        page_size,
        total,
        start,
        end,
        rows,
    })
}

fn project_assertion_history(
    assertions: &[ManualAssetBalanceAssertionRecord],
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
    sort: TransactionSortDirection,
    page: u32,
    page_size: u32,
) -> Result<ManualAssetBalanceAssertionPage, DbError> {
    let mut filtered = filter_assertions_by_date_range(assertions, start_date, end_date);
    filtered.sort_by(|left, right| {
        left.asserted_on.cmp(&right.asserted_on).then(
            left.assertion_id
                .to_string()
                .cmp(&right.assertion_id.to_string()),
        )
    });
    if sort == TransactionSortDirection::Descending {
        filtered.reverse();
    }
    paginate_assertions(filtered, page, page_size)
}

fn load_manual_asset_account_meta(
    conn: &rusqlite::Connection,
    account_id: WalletAccountId,
) -> Result<ManualAssetAccountMeta, DbError> {
    load_manual_table_account_meta(conn, account_id)?
        .ok_or_else(|| DbError::new("Manual asset account not found"))
}

fn load_manual_table_account_meta(
    conn: &rusqlite::Connection,
    account_id: WalletAccountId,
) -> Result<Option<ManualAssetAccountMeta>, DbError> {
    let row = conn
        .query_row(
            "SELECT a.wallet_id, w.label, a.label, a.asset_id, a.network_id,
                    a.decimal_precision, a.unit_code, a.symbol, a.asset_name,
                    a.network_name, a.coingecko_id
             FROM manual_asset_accounts a
             JOIN wallets w ON w.id = a.wallet_id
             WHERE a.id = ?1
             LIMIT 1",
            params![account_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to load manual asset account meta: {err}")))?;

    let Some((
        wallet_id_raw,
        wallet_label_raw,
        account_label_raw,
        asset_id_raw,
        network_id_raw,
        decimal_precision_raw,
        unit_code_raw,
        symbol,
        asset_name,
        network_name,
        coingecko_id_raw,
    )) = row
    else {
        return Ok(None);
    };

    let wallet_id = WalletId::from_str(&wallet_id_raw)
        .map_err(|err| DbError::new(format!("Invalid manual asset wallet_id in DB: {err}")))?;
    let wallet_label = Label::parse_with_limit(&wallet_label_raw, WALLET_LABEL_MAX_LENGTH)
        .map_err(|err| DbError::new(format!("Invalid manual asset wallet label in DB: {err}")))?;
    let account_label = Label::parse_with_limit(&account_label_raw, ACCOUNT_LABEL_MAX_LENGTH)
        .map_err(|err| DbError::new(format!("Invalid manual asset account label in DB: {err}")))?;
    let snapshot = parse_manual_asset_account_snapshot_for_assertions(
        ManualAssetAccountSnapshotRaw {
            asset_id: asset_id_raw,
            network_id: network_id_raw,
            decimal_precision: decimal_precision_raw,
            unit_code: unit_code_raw,
            symbol,
            asset_name,
            network_name,
            coingecko_id: coingecko_id_raw,
        },
        "manual asset account meta",
    )?;
    snapshot.observe_metadata_fields();

    Ok(Some(ManualAssetAccountMeta {
        wallet_id,
        wallet_label,
        account_label,
        unit_code: snapshot.unit_code,
        decimal_precision: snapshot.decimal_precision,
        symbol: snapshot.symbol,
        asset_name: Some(snapshot.asset_name),
        network_name: Some(snapshot.network_name),
    }))
}

fn load_assertions_for_account(
    conn: &rusqlite::Connection,
    account_id: WalletAccountId,
) -> Result<Vec<ManualAssetBalanceAssertionRecord>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, asserted_on, balance_amount_hi, balance_amount_lo, note
         FROM manual_asset_balance_assertions
         WHERE account_id = ?1
         ORDER BY asserted_on ASC, id ASC",
        )
        .map_err(|err| DbError::new(format!("Failed to prepare assertion query: {err}")))?;

    let rows = stmt
        .query_map(params![account_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|err| DbError::new(format!("Failed to query assertions: {err}")))?;

    let mut assertions = Vec::new();
    for row_result in rows {
        let (assertion_id_raw, asserted_on_raw, balance_hi, balance_lo, note) = row_result
            .map_err(|err| DbError::new(format!("Failed to map assertion row: {err}")))?;
        let assertion_id = ManualAssetBalanceAssertionId::from_str(&assertion_id_raw)
            .map_err(|err| DbError::new(format!("Invalid assertion id in DB: {err}")))?;
        let asserted_on = parse_asserted_on(&asserted_on_raw, "asserted_on")?;
        let balance = parse_split_amount(balance_hi, balance_lo, "assertion balance")?;
        assertions.push(ManualAssetBalanceAssertionRecord {
            assertion_id,
            asserted_on,
            balance,
            note,
        });
    }

    Ok(assertions)
}

fn load_same_unit_precision_read_meta(
    conn: &rusqlite::Connection,
    unit_code: &ValidatedManualAssetUnitCode,
    decimal_precision: ManualAssetDisplayScale,
) -> Result<SameUnitPrecisionReadMeta, DbError> {
    let mut account_ids = HashSet::new();
    let mut has_any_assertions = false;
    let mut matched_decimal_precision = false;

    // Manual asset accounts: snapshot-driven decimal_precision. Any assertion is
    // implicitly stored at the snapshot scale, so treat any non-empty assertion
    // history as evidence of an inferred precision match.
    let manual_account_ids = load_manual_table_account_ids_for_unit_code_conn(conn, unit_code)?;
    for account_id in &manual_account_ids {
        account_ids.insert(account_id.to_string());
        let assertion_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM manual_asset_balance_assertions WHERE account_id = ?1",
                params![account_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to count manual assertions for precision metadata: {err}"
                ))
            })?;
        if assertion_count > 0 {
            has_any_assertions = true;
            matched_decimal_precision = true;
        }
    }

    if account_ids.is_empty() {
        return Err(DbError::new(format!(
            "Missing same-unit manual asset accounts for {}",
            unit_code
        )));
    }

    let precision_status = if !has_any_assertions && decimal_precision.as_u8() == 0 {
        ManualAssetPrecisionStatus::NotInferredYet
    } else if matched_decimal_precision {
        ManualAssetPrecisionStatus::Inferred
    } else {
        ManualAssetPrecisionStatus::LegacyBaseline
    };

    Ok(SameUnitPrecisionReadMeta {
        precision_status,
        precision_shared_with_other_accounts: account_ids.len() > 1,
    })
}

fn load_wallet_report_manual_accounts(
    conn: &rusqlite::Connection,
    wallet_id: WalletId,
) -> Result<Vec<ManualAssetWalletReportMeta>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, label, asset_id, network_id, decimal_precision, unit_code,
                    symbol, asset_name, network_name, coingecko_id, created_at
             FROM manual_asset_accounts
             WHERE wallet_id = ?1
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare manual wallet report accounts query: {err}"
            ))
        })?;

    let rows = stmt
        .query_map(params![wallet_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
            ))
        })
        .map_err(|err| {
            DbError::new(format!(
                "Failed to query manual wallet report accounts: {err}"
            ))
        })?;

    let mut accounts = Vec::new();
    for row_result in rows {
        let (
            account_id_raw,
            account_label_raw,
            asset_id_raw,
            network_id_raw,
            decimal_precision_raw,
            unit_code_raw,
            symbol,
            asset_name,
            network_name,
            coingecko_id_raw,
            created_at_raw,
        ) = row_result.map_err(|err| {
            DbError::new(format!(
                "Failed to map manual wallet report account row: {err}"
            ))
        })?;
        let account_id = WalletAccountId::from_str(&account_id_raw).map_err(|err| {
            DbError::new(format!(
                "Invalid manual wallet report account id in DB: {err}"
            ))
        })?;
        let account_label = Label::parse_with_limit(&account_label_raw, ACCOUNT_LABEL_MAX_LENGTH)
            .map_err(|err| {
            DbError::new(format!(
                "Invalid manual wallet report account label in DB: {err}"
            ))
        })?;
        let snapshot = parse_manual_asset_account_snapshot_for_assertions(
            ManualAssetAccountSnapshotRaw {
                asset_id: asset_id_raw,
                network_id: network_id_raw,
                decimal_precision: decimal_precision_raw,
                unit_code: unit_code_raw,
                symbol,
                asset_name,
                network_name,
                coingecko_id: coingecko_id_raw,
            },
            "manual wallet report account",
        )?;
        snapshot.observe_metadata_fields();
        let created_at_date = crate::models::parse_datetime(&created_at_raw)
            .map_err(|err| {
                DbError::new(format!(
                    "Invalid manual wallet report account created_at in DB: {err}"
                ))
            })?
            .date_naive();

        accounts.push(ManualAssetWalletReportMeta {
            account_id,
            account_label,
            asset_id: snapshot.asset_id,
            unit_code: snapshot.unit_code,
            decimal_precision: snapshot.decimal_precision,
            created_at_date,
        });
    }

    Ok(accounts)
}

fn parse_manual_asset_account_snapshot_for_assertions(
    raw: ManualAssetAccountSnapshotRaw,
    context: &str,
) -> Result<ManualAssetAccountSnapshotForAssertions, DbError> {
    let asset_id = crate::asset_capabilities::AssetId::owned(raw.asset_id)
        .map_err(|err| DbError::new(format!("Invalid {context} asset_id in DB: {err}")))?;
    let network_id = crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(&raw.network_id)
        .map_err(|err| DbError::new(format!("Invalid {context} network_id in DB: {err}")))?;
    let decimal_precision = ManualAssetDisplayScale::try_from(raw.decimal_precision)
        .map_err(|err| DbError::new(format!("Invalid {context} decimal_precision in DB: {err}")))?;
    let unit_code = ValidatedManualAssetUnitCode::parse(&raw.unit_code)
        .map_err(|err| DbError::new(format!("Invalid {context} unit_code in DB: {err}")))?;
    let coingecko_id =
        crate::asset_capabilities::unsynced::CoingeckoAssetId::parse(&raw.coingecko_id)
            .map_err(|err| DbError::new(format!("Invalid {context} coingecko_id in DB: {err}")))?;

    Ok(ManualAssetAccountSnapshotForAssertions {
        asset_id,
        network_id,
        unit_code,
        decimal_precision,
        symbol: raw.symbol,
        asset_name: raw.asset_name,
        network_name: raw.network_name,
        coingecko_id,
    })
}

fn duplicate_asserted_on_violation(error: &rusqlite::Error) -> bool {
    match error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => {
            message.contains(
                "manual_asset_balance_assertions.account_id, manual_asset_balance_assertions.asserted_on",
            )
        }
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManualAssetAssertionWriteContext {
    account_id: WalletAccountId,
    unit_code: ValidatedManualAssetUnitCode,
    current_precision: ManualAssetDisplayScale,
}

fn load_manual_asset_assertion_active_limit(
    user_id: crate::models::UserId,
    now: DateTime<Utc>,
) -> Result<usize, ManualAssetAssertionDbError> {
    let entitlements = crate::payments::entitlements::load_feature_entitlements(user_id, now)?;
    Ok(usize::from(entitlements.sync_account_slots_limit))
}

fn ensure_manual_account_assertions_writable_in_tx(
    tx: &rusqlite::Transaction<'_>,
    account_id: WalletAccountId,
    active_limit: usize,
) -> Result<(), ManualAssetAssertionDbError> {
    let mut stmt = tx
        .prepare(
            "SELECT id, 'native' AS kind, created_at
             FROM digital_asset_accounts
             UNION ALL
             SELECT id, 'manual_asset' AS kind, created_at
             FROM manual_asset_accounts",
        )
        .map_err(|err| DbError::new(format!("Failed to prepare supported account scan: {err}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|err| DbError::new(format!("Failed to query supported accounts: {err}")))?;

    let mut records = Vec::new();
    for row in rows {
        let (account_id_raw, kind_raw, created_at_raw) = row
            .map_err(|err| DbError::new(format!("Failed to read supported account row: {err}")))?;
        let kind = match kind_raw.as_str() {
            "native" => crate::account_limits::SupportedAccountKind::Native,
            "manual_asset" => crate::account_limits::SupportedAccountKind::ManualAsset,
            _ => {
                return Err(ManualAssetAssertionDbError::Database(DbError::new(
                    format!("Invalid supported account kind: {kind_raw}"),
                )));
            }
        };
        let created_at = DateTime::parse_from_rfc3339(&created_at_raw)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|err| {
                DbError::new(format!(
                    "Invalid supported account created_at for assertion write: {err}"
                ))
            })?;
        records.push(crate::account_limits::SupportedAccountLimitRecord {
            account_id: WalletAccountId::from_str(&account_id_raw)
                .map_err(|err| DbError::new(format!("Invalid supported account id: {err}")))?,
            kind,
            created_at,
        });
    }

    let classified = crate::account_limits::classify_supported_accounts(records, active_limit);
    if crate::db::account_limits::account_state_for(&classified, &account_id)
        == crate::account_limits::AccountActivationState::Inactive
    {
        return Err(ManualAssetAssertionDbError::InactiveAccountReadOnly);
    }

    Ok(())
}

fn load_manual_asset_account_unit_code_and_scale_in_tx(
    tx: &rusqlite::Transaction<'_>,
    account_id: WalletAccountId,
) -> Result<Option<(ValidatedManualAssetUnitCode, ManualAssetDisplayScale)>, DbError> {
    let row = tx
        .query_row(
            "SELECT asset_id, network_id, decimal_precision, unit_code,
                    symbol, asset_name, network_name, coingecko_id
             FROM manual_asset_accounts
             WHERE id = ?1
             LIMIT 1",
            params![account_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|err| {
            DbError::new(format!(
                "Failed to load manual asset account registry context: {err}"
            ))
        })?;

    let Some((
        asset_id_raw,
        network_id_raw,
        decimal_precision_raw,
        unit_code_raw,
        symbol,
        asset_name,
        network_name,
        coingecko_id_raw,
    )) = row
    else {
        return Ok(None);
    };

    let snapshot = parse_manual_asset_account_snapshot_for_assertions(
        ManualAssetAccountSnapshotRaw {
            asset_id: asset_id_raw,
            network_id: network_id_raw,
            decimal_precision: decimal_precision_raw,
            unit_code: unit_code_raw,
            symbol,
            asset_name,
            network_name,
            coingecko_id: coingecko_id_raw,
        },
        "manual asset assertion write context",
    )?;
    snapshot.observe_metadata_fields();
    Ok(Some((snapshot.unit_code, snapshot.decimal_precision)))
}

fn load_add_assertion_write_context_in_tx(
    tx: &rusqlite::Transaction<'_>,
    account_id: WalletAccountId,
) -> Result<ManualAssetAssertionWriteContext, ManualAssetAssertionDbError> {
    let (unit_code, current_precision) =
        load_manual_asset_account_unit_code_and_scale_in_tx(tx, account_id)?
            .ok_or(ManualAssetAssertionDbError::AccountNotFound)?;
    Ok(ManualAssetAssertionWriteContext {
        account_id,
        unit_code,
        current_precision,
    })
}

fn load_manual_assertion_account_id_for_write_in_tx(
    tx: &rusqlite::Transaction<'_>,
    assertion_id: ManualAssetBalanceAssertionId,
) -> Result<WalletAccountId, ManualAssetAssertionDbError> {
    let account_id_raw = tx
        .query_row(
            "SELECT account_id
             FROM manual_asset_balance_assertions
             WHERE id = ?1
             LIMIT 1",
            params![assertion_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to load manual assertion target: {err}")))?;

    if let Some(account_id_raw) = account_id_raw {
        return WalletAccountId::from_str(&account_id_raw).map_err(|err| {
            ManualAssetAssertionDbError::Database(DbError::new(format!(
                "Invalid manual assertion account id in DB: {err}"
            )))
        });
    }

    Err(ManualAssetAssertionDbError::AssertionNotFound)
}

fn load_update_assertion_write_context_in_tx(
    tx: &rusqlite::Transaction<'_>,
    assertion_id: ManualAssetBalanceAssertionId,
) -> Result<ManualAssetAssertionWriteContext, ManualAssetAssertionDbError> {
    let account_id = load_manual_assertion_account_id_for_write_in_tx(tx, assertion_id)?;
    let (unit_code, current_precision) =
        load_manual_asset_account_unit_code_and_scale_in_tx(tx, account_id)?
            .ok_or(ManualAssetAssertionDbError::AccountNotFound)?;
    Ok(ManualAssetAssertionWriteContext {
        account_id,
        unit_code,
        current_precision,
    })
}

fn load_manual_table_account_ids_for_unit_code_conn(
    conn: &rusqlite::Connection,
    unit_code: &ValidatedManualAssetUnitCode,
) -> Result<Vec<WalletAccountId>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, asset_id, network_id, decimal_precision, unit_code,
                    symbol, asset_name, network_name, coingecko_id
             FROM manual_asset_accounts",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare manual asset accounts scan for {}: {err}",
                unit_code
            ))
        })?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(|err| {
            DbError::new(format!(
                "Failed to scan manual asset accounts for {}: {err}",
                unit_code
            ))
        })?;

    let mut ids = Vec::new();
    for row_result in rows {
        let (
            id_raw,
            asset_id_raw,
            network_id_raw,
            decimal_precision_raw,
            unit_code_raw,
            symbol,
            asset_name,
            network_name,
            coingecko_id_raw,
        ) = row_result.map_err(|err| {
            DbError::new(format!(
                "Failed to read manual asset account row for {}: {err}",
                unit_code
            ))
        })?;
        let snapshot = parse_manual_asset_account_snapshot_for_assertions(
            ManualAssetAccountSnapshotRaw {
                asset_id: asset_id_raw,
                network_id: network_id_raw,
                decimal_precision: decimal_precision_raw,
                unit_code: unit_code_raw,
                symbol,
                asset_name,
                network_name,
                coingecko_id: coingecko_id_raw,
            },
            "manual asset account scan",
        )?;
        snapshot.observe_metadata_fields();
        if snapshot
            .unit_code
            .as_str()
            .eq_ignore_ascii_case(unit_code.as_str())
        {
            let account_id = WalletAccountId::from_str(&id_raw)
                .map_err(|err| DbError::new(format!("Invalid manual account id in DB: {err}")))?;
            ids.push(account_id);
        }
    }
    Ok(ids)
}

pub(crate) fn load_manual_asset_current_balances(
    user_id: crate::models::UserId,
) -> Result<HashMap<WalletAccountId, ManualAssetBalanceState>, DbError> {
    with_user_db(user_id, |conn| {
        let mut balances = HashMap::new();

        load_current_balances_from(
            conn,
            "manual_asset_accounts",
            "manual_asset_balance_assertions",
            "manual",
            &mut balances,
        )?;
        Ok(balances)
    })
}

fn load_current_balances_from(
    conn: &rusqlite::Connection,
    accounts_table: &str,
    assertions_table: &str,
    label: &str,
    out: &mut HashMap<WalletAccountId, ManualAssetBalanceState>,
) -> Result<(), DbError> {
    let sql = format!(
        "WITH latest_assertions AS (
            SELECT account_id, MAX(asserted_on) AS asserted_on
            FROM {assertions_table}
            GROUP BY account_id
         )
         SELECT a.id,
                latest.asserted_on,
                balance.balance_amount_hi,
                balance.balance_amount_lo
         FROM {accounts_table} a
         LEFT JOIN latest_assertions latest
           ON latest.account_id = a.id
         LEFT JOIN {assertions_table} balance
           ON balance.account_id = latest.account_id
          AND balance.asserted_on = latest.asserted_on"
    );
    let mut stmt = conn.prepare(&sql).map_err(|err| {
        DbError::new(format!(
            "Failed to prepare {label} asset current balance query: {err}"
        ))
    })?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(|err| {
            DbError::new(format!(
                "Failed to query {label} asset current balances: {err}"
            ))
        })?;

    for row_result in rows {
        let (account_id_raw, asserted_on_raw, balance_hi, balance_lo) =
            row_result.map_err(|err| {
                DbError::new(format!(
                    "Failed to map {label} asset current balance row: {err}"
                ))
            })?;
        let account_id = WalletAccountId::from_str(&account_id_raw).map_err(|err| {
            DbError::new(format!("Invalid {label} asset account id in DB: {err}"))
        })?;

        let balance_state = match asserted_on_raw {
            Some(_) => {
                let hi = balance_hi.ok_or_else(|| {
                    DbError::new(format!(
                        "{label} asset current balance missing hi split amount"
                    ))
                })?;
                let lo = balance_lo.ok_or_else(|| {
                    DbError::new(format!(
                        "{label} asset current balance missing lo split amount"
                    ))
                })?;
                ManualAssetBalanceState::Known(parse_split_amount(
                    hi,
                    lo,
                    "manual asset current balance",
                )?)
            }
            None => ManualAssetBalanceState::Unknown,
        };

        out.insert(account_id, balance_state);
    }

    Ok(())
}

pub(crate) fn add_manual_asset_balance_assertion(
    user_id: crate::models::UserId,
    request: ValidatedAddManualAssetBalanceAssertionRequest,
    now: DateTime<Utc>,
) -> Result<ManualAssetBalanceAssertionId, ManualAssetAssertionDbError> {
    let active_limit = load_manual_asset_assertion_active_limit(user_id, now)?;
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::new(format!("Failed to start manual assertion insert: {err}"))
        })?;
        let context = load_add_assertion_write_context_in_tx(&tx, request.account_id)?;
        ensure_manual_account_assertions_writable_in_tx(&tx, context.account_id, active_limit)?;

        let assertion_id = ManualAssetBalanceAssertionId::new();
        let balance = request
            .balance
            .parse_at_scale(context.current_precision)
            .map_err(|err| {
                ManualAssetAssertionDbError::Database(DbError::new(format!(
                    "Failed to normalize manual assertion balance: {err}"
                )))
            })?;
        let balance_parts = split_unsigned_amount(balance.amount(), "manual assertion balance")?;

        let note = request
            .note
            .as_ref()
            .map(ValidatedManualAssetAssertionNote::as_str);
        let inserted = tx.execute(
            "INSERT INTO manual_asset_balance_assertions
             (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo, note, entered_balance_text, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                assertion_id.to_string(),
                request.account_id.to_string(),
                request.asserted_on.format("%Y-%m-%d").to_string(),
                balance_parts.hi,
                balance_parts.lo,
                note,
                request.balance.trimmed(),
                now.to_rfc3339(),
                now.to_rfc3339(),
            ],
        );

        match inserted {
            Ok(_) => {}
            Err(err) if duplicate_asserted_on_violation(&err) => {
                return Err(ManualAssetAssertionDbError::DuplicateAssertionDate);
            }
            Err(err) => {
                return Err(ManualAssetAssertionDbError::Database(DbError::new(
                    format!("Failed to insert manual assertion: {err}"),
                )));
            }
        }

        tx.commit().map_err(|err| {
            ManualAssetAssertionDbError::Database(DbError::new(format!(
                "Failed to commit manual assertion insert: {err}"
            )))
        })?;
        Ok(assertion_id)
    })
}

pub(crate) fn update_manual_asset_balance_assertion(
    user_id: crate::models::UserId,
    request: ValidatedUpdateManualAssetBalanceAssertionRequest,
    now: DateTime<Utc>,
) -> Result<(), ManualAssetAssertionDbError> {
    let active_limit = load_manual_asset_assertion_active_limit(user_id, now)?;
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::new(format!("Failed to start manual assertion update: {err}"))
        })?;
        let context = load_update_assertion_write_context_in_tx(&tx, request.assertion_id)?;
        ensure_manual_account_assertions_writable_in_tx(&tx, context.account_id, active_limit)?;

        let balance = request
            .balance
            .parse_at_scale(context.current_precision)
            .map_err(|err| {
                ManualAssetAssertionDbError::Database(DbError::new(format!(
                    "Failed to normalize manual assertion balance: {err}"
                )))
            })?;
        let balance_parts = split_unsigned_amount(balance.amount(), "manual assertion balance")?;
        let note = request
            .note
            .as_ref()
            .map(ValidatedManualAssetAssertionNote::as_str);

        let updated = match tx.execute(
            "UPDATE manual_asset_balance_assertions
             SET asserted_on = ?1,
                 balance_amount_hi = ?2,
                 balance_amount_lo = ?3,
                 note = ?4,
                 entered_balance_text = ?5,
                 updated_at = ?6
             WHERE id = ?7",
            params![
                request.asserted_on.format("%Y-%m-%d").to_string(),
                balance_parts.hi,
                balance_parts.lo,
                note,
                request.balance.trimmed(),
                now.to_rfc3339(),
                request.assertion_id.to_string(),
            ],
        ) {
            Ok(updated) => updated,
            Err(err) if duplicate_asserted_on_violation(&err) => {
                return Err(ManualAssetAssertionDbError::DuplicateAssertionDate);
            }
            Err(err) => {
                return Err(ManualAssetAssertionDbError::Database(DbError::new(
                    format!("Failed to update manual assertion: {err}"),
                )));
            }
        };

        if updated == 0 {
            return Err(ManualAssetAssertionDbError::AssertionNotFound);
        }

        tx.commit().map_err(|err| {
            ManualAssetAssertionDbError::Database(DbError::new(format!(
                "Failed to commit manual assertion update: {err}"
            )))
        })?;
        Ok(())
    })
}

pub(crate) fn delete_manual_asset_balance_assertion(
    user_id: crate::models::UserId,
    assertion_id: ManualAssetBalanceAssertionId,
) -> Result<(), ManualAssetAssertionDbError> {
    let active_limit = load_manual_asset_assertion_active_limit(user_id, Utc::now())?;
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::new(format!("Failed to start manual assertion delete: {err}"))
        })?;
        let account_id = load_manual_assertion_account_id_for_write_in_tx(&tx, assertion_id)?;
        ensure_manual_account_assertions_writable_in_tx(&tx, account_id, active_limit)?;

        let deleted = tx
            .execute(
                "DELETE FROM manual_asset_balance_assertions WHERE id = ?1",
                params![assertion_id.to_string()],
            )
            .map_err(|err| DbError::new(format!("Failed to delete manual assertion: {err}")))?;

        if deleted == 0 {
            return Err(ManualAssetAssertionDbError::AssertionNotFound);
        }

        tx.commit().map_err(|err| {
            ManualAssetAssertionDbError::Database(DbError::new(format!(
                "Failed to commit manual assertion delete: {err}"
            )))
        })?;
        Ok(())
    })
}

pub(crate) fn load_manual_asset_account_history(
    user_id: crate::models::UserId,
    account_id: WalletAccountId,
    page: u32,
    page_size: u32,
    sort: TransactionSortDirection,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
) -> Result<ManualAssetAccountHistoryData, ManualAssetAssertionDbError> {
    with_user_db(user_id, |conn| {
        let meta = load_manual_asset_account_meta(conn, account_id).map_err(|err| {
            if err.to_string() == "Manual asset account not found" {
                ManualAssetAssertionDbError::AccountNotFound
            } else {
                ManualAssetAssertionDbError::Database(err)
            }
        })?;
        let precision_meta =
            load_same_unit_precision_read_meta(conn, &meta.unit_code, meta.decimal_precision)?;
        let assertions = load_assertions_for_account(conn, account_id)?;
        let (opening_balance_state, opening_balance_date) =
            resolve_opening_balance_state(&assertions, start_date);
        let (closing_balance_state, closing_balance_date) =
            resolve_closing_balance_state(&assertions, end_date);
        let assertions =
            project_assertion_history(&assertions, start_date, end_date, sort, page, page_size)?;

        Ok(ManualAssetAccountHistoryData {
            account_id,
            wallet_id: meta.wallet_id,
            wallet_label: meta.wallet_label,
            account_label: meta.account_label,
            unit_code: meta.unit_code,
            decimal_precision: meta.decimal_precision,
            symbol: meta.symbol,
            asset_name: meta.asset_name,
            network_name: meta.network_name,
            precision_status: precision_meta.precision_status,
            precision_shared_with_other_accounts: precision_meta
                .precision_shared_with_other_accounts,
            opening_balance_state,
            opening_balance_date,
            closing_balance_state,
            closing_balance_date,
            assertions,
        })
    })
}

pub(crate) fn load_manual_asset_wallet_report_rows(
    user_id: crate::models::UserId,
    wallet_id: WalletId,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> Result<Vec<ManualAssetWalletReportRowData>, DbError> {
    with_user_db(user_id, |conn| {
        let mut rows = Vec::new();

        for account in load_wallet_report_manual_accounts(conn, wallet_id)? {
            let assertions = load_assertions_for_account(conn, account.account_id)?;
            let (opening_balance_state, opening_balance_date) = resolve_wallet_report_balance_state(
                &assertions,
                start_date,
                account.created_at_date,
            );
            let (closing_balance_state, closing_balance_date) =
                resolve_wallet_report_balance_state(&assertions, end_date, account.created_at_date);

            rows.push(ManualAssetWalletReportRowData {
                account_id: account.account_id,
                account_label: account.account_label,
                asset_id: account.asset_id,
                unit_code: account.unit_code,
                decimal_precision: account.decimal_precision,
                opening_balance_state,
                opening_balance_date,
                closing_balance_state,
                closing_balance_date,
            });
        }

        Ok(rows)
    })
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod pure_tests {
    use super::*;
    use crate::amounts::UnsignedAmount;

    fn amount(value: u128) -> UnsignedAmount {
        UnsignedAmount::from_u128(value)
    }

    fn assertion(
        id: u128,
        year: i32,
        month: u32,
        day: u32,
        balance: u128,
    ) -> ManualAssetBalanceAssertionRecord {
        ManualAssetBalanceAssertionRecord {
            assertion_id: ManualAssetBalanceAssertionId::from_str(&format!("{id:026}"))
                .unwrap_or_else(|_| ManualAssetBalanceAssertionId::new()),
            asserted_on: NaiveDate::from_ymd_opt(year, month, day).expect("valid date"),
            balance: amount(balance),
            note: None,
        }
    }

    #[test]
    fn wallet_report_backdated_assertion_carries_forward_across_years() {
        // Account row created in 2026, but a balance assertion is back-dated to
        // 2025-12-31. The 2026 opening boundary (2026-01-01) is before the
        // account creation date yet on/after the assertion, so the asserted
        // balance must carry forward rather than being floored to zero.
        let assertions = vec![assertion(1, 2025, 12, 31, 100)];
        let created_at = NaiveDate::from_ymd_opt(2026, 6, 16).expect("valid date");

        let (opening_state, opening_date) = resolve_wallet_report_balance_state(
            &assertions,
            NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"),
            created_at,
        );

        assert_eq!(opening_state, ManualAssetBalanceState::Known(amount(100)));
        assert_eq!(
            opening_date,
            Some(NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"))
        );
    }

    #[test]
    fn wallet_report_boundary_before_manual_account_creation_is_zero() {
        let assertions = vec![assertion(1, 2026, 6, 13, 123)];
        let created_at = NaiveDate::from_ymd_opt(2026, 6, 13).expect("valid date");

        let (opening_state, opening_date) = resolve_wallet_report_balance_state(
            &assertions,
            NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"),
            created_at,
        );
        let (closing_state, closing_date) = resolve_wallet_report_balance_state(
            &assertions,
            NaiveDate::from_ymd_opt(2026, 6, 16).expect("valid date"),
            created_at,
        );

        assert_eq!(
            opening_state,
            ManualAssetBalanceState::Known(UnsignedAmount::zero())
        );
        assert_eq!(
            opening_date,
            Some(NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"))
        );
        assert_eq!(closing_state, ManualAssetBalanceState::Known(amount(123)));
        assert_eq!(
            closing_date,
            Some(NaiveDate::from_ymd_opt(2026, 6, 16).expect("valid date"))
        );
    }

    #[test]
    fn resolve_opening_balance_state_uses_latest_assertion_on_or_before_start_date() {
        let assertions = vec![
            assertion(1, 2026, 1, 2, 100),
            assertion(2, 2026, 1, 10, 250),
            assertion(3, 2026, 1, 20, 50),
        ];

        let (state, date) = resolve_opening_balance_state(
            &assertions,
            Some(NaiveDate::from_ymd_opt(2026, 1, 15).expect("valid date")),
        );

        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 1, 15));
        assert_eq!(state, ManualAssetBalanceState::Known(amount(250)));
    }

    #[test]
    fn resolve_opening_balance_state_includes_assertion_on_start_date() {
        let assertions = vec![
            assertion(1, 2026, 1, 2, 100),
            assertion(2, 2026, 1, 10, 250),
        ];

        let (state, date) = resolve_opening_balance_state(
            &assertions,
            Some(NaiveDate::from_ymd_opt(2026, 1, 10).expect("valid date")),
        );

        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 1, 10));
        assert_eq!(state, ManualAssetBalanceState::Known(amount(250)));
    }

    #[test]
    fn resolve_opening_balance_state_uses_first_assertion_when_no_start_date() {
        let assertions = vec![assertion(1, 2026, 1, 2, 100)];

        let (state, date) = resolve_opening_balance_state(&assertions, None);

        assert_eq!(state, ManualAssetBalanceState::Known(amount(100)));
        assert_eq!(
            date,
            Some(NaiveDate::from_ymd_opt(2026, 1, 2).expect("valid date"))
        );
    }

    #[test]
    fn resolve_opening_balance_state_is_unknown_when_no_assertions() {
        let assertions: Vec<ManualAssetBalanceAssertionRecord> = vec![];

        let (state, date) = resolve_opening_balance_state(&assertions, None);

        assert_eq!(state, ManualAssetBalanceState::Unknown);
        assert_eq!(date, None);
    }

    #[test]
    fn resolve_opening_balance_state_is_unknown_before_first_assertion() {
        let assertions = vec![assertion(1, 2026, 1, 10, 100)];

        let (state, date) = resolve_opening_balance_state(
            &assertions,
            Some(NaiveDate::from_ymd_opt(2026, 1, 5).expect("valid date")),
        );

        assert_eq!(state, ManualAssetBalanceState::Unknown);
        assert_eq!(
            date,
            Some(NaiveDate::from_ymd_opt(2026, 1, 5).expect("valid date"))
        );
    }

    #[test]
    fn resolve_closing_balance_state_keeps_known_zero() {
        let assertions = vec![assertion(1, 2026, 1, 2, 100), assertion(2, 2026, 1, 10, 0)];

        let (state, date) = resolve_closing_balance_state(
            &assertions,
            Some(NaiveDate::from_ymd_opt(2026, 1, 10).expect("valid date")),
        );

        assert_eq!(
            date,
            Some(NaiveDate::from_ymd_opt(2026, 1, 10).expect("valid date"))
        );
        assert_eq!(
            state,
            ManualAssetBalanceState::Known(UnsignedAmount::zero())
        );
    }

    #[test]
    fn project_assertion_history_filters_and_sorts_descending() {
        let assertions = vec![
            assertion(1, 2026, 1, 2, 100),
            assertion(2, 2026, 1, 10, 250),
            assertion(3, 2026, 1, 20, 50),
        ];

        let page = project_assertion_history(
            &assertions,
            Some(NaiveDate::from_ymd_opt(2026, 1, 5).expect("valid date")),
            Some(NaiveDate::from_ymd_opt(2026, 1, 31).expect("valid date")),
            TransactionSortDirection::Descending,
            1,
            50,
        )
        .expect("projection should succeed");

        assert_eq!(page.total, 2);
        assert_eq!(
            page.rows[0].asserted_on,
            NaiveDate::from_ymd_opt(2026, 1, 20).expect("valid date")
        );
        assert_eq!(
            page.rows[1].asserted_on,
            NaiveDate::from_ymd_opt(2026, 1, 10).expect("valid date")
        );
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod inactive_account_tests {
    use super::*;
    use crate::db::user_db::with_user_db_mut;
    use crate::wallets::{
        AddManualAssetBalanceAssertionRequest, IdentitySource, RawManualAssetBalance,
        ReportDateParam, UpdateManualAssetBalanceAssertionRequest,
    };
    use chrono::TimeZone;

    struct InactiveManualAccountFixture {
        user_id: crate::models::UserId,
        inactive_account_id: WalletAccountId,
        assertion_id: ManualAssetBalanceAssertionId,
    }

    fn timestamp(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 18, 12, 0, second)
            .single()
            .expect("valid timestamp")
    }

    fn timestamp_after_start(seconds: u32) -> DateTime<Utc> {
        timestamp(0) + chrono::Duration::seconds(i64::from(seconds))
    }

    fn assertion_date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 6, day).expect("valid date")
    }

    fn validated_add_request(
        account_id: WalletAccountId,
        asserted_on: NaiveDate,
        balance: &str,
    ) -> ValidatedAddManualAssetBalanceAssertionRequest {
        AddManualAssetBalanceAssertionRequest {
            account_id,
            asserted_on: ReportDateParam::from_naive_date(asserted_on),
            balance: RawManualAssetBalance::new(balance.to_string()),
            note: None,
        }
        .try_into_validated_at(assertion_date(30), 6)
        .expect("valid add assertion request")
    }

    fn validated_update_request(
        assertion_id: ManualAssetBalanceAssertionId,
        account_id: WalletAccountId,
        asserted_on: NaiveDate,
        balance: &str,
    ) -> ValidatedUpdateManualAssetBalanceAssertionRequest {
        UpdateManualAssetBalanceAssertionRequest {
            assertion_id,
            account_id,
            asserted_on: ReportDateParam::from_naive_date(asserted_on),
            balance: RawManualAssetBalance::new(balance.to_string()),
            note: None,
        }
        .try_into_validated_at(assertion_date(30), 6)
        .expect("valid update assertion request")
    }

    fn insert_wallet(user_id: crate::models::UserId, wallet_id: WalletId, now: DateTime<Utc>) {
        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO wallets
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at)
                 VALUES (?1, 'Inactive Manual Wallet', 'inactive manual wallet', NULL, ?2, NULL, ?3, ?3)",
                params![
                    wallet_id.to_string(),
                    IdentitySource::UserProvided.as_str(),
                    now.to_rfc3339(),
                ],
            )
            .map_err(|err| DbError::new(format!("wallet insert failed: {err}")))?;
            Ok(())
        })
        .expect("wallet fixture inserts");
    }

    fn insert_manual_account(
        user_id: crate::models::UserId,
        wallet_id: WalletId,
        account_id: WalletAccountId,
        label: &str,
        created_at: DateTime<Utc>,
    ) {
        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO manual_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, coingecko_platform_id, provider_platform_asset_ref,
                  created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'algorand', 'algorand-mainnet', 6,
                         'ALGO', NULL, 'Algorand', 'Algorand', 'algorand',
                         'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, ?5, ?5)",
                params![
                    account_id.to_string(),
                    wallet_id.to_string(),
                    label,
                    label.to_ascii_lowercase(),
                    created_at.to_rfc3339(),
                ],
            )
            .map_err(|err| DbError::new(format!("manual account insert failed: {err}")))?;
            Ok(())
        })
        .expect("manual account fixture inserts");
    }

    fn insert_assertion(
        user_id: crate::models::UserId,
        account_id: WalletAccountId,
        assertion_id: ManualAssetBalanceAssertionId,
        asserted_on: NaiveDate,
        balance_lo: i64,
        now: DateTime<Utc>,
    ) {
        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO manual_asset_balance_assertions
                 (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo,
                  entered_balance_text, note, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, NULL, ?6, ?6)",
                params![
                    assertion_id.to_string(),
                    account_id.to_string(),
                    asserted_on.format("%Y-%m-%d").to_string(),
                    balance_lo,
                    balance_lo.to_string(),
                    now.to_rfc3339(),
                ],
            )
            .map_err(|err| DbError::new(format!("assertion insert failed: {err}")))?;
            Ok(())
        })
        .expect("assertion fixture inserts");
    }

    fn inactive_manual_account_fixture() -> InactiveManualAccountFixture {
        let user_id = crate::models::UserId::new();
        super::super::app_db::enable_test_mode();
        super::super::app_db::reset_test_db();
        super::super::user_db::enable_test_mode();
        let sqlcipher_compatibility = super::super::encryption::current_sqlcipher_compatibility()
            .expect("SQLCipher compatibility should probe");
        super::super::user_db::initialize_user_db(
            user_id,
            super::super::encryption::UserDbOpenMode::Encrypted {
                dek: super::super::encryption::Dek::generate(),
                authority: super::super::encryption::UnlockAuthority::PasswordLogin,
                sqlcipher_compatibility,
            },
        )
        .expect("user db should initialize");
        let wallet_id = WalletId::new();
        insert_wallet(user_id, wallet_id, timestamp(0));

        let free_account_limit = crate::payments::free_tier::baked_free_tier_snapshot()
            .capabilities
            .limits
            .accounts
            .total;
        for index in 0..free_account_limit {
            insert_manual_account(
                user_id,
                wallet_id,
                WalletAccountId::new(),
                &format!("Active ALGO {index}"),
                timestamp_after_start(u32::from(index) + 1),
            );
        }

        let inactive_account_id = WalletAccountId::new();
        insert_manual_account(
            user_id,
            wallet_id,
            inactive_account_id,
            "Inactive ALGO",
            timestamp_after_start(u32::from(free_account_limit) + 1),
        );
        let assertion_id = ManualAssetBalanceAssertionId::new();
        insert_assertion(
            user_id,
            inactive_account_id,
            assertion_id,
            assertion_date(18),
            123_000_000,
            timestamp_after_start(u32::from(free_account_limit) + 2),
        );

        InactiveManualAccountFixture {
            user_id,
            inactive_account_id,
            assertion_id,
        }
    }

    #[test]
    fn manual_asset_assertion_inactive_account_can_be_read() {
        let fixture = inactive_manual_account_fixture();

        let history = load_manual_asset_account_history(
            fixture.user_id,
            fixture.inactive_account_id,
            1,
            50,
            TransactionSortDirection::Descending,
            None,
            None,
        )
        .expect("inactive manual account history should load");

        assert_eq!(history.account_id, fixture.inactive_account_id);
        assert_eq!(history.assertions.total, 1);
        assert_eq!(
            history.assertions.rows[0].assertion_id,
            fixture.assertion_id
        );
    }

    #[test]
    fn manual_asset_assertion_inactive_account_add_is_rejected() {
        let fixture = inactive_manual_account_fixture();
        let request = validated_add_request(fixture.inactive_account_id, assertion_date(19), "456");

        let result = add_manual_asset_balance_assertion(fixture.user_id, request, timestamp(12));

        assert!(matches!(
            result,
            Err(ManualAssetAssertionDbError::InactiveAccountReadOnly)
        ));
    }

    #[test]
    fn manual_asset_assertion_inactive_account_update_is_rejected() {
        let fixture = inactive_manual_account_fixture();
        let request = validated_update_request(
            fixture.assertion_id,
            fixture.inactive_account_id,
            assertion_date(19),
            "456",
        );

        let result = update_manual_asset_balance_assertion(fixture.user_id, request, timestamp(12));

        assert!(matches!(
            result,
            Err(ManualAssetAssertionDbError::InactiveAccountReadOnly)
        ));
    }

    #[test]
    fn manual_asset_assertion_inactive_account_delete_is_rejected() {
        let fixture = inactive_manual_account_fixture();

        let result = delete_manual_asset_balance_assertion(fixture.user_id, fixture.assertion_id);

        assert!(matches!(
            result,
            Err(ManualAssetAssertionDbError::InactiveAccountReadOnly)
        ));
    }

    #[test]
    fn manual_asset_assertion_inactive_account_itself_can_be_deleted() {
        let fixture = inactive_manual_account_fixture();

        crate::db::wallets::delete_account(fixture.user_id, fixture.inactive_account_id)
            .expect("inactive manual account delete should be allowed");

        let history = load_manual_asset_account_history(
            fixture.user_id,
            fixture.inactive_account_id,
            1,
            50,
            TransactionSortDirection::Descending,
            None,
            None,
        );
        assert!(matches!(
            history,
            Err(ManualAssetAssertionDbError::AccountNotFound)
        ));
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::amounts::UnsignedAmount;
    use crate::db::test_fixtures::{setup_test_user, unique_user_id};
    use crate::db::user_db::with_user_db_mut;
    use crate::wallets::IdentitySource;

    #[cfg(feature = "db-tests")]
    #[test]
    fn manual_assertion_loaders_use_manual_asset_account_snapshot_columns() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let wallet_id = WalletId::new();
        let account_id = WalletAccountId::new();
        let assertion_id = ManualAssetBalanceAssertionId::new();
        let now = Utc::now().to_rfc3339();
        let asserted_on = NaiveDate::from_ymd_opt(2026, 2, 5).expect("valid date");

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO wallets
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at)
                 VALUES (?1, 'Manual Snapshot Wallet', 'manual snapshot wallet', NULL, ?2, NULL, ?3, ?3)",
                params![wallet_id.to_string(), IdentitySource::UserProvided.as_str(), &now],
            )
            .map_err(|err| DbError::new(format!("wallet insert failed: {err}")))?;
            conn.execute(
                "INSERT INTO manual_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, coingecko_platform_id, provider_platform_asset_ref,
                  created_at, updated_at)
                 VALUES (?1, ?2, 'ADA Account 1', 'ada account 1', 'cardano', 'cardano-mainnet',
                         6, 'ADA', NULL, 'Cardano', 'Cardano', 'cardano',
                         'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, ?3, ?3)",
                params![account_id.to_string(), wallet_id.to_string(), &now],
            )
            .map_err(|err| DbError::new(format!("manual account insert failed: {err}")))?;
            conn.execute(
                "INSERT INTO manual_asset_balance_assertions
                 (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo,
                  entered_balance_text, note, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 0, 1234000, '1.234', NULL, ?4, ?4)",
                params![
                    assertion_id.to_string(),
                    account_id.to_string(),
                    asserted_on.format("%Y-%m-%d").to_string(),
                    &now,
                ],
            )
            .map_err(|err| DbError::new(format!("assertion insert failed: {err}")))?;
            Ok(())
        })
        .expect("snapshot-only fixture inserts");

        let history = load_manual_asset_account_history(
            user_id,
            account_id,
            1,
            50,
            TransactionSortDirection::Descending,
            None,
            None,
        )
        .expect("history loads from snapshot columns");
        assert_eq!(history.unit_code.as_str(), "ADA");
        assert_eq!(history.decimal_precision.as_u8(), 6);
        assert_eq!(history.assertions.total, 1);

        let balances =
            load_manual_asset_current_balances(user_id).expect("current balances load snapshots");
        assert_eq!(
            balances.get(&account_id),
            Some(&ManualAssetBalanceState::Known(UnsignedAmount::from_u128(
                1_234_000
            )))
        );

        let rows = load_manual_asset_wallet_report_rows(
            user_id,
            wallet_id,
            NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"),
            NaiveDate::from_ymd_opt(2026, 12, 31).expect("valid date"),
        )
        .expect("wallet report loads from snapshot columns");
        let row = rows
            .iter()
            .find(|row| row.account_id == account_id)
            .expect("manual account report row");
        assert_eq!(row.asset_id.as_str(), "cardano");
        assert_eq!(row.unit_code.as_str(), "ADA");
        assert_eq!(row.decimal_precision.as_u8(), 6);
    }
}
