use super::ledger_rebuild::{
    compute_utxo_opening_balance_for_read_path, sum_complete_api_confirmed_balance,
};
use super::types::*;
use crate::account_model::AccountModel;
use crate::amounts::UnsignedAmount;
use crate::asset_capabilities::account_model_for;
use crate::balance_reliability::BalanceReliability;
use crate::db::account_balance_resolution::{
    AccountBalanceBoundaryKind, AccountBalanceDisplayState, BoundaryAccountBalanceInputs,
    resolve_boundary_account_balance_state,
};
use crate::db::balance_reliability::AccountBalanceReliabilityContext;
use crate::db::error::DbError;
use crate::wallets::DigitalAssetAccountId;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

pub(super) fn native_balance_state_from_amount(amount: UnsignedAmount) -> NativeBalanceState {
    NativeBalanceState::KnownAmount(amount)
}

pub(super) fn native_balance_resolution_from_amount(
    amount: UnsignedAmount,
    balance_date: Option<DateTime<Utc>>,
    balance_reliability: &BalanceReliability,
) -> NativeBalanceBoundaryResolution {
    NativeBalanceBoundaryResolution {
        state: native_balance_state_from_amount(amount),
        amount: Some(amount),
        balance_reliability: balance_reliability.clone(),
        balance_date,
    }
}

pub(super) fn canonical_zero_balance_resolution(
    balance_date: Option<DateTime<Utc>>,
    balance_reliability: &BalanceReliability,
) -> NativeBalanceBoundaryResolution {
    NativeBalanceBoundaryResolution {
        state: NativeBalanceState::CanonicalZero,
        amount: None,
        balance_reliability: balance_reliability.clone(),
        balance_date,
    }
}

pub(super) fn unknown_balance_resolution(
    balance_date: Option<DateTime<Utc>>,
    balance_reliability: &BalanceReliability,
) -> NativeBalanceBoundaryResolution {
    NativeBalanceBoundaryResolution {
        state: NativeBalanceState::Unknown,
        amount: None,
        balance_reliability: balance_reliability.clone(),
        balance_date,
    }
}

