use crate::balance_reliability::{BalanceProvisionalReason, BalanceReliability};
use crate::db::error::DbError;
use crate::models::parse_datetime;
use crate::transactions::TransactionCount;
use crate::wallets::DigitalAssetAccountId;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use std::collections::HashMap;

const ADDRESS_SYNC_SCOPE: &str = "address";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountBalanceReliabilityContext {
    pub(crate) last_successful_sync_date: Option<DateTime<Utc>>,
    pub(crate) balance_reliability: BalanceReliability,
    pub(crate) bitcoin_history_coverage:
        Option<crate::db::transaction_sync::BitcoinAccountHistoryCoverage>,
}

pub(crate) fn load_latest_successful_sync_date(
    account_level: Option<DateTime<Utc>>,
    address_level: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (account_level, address_level) {
        (Some(account_date), Some(address_date)) => Some(account_date.max(address_date)),
        (Some(date), None) | (None, Some(date)) => Some(date),
        (None, None) => None,
    }
}

pub(crate) fn derive_account_balance_reliability(
    last_successful_sync_date: Option<DateTime<Utc>>,
    has_active_backfill: bool,
) -> BalanceReliability {
    let mut reasons = Vec::new();

    if last_successful_sync_date.is_none() {
        reasons.push(BalanceProvisionalReason::FirstSuccessfulSyncPending);
    }
    if has_active_backfill {
        reasons.push(BalanceProvisionalReason::HistoricalBackfillInProgress);
    }

    BalanceReliability::from_reasons(reasons)
}

fn resolve_bitcoin_history_coverage_for_cap(
    coverage: crate::db::transaction_sync::BitcoinAccountHistoryCoverage,
    canonical_transaction_count: TransactionCount,
    history_cap: TransactionCount,
) -> crate::db::transaction_sync::BitcoinAccountHistoryCoverage {
    if matches!(
        coverage,
        crate::db::transaction_sync::BitcoinAccountHistoryCoverage::Complete { .. }
    ) {
        return coverage;
    }
    if canonical_transaction_count.value() >= history_cap.value() {
        return crate::db::transaction_sync::BitcoinAccountHistoryCoverage::Limited;
    }
    coverage
}

