use super::balance::{
    BalanceResolutionInputs, NativeBalanceBoundaryRequest, load_account_meta,
    load_account_reference_info, load_first_transaction_date, resolve_balance_dates,
    resolve_native_balance_at_boundary,
};
use super::types::*;
use crate::balance_reliability::{BalanceProvisionalReason, BalanceReliability};
use crate::db::balance_reliability::load_account_balance_reliability_context_for_history;
use crate::db::error::DbError;
use crate::db::user_db::with_user_db;
use crate::models::UserId;
use crate::models::parse_datetime;
use crate::wallets::{DigitalAssetAccountId, TransactionFilters};

struct PageLoadRequest<'a> {
    page: u32,
    page_size: u32,
    sort: crate::wallets::TransactionSortDirection,
    filters: &'a TransactionFilters,
    account_balance_reliability: &'a BalanceReliability,
}

pub(super) fn statuses_for_kind(
    kind: AccountTransactionTableKind,
    filters: &TransactionFilters,
) -> Vec<&'static str> {
    let default_statuses = match kind {
        AccountTransactionTableKind::Pending => vec!["pending", "dropped"],
        AccountTransactionTableKind::Confirmed => vec!["confirmed", "failed"],
    };

    if filters.status.is_empty() {
        return default_statuses;
    }

    let default_set: std::collections::HashSet<&str> = default_statuses.iter().copied().collect();
    filters
        .status
        .iter()
        .map(|s| s.as_db_value())
        .filter(|s| default_set.contains(s))
        .collect()
}

pub(super) fn page_total_for_kind(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    kind: AccountTransactionTableKind,
    filters: &TransactionFilters,
) -> Result<u32, DbError> {
    let account_id_raw = account_id.to_string();
    let statuses = statuses_for_kind(kind, filters);

    if statuses.is_empty() {
        return Ok(0);
    }

    let placeholders: Vec<String> = (0..statuses.len()).map(|i| format!("?{}", i + 2)).collect();
    let status_list = placeholders.join(", ");

    let mut query = format!(
        "SELECT COUNT(*)
         FROM account_transaction_ledger
         WHERE account_id = ?1
           AND status IN ({status_list})"
    );

    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(account_id_raw)];
    for s in &statuses {
        param_values.push(Box::new(s.to_string()));
    }

    if let Some(from_date) = &filters.from_date {
        let idx = param_values.len() + 1;
        query.push_str(&format!("\n           AND occurred_at >= ?{idx}"));
        param_values.push(Box::new(from_date.to_rfc3339()));
    }
    if let Some(to_date) = &filters.to_date {
        let idx = param_values.len() + 1;
        query.push_str(&format!("\n           AND occurred_at <= ?{idx}"));
        param_values.push(Box::new(to_date.to_rfc3339()));
    }

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let total_i64: i64 = conn
        .query_row(&query, param_refs.as_slice(), |row| row.get(0))
        .map_err(|err| {
            DbError::new(format!(
                "Failed to count account transaction ledger rows: {err}"
            ))
        })?;
    i64_to_u32(total_i64, "transaction total")
}

pub(super) fn decode_address_list(
    raw_json: &str,
    field: &'static str,
) -> Result<Vec<String>, DbError> {
    let parsed: Vec<String> = serde_json::from_str(raw_json)
        .map_err(|err| DbError::new(format!("Failed to parse {field} JSON: {err}")))?;
    Ok(parsed
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect())
}