pub(super) fn load_account_meta(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<AccountMeta, DbError> {
    let row = conn
        .query_row(
            "SELECT daa.asset_id, daa.network, daa.label, w.label, daa.wallet_id
             FROM digital_asset_accounts daa
             JOIN wallets w ON w.id = daa.wallet_id
             WHERE daa.id = ?1
             LIMIT 1",
            params![account_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to load account metadata: {err}")))?;

    let Some((asset_id_raw, network_raw, label, wallet_label, wallet_id_raw)) = row else {
        return Err(DbError::new("Account not found"));
    };

    Ok(AccountMeta {
        wallet_id: parse_wallet_id(&wallet_id_raw)?,
        asset_id: parse_asset_id(&asset_id_raw)?,
        network: parse_network(&network_raw)?,
        label,
        wallet_label,
    })
}

pub(super) fn load_account_reference_info(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<AccountReferenceInfo, DbError> {
    let account_id_str = account_id.to_string();

    // Try HD key first
    let hd_row: Option<(String, String)> = conn
        .query_row(
            "SELECT extended_pubkey, address_scheme
             FROM digital_asset_account_hd_keys
             WHERE account_id = ?1
             LIMIT 1",
            params![&account_id_str],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to load HD key info: {err}")))?;

    if let Some((extended_pubkey, scheme_raw)) = hd_row {
        let address_scheme = crate::wallets::AddressScheme::from_str(&scheme_raw)
            .ok_or_else(|| DbError::new(format!("Invalid address_scheme in DB: {scheme_raw}")))?;
        return Ok(AccountReferenceInfo {
            is_hd: true,
            address_scheme,
            reference_value: extended_pubkey,
        });
    }

    // Single-address account: get the address
    let addr_row: Option<(String, String)> = conn
        .query_row(
            "SELECT address, address_scheme
             FROM digital_asset_addresses
             WHERE account_id = ?1
             LIMIT 1",
            params![&account_id_str],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to load address info: {err}")))?;

    let Some((address, scheme_raw)) = addr_row else {
        return Err(DbError::new("Account has neither HD keys nor addresses"));
    };

    let address_scheme = crate::wallets::AddressScheme::from_str(&scheme_raw)
        .ok_or_else(|| DbError::new(format!("Invalid address_scheme in DB: {scheme_raw}")))?;

    Ok(AccountReferenceInfo {
        is_hd: false,
        address_scheme,
        reference_value: address,
    })
}

pub(super) fn load_account_overall_balance(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Option<UnsignedAmount>, DbError> {
    let account_id_raw = account_id.to_string();
    let row = conn
        .query_row(
            "SELECT closing_balance_hi, closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1
               AND status = 'confirmed'
               AND closing_balance_hi IS NOT NULL
               AND closing_balance_lo IS NOT NULL
             ORDER BY occurred_at DESC,
                      COALESCE(block_height, 0) DESC,
                      COALESCE(nonce, 0) DESC,
                      tx_hash DESC
             LIMIT 1",
            params![account_id_raw],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to query account overall balance: {err}")))?;

    match row {
        Some((hi, lo)) => parse_split_amount(hi, lo, "overall_balance").map(Some),
        None => Ok(None),
    }
}

fn load_account_api_confirmed_balance(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Option<UnsignedAmount>, DbError> {
    let api_balances = crate::db::transaction_sync::load_api_confirmed_balances_for_account_conn(
        conn, account_id,
    )?;
    sum_complete_api_confirmed_balance(&api_balances)
}

pub(super) fn load_balance_as_of_date(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    as_of: DateTime<Utc>,
) -> Result<Option<UnsignedAmount>, DbError> {
    let account_id_raw = account_id.to_string();
    let as_of_raw = as_of.to_rfc3339();
    let row = conn
        .query_row(
            "SELECT closing_balance_hi, closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1
               AND status = 'confirmed'
               AND occurred_at <= ?2
               AND closing_balance_hi IS NOT NULL
               AND closing_balance_lo IS NOT NULL
             ORDER BY occurred_at DESC,
                      COALESCE(block_height, 0) DESC,
                      COALESCE(nonce, 0) DESC,
                      tx_hash DESC
             LIMIT 1",
            params![account_id_raw, as_of_raw],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to query balance as of {as_of}: {err}")))?;

    match row {
        Some((hi, lo)) => parse_split_amount(hi, lo, "balance_as_of_date").map(Some),
        None => Ok(None),
    }
}

pub(super) fn load_balance_before_date(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    before: DateTime<Utc>,
) -> Result<Option<UnsignedAmount>, DbError> {
    let account_id_raw = account_id.to_string();
    let before_raw = before.to_rfc3339();
    let row = conn
        .query_row(
            "SELECT closing_balance_hi, closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1
               AND status = 'confirmed'
               AND occurred_at < ?2
               AND closing_balance_hi IS NOT NULL
               AND closing_balance_lo IS NOT NULL
             ORDER BY occurred_at DESC,
                      COALESCE(block_height, 0) DESC,
                      COALESCE(nonce, 0) DESC,
                      tx_hash DESC
             LIMIT 1",
            params![account_id_raw, before_raw],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to query balance before {before}: {err}")))?;

    match row {
        Some((hi, lo)) => parse_split_amount(hi, lo, "balance_before_date").map(Some),
        None => Ok(None),
    }
}

pub(super) fn load_first_transaction_date(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Option<DateTime<Utc>>, DbError> {
    let account_id_raw = account_id.to_string();
    let raw: Option<String> = conn
        .query_row(
            "SELECT MIN(occurred_at)
             FROM account_transaction_ledger
             WHERE account_id = ?1
               AND status = 'confirmed'",
            params![account_id_raw],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to query first transaction date: {err}")))?
        .flatten();

    raw.map(|s| {
        crate::models::parse_datetime(&s)
            .map_err(|err| DbError::new(format!("Invalid first transaction date in DB: {err}")))
    })
    .transpose()
}

/// Raw inputs from DB queries, before resolution into final opening/closing values.
#[cfg_attr(test, derive(Debug, Clone, PartialEq, Eq))]
pub(crate) struct BalanceResolutionInputs {
    pub(crate) from_date: Option<DateTime<Utc>>,
    pub(crate) to_date: Option<DateTime<Utc>>,
    pub(crate) first_transaction_date: Option<DateTime<Utc>>,
    pub(crate) last_successful_sync_date: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct NativeBalanceBoundaryRequest {
    pub(super) boundary_kind: NativeBalanceBoundaryKind,
    pub(super) requested_boundary_date: Option<DateTime<Utc>>,
    pub(super) first_transaction_date: Option<DateTime<Utc>>,
    pub(super) transaction_history_pending: bool,
}

/// Resolved opening/closing balance dates for the transactions page.
#[cfg_attr(test, derive(Debug, Clone, PartialEq, Eq))]
pub(crate) struct ResolvedBalanceDates {
    pub(crate) opening_balance_date: Option<DateTime<Utc>>,
    pub(crate) closing_balance_date: Option<DateTime<Utc>>,
}

pub(crate) fn resolve_balance_dates(inputs: BalanceResolutionInputs) -> ResolvedBalanceDates {
    if inputs.first_transaction_date.is_none() {
        return ResolvedBalanceDates {
            opening_balance_date: inputs.from_date.or(inputs.last_successful_sync_date),
            closing_balance_date: inputs.to_date.or(inputs.last_successful_sync_date),
        };
    }

    let opening_balance_date = inputs
        .from_date
        .or(inputs.first_transaction_date)
        .or(inputs.last_successful_sync_date);
    let closing_balance_date = inputs.to_date.or(inputs.last_successful_sync_date);

    ResolvedBalanceDates {
        opening_balance_date,
        closing_balance_date,
    }
}

pub(super) fn resolve_native_balance_at_boundary(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    meta: &AccountMeta,
    request: NativeBalanceBoundaryRequest,
    balance_reliability_context: &AccountBalanceReliabilityContext,
) -> Result<NativeBalanceBoundaryResolution, DbError> {
    let NativeBalanceBoundaryRequest {
        boundary_kind,
        requested_boundary_date,
        first_transaction_date,
        transaction_history_pending,
    } = request;
    let last_successful_sync_date = balance_reliability_context.last_successful_sync_date;
    let balance_reliability = &balance_reliability_context.balance_reliability;
    let account_model = account_model_for(meta.asset_id);
    let has_complete_bitcoin_history = matches!(
        balance_reliability_context.bitcoin_history_coverage,
        Some(crate::db::transaction_sync::BitcoinAccountHistoryCoverage::Complete { .. })
    );
    if meta.asset_id == crate::wallets::SyncedAssetId::Bitcoin && !has_complete_bitcoin_history {
        return Ok(unknown_balance_resolution(
            requested_boundary_date,
            balance_reliability,
        ));
    }

    let resolved_amount = match (boundary_kind, requested_boundary_date) {
        (NativeBalanceBoundaryKind::Opening, Some(boundary_date)) => {
            load_balance_before_date(conn, account_id, boundary_date)?
        }
        (NativeBalanceBoundaryKind::Opening, None) => None,
        (NativeBalanceBoundaryKind::Closing, Some(boundary_date)) => {
            load_balance_as_of_date(conn, account_id, boundary_date)?
        }
        (NativeBalanceBoundaryKind::Closing, None) => {
            load_account_overall_balance(conn, account_id)?
        }
    };

    let boundary_state = resolve_boundary_account_balance_state(BoundaryAccountBalanceInputs {
        boundary_kind: match boundary_kind {
            NativeBalanceBoundaryKind::Opening => AccountBalanceBoundaryKind::Opening,
            NativeBalanceBoundaryKind::Closing => AccountBalanceBoundaryKind::Closing,
        },
        requested_boundary_date,
        first_transaction_date,
        last_successful_sync_date,
        ledger_amount: resolved_amount,
        api_confirmed_amount: load_account_api_confirmed_balance(conn, account_id)?,
        free_balance_unavailable: false,
        transaction_history_pending,
    });

    match boundary_state {
        AccountBalanceDisplayState::KnownLedger { amount, .. }
        | AccountBalanceDisplayState::KnownApiConfirmed { amount, .. } => {
            if amount == UnsignedAmount::zero() && has_complete_bitcoin_history {
                return Ok(canonical_zero_balance_resolution(
                    requested_boundary_date,
                    balance_reliability,
                ));
            }
            return Ok(native_balance_resolution_from_amount(
                amount,
                requested_boundary_date,
                balance_reliability,
            ));
        }
        AccountBalanceDisplayState::CanonicalZero => {
            if account_model != AccountModel::Utxo || has_complete_bitcoin_history {
                return Ok(canonical_zero_balance_resolution(
                    requested_boundary_date,
                    balance_reliability,
                ));
            }
        }
        AccountBalanceDisplayState::Unknown | AccountBalanceDisplayState::UnavailableOnFree => {}
    }

    match boundary_kind {
        NativeBalanceBoundaryKind::Opening => {
            if account_model == AccountModel::Utxo
                && !transaction_history_pending
                && let (Some(boundary_date), Some(first_transaction_date)) =
                    (requested_boundary_date, first_transaction_date)
                && boundary_date <= first_transaction_date
                && let Some(opening_balance) =
                    compute_utxo_opening_balance_for_read_path(conn, account_id, meta)?
            {
                return Ok(native_balance_resolution_from_amount(
                    opening_balance.amount(),
                    requested_boundary_date,
                    balance_reliability,
                ));
            }

            if account_model == AccountModel::Account
                && let (Some(boundary_date), Some(first_transaction_date)) =
                    (requested_boundary_date, first_transaction_date)
                && boundary_date <= first_transaction_date
            {
                return Ok(canonical_zero_balance_resolution(
                    requested_boundary_date,
                    balance_reliability,
                ));
            }
        }
        NativeBalanceBoundaryKind::Closing => {
            // For UTXO-model accounts with no ledger entries: when the closing
            // boundary falls before the last successful sync and there are no
            // confirmed transactions to adjust the balance, the correct value
            // is the API-confirmed total observed during sync. The opening
            // balance correction already computes this baseline.
            if account_model == AccountModel::Utxo
                && !transaction_history_pending
                && let (Some(boundary_date), Some(last_sync)) =
                    (requested_boundary_date, last_successful_sync_date)
                && boundary_date < last_sync
                && first_transaction_date.is_none()
                && let Some(opening_balance) =
                    compute_utxo_opening_balance_for_read_path(conn, account_id, meta)?
            {
                return Ok(native_balance_resolution_from_amount(
                    opening_balance.amount(),
                    requested_boundary_date,
                    balance_reliability,
                ));
            }

            if account_model == AccountModel::Account
                && let (Some(boundary_date), Some(first_transaction_date)) =
                    (requested_boundary_date, first_transaction_date)
                && boundary_date < first_transaction_date
            {
                return Ok(canonical_zero_balance_resolution(
                    requested_boundary_date,
                    balance_reliability,
                ));
            }
        }
    }

    Ok(unknown_balance_resolution(
        requested_boundary_date,
        balance_reliability,
    ))
}
