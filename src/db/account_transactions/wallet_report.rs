use super::balance::{
    BalanceResolutionInputs, NativeBalanceBoundaryRequest, load_first_transaction_date,
    resolve_balance_dates, resolve_native_balance_at_boundary,
};
use super::types::*;
use crate::amounts::UnsignedAmount;
use crate::balance_reliability::BalanceReliability;
use crate::db::balance_reliability::{
    load_account_balance_reliability_context, load_account_balance_reliability_context_for_history,
};
use crate::db::error::DbError;
use crate::db::user_db::with_user_db;
use crate::models::{UserId, UserTimezone};
use crate::report_dates::{
    DateBoundaryKind, LocalReportDateRange, LocalReportDateRangeError,
    local_report_date_to_utc_boundary,
};
use crate::wallets::{DigitalAssetAccountId, WalletId};
use chrono::{Datelike, NaiveDate, Utc};
use rusqlite::{OptionalExtension, params};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalletReportAccountRow {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) account_label: String,
    pub(crate) asset_id: crate::wallets::SyncedAssetId,
    pub(crate) bitcoin_history_coverage:
        Option<crate::db::transaction_sync::BitcoinAccountHistoryCoverage>,
    pub(crate) opening_balance_state: WalletReportBalanceState,
    pub(crate) opening_balance: Option<UnsignedAmount>,
    pub(crate) opening_balance_reliability: BalanceReliability,
    pub(crate) opening_balance_date: Option<NaiveDate>,
    pub(crate) closing_balance_state: WalletReportBalanceState,
    pub(crate) closing_balance: Option<UnsignedAmount>,
    pub(crate) closing_balance_reliability: BalanceReliability,
    pub(crate) closing_balance_date: Option<NaiveDate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalletReportBalanceState {
    CanonicalZero,
    KnownAmount(UnsignedAmount),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalletReportData {
    pub(crate) wallet_label: String,
    pub(crate) resolved_from: NaiveDate,
    pub(crate) resolved_to: NaiveDate,
    pub(crate) default_this_year_from: NaiveDate,
    pub(crate) default_this_year_to: NaiveDate,
    pub(crate) accounts: Vec<WalletReportAccountRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoldingsReportWalletData {
    pub(crate) wallet_id: WalletId,
    pub(crate) wallet_label: String,
    pub(crate) accounts: Vec<WalletReportAccountRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoldingsReportData {
    pub(crate) resolved_from: NaiveDate,
    pub(crate) resolved_to: NaiveDate,
    pub(crate) default_this_year_from: NaiveDate,
    pub(crate) default_this_year_to: NaiveDate,
    pub(crate) wallets: Vec<HoldingsReportWalletData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WalletReportRangePlan {
    pub(crate) requested_range: LocalReportDateRange,
    pub(crate) default_range: LocalReportDateRange,
}

#[derive(Debug)]
pub(crate) enum WalletReportLoadError {
    WalletNotFound,
    InvalidDateRange(LocalReportDateRangeError),
    Database(DbError),
}

impl std::fmt::Display for WalletReportLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WalletNotFound => write!(f, "Wallet not found"),
            Self::InvalidDateRange(err) => write!(f, "{err}"),
            Self::Database(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for WalletReportLoadError {}

impl From<DbError> for WalletReportLoadError {
    fn from(value: DbError) -> Self {
        Self::Database(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WalletReportAccountMeta {
    pub(super) account_id: DigitalAssetAccountId,
    pub(super) account_label: String,
    pub(super) asset_id: crate::wallets::SyncedAssetId,
    pub(super) network: crate::wallets::Network,
}

pub(super) fn local_date_in_timezone(
    timestamp: chrono::DateTime<Utc>,
    timezone: UserTimezone,
) -> NaiveDate {
    timestamp.with_timezone(&timezone.0).date_naive()
}

pub(super) fn wallet_consistent_sync_date(
    sync_dates: &[Option<chrono::DateTime<Utc>>],
    timezone: UserTimezone,
) -> Option<NaiveDate> {
    if sync_dates.iter().any(Option::is_none) {
        return None;
    }

    sync_dates
        .iter()
        .flatten()
        .map(|timestamp| local_date_in_timezone(*timestamp, timezone))
        .min()
}

pub(super) fn resolve_wallet_report_default_range(
    today_local: NaiveDate,
    wallet_consistent_sync_date: Option<NaiveDate>,
) -> Result<LocalReportDateRange, WalletReportLoadError> {
    let from = NaiveDate::from_ymd_opt(today_local.year(), 1, 1).ok_or_else(|| {
        WalletReportLoadError::Database(DbError::new(
            "Failed to resolve wallet report default start date",
        ))
    })?;
    let to = wallet_consistent_sync_date.unwrap_or(today_local).max(from);
    LocalReportDateRange::new(from, to).map_err(WalletReportLoadError::InvalidDateRange)
}

pub(super) fn resolve_wallet_report_range(
    defaults: LocalReportDateRange,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Result<LocalReportDateRange, WalletReportLoadError> {
    LocalReportDateRange::new(from.unwrap_or(defaults.from()), to.unwrap_or(defaults.to()))
        .map_err(WalletReportLoadError::InvalidDateRange)
}

fn current_year_to_date_range(
    today: NaiveDate,
) -> Result<LocalReportDateRange, WalletReportLoadError> {
    let first_day = NaiveDate::from_ymd_opt(today.year(), 1, 1).ok_or(
        WalletReportLoadError::InvalidDateRange(LocalReportDateRangeError::InvertedRange),
    )?;
    LocalReportDateRange::new(first_day, today).map_err(WalletReportLoadError::InvalidDateRange)
}

pub(super) fn resolve_wallet_report_range_plan(
    today_local: NaiveDate,
    sync_dates: &[Option<chrono::DateTime<Utc>>],
    timezone: UserTimezone,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Result<WalletReportRangePlan, WalletReportLoadError> {
    let default_range = resolve_wallet_report_default_range(
        today_local,
        wallet_consistent_sync_date(sync_dates, timezone),
    )?;
    let requested_range = resolve_wallet_report_range(default_range, from, to)?;

    Ok(WalletReportRangePlan {
        requested_range,
        default_range,
    })
}

pub(super) fn resolve_holdings_report_default_range(
    today_local: NaiveDate,
    sync_dates: &[Option<chrono::DateTime<Utc>>],
    timezone: UserTimezone,
) -> Result<LocalReportDateRange, WalletReportLoadError> {
    if sync_dates.is_empty() {
        return current_year_to_date_range(today_local);
    }

    resolve_wallet_report_default_range(
        today_local,
        wallet_consistent_sync_date(sync_dates, timezone),
    )
}

fn resolve_holdings_report_range_plan(
    today_local: NaiveDate,
    sync_dates: &[Option<chrono::DateTime<Utc>>],
    timezone: UserTimezone,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Result<WalletReportRangePlan, WalletReportLoadError> {
    let default_range = resolve_holdings_report_default_range(today_local, sync_dates, timezone)?;
    let requested_range = resolve_wallet_report_range(default_range, from, to)?;

    Ok(WalletReportRangePlan {
        requested_range,
        default_range,
    })
}

pub(super) fn load_wallet_label(
    conn: &rusqlite::Connection,
    wallet_id: WalletId,
) -> Result<String, WalletReportLoadError> {
    let label = conn
        .query_row(
            "SELECT label
             FROM wallets
             WHERE id = ?1
             LIMIT 1",
            params![wallet_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to load wallet label: {err}")))?;

    label.ok_or(WalletReportLoadError::WalletNotFound)
}

pub(super) fn load_wallet_report_account_meta(
    conn: &rusqlite::Connection,
    wallet_id: WalletId,
) -> Result<Vec<WalletReportAccountMeta>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, label, asset_id, network
             FROM digital_asset_accounts
             WHERE wallet_id = ?1
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare wallet report accounts query: {err}"
            ))
        })?;

    let rows = stmt
        .query_map(params![wallet_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|err| DbError::new(format!("Failed to query wallet report accounts: {err}")))?;

    let mut accounts = Vec::new();
    for row in rows {
        let (account_id_raw, account_label, asset_id_raw, network_raw) = row.map_err(|err| {
            DbError::new(format!("Failed to map wallet report account row: {err}"))
        })?;
        accounts.push(WalletReportAccountMeta {
            account_id: DigitalAssetAccountId::from_str(&account_id_raw)
                .map_err(|err| DbError::new(format!("Invalid account id in DB: {err}")))?,
            account_label,
            asset_id: parse_asset_id(&asset_id_raw)?,
            network: parse_network(&network_raw)?,
        });
    }

    Ok(accounts)
}

pub(super) fn wallet_report_balance_state_from_native(
    state: NativeBalanceState,
) -> WalletReportBalanceState {
    match state {
        NativeBalanceState::KnownAmount(amount) => WalletReportBalanceState::KnownAmount(amount),
        NativeBalanceState::CanonicalZero => WalletReportBalanceState::CanonicalZero,
        NativeBalanceState::Unknown => WalletReportBalanceState::Unknown,
    }
}

pub(crate) fn load_wallet_report(
    user_id: UserId,
    wallet_id: WalletId,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    timezone: UserTimezone,
    history_cap: crate::transactions::TransactionCount,
) -> Result<WalletReportData, WalletReportLoadError> {
    with_user_db(user_id, |conn| {
        let wallet_label = load_wallet_label(conn, wallet_id)?;
        let account_meta = load_wallet_report_account_meta(conn, wallet_id)?;

        let mut account_inputs = Vec::with_capacity(account_meta.len());
        for account in account_meta {
            let first_transaction_date = load_first_transaction_date(conn, account.account_id)?;
            let reliability_context = load_account_balance_reliability_context_for_history(
                conn,
                account.account_id,
                Some(history_cap),
            )?;
            account_inputs.push((account, first_transaction_date, reliability_context));
        }

        let sync_dates = account_inputs
            .iter()
            .map(|(_, _, reliability_context)| reliability_context.last_successful_sync_date)
            .collect::<Vec<_>>();
        let today_local = local_date_in_timezone(Utc::now(), timezone);
        let default_range = resolve_wallet_report_default_range(
            today_local,
            wallet_consistent_sync_date(&sync_dates, timezone),
        )?;
        let resolved_range = resolve_wallet_report_range(default_range, from, to)?;

        let opening_boundary = local_report_date_to_utc_boundary(
            resolved_range.from(),
            timezone,
            DateBoundaryKind::StartOfDay,
        );
        let closing_boundary = local_report_date_to_utc_boundary(
            resolved_range.to(),
            timezone,
            DateBoundaryKind::EndOfDay,
        );

        let mut accounts = Vec::with_capacity(account_inputs.len());
        for (account, first_transaction_date, reliability_context) in account_inputs {
            let last_successful_sync_date = reliability_context.last_successful_sync_date;
            let native_account_meta = AccountMeta {
                wallet_id,
                asset_id: account.asset_id,
                network: account.network,
                label: Some(account.account_label.clone()),
                wallet_label: wallet_label.clone(),
            };
            let resolved = resolve_balance_dates(BalanceResolutionInputs {
                from_date: Some(opening_boundary),
                to_date: Some(closing_boundary),
                first_transaction_date,
                last_successful_sync_date,
            });

            let opening_balance = resolve_native_balance_at_boundary(
                conn,
                account.account_id,
                &native_account_meta,
                NativeBalanceBoundaryRequest {
                    boundary_kind: NativeBalanceBoundaryKind::Opening,
                    requested_boundary_date: resolved.opening_balance_date,
                    first_transaction_date,
                    transaction_history_pending: false,
                },
                &reliability_context,
            )?;
            let closing_balance = resolve_native_balance_at_boundary(
                conn,
                account.account_id,
                &native_account_meta,
                NativeBalanceBoundaryRequest {
                    boundary_kind: NativeBalanceBoundaryKind::Closing,
                    requested_boundary_date: resolved.closing_balance_date,
                    first_transaction_date,
                    transaction_history_pending: false,
                },
                &reliability_context,
            )?;

            accounts.push(WalletReportAccountRow {
                account_id: account.account_id,
                account_label: account.account_label,
                asset_id: account.asset_id,
                bitcoin_history_coverage: reliability_context.bitcoin_history_coverage,
                opening_balance_state: wallet_report_balance_state_from_native(
                    opening_balance.state,
                ),
                opening_balance: opening_balance.amount,
                opening_balance_reliability: opening_balance.balance_reliability,
                opening_balance_date: opening_balance
                    .balance_date
                    .map(|date| local_date_in_timezone(date, timezone)),
                closing_balance_state: wallet_report_balance_state_from_native(
                    closing_balance.state,
                ),
                closing_balance: closing_balance.amount,
                closing_balance_reliability: closing_balance.balance_reliability,
                closing_balance_date: closing_balance
                    .balance_date
                    .map(|date| local_date_in_timezone(date, timezone)),
            });
        }

        Ok(WalletReportData {
            wallet_label,
            resolved_from: resolved_range.from(),
            resolved_to: resolved_range.to(),
            default_this_year_from: default_range.from(),
            default_this_year_to: default_range.to(),
            accounts,
        })
    })
}

pub(crate) fn load_holdings_report(
    user_id: UserId,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    timezone: UserTimezone,
    today: NaiveDate,
    history_cap: crate::transactions::TransactionCount,
) -> Result<HoldingsReportData, WalletReportLoadError> {
    let range_plan = load_holdings_report_range_plan(user_id, from, to, timezone, today)?;
    let default_range = range_plan.default_range;
    let resolved_range = range_plan.requested_range;
    let wallets = crate::db::list_wallets(user_id).map_err(WalletReportLoadError::Database)?;

    let mut rows = Vec::with_capacity(wallets.len());
    for wallet in wallets {
        let wallet_id = wallet.wallet.id;
        let wallet_report = load_wallet_report(
            user_id,
            wallet_id,
            Some(resolved_range.from()),
            Some(resolved_range.to()),
            timezone,
            history_cap,
        )?;
        rows.push(HoldingsReportWalletData {
            wallet_id,
            wallet_label: wallet_report.wallet_label,
            accounts: wallet_report.accounts,
        });
    }

    Ok(HoldingsReportData {
        resolved_from: resolved_range.from(),
        resolved_to: resolved_range.to(),
        default_this_year_from: default_range.from(),
        default_this_year_to: default_range.to(),
        wallets: rows,
    })
}

pub(crate) fn load_holdings_report_range_plan(
    user_id: UserId,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    timezone: UserTimezone,
    today_local: NaiveDate,
) -> Result<WalletReportRangePlan, WalletReportLoadError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare("SELECT id FROM digital_asset_accounts ORDER BY created_at ASC, id ASC")
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare holdings report accounts query: {err}"
                ))
            })?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| {
                DbError::new(format!("Failed to query holdings report accounts: {err}"))
            })?;
        let mut sync_dates = Vec::new();

        for row in rows {
            let account_id_raw = row.map_err(|err| {
                DbError::new(format!("Failed to map holdings report account row: {err}"))
            })?;
            let account_id = DigitalAssetAccountId::from_str(&account_id_raw)
                .map_err(|err| DbError::new(format!("Invalid account id in DB: {err}")))?;
            let reliability_context = load_account_balance_reliability_context(conn, account_id)?;
            sync_dates.push(reliability_context.last_successful_sync_date);
        }

        resolve_holdings_report_range_plan(today_local, &sync_dates, timezone, from, to)
    })
}

pub(crate) fn load_wallet_report_range_plan(
    user_id: UserId,
    wallet_id: WalletId,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    timezone: UserTimezone,
    today_local: NaiveDate,
) -> Result<WalletReportRangePlan, WalletReportLoadError> {
    with_user_db(user_id, |conn| {
        load_wallet_label(conn, wallet_id)?;
        let account_meta = load_wallet_report_account_meta(conn, wallet_id)?;

        let mut sync_dates = Vec::with_capacity(account_meta.len());
        for account in account_meta {
            let reliability_context =
                load_account_balance_reliability_context(conn, account.account_id)?;
            sync_dates.push(reliability_context.last_successful_sync_date);
        }

        resolve_wallet_report_range_plan(today_local, &sync_dates, timezone, from, to)
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;

    #[test]
    fn holdings_report_empty_user_uses_current_year_to_date_default() {
        let user_id = crate::db::test_fixtures::unique_user_id();
        crate::db::test_fixtures::setup_test_user(user_id);
        let today = NaiveDate::from_ymd_opt(2026, 7, 4).expect("valid date");
        let timezone =
            crate::models::UserTimezone("Europe/Amsterdam".parse().expect("valid timezone"));

        let report = load_holdings_report(
            user_id,
            None,
            None,
            timezone,
            today,
            crate::transactions::TransactionCount::from_u32(u32::MAX),
        )
        .expect("report loads");

        assert_eq!(
            report.default_this_year_from,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
        );
        assert_eq!(report.default_this_year_to, today);
        assert_eq!(
            report.resolved_from,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
        );
        assert_eq!(report.resolved_to, today);
        assert!(report.wallets.is_empty());
    }
}