fn load_page_rows(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    kind: AccountTransactionTableKind,
    page_request: &PageLoadRequest<'_>,
) -> Result<Vec<AccountTransactionLedgerRow>, DbError> {
    let offset_u64 = u64::from(page_request.page.saturating_sub(1))
        .saturating_mul(u64::from(page_request.page_size));
    let offset = i64::try_from(offset_u64)
        .map_err(|_| DbError::new("Transaction paging offset exceeds supported range"))?;
    let limit = i64::from(page_request.page_size);
    let account_id_raw = account_id.to_string();
    let statuses = statuses_for_kind(kind, page_request.filters);

    if statuses.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders: Vec<String> = (0..statuses.len()).map(|i| format!("?{}", i + 2)).collect();
    let status_list = placeholders.join(", ");

    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(account_id_raw)];
    for s in &statuses {
        param_values.push(Box::new(s.to_string()));
    }

    let mut where_extra = String::new();
    if let Some(from_date) = &page_request.filters.from_date {
        let idx = param_values.len() + 1;
        where_extra.push_str(&format!("\n               AND occurred_at >= ?{idx}"));
        param_values.push(Box::new(from_date.to_rfc3339()));
    }
    if let Some(to_date) = &page_request.filters.to_date {
        let idx = param_values.len() + 1;
        where_extra.push_str(&format!("\n               AND occurred_at <= ?{idx}"));
        param_values.push(Box::new(to_date.to_rfc3339()));
    }

    let limit_idx = param_values.len() + 1;
    let offset_idx = param_values.len() + 2;
    param_values.push(Box::new(limit));
    param_values.push(Box::new(offset));

    let order_by = match kind {
        AccountTransactionTableKind::Pending => "first_seen_at ASC,
                COALESCE(nonce, 9223372036854775807) ASC,
                tx_hash ASC"
            .to_string(),
        AccountTransactionTableKind::Confirmed => {
            use crate::wallets::TransactionSortDirection;
            match page_request.sort {
                TransactionSortDirection::Ascending => "occurred_at ASC,
                COALESCE(block_height, 9223372036854775807) ASC,
                COALESCE(nonce, 9223372036854775807) ASC,
                COALESCE(min_transfer_index, 9223372036854775807) ASC,
                tx_hash ASC"
                    .to_string(),
                TransactionSortDirection::Descending => "occurred_at DESC,
                COALESCE(block_height, 0) DESC,
                COALESCE(nonce, 0) DESC,
                COALESCE(min_transfer_index, 0) DESC,
                tx_hash DESC"
                    .to_string(),
            }
        }
    };

    let query = format!(
        "SELECT
                tx_hash,
                status,
                tx_type,
                occurred_at,
                from_addresses_json,
                to_addresses_json,
                value_amount_hi,
                value_amount_lo,
                fee_amount_hi,
                fee_amount_lo,
                closing_balance_hi,
                closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1
               AND status IN ({status_list}){where_extra}
             ORDER BY
                {order_by}
             LIMIT ?{limit_idx} OFFSET ?{offset_idx}"
    );

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&query).map_err(|err| {
        DbError::new(format!(
            "Failed to prepare account transaction page query: {err}"
        ))
    })?;
    let rows = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, Option<i64>>(11)?,
            ))
        })
        .map_err(|err| {
            DbError::new(format!(
                "Failed to query account transaction page rows: {err}"
            ))
        })?;

    let mut result = Vec::new();
    for row in rows {
        let (
            tx_hash,
            status_raw,
            tx_type_raw,
            occurred_at_raw,
            from_addresses_json,
            to_addresses_json,
            value_hi,
            value_lo,
            fee_hi,
            fee_lo,
            closing_hi,
            closing_lo,
        ) = row.map_err(|err| {
            DbError::new(format!("Failed to map account transaction page row: {err}"))
        })?;

        let status = parse_chain_status(&status_raw)?;
        let direction = parse_direction(&tx_type_raw)?;
        let occurred_at = parse_datetime(&occurred_at_raw)
            .map_err(|err| DbError::new(format!("Invalid occurred_at in DB: {err}")))?;
        let value = parse_split_amount(value_hi, value_lo, "value_amount")?;
        let fee = parse_optional_split_amount(fee_hi, fee_lo, "fee_amount")?;
        // The DB columns are `closing_balance_*`, and this read model exposes the
        // user-facing meaning: the transaction row's post-transaction closing balance.
        let closing_balance =
            parse_optional_split_amount(closing_hi, closing_lo, "closing_balance")?;

        let balance_reliability = if status == crate::transactions::ChainTransactionStatus::Pending
        {
            page_request
                .account_balance_reliability
                .combine(&BalanceReliability::from_reasons([
                    BalanceProvisionalReason::PendingLedgerState,
                ]))
        } else {
            page_request.account_balance_reliability.clone()
        };

        result.push(AccountTransactionLedgerRow {
            tx_hash,
            status,
            direction,
            occurred_at,
            from_addresses: decode_address_list(&from_addresses_json, "from_addresses_json")?,
            to_addresses: decode_address_list(&to_addresses_json, "to_addresses_json")?,
            value,
            fee,
            closing_balance,
            balance_reliability,
        });
    }

    Ok(result)
}