fn load_account_integration_last_successful_sync_date(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Option<DateTime<Utc>>, DbError> {
    let account_id_raw = account_id.to_string();
    let raw: Option<String> = conn
        .query_row(
            "SELECT MAX(last_completed_at)
             FROM account_integration_sync_state
             WHERE account_id = ?1
               AND last_result = 'success'
               AND last_completed_at IS NOT NULL",
            params![account_id_raw],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to query last successful sync date: {err}")))?
        .flatten();

    raw.map(|value| {
        parse_datetime(&value).map_err(|err| {
            DbError::new(format!(
                "Invalid last_completed_at in account_integration_sync_state: {err}"
            ))
        })
    })
    .transpose()
}

fn load_address_last_successful_sync_date(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Option<DateTime<Utc>>, DbError> {
    let account_id_raw = account_id.to_string();
    let raw: Option<String> = conn
        .query_row(
            "SELECT MAX(tss.last_completed_at)
             FROM transaction_sync_state tss
             JOIN digital_asset_addresses da ON da.id = tss.address_id
             WHERE da.account_id = ?1
               AND tss.scope = ?2
               AND tss.last_result = 'success'
               AND tss.last_completed_at IS NOT NULL",
            params![account_id_raw, ADDRESS_SYNC_SCOPE],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| {
            DbError::new(format!(
                "Failed to query address-level last successful sync date: {err}"
            ))
        })?
        .flatten();

    raw.map(|value| {
        parse_datetime(&value).map_err(|err| {
            DbError::new(format!(
                "Invalid last_completed_at in transaction_sync_state: {err}"
            ))
        })
    })
    .transpose()
}

fn load_account_has_active_backfill(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<bool, DbError> {
    let account_id_raw = account_id.to_string();
    let has_active_backfill = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM digital_asset_addresses da
                JOIN transaction_sync_state tss ON tss.address_id = da.id
                WHERE da.account_id = ?1
                  AND tss.scope = ?2
                  AND (
                    tss.mempool_backfill_cursor_txid IS NOT NULL
                    OR tss.etherscan_backfill_end_block IS NOT NULL
                  )
             )",
            params![account_id_raw, ADDRESS_SYNC_SCOPE],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|err| DbError::new(format!("Failed to query active backfill state: {err}")))?;

    Ok(has_active_backfill)
}

pub(crate) fn load_account_last_successful_sync_date(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Option<DateTime<Utc>>, DbError> {
    let account_level = load_account_integration_last_successful_sync_date(conn, account_id)?;
    let address_level = load_address_last_successful_sync_date(conn, account_id)?;
    Ok(load_latest_successful_sync_date(
        account_level,
        address_level,
    ))
}

pub(crate) fn load_account_balance_reliability_context(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<AccountBalanceReliabilityContext, DbError> {
    load_account_balance_reliability_context_for_history(conn, account_id, None)
}

pub(crate) fn load_account_balance_reliability_context_for_history(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    history_cap: Option<TransactionCount>,
) -> Result<AccountBalanceReliabilityContext, DbError> {
    let last_successful_sync_date = load_account_last_successful_sync_date(conn, account_id)?;
    let has_active_backfill = load_account_has_active_backfill(conn, account_id)?;
    let mut bitcoin_history_coverage =
        crate::db::account_transactions::load_bitcoin_account_history_coverage(conn, account_id)?;
    if let (Some(coverage), Some(history_cap)) = (bitcoin_history_coverage, history_cap)
        && !crate::db::account_transactions::load_bitcoin_history_repair_pending(conn, account_id)?
    {
        let count =
            crate::db::transaction_sync::load_canonical_account_transaction_count_bounded_conn(
                conn,
                account_id,
                history_cap,
            )?;
        bitcoin_history_coverage = Some(resolve_bitcoin_history_coverage_for_cap(
            coverage,
            count,
            history_cap,
        ));
    }
    let coverage_reason = bitcoin_history_coverage.and_then(|coverage| match coverage {
        crate::db::transaction_sync::BitcoinAccountHistoryCoverage::Unscanned
        | crate::db::transaction_sync::BitcoinAccountHistoryCoverage::Syncing => {
            Some(BalanceProvisionalReason::HistoricalBackfillInProgress)
        }
        crate::db::transaction_sync::BitcoinAccountHistoryCoverage::Limited => {
            Some(BalanceProvisionalReason::HistoricalCoverageLimited)
        }
        crate::db::transaction_sync::BitcoinAccountHistoryCoverage::Complete { .. } => None,
    });
    let coverage_reliability = BalanceReliability::from_reasons(coverage_reason);

    Ok(AccountBalanceReliabilityContext {
        last_successful_sync_date,
        balance_reliability: derive_account_balance_reliability(
            last_successful_sync_date,
            has_active_backfill
                && !matches!(
                    bitcoin_history_coverage,
                    Some(crate::db::transaction_sync::BitcoinAccountHistoryCoverage::Limited)
                ),
        )
        .combine(&coverage_reliability),
        bitcoin_history_coverage,
    })
}

#[cfg(any(test, not(feature = "desktop")))]
pub(crate) fn load_effective_bitcoin_history_coverage(
    user_id: crate::models::UserId,
    account_id: DigitalAssetAccountId,
    history_cap: TransactionCount,
) -> Result<Option<crate::db::transaction_sync::BitcoinAccountHistoryCoverage>, DbError> {
    crate::db::with_user_db(user_id, |conn| {
        Ok(load_account_balance_reliability_context_for_history(
            conn,
            account_id,
            Some(history_cap),
        )?
        .bitcoin_history_coverage)
    })
}

pub(crate) fn load_account_balance_reliability_contexts(
    conn: &rusqlite::Connection,
    account_ids: &[DigitalAssetAccountId],
) -> Result<HashMap<DigitalAssetAccountId, AccountBalanceReliabilityContext>, DbError> {
    let mut result = HashMap::with_capacity(account_ids.len());

    for account_id in account_ids {
        let context = load_account_balance_reliability_context(conn, *account_id)?;
        result.insert(*account_id, context);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "db-tests")]
    use crate::db::{acquire_test_runtime, initialize_user_db_for_test};
    #[cfg(feature = "db-tests")]
    use crate::models::UserId;

    #[cfg(not(bitgarth_db_unit_only))]
    fn dt(raw: &str) -> DateTime<Utc> {
        parse_datetime(raw).expect("valid datetime")
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn load_latest_successful_sync_date_prefers_most_recent_source() {
        let account_level = Some(dt("2026-01-10T12:00:00Z"));
        let address_level = Some(dt("2026-01-11T09:00:00Z"));

        assert_eq!(
            load_latest_successful_sync_date(account_level, address_level),
            address_level
        );
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn derive_account_balance_reliability_marks_first_sync_pending_without_success() {
        assert_eq!(
            derive_account_balance_reliability(None, false),
            BalanceReliability::Provisional {
                reasons: vec![BalanceProvisionalReason::FirstSuccessfulSyncPending],
            }
        );
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn derive_account_balance_reliability_marks_backfill_even_after_success() {
        assert_eq!(
            derive_account_balance_reliability(Some(dt("2026-01-10T12:00:00Z")), true),
            BalanceReliability::Provisional {
                reasons: vec![BalanceProvisionalReason::HistoricalBackfillInProgress],
            }
        );
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn bitcoin_coverage_limit_resolution_preserves_all_four_states() {
        use crate::db::transaction_sync::BitcoinAccountHistoryCoverage;
        use crate::transactions::{ChainTipHeight, TransactionCount};

        let complete = BitcoinAccountHistoryCoverage::Complete {
            coverage_height: ChainTipHeight::try_new(100).expect("height should parse"),
        };
        assert_eq!(
            resolve_bitcoin_history_coverage_for_cap(
                complete,
                TransactionCount::from_u32(100),
                TransactionCount::from_u32(0),
            ),
            complete
        );
        assert_eq!(
            resolve_bitcoin_history_coverage_for_cap(
                BitcoinAccountHistoryCoverage::Unscanned,
                TransactionCount::zero(),
                TransactionCount::from_u32(100),
            ),
            BitcoinAccountHistoryCoverage::Unscanned
        );
        assert_eq!(
            resolve_bitcoin_history_coverage_for_cap(
                BitcoinAccountHistoryCoverage::Syncing,
                TransactionCount::from_u32(99),
                TransactionCount::from_u32(100),
            ),
            BitcoinAccountHistoryCoverage::Syncing
        );
        assert_eq!(
            resolve_bitcoin_history_coverage_for_cap(
                BitcoinAccountHistoryCoverage::Syncing,
                TransactionCount::from_u32(100),
                TransactionCount::from_u32(100),
            ),
            BitcoinAccountHistoryCoverage::Limited
        );
        assert_eq!(
            resolve_bitcoin_history_coverage_for_cap(
                BitcoinAccountHistoryCoverage::Unscanned,
                TransactionCount::zero(),
                TransactionCount::zero(),
            ),
            BitcoinAccountHistoryCoverage::Limited
        );
    }

    #[cfg(feature = "db-tests")]
    #[test]
    fn load_account_balance_reliability_context_defaults_to_first_sync_pending() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");

        crate::db::user_db::with_user_db(user_id, |conn| {
            let account_id = DigitalAssetAccountId::new();
            let context = load_account_balance_reliability_context(conn, account_id)?;
            assert_eq!(context.last_successful_sync_date, None);
            assert_eq!(
                context.balance_reliability,
                BalanceReliability::Provisional {
                    reasons: vec![BalanceProvisionalReason::FirstSuccessfulSyncPending],
                }
            );
            assert_eq!(context.bitcoin_history_coverage, None);
            Ok::<(), DbError>(())
        })
        .expect("db access should succeed");
    }
}