fn load_page(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    kind: AccountTransactionTableKind,
    page_request: &PageLoadRequest<'_>,
) -> Result<AccountTransactionLedgerPage, DbError> {
    let total = page_total_for_kind(conn, account_id, kind, page_request.filters)?;
    let rows = load_page_rows(conn, account_id, kind, page_request)?;
    Ok(AccountTransactionLedgerPage {
        page: page_request.page,
        page_size: page_request.page_size,
        total,
        rows,
    })
}

pub(crate) fn load_account_transactions_pages(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    pages: (u32, u32),
    page_size: u32,
    sort: crate::wallets::TransactionSortDirection,
    filters: &TransactionFilters,
    history_cap: crate::transactions::TransactionCount,
) -> Result<AccountTransactionsPages, DbError> {
    let (pending_page, confirmed_page) = pages;
    with_user_db(user_id, |conn| {
        let meta = load_account_meta(conn, account_id)?;
        let account_reference = load_account_reference_info(conn, account_id)?;
        let current_balance =
            crate::db::transaction_sync::load_api_confirmed_balances_for_account_conn(
                conn, account_id,
            )
            .and_then(|rows| {
                crate::db::transactions::complete_api_confirmed_balance_with_as_of(&rows)
            })?;

        let first_transaction_date = load_first_transaction_date(conn, account_id)?;
        let transaction_history_pending =
            crate::db::account_has_incomplete_mempool_history_with_conn(conn, account_id)?;
        let balance_reliability_context = load_account_balance_reliability_context_for_history(
            conn,
            account_id,
            Some(history_cap),
        )?;
        let last_successful_sync_date = balance_reliability_context.last_successful_sync_date;

        let resolved = resolve_balance_dates(BalanceResolutionInputs {
            from_date: filters.from_date,
            to_date: filters.to_date,
            first_transaction_date,
            last_successful_sync_date,
        });

        let opening_balance = resolve_native_balance_at_boundary(
            conn,
            account_id,
            &meta,
            NativeBalanceBoundaryRequest {
                boundary_kind: NativeBalanceBoundaryKind::Opening,
                requested_boundary_date: resolved.opening_balance_date,
                first_transaction_date,
                transaction_history_pending,
            },
            &balance_reliability_context,
        )?;
        let closing_balance = resolve_native_balance_at_boundary(
            conn,
            account_id,
            &meta,
            NativeBalanceBoundaryRequest {
                boundary_kind: NativeBalanceBoundaryKind::Closing,
                requested_boundary_date: resolved.closing_balance_date,
                first_transaction_date,
                transaction_history_pending,
            },
            &balance_reliability_context,
        )?;

        let pending_page_request = PageLoadRequest {
            page: pending_page,
            page_size,
            sort,
            filters,
            account_balance_reliability: &balance_reliability_context.balance_reliability,
        };
        let confirmed_page_request = PageLoadRequest {
            page: confirmed_page,
            page_size,
            sort,
            filters,
            account_balance_reliability: &balance_reliability_context.balance_reliability,
        };

        let pending = load_page(
            conn,
            account_id,
            AccountTransactionTableKind::Pending,
            &pending_page_request,
        )?;
        let confirmed = load_page(
            conn,
            account_id,
            AccountTransactionTableKind::Confirmed,
            &confirmed_page_request,
        )?;

        Ok(AccountTransactionsPages {
            account_id,
            wallet_id: meta.wallet_id,
            wallet_label: meta.wallet_label,
            account_label: meta.label,
            asset_id: meta.asset_id,
            network: meta.network,
            bitcoin_history_coverage: balance_reliability_context.bitcoin_history_coverage,
            account_reference,
            has_ingested_history: first_transaction_date.is_some(),
            current_balance_state: current_balance
                .as_ref()
                .map_or(NativeBalanceState::Unknown, |(balance, _)| {
                    NativeBalanceState::KnownAmount(balance.amount())
                }),
            current_balance_checked_at: current_balance.map(|(_, checked_at)| checked_at),
            opening_balance_state: opening_balance.state,
            opening_balance_reliability: opening_balance.balance_reliability,
            opening_balance_date: opening_balance.balance_date,
            closing_balance_state: closing_balance.state,
            closing_balance_reliability: closing_balance.balance_reliability,
            closing_balance_date: closing_balance.balance_date,
            pending,
            confirmed,
        })
    })
}
