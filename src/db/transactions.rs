use super::account_balance_resolution::{
    AccountBalanceDisplayState, CurrentAccountBalanceInputs, resolve_current_account_balance_state,
};
use super::amount_storage::{
    parse_optional_split_amount as parse_optional_split_amount_parts,
    parse_split_amount as parse_split_amount_parts,
};
use super::error::DbError;
#[cfg(all(test, feature = "db-tests"))]
use super::raw_ingestion::ensure_source_connection_for_address_tx;
use super::user_db::with_user_db;
#[cfg(all(test, feature = "db-tests"))]
use super::user_db::with_user_db_mut;
use crate::account_model::AccountModel;
use crate::amounts::UnsignedAmount;
use crate::asset_capabilities::account_model_for;
use crate::db::transaction_sync::AddressApiConfirmedBalanceRow;
use crate::models::{UserId, parse_datetime};
use crate::transactions::{
    AccountBalanceEntry, AccountBalancesResponse, AccountTransactionCounts,
    AccountTransactionDirection, AccountTransactionEntry, AddressBalanceEntry,
    AddressBalanceSummary, ApiConfirmedBalance, ChainTransactionStatus, NativeBalanceState,
    TrackedAddress, aggregate_address_balances, sum_address_balances,
};
use crate::wallets::{
    AccountWithHdKeys, DigitalAssetAccountId, DigitalAssetAddressId, Network, SyncedAssetId,
    WalletId,
};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use std::collections::HashMap;
use std::str::FromStr;
#[cfg(all(test, feature = "db-tests"))]
use ulid::Ulid;

const WALLET_HISTORY_PREVIEW_LIMIT: u32 = crate::wallets::ACCOUNT_TRANSACTIONS_PAGE_SIZE;

fn parse_split_amount(
    hi: i64,
    lo: i64,
    field_name: &'static str,
) -> Result<UnsignedAmount, DbError> {
    parse_split_amount_parts(hi, lo)
        .map_err(|err| DbError::new(format!("Invalid {field_name} split amount from DB: {err}")))
}

fn parse_optional_split_amount(
    hi: Option<i64>,
    lo: Option<i64>,
    field_name: &'static str,
) -> Result<Option<UnsignedAmount>, DbError> {
    parse_optional_split_amount_parts(hi, lo)
        .map_err(|err| DbError::new(format!("Invalid {field_name} split amount from DB: {err}")))
}

fn parse_optional_split_amount_or_zero(
    hi: Option<i64>,
    lo: Option<i64>,
    field_name: &'static str,
) -> Result<UnsignedAmount, DbError> {
    Ok(parse_optional_split_amount(hi, lo, field_name)?.unwrap_or_else(UnsignedAmount::zero))
}

fn load_utxo_address_balance(
    conn: &rusqlite::Connection,
    asset_id: SyncedAssetId,
    address_id: DigitalAssetAddressId,
    network: Network,
) -> Result<AddressBalanceSummary, DbError> {
    let api_row: Option<(Option<i64>, Option<i64>, Option<String>)> = conn
        .query_row(
            "SELECT tss.api_confirmed_balance_hi,
                    tss.api_confirmed_balance_lo,
                    tss.last_completed_at
             FROM digital_asset_addresses AS address
             LEFT JOIN transaction_sync_state AS tss
               ON tss.address_id = address.id
              AND tss.scope = 'address'
             WHERE address.id = ?1
               AND address.asset_id = ?2
               AND address.network = ?3",
            params![address_id.to_string(), asset_id.as_str(), network.as_str()],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|err| {
            DbError::new(format!(
                "Failed to load api confirmed balance for address: {err}"
            ))
        })?;

    Ok(AddressBalanceSummary {
        asset_id,
        confirmed: api_row.map_or(NativeBalanceState::Unknown, |(hi, lo, completed)| {
            provider_balance_state(hi, lo, completed.as_deref())
        }),
    })
}

fn provider_balance_state(
    hi: Option<i64>,
    lo: Option<i64>,
    last_completed_at: Option<&str>,
) -> NativeBalanceState {
    let (Some(hi), Some(lo), Some(last_completed_at)) = (hi, lo, last_completed_at) else {
        return NativeBalanceState::Unknown;
    };
    if parse_datetime(last_completed_at).is_err() {
        return NativeBalanceState::Unknown;
    }
    crate::db::amount_storage::parse_split_amount(hi, lo)
        .map_or(NativeBalanceState::Unknown, NativeBalanceState::KnownAmount)
}

fn checked_sum_native_balances(
    states: &[NativeBalanceState],
) -> Result<NativeBalanceState, DbError> {
    if states.contains(&NativeBalanceState::CanonicalZero) {
        return Err(DbError::new(
            "Canonical zero is only valid at historical boundaries",
        ));
    }
    if states.contains(&NativeBalanceState::Unknown) {
        return Ok(NativeBalanceState::Unknown);
    }

    let mut total = UnsignedAmount::zero();
    for state in states {
        match state {
            NativeBalanceState::KnownAmount(amount) => {
                total = total.checked_add(*amount).map_err(|err| {
                    DbError::new(format!("Overflow while summing current balances: {err}"))
                })?;
            }
            NativeBalanceState::CanonicalZero => {
                unreachable!("canonical zero was rejected before summation");
            }
            NativeBalanceState::Unknown => unreachable!("unknown was handled before summation"),
        }
    }
    Ok(NativeBalanceState::KnownAmount(total))
}

fn load_bitcoin_provider_address_balances(
    conn: &rusqlite::Connection,
) -> Result<
    Vec<(
        DigitalAssetAccountId,
        DigitalAssetAddressId,
        NativeBalanceState,
    )>,
    DbError,
> {
    let mut stmt = conn
        .prepare(
            "SELECT address.account_id,
                    address.id,
                    tss.api_confirmed_balance_hi,
                    tss.api_confirmed_balance_lo,
                    tss.last_completed_at
             FROM digital_asset_addresses AS address
             LEFT JOIN transaction_sync_state AS tss
               ON tss.address_id = address.id
              AND tss.scope = 'address'
             WHERE address.asset_id = 'bitcoin'
             ORDER BY address.account_id, address.id",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare current Bitcoin provider balances query: {err}"
            ))
        })?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|err| {
            DbError::new(format!(
                "Failed to execute current Bitcoin provider balances query: {err}"
            ))
        })?;

    let mut balances = Vec::new();
    for row in rows {
        let (account_id, address_id, hi, lo, completed) = row.map_err(|err| {
            DbError::new(format!(
                "Failed to map current Bitcoin provider balance row: {err}"
            ))
        })?;
        balances.push((
            DigitalAssetAccountId::from_str(&account_id)
                .map_err(|err| DbError::new(format!("Invalid account id in DB: {err}")))?,
            DigitalAssetAddressId::from_str(&address_id)
                .map_err(|err| DbError::new(format!("Invalid address id in DB: {err}")))?,
            provider_balance_state(hi, lo, completed.as_deref()),
        ));
    }
    Ok(balances)
}

fn load_account_model_balance(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    asset_id: SyncedAssetId,
    _network: Network,
) -> Result<AddressBalanceSummary, DbError> {
    let account_id_raw = account_id.to_string();

    // Read the confirmed balance from the most recent confirmed ledger entry's closing_balance.
    // Uses idx_account_tx_ledger_confirmed_page partial index for O(1) lookup.
    let confirmed_row = conn
        .query_row(
            "SELECT closing_balance_hi, closing_balance_lo, occurred_at
             FROM account_transaction_ledger INDEXED BY idx_account_tx_ledger_confirmed_page
             WHERE account_id = ?1
               AND status = 'confirmed'
             ORDER BY
                occurred_at DESC,
                COALESCE(block_height, 9223372036854775807) DESC,
                COALESCE(nonce, 9223372036854775807) DESC,
                COALESCE(min_transfer_index, 9223372036854775807) DESC,
                tx_hash DESC
            LIMIT 1",
            params![account_id_raw],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|err| {
            DbError::new(format!(
                "Failed to load confirmed balance from ledger: {err}"
            ))
        })?;
    let confirmed_ledger_amount = confirmed_row
        .as_ref()
        .map(|(hi, lo, _)| {
            parse_optional_split_amount_or_zero(*hi, *lo, "confirmed closing_balance")
        })
        .transpose()?;
    let confirmed_ledger_as_of = confirmed_row
        .as_ref()
        .map(|(_, _, occurred_at_raw)| {
            parse_datetime(occurred_at_raw)
                .map_err(|err| DbError::new(format!("Invalid occurred_at in DB: {err}")))
        })
        .transpose()?;
    let api_confirmed =
        crate::db::transaction_sync::load_api_confirmed_balances_for_account_conn(conn, account_id)
            .and_then(|rows| complete_api_confirmed_balance_with_as_of(&rows))?;
    let confirmed = match resolve_current_account_balance_state(CurrentAccountBalanceInputs {
        ledger_amount: confirmed_ledger_amount,
        ledger_as_of: confirmed_ledger_as_of,
        api_confirmed_amount: api_confirmed.map(|(balance, _)| balance.amount()),
        api_confirmed_as_of: api_confirmed.map(|(_, as_of)| as_of),
        free_balance_unavailable: false,
    }) {
        AccountBalanceDisplayState::KnownLedger { amount, .. }
        | AccountBalanceDisplayState::KnownApiConfirmed { amount, .. } => {
            NativeBalanceState::KnownAmount(amount)
        }
        AccountBalanceDisplayState::CanonicalZero => {
            NativeBalanceState::KnownAmount(UnsignedAmount::zero())
        }
        AccountBalanceDisplayState::Unknown | AccountBalanceDisplayState::UnavailableOnFree => {
            NativeBalanceState::KnownAmount(UnsignedAmount::zero())
        }
    };

    Ok(AddressBalanceSummary {
        asset_id,
        confirmed,
    })
}

fn load_address_balance(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    address_id: DigitalAssetAddressId,
    asset_id: SyncedAssetId,
    network: Network,
) -> Result<AddressBalanceSummary, DbError> {
    match account_model_for(asset_id) {
        AccountModel::Utxo => load_utxo_address_balance(conn, asset_id, address_id, network),
        AccountModel::Account => load_account_model_balance(conn, account_id, asset_id, network),
    }
}

pub(in crate::db) fn complete_api_confirmed_balance_with_as_of(
    api_balances: &[AddressApiConfirmedBalanceRow],
) -> Result<Option<(ApiConfirmedBalance, DateTime<Utc>)>, DbError> {
    if api_balances.is_empty() {
        return Ok(None);
    }

    let mut total = UnsignedAmount::zero();
    let mut as_of: Option<DateTime<Utc>> = None;
    for row in api_balances {
        let (Some(balance), Some(last_completed_at)) =
            (row.api_confirmed_balance, row.last_completed_at)
        else {
            return Ok(None);
        };
        total = total.checked_add(balance.amount()).map_err(|err| {
            DbError::new(format!(
                "Overflow while summing api confirmed balance total: {err}"
            ))
        })?;
        as_of = Some(match as_of {
            Some(current) => current.min(last_completed_at),
            None => last_completed_at,
        });
    }

    let Some(as_of) = as_of else {
        return Ok(None);
    };
    Ok(Some((ApiConfirmedBalance::from_amount(total), as_of)))
}

fn load_grouped_utxo_address_balances(
    conn: &rusqlite::Connection,
) -> Result<HashMap<DigitalAssetAddressId, AddressBalanceSummary>, DbError> {
    let mut balances = HashMap::new();
    for (_, address_id, confirmed) in load_bitcoin_provider_address_balances(conn)? {
        balances.insert(
            address_id,
            AddressBalanceSummary {
                asset_id: SyncedAssetId::Bitcoin,
                confirmed,
            },
        );
    }

    Ok(balances)
}

pub(super) fn load_grouped_account_ledger_balances(
    conn: &rusqlite::Connection,
) -> Result<HashMap<DigitalAssetAccountId, AddressBalanceSummary>, DbError> {
    let mut stmt = conn
        .prepare(
            "WITH latest_confirmed AS (
                 SELECT account_id, occurred_at, closing_balance_hi, closing_balance_lo
                 FROM (
                     SELECT account_id,
                            occurred_at,
                            closing_balance_hi,
                            closing_balance_lo,
                            ROW_NUMBER() OVER (
                                PARTITION BY account_id
                                ORDER BY
                                    occurred_at DESC,
                                    COALESCE(block_height, 9223372036854775807) DESC,
                                    COALESCE(nonce, 9223372036854775807) DESC,
                                    COALESCE(min_transfer_index, 9223372036854775807) DESC,
                                    tx_hash DESC
                            ) AS row_num
                     FROM account_transaction_ledger
                     WHERE status = 'confirmed'
                       AND closing_balance_hi IS NOT NULL
                       AND closing_balance_lo IS NOT NULL
                       AND account_id NOT IN (
                           SELECT id
                           FROM digital_asset_accounts
                           WHERE asset_id = 'bitcoin'
                       )
                 )
                 WHERE row_num = 1
             )
             SELECT
                 accounts.id,
                 accounts.asset_id,
                 confirmed.occurred_at,
                 confirmed.closing_balance_hi,
                 confirmed.closing_balance_lo
             FROM digital_asset_accounts AS accounts
             LEFT JOIN latest_confirmed AS confirmed
               ON confirmed.account_id = accounts.id",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare grouped account ledger balances query: {err}"
            ))
        })?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|err| {
            DbError::new(format!(
                "Failed to execute grouped account ledger balances query: {err}"
            ))
        })?;

    let mut balances = HashMap::new();
    let mut bitcoin_balances_by_account: HashMap<DigitalAssetAccountId, Vec<NativeBalanceState>> =
        HashMap::new();
    for (account_id, _, balance) in load_bitcoin_provider_address_balances(conn)? {
        bitcoin_balances_by_account
            .entry(account_id)
            .or_default()
            .push(balance);
    }
    for row_result in rows {
        let (account_id_raw, asset_id_raw, confirmed_occurred_at_raw, confirmed_hi, confirmed_lo) =
            row_result.map_err(|err| {
                DbError::new(format!("Failed to map grouped account balance row: {err}"))
            })?;

        let account_id = DigitalAssetAccountId::from_str(&account_id_raw)
            .map_err(|err| DbError::new(format!("Invalid account id in DB: {err}")))?;
        let asset_id = SyncedAssetId::from_str(&asset_id_raw)
            .ok_or_else(|| DbError::new(format!("Invalid asset_id in DB: {asset_id_raw}")))?;

        let confirmed = if asset_id == SyncedAssetId::Bitcoin {
            match bitcoin_balances_by_account.get(&account_id) {
                Some(states) => checked_sum_native_balances(states)?,
                None => NativeBalanceState::Unknown,
            }
        } else {
            let confirmed_ledger_amount = parse_optional_split_amount(
                confirmed_hi,
                confirmed_lo,
                "confirmed closing_balance",
            )?;
            let confirmed_ledger_as_of = confirmed_occurred_at_raw
                .as_deref()
                .map(parse_datetime)
                .transpose()
                .map_err(|err| {
                    DbError::new(format!("Invalid confirmed occurred_at in DB: {err}"))
                })?;
            let api_confirmed =
                crate::db::transaction_sync::load_api_confirmed_balances_for_account_conn(
                    conn, account_id,
                )
                .and_then(|rows| complete_api_confirmed_balance_with_as_of(&rows))?;
            match resolve_current_account_balance_state(CurrentAccountBalanceInputs {
                ledger_amount: confirmed_ledger_amount,
                ledger_as_of: confirmed_ledger_as_of,
                api_confirmed_amount: api_confirmed.map(|(balance, _)| balance.amount()),
                api_confirmed_as_of: api_confirmed.map(|(_, as_of)| as_of),
                free_balance_unavailable: false,
            }) {
                AccountBalanceDisplayState::KnownLedger { amount, .. }
                | AccountBalanceDisplayState::KnownApiConfirmed { amount, .. } => {
                    NativeBalanceState::KnownAmount(amount)
                }
                AccountBalanceDisplayState::CanonicalZero => {
                    NativeBalanceState::KnownAmount(UnsignedAmount::zero())
                }
                AccountBalanceDisplayState::Unknown
                | AccountBalanceDisplayState::UnavailableOnFree => {
                    NativeBalanceState::KnownAmount(UnsignedAmount::zero())
                }
            }
        };

        balances.insert(
            account_id,
            AddressBalanceSummary {
                asset_id,
                confirmed,
            },
        );
    }

    Ok(balances)
}

fn load_grouped_account_transaction_counts_rows(
    conn: &rusqlite::Connection,
) -> Result<Vec<(DigitalAssetAccountId, String, u32)>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT account_id, status, COUNT(*)
             FROM account_transaction_ledger
             GROUP BY account_id, status",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare grouped transaction counts query: {err}"
            ))
        })?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
            ))
        })
        .map_err(|err| {
            DbError::new(format!(
                "Failed to execute grouped transaction counts query: {err}"
            ))
        })?;

    let mut grouped_rows = Vec::new();
    for row_result in rows {
        let (account_id_raw, status_raw, count) = row_result.map_err(|err| {
            DbError::new(format!(
                "Failed to map grouped transaction counts row: {err}"
            ))
        })?;
        let account_id = DigitalAssetAccountId::from_str(&account_id_raw)
            .map_err(|err| DbError::new(format!("Invalid account id in DB: {err}")))?;
        grouped_rows.push((account_id, status_raw, count));
    }

    Ok(grouped_rows)
}

pub(crate) fn load_wallet_summary_address_balances(
    conn: &rusqlite::Connection,
    accounts: &[AccountWithHdKeys],
    account_balances: &HashMap<DigitalAssetAccountId, AddressBalanceSummary>,
) -> Result<HashMap<String, AddressBalanceSummary>, DbError> {
    let utxo_balances = load_grouped_utxo_address_balances(conn)?;

    let mut address_balances = HashMap::new();
    for account in accounts {
        match account.account_model {
            AccountModel::Utxo => {
                for address in &account.addresses {
                    let summary = match utxo_balances.get(&address.id).cloned() {
                        Some(balance) => balance,
                        None => load_utxo_address_balance(
                            conn,
                            account.asset_id,
                            address.id,
                            account.network,
                        )?,
                    };
                    address_balances.insert(address.address.clone(), summary);
                }
            }
            AccountModel::Account => {
                let summary = match account_balances.get(&account.id) {
                    Some(balance) => balance.clone(),
                    None => load_account_model_balance(
                        conn,
                        account.id,
                        account.asset_id,
                        account.network,
                    )?,
                };
                for address in &account.addresses {
                    address_balances.insert(address.address.clone(), summary.clone());
                }
            }
        }
    }

    Ok(address_balances)
}

pub(crate) fn load_wallet_summary_transaction_counts(
    conn: &rusqlite::Connection,
    account_ids: &[DigitalAssetAccountId],
) -> Result<HashMap<DigitalAssetAccountId, AccountTransactionCounts>, DbError> {
    let mut counts_by_account = account_ids
        .iter()
        .copied()
        .map(|account_id| (account_id, AccountTransactionCounts::default()))
        .collect::<HashMap<_, _>>();

    for (account_id, status_raw, count) in load_grouped_account_transaction_counts_rows(conn)? {
        let Some(counts) = counts_by_account.get_mut(&account_id) else {
            continue;
        };

        match status_raw.as_str() {
            "pending" => counts.pending = count,
            "confirmed" => counts.confirmed = count,
            "dropped" => counts.dropped = count,
            "failed" => counts.failed = count,
            _ => {
                return Err(DbError::new(format!(
                    "Invalid status in account_transaction_ledger: {status_raw}"
                )));
            }
        }
    }

    Ok(counts_by_account)
}

pub(crate) fn load_all_account_balances(
    user_id: UserId,
) -> Result<AccountBalancesResponse, DbError> {
    with_user_db(user_id, |conn| {
        // Load all accounts
        let mut account_stmt = conn
            .prepare(
                "SELECT id, wallet_id, asset_id, network, label, created_at
                 FROM digital_asset_accounts
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|err| DbError::new(format!("Failed to prepare accounts query: {err}")))?;

        let account_rows = account_stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|err| DbError::new(format!("Failed to execute accounts query: {err}")))?;

        let mut accounts = Vec::new();
        for row_result in account_rows {
            let (id_raw, wallet_id_raw, asset_id_raw, network_raw, label, created_at_raw) =
                row_result
                    .map_err(|err| DbError::new(format!("Failed to map account row: {err}")))?;

            let account_id = DigitalAssetAccountId::from_str(&id_raw)
                .map_err(|err| DbError::new(format!("Invalid account id in DB: {err}")))?;
            let wallet_id = wallet_id_raw
                .as_deref()
                .map(WalletId::from_str)
                .transpose()
                .map_err(|err| DbError::new(format!("Invalid wallet id in DB: {err}")))?;
            let asset_id = SyncedAssetId::from_str(&asset_id_raw)
                .ok_or_else(|| DbError::new(format!("Invalid asset_id in DB: {asset_id_raw}")))?;
            let network = Network::from_str(&network_raw)
                .ok_or_else(|| DbError::new(format!("Invalid network in DB: {network_raw}")))?;
            let asset_linked_at = parse_datetime(&created_at_raw)
                .map_err(|err| DbError::new(format!("Invalid account created_at in DB: {err}")))?;

            // Load addresses for this account
            let mut addr_stmt = conn
                .prepare(
                    "SELECT id, address, derivation_change, derivation_index
                     FROM digital_asset_addresses
                     WHERE account_id = ?1
                     ORDER BY derivation_change ASC, derivation_index ASC, id ASC",
                )
                .map_err(|err| DbError::new(format!("Failed to prepare addresses query: {err}")))?;

            let addr_rows = addr_stmt
                .query_map(params![account_id.to_string()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                })
                .map_err(|err| DbError::new(format!("Failed to execute addresses query: {err}")))?;

            let mut address_entries = Vec::new();
            let mut address_balances = Vec::new();

            for addr_result in addr_rows {
                let (addr_id_raw, address_raw, derivation_change_raw, derivation_index_raw) =
                    addr_result
                        .map_err(|err| DbError::new(format!("Failed to map address row: {err}")))?;

                let addr_id = DigitalAssetAddressId::from_str(&addr_id_raw)
                    .map_err(|err| DbError::new(format!("Invalid address id in DB: {err}")))?;
                let address = TrackedAddress::parse(&address_raw)
                    .map_err(|err| DbError::new(format!("Invalid address in DB: {err}")))?;
                let derivation_change = derivation_change_raw
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|err| {
                        DbError::new(format!("Invalid derivation_change in DB: {err}"))
                    })?;
                let derivation_index = derivation_index_raw
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|err| {
                        DbError::new(format!("Invalid derivation_index in DB: {err}"))
                    })?;

                let balance = load_address_balance(conn, account_id, addr_id, asset_id, network)?;
                address_balances.push(balance.clone());

                address_entries.push(AddressBalanceEntry {
                    address_id: addr_id,
                    address,
                    derivation_change,
                    derivation_index,
                    balance,
                });
            }

            let account_balance =
                sum_address_balances(asset_id, &address_balances).map_err(|err| {
                    DbError::new(format!(
                        "Failed to compute account balance for {account_id}: {err}"
                    ))
                })?;

            accounts.push(AccountBalanceEntry {
                wallet_id,
                account_id,
                asset_id,
                network,
                asset_linked_at,
                account_label: label,
                account_balance,
                addresses: address_entries,
            });
        }

        let totals = aggregate_address_balances(&accounts).map_err(|err| {
            DbError::new(format!(
                "Failed to aggregate account balances across assets: {err}"
            ))
        })?;

        Ok(AccountBalancesResponse { accounts, totals })
    })
}

fn parse_tx_type_to_direction(raw: &str) -> Result<AccountTransactionDirection, DbError> {
    match raw {
        "receive" => Ok(AccountTransactionDirection::Incoming),
        "send" => Ok(AccountTransactionDirection::Outgoing),
        "self_transfer" => Ok(AccountTransactionDirection::SelfTransfer),
        _ => Err(DbError::new(format!("Invalid tx_type in DB: {raw}"))),
    }
}

fn parse_first_address_from_json(
    raw_json: &str,
    field: &'static str,
) -> Result<Option<TrackedAddress>, DbError> {
    let parsed: Vec<String> = serde_json::from_str(raw_json)
        .map_err(|err| DbError::new(format!("Failed to parse {field} JSON: {err}")))?;
    match parsed.into_iter().find(|v| !v.trim().is_empty()) {
        Some(addr) => TrackedAddress::parse(&addr)
            .map(Some)
            .map_err(|err| DbError::new(format!("Invalid address in {field}: {err}"))),
        None => Ok(None),
    }
}

type LedgerHistoryRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
);

fn ledger_history_entry_from_row(
    account_id: DigitalAssetAccountId,
    row: LedgerHistoryRow,
) -> Result<AccountTransactionEntry, DbError> {
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
    ) = row;

    if tx_hash.trim().is_empty() {
        return Err(DbError::new(
            "Invalid transaction history row: empty tx_hash",
        ));
    }

    let status = ChainTransactionStatus::from_db_value(&status_raw)
        .ok_or_else(|| DbError::new(format!("Invalid transaction status in DB: {status_raw}")))?;
    let direction = parse_tx_type_to_direction(&tx_type_raw)?;
    let block_time = Some(
        parse_datetime(&occurred_at_raw)
            .map_err(|err| DbError::new(format!("Invalid occurred_at in DB: {err}")))?,
    );

    let value = parse_split_amount(value_hi, value_lo, "value_amount")?;
    let fee = match direction {
        AccountTransactionDirection::Outgoing | AccountTransactionDirection::SelfTransfer => Some(
            parse_optional_split_amount_or_zero(fee_hi, fee_lo, "fee_amount")?,
        ),
        AccountTransactionDirection::Incoming => None,
    };

    let from_address = parse_first_address_from_json(&from_addresses_json, "from_addresses_json")?;
    let to_address = parse_first_address_from_json(&to_addresses_json, "to_addresses_json")?;

    Ok(AccountTransactionEntry {
        account_id,
        tx_hash,
        status,
        direction,
        transfer_kind: None,
        value,
        fee,
        from_address,
        to_address,
        block_time,
    })
}

fn load_ledger_transaction_history(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
    asset_id: SyncedAssetId,
) -> Result<Vec<AccountTransactionEntry>, DbError> {
    let account_id_raw = account_id.to_string();
    let preview_limit = i64::from(WALLET_HISTORY_PREVIEW_LIMIT);
    let mut entries = Vec::new();

    // Keep status groups in separate queries so each ORDER BY can use its paging index.
    let mut pending_stmt = conn
        .prepare(
            "SELECT tx_hash, status, tx_type, occurred_at,
                    from_addresses_json, to_addresses_json,
                    value_amount_hi, value_amount_lo,
                    fee_amount_hi, fee_amount_lo
             FROM account_transaction_ledger INDEXED BY idx_account_tx_ledger_pending_page
             WHERE account_id = ?1
               AND status IN ('pending', 'dropped', 'failed')
             ORDER BY
                first_seen_at DESC,
                COALESCE(nonce, 9223372036854775807) DESC,
                tx_hash DESC
             LIMIT ?2",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare {} non-confirmed transaction history query: {err}",
                asset_id.as_str()
            ))
        })?;

    let pending_rows = pending_stmt
        .query_map(params![&account_id_raw, preview_limit], |row| {
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
            ))
        })
        .map_err(|err| {
            DbError::new(format!(
                "Failed to execute {} non-confirmed transaction history query: {err}",
                asset_id.as_str()
            ))
        })?;

    for row_result in pending_rows {
        let row = row_result.map_err(|err| {
            DbError::new(format!(
                "Failed to map {} non-confirmed transaction history row: {err}",
                asset_id.as_str()
            ))
        })?;
        entries.push(ledger_history_entry_from_row(account_id, row)?);
    }

    let entries_len = i64::try_from(entries.len())
        .map_err(|_| DbError::new("Transaction history row count exceeds i64 range"))?;
    let remaining = preview_limit.saturating_sub(entries_len);
    if remaining == 0 {
        return Ok(entries);
    }

    let mut confirmed_stmt = conn
        .prepare(
            "SELECT tx_hash, status, tx_type, occurred_at,
                    from_addresses_json, to_addresses_json,
                    value_amount_hi, value_amount_lo,
                    fee_amount_hi, fee_amount_lo
             FROM account_transaction_ledger INDEXED BY idx_account_tx_ledger_confirmed_page
             WHERE account_id = ?1
               AND status = 'confirmed'
             ORDER BY
                occurred_at DESC,
                COALESCE(block_height, 9223372036854775807) DESC,
                COALESCE(nonce, 9223372036854775807) DESC,
                COALESCE(min_transfer_index, 9223372036854775807) DESC,
                tx_hash DESC
             LIMIT ?2",
        )
        .map_err(|err| {
            DbError::new(format!(
                "Failed to prepare {} confirmed transaction history query: {err}",
                asset_id.as_str()
            ))
        })?;

    let confirmed_rows = confirmed_stmt
        .query_map(params![&account_id_raw, remaining], |row| {
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
            ))
        })
        .map_err(|err| {
            DbError::new(format!(
                "Failed to execute {} confirmed transaction history query: {err}",
                asset_id.as_str()
            ))
        })?;

    for row_result in confirmed_rows {
        let row = row_result.map_err(|err| {
            DbError::new(format!(
                "Failed to map {} confirmed transaction history row: {err}",
                asset_id.as_str()
            ))
        })?;
        entries.push(ledger_history_entry_from_row(account_id, row)?);
    }

    Ok(entries)
}

pub(crate) fn load_account_transaction_history(
    user_id: UserId,
) -> Result<HashMap<DigitalAssetAccountId, Vec<AccountTransactionEntry>>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, asset_id, network
                 FROM digital_asset_accounts
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare account transaction history query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to execute account transaction history query: {err}"
                ))
            })?;

        let mut by_account = HashMap::new();
        for row_result in rows {
            let (account_id_raw, asset_id_raw, network_raw) = row_result
                .map_err(|err| DbError::new(format!("Failed to map account history row: {err}")))?;

            let account_id = DigitalAssetAccountId::from_str(&account_id_raw)
                .map_err(|err| DbError::new(format!("Invalid account id in DB: {err}")))?;
            let asset_id = SyncedAssetId::from_str(&asset_id_raw)
                .ok_or_else(|| DbError::new(format!("Invalid asset_id in DB: {asset_id_raw}")))?;
            let _network = Network::from_str(&network_raw)
                .ok_or_else(|| DbError::new(format!("Invalid network in DB: {network_raw}")))?;

            let entries = load_ledger_transaction_history(conn, account_id, asset_id)?;
            by_account.insert(account_id, entries);
        }

        Ok(by_account)
    })
}

/// Count transactions per status for each account, queried directly from
/// the database without any row limit.
pub(crate) fn load_account_transaction_counts(
    user_id: UserId,
) -> Result<HashMap<DigitalAssetAccountId, AccountTransactionCounts>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, asset_id, network
                 FROM digital_asset_accounts
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to prepare account transaction counts query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to execute account transaction counts query: {err}"
                ))
            })?;

        let mut account_ids = Vec::new();
        for row_result in rows {
            let (account_id_raw, _asset_id_raw, _network_raw) = row_result
                .map_err(|err| DbError::new(format!("Failed to map account counts row: {err}")))?;

            let account_id = DigitalAssetAccountId::from_str(&account_id_raw)
                .map_err(|err| DbError::new(format!("Invalid account id in DB: {err}")))?;
            account_ids.push(account_id);
        }

        load_wallet_summary_transaction_counts(conn, &account_ids)
    })
}

#[cfg(all(test, feature = "db-tests"))]
fn insert_account_balances_fixture_internal(
    conn: &mut rusqlite::Connection,
) -> Result<(DigitalAssetAccountId, DigitalAssetAddressId), DbError> {
    let now = "2026-02-12T10:00:00Z";
    let fixture_time = parse_datetime(now)
        .map_err(|err| DbError::new(format!("Invalid bitcoin fixture time: {err}")))?;
    let wallet_id = crate::wallets::WalletId::new();
    let account_id = DigitalAssetAccountId::new();
    let address_id = DigitalAssetAddressId::new();
    let address_value = "bc1qmvpf5f22q6hjzm6p8m0mdx2zv2h9d6q6k2u8s5";

    let tx = conn.transaction().map_err(|err| {
        DbError::new(format!(
            "Failed to start account balances fixture tx: {err}"
        ))
    })?;
    tx.execute(
        "INSERT INTO wallets \
         (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            wallet_id.to_string(),
            "Test Wallet",
            "test wallet",
            Option::<String>::None,
            "user_provided",
            Option::<String>::None,
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert wallet fixture: {err}")))?;

    tx.execute(
        "INSERT INTO digital_asset_accounts
         (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            account_id.to_string(),
            wallet_id.to_string(),
            "Bitcoin Account 1",
            "bitcoin account 1",
            SyncedAssetId::Bitcoin.as_str(),
            Network::Mainnet.as_str(),
            "hd_pubkey",
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert account fixture: {err}")))?;

    tx.execute(
        "INSERT INTO digital_asset_addresses
         (id, account_id, asset_id, network, address, address_normalized, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            address_id.to_string(),
            account_id.to_string(),
            SyncedAssetId::Bitcoin.as_str(),
            Network::Mainnet.as_str(),
            address_value,
            address_value,
            "native_segwit",
            Some(0_i64),
            Some(0_i64),
            "derived",
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert address fixture: {err}")))?;
    ensure_source_connection_for_address_tx(
        &tx,
        address_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        address_value,
        fixture_time,
    )?;

    let tx_pending_in_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let tx_confirmed_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    // Pending UTXO: 20k sats
    tx.execute(
        "INSERT INTO utxos
         (id, asset_id, network, tx_hash, output_index, address_id, value_amount_hi, value_amount_lo, status, replaced_by_tx_hash, spent_by_tx_hash, spent_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            Ulid::new().to_string(),
            SyncedAssetId::Bitcoin.as_str(),
            Network::Mainnet.as_str(),
            tx_pending_in_hash,
            0_i64,
            address_id.to_string(),
            0_i64,
            20_000_i64,
            "pending",
            Option::<String>::None,
            Option::<String>::None,
            Option::<String>::None,
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert pending UTXO: {err}")))?;

    // Confirmed UTXO: 30k sats
    tx.execute(
        "INSERT INTO utxos
         (id, asset_id, network, tx_hash, output_index, address_id, value_amount_hi, value_amount_lo, status, replaced_by_tx_hash, spent_by_tx_hash, spent_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            Ulid::new().to_string(),
            SyncedAssetId::Bitcoin.as_str(),
            Network::Mainnet.as_str(),
            tx_confirmed_hash,
            0_i64,
            address_id.to_string(),
            0_i64,
            30_000_i64,
            "confirmed",
            Option::<String>::None,
            Option::<String>::None,
            Option::<String>::None,
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert confirmed UTXO: {err}")))?;

    tx.commit().map_err(|err| {
        DbError::new(format!(
            "Failed to commit bitcoin account balances fixture transaction: {err}"
        ))
    })?;

    Ok((account_id, address_id))
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn seed_account_balances_fixture(
    user_id: UserId,
) -> Result<(DigitalAssetAccountId, DigitalAssetAddressId), DbError> {
    with_user_db_mut(user_id, |conn| {
        insert_account_balances_fixture_internal(conn)
    })
}

#[cfg(all(test, feature = "db-tests"))]
fn insert_ethereum_account_balances_fixture_internal(
    conn: &mut rusqlite::Connection,
) -> Result<(DigitalAssetAccountId, DigitalAssetAddressId), DbError> {
    let now = "2026-02-12T10:00:00Z";
    let fixture_time = parse_datetime(now)
        .map_err(|err| DbError::new(format!("Invalid ethereum fixture time: {err}")))?;
    let wallet_id = crate::wallets::WalletId::new();
    let account_id = DigitalAssetAccountId::new();
    let address_id = DigitalAssetAddressId::new();
    let address_value = "0x52908400098527886E0F7030069857D2E4169EE7";
    let normalized = "0x52908400098527886e0f7030069857d2e4169ee7";
    let incoming_hash = "1111111111111111111111111111111111111111111111111111111111111111";
    let outgoing_hash = "2222222222222222222222222222222222222222222222222222222222222222";
    let pending_hash = "3333333333333333333333333333333333333333333333333333333333333333";

    let tx = conn.transaction().map_err(|err| {
        DbError::new(format!(
            "Failed to start ethereum account balances fixture tx: {err}"
        ))
    })?;
    tx.execute(
        "INSERT INTO wallets \
         (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            wallet_id.to_string(),
            "ETH Test Wallet",
            "eth test wallet",
            Option::<String>::None,
            "user_provided",
            Option::<String>::None,
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert wallet fixture: {err}")))?;

    tx.execute(
        "INSERT INTO digital_asset_accounts
         (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            account_id.to_string(),
            wallet_id.to_string(),
            "Ethereum Account 1",
            "ethereum account 1",
            SyncedAssetId::Ethereum.as_str(),
            Network::Mainnet.as_str(),
            "single_address",
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert Ethereum account fixture: {err}")))?;

    tx.execute(
        "INSERT INTO digital_asset_addresses
         (id, account_id, asset_id, network, address, address_normalized, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            address_id.to_string(),
            account_id.to_string(),
            SyncedAssetId::Ethereum.as_str(),
            Network::Mainnet.as_str(),
            address_value,
            normalized,
            "standard",
            Option::<i64>::None,
            Option::<i64>::None,
            "user_provided",
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert Ethereum address fixture: {err}")))?;
    ensure_source_connection_for_address_tx(
        &tx,
        address_id,
        SyncedAssetId::Ethereum,
        Network::Mainnet,
        normalized,
        fixture_time,
    )?;

    let incoming_tx_id = Ulid::new().to_string();
    let outgoing_tx_id = Ulid::new().to_string();
    let pending_tx_id = Ulid::new().to_string();

    tx.execute(
        "INSERT INTO chain_transactions
         (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            incoming_tx_id,
            SyncedAssetId::Ethereum.as_str(),
            Network::Mainnet.as_str(),
            incoming_hash,
            "confirmed",
            Some(100_i64),
            Some("blockhash-100".to_string()),
            Some("2026-02-12T10:00:00Z".to_string()),
            Some(0_i64),
            Some(0_i64),
            Option::<i64>::None,
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert incoming chain tx fixture: {err}")))?;

    tx.execute(
        "INSERT INTO chain_transactions
         (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            outgoing_tx_id,
            SyncedAssetId::Ethereum.as_str(),
            Network::Mainnet.as_str(),
            outgoing_hash,
            "confirmed",
            Some(105_i64),
            Some("blockhash-105".to_string()),
            Some("2026-02-12T10:05:00Z".to_string()),
            Some(10_000_000_000_000_000_i64),
            Some(0_i64),
            Some(7_i64),
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert outgoing chain tx fixture: {err}")))?;

    tx.execute(
        "INSERT INTO chain_transactions
         (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            pending_tx_id,
            SyncedAssetId::Ethereum.as_str(),
            Network::Mainnet.as_str(),
            pending_hash,
            "pending",
            Option::<i64>::None,
            Option::<String>::None,
            Option::<String>::None,
            Some(0_i64),
            Some(0_i64),
            Some(8_i64),
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert pending chain tx fixture: {err}")))?;

    tx.execute(
        "INSERT INTO account_transfers
         (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, from_address, from_address_id, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'legacy:' || ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            Ulid::new().to_string(),
            incoming_tx_id,
            SyncedAssetId::Ethereum.as_str(),
            Network::Mainnet.as_str(),
            incoming_hash,
            0_i64,
            "normal",
            "0x1111111111111111111111111111111111111111",
            Option::<String>::None,
            address_value,
            Some(address_id.to_string()),
            2_i64,
            0_i64,
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert incoming transfer fixture: {err}")))?;

    tx.execute(
        "INSERT INTO account_transfers
         (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, from_address, from_address_id, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'legacy:' || ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            Ulid::new().to_string(),
            outgoing_tx_id,
            SyncedAssetId::Ethereum.as_str(),
            Network::Mainnet.as_str(),
            outgoing_hash,
            0_i64,
            "normal",
            address_value,
            Some(address_id.to_string()),
            "0x2222222222222222222222222222222222222222",
            Option::<String>::None,
            1_i64,
            0_i64,
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert outgoing transfer fixture: {err}")))?;

    tx.execute(
        "INSERT INTO account_transfers
         (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, from_address, from_address_id, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'legacy:' || ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            Ulid::new().to_string(),
            pending_tx_id,
            SyncedAssetId::Ethereum.as_str(),
            Network::Mainnet.as_str(),
            pending_hash,
            0_i64,
            "internal",
            "0x3333333333333333333333333333333333333333",
            Option::<String>::None,
            address_value,
            Some(address_id.to_string()),
            0_i64,
            500_000_000_000_000_000_i64,
            now,
            now,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert pending transfer fixture: {err}")))?;

    tx.commit().map_err(|err| {
        DbError::new(format!(
            "Failed to commit ethereum account balances fixture transaction: {err}"
        ))
    })?;

    Ok((account_id, address_id))
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn seed_ethereum_account_balances_fixture(
    user_id: UserId,
) -> Result<(DigitalAssetAccountId, DigitalAssetAddressId), DbError> {
    let (account_id, address_id) = with_user_db_mut(user_id, |conn| {
        insert_ethereum_account_balances_fixture_internal(conn)
    })?;
    // Rebuild the ledger so that ledger-based balance, history, and count queries work.
    crate::db::account_transactions::rebuild_account_transaction_ledger(
        user_id,
        account_id,
        chrono::Utc::now(),
    )?;
    Ok((account_id, address_id))
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::{acquire_test_runtime, initialize_user_db_for_test};
    use crate::transactions::TxHash;
    use crate::wallets::{BtcAddress, Label, RawBtcAddress, WALLET_LABEL_MAX_LENGTH};

    fn seed_bitcoin_transaction_history_fixture(
        user_id: UserId,
    ) -> Result<DigitalAssetAccountId, DbError> {
        let now = crate::models::parse_datetime("2026-02-12T10:00:00Z")
            .map_err(|err| DbError::new(format!("Invalid bitcoin fixture timestamp: {err}")))?;
        let wallet_label =
            Label::parse_with_limit("Bitcoin History Wallet", WALLET_LABEL_MAX_LENGTH).map_err(
                |err| DbError::new(format!("Invalid bitcoin fixture wallet label: {err}")),
            )?;
        let raw_address =
            RawBtcAddress::new("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string());
        let owned_address = BtcAddress::parse(&raw_address, Network::Mainnet)
            .map_err(|err| DbError::new(format!("Invalid bitcoin fixture address: {err}")))?;
        let account = crate::db::add_bitcoin_address(
            user_id,
            &owned_address,
            Network::Mainnet,
            None,
            Some(&wallet_label),
            now,
        )?;
        let owned_tracked = TrackedAddress::parse(owned_address.canonical()).map_err(|err| {
            DbError::new(format!("Invalid tracked bitcoin fixture address: {err}"))
        })?;
        let incoming_hash =
            TxHash::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .map_err(|err| DbError::new(format!("Invalid incoming tx hash fixture: {err}")))?;
        let pending_hash =
            TxHash::parse("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
                .map_err(|err| DbError::new(format!("Invalid pending tx hash fixture: {err}")))?;
        let records = vec![
            crate::db::SyncTransactionRecord {
                tx_hash: incoming_hash.clone(),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(100_i64),
                block_hash: Some("blockhash-100".to_string()),
                block_time: Some(now),
                fee_amount: Some(200_i64),
                inputs: Vec::new(),
                outputs: vec![crate::db::SyncTransactionOutputRecord {
                    output_index: 0_i64,
                    raw_address: Some(owned_tracked.clone()),
                    script_pubkey_hex: "0014deadbeef".to_string(),
                    value_amount: 50_000_i64,
                }],
            },
            crate::db::SyncTransactionRecord {
                tx_hash: pending_hash.clone(),
                status: ChainTransactionStatus::Pending,
                block_height: None,
                block_hash: None,
                block_time: None,
                fee_amount: Some(250_i64),
                inputs: vec![crate::db::SyncTransactionInputRecord {
                    input_index: 0_i64,
                    prev_tx_hash: incoming_hash,
                    prev_output_index: 0_i64,
                    prev_address: Some(owned_tracked),
                    value_amount: Some(50_000_i64),
                }],
                outputs: Vec::new(),
            },
        ];
        crate::db::reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &records,
            now,
        )?;
        crate::db::rebuild_account_transaction_ledger(user_id, account.account_id, now)?;
        Ok(account.account_id)
    }

    #[test]
    fn load_all_account_balances_returns_empty_when_no_accounts() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");

        let result = load_all_account_balances(user_id).expect("query should succeed");
        assert!(result.accounts.is_empty());
        assert!(result.totals.is_empty());
    }

    #[test]
    fn load_all_account_balances_preserves_unknown_without_provider_balance() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (expected_account_id, expected_address_id) =
            seed_account_balances_fixture(user_id).expect("fixture should insert");

        let result = load_all_account_balances(user_id).expect("query should succeed");
        assert_eq!(result.accounts.len(), 1);

        let account = &result.accounts[0];
        assert_eq!(account.account_id, expected_account_id);
        assert_eq!(account.asset_id, SyncedAssetId::Bitcoin);
        assert_eq!(account.network, Network::Mainnet);
        assert_eq!(
            account.account_balance.confirmed,
            NativeBalanceState::Unknown
        );

        assert_eq!(account.addresses.len(), 1);
        let addr = &account.addresses[0];
        assert_eq!(addr.address_id, expected_address_id);
        assert_eq!(
            addr.address.as_str(),
            "bc1qmvpf5f22q6hjzm6p8m0mdx2zv2h9d6q6k2u8s5"
        );
        assert_eq!(addr.balance.confirmed, NativeBalanceState::Unknown);

        assert_eq!(result.totals.len(), 1);
        assert_eq!(result.totals[0].confirmed, NativeBalanceState::Unknown);
    }

    #[test]
    fn load_all_account_balances_returns_expected_ethereum_balances() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (expected_account_id, expected_address_id) =
            seed_ethereum_account_balances_fixture(user_id).expect("fixture should insert");

        let result = load_all_account_balances(user_id).expect("query should succeed");
        assert_eq!(result.accounts.len(), 1);

        let account = &result.accounts[0];
        assert_eq!(account.account_id, expected_account_id);
        assert_eq!(account.asset_id, SyncedAssetId::Ethereum);
        assert_eq!(account.network, Network::Mainnet);
        assert_eq!(
            account.account_balance.confirmed,
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(
                990_000_000_000_000_000_u128
            ))
        );

        assert_eq!(account.addresses.len(), 1);
        let addr = &account.addresses[0];
        assert_eq!(addr.address_id, expected_address_id);
        assert_eq!(
            addr.address.as_str(),
            "0x52908400098527886E0F7030069857D2E4169EE7"
        );
        assert_eq!(
            addr.balance.confirmed,
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(
                990_000_000_000_000_000_u128
            ))
        );

        assert_eq!(result.totals.len(), 1);
        assert_eq!(
            result.totals[0].confirmed,
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(
                990_000_000_000_000_000_u128
            ))
        );
    }

    #[test]
    fn load_grouped_account_ledger_balances_returns_account_level_balances() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (account_id, _address_id) =
            seed_ethereum_account_balances_fixture(user_id).expect("fixture should insert");

        let balances = with_user_db(user_id, load_grouped_account_ledger_balances)
            .expect("query should succeed");

        let balance = balances
            .get(&account_id)
            .expect("account should have grouped ledger balances");
        assert_eq!(
            balance.confirmed,
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(
                990_000_000_000_000_000_u128
            ))
        );
    }

    #[test]
    fn load_grouped_account_ledger_balances_returns_unknown_without_provider_balance() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (account_id, _address_id) =
            seed_account_balances_fixture(user_id).expect("fixture should insert");

        let balances = with_user_db(user_id, load_grouped_account_ledger_balances)
            .expect("query should succeed");

        let balance = balances
            .get(&account_id)
            .expect("account should be present even without ledger entries");
        assert_eq!(balance.confirmed, NativeBalanceState::Unknown);
    }

    #[test]
    fn current_bitcoin_provider_balance_overrides_old_ledger_closing() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (account_id, address_id) =
            seed_account_balances_fixture(user_id).expect("fixture should insert");
        with_user_db_mut(user_id, |conn| {
            let chain_transaction_id = Ulid::new().to_string();
            let tx_hash = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
            conn.execute(
                "INSERT INTO chain_transactions
                 (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time,
                  fee_amount_hi, fee_amount_lo, nonce, created_at, updated_at)
                 VALUES (?1, 'bitcoin', 'mainnet', ?2, 'confirmed', 1, 'block-1', ?3,
                         0, 0, NULL, ?3, ?3)",
                params![chain_transaction_id, tx_hash, "2026-02-12T10:00:00Z",],
            )
            .map_err(|err| DbError::new(format!("old chain fixture failed: {err}")))?;
            conn.execute(
                "INSERT INTO account_transaction_ledger
                 (id, account_id, chain_transaction_id, asset_id, network, tx_hash, status,
                  occurred_at, first_seen_at, block_height, nonce, min_transfer_index, tx_type,
                  from_addresses_json, to_addresses_json, value_amount_hi, value_amount_lo,
                  fee_amount_hi, fee_amount_lo, closing_balance_hi, closing_balance_lo,
                  created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'bitcoin', 'mainnet', ?4, 'confirmed', ?5, ?5, 1,
                         NULL, NULL, 'receive', '[]', '[]', 0, 1, 0, 0, 0, 12345, ?5, ?5)",
                params![
                    Ulid::new().to_string(),
                    account_id.to_string(),
                    chain_transaction_id,
                    tx_hash,
                    "2026-02-12T10:00:00Z",
                ],
            )
            .map(|_| ())
            .map_err(|err| DbError::new(format!("old ledger fixture failed: {err}")))
        })
        .expect("old ledger fixture should persist");
        let run_id = crate::transactions::TransactionSyncRunId::new();
        let started = parse_datetime("2026-05-03T18:00:00Z").expect("valid started_at");
        let completed = parse_datetime("2026-05-03T18:02:05Z").expect("valid completed_at");
        let balance =
            ApiConfirmedBalance::from_amount(UnsignedAmount::from_u128(2_441_190_093_160));

        crate::db::mark_address_sync_started(user_id, address_id, run_id, started)
            .expect("sync start should persist");
        crate::db::mark_address_sync_completed_success(
            user_id,
            &crate::db::AddressSyncSuccess {
                address_id,
                run_id,
                started_at: started,
                completed_at: completed,
                last_tip_height: crate::transactions::ChainTipHeight::try_new(1)
                    .expect("tip should be valid"),
                new_tx_count: crate::transactions::TransactionCount::zero(),
                updated_tx_count: crate::transactions::TransactionCount::zero(),
                api_confirmed_balance: Some(balance),
            },
        )
        .expect("sync success should persist");

        let balances = with_user_db(user_id, load_grouped_account_ledger_balances)
            .expect("query should succeed");

        let balance = balances
            .get(&account_id)
            .expect("account should be present with api balance");
        assert_eq!(
            balance.confirmed,
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(2_441_190_093_160))
        );
    }

    fn record_current_bitcoin_provider_balance(
        user_id: UserId,
        address_id: DigitalAssetAddressId,
        amount: u128,
    ) {
        let run_id = crate::transactions::TransactionSyncRunId::new();
        let started = parse_datetime("2026-05-03T18:00:00Z").expect("valid started_at");
        let completed = parse_datetime("2026-05-03T18:02:05Z").expect("valid completed_at");
        crate::db::mark_address_sync_started(user_id, address_id, run_id, started)
            .expect("sync start should persist");
        crate::db::mark_address_sync_completed_success(
            user_id,
            &crate::db::AddressSyncSuccess {
                address_id,
                run_id,
                started_at: started,
                completed_at: completed,
                last_tip_height: crate::transactions::ChainTipHeight::try_new(1)
                    .expect("tip should be valid"),
                new_tx_count: crate::transactions::TransactionCount::zero(),
                updated_tx_count: crate::transactions::TransactionCount::zero(),
                api_confirmed_balance: Some(ApiConfirmedBalance::from_amount(
                    UnsignedAmount::from_u128(amount),
                )),
            },
        )
        .expect("sync success should persist");
    }

    fn assert_current_balance(
        balance: &AddressBalanceSummary,
        expected: crate::transactions::NativeBalanceState,
    ) {
        assert_eq!(balance.confirmed, expected);
    }

    #[test]
    fn current_bitcoin_provider_balance_drives_every_loader_and_excludes_pending() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (account_id, address_id) =
            seed_account_balances_fixture(user_id).expect("fixture should insert");
        record_current_bitcoin_provider_balance(user_id, address_id, 75_000);

        with_user_db(user_id, |conn| {
            assert_current_balance(
                &load_utxo_address_balance(
                    conn,
                    SyncedAssetId::Bitcoin,
                    address_id,
                    Network::Mainnet,
                )?,
                crate::transactions::NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(
                    75_000,
                )),
            );
            assert_current_balance(
                load_grouped_utxo_address_balances(conn)?
                    .get(&address_id)
                    .expect("address should be grouped"),
                crate::transactions::NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(
                    75_000,
                )),
            );
            assert_current_balance(
                load_grouped_account_ledger_balances(conn)?
                    .get(&account_id)
                    .expect("account should be grouped"),
                crate::transactions::NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(
                    75_000,
                )),
            );
            let pending_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM utxos WHERE address_id = ?1 AND status = 'pending'",
                    params![address_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|err| DbError::new(format!("pending fixture query failed: {err}")))?;
            assert_eq!(pending_count, 1);
            Ok::<(), DbError>(())
        })
        .expect("focused loaders should succeed");

        let accounts = crate::db::list_wallets(user_id)
            .expect("wallets should load")
            .into_iter()
            .flat_map(|wallet| wallet.accounts)
            .collect::<Vec<_>>();
        with_user_db(user_id, |conn| {
            let account_balances = load_grouped_account_ledger_balances(conn)?;
            let wallet_balances =
                load_wallet_summary_address_balances(conn, &accounts, &account_balances)?;
            assert_current_balance(
                wallet_balances
                    .get("bc1qmvpf5f22q6hjzm6p8m0mdx2zv2h9d6q6k2u8s5")
                    .expect("wallet address should have a balance"),
                crate::transactions::NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(
                    75_000,
                )),
            );
            Ok::<(), DbError>(())
        })
        .expect("wallet loader should succeed");
    }

    #[test]
    fn current_bitcoin_provider_balance_requires_every_address() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (account_id, address_id) =
            seed_account_balances_fixture(user_id).expect("fixture should insert");
        record_current_bitcoin_provider_balance(user_id, address_id, 75_000);
        let missing_address_id = DigitalAssetAddressId::new();
        let missing_address = "bc1qmissingprovider000000000000000000000000000";

        with_user_db_mut(user_id, |conn| {
            conn.execute(
                "INSERT INTO digital_asset_addresses
                 (id, account_id, asset_id, network, address, address_normalized, address_scheme,
                  derivation_change, derivation_index, source_type, created_at, updated_at)
                 VALUES (?1, ?2, 'bitcoin', 'mainnet', ?3, ?3, 'native_segwit',
                         0, 1, 'derived', ?4, ?4)",
                params![
                    missing_address_id.to_string(),
                    account_id.to_string(),
                    missing_address,
                    "2026-02-12T10:00:00Z",
                ],
            )
            .map(|_| ())
            .map_err(|err| DbError::new(format!("missing address fixture failed: {err}")))
        })
        .expect("missing address fixture should persist");

        let accounts = crate::db::list_wallets(user_id)
            .expect("wallets should load")
            .into_iter()
            .flat_map(|wallet| wallet.accounts)
            .collect::<Vec<_>>();
        with_user_db(user_id, |conn| {
            assert_current_balance(
                load_grouped_utxo_address_balances(conn)?
                    .get(&missing_address_id)
                    .expect("missing provider address should still be grouped"),
                crate::transactions::NativeBalanceState::Unknown,
            );
            let account_balances = load_grouped_account_ledger_balances(conn)?;
            assert_current_balance(
                account_balances
                    .get(&account_id)
                    .expect("account should be grouped"),
                crate::transactions::NativeBalanceState::Unknown,
            );
            let wallet_balances =
                load_wallet_summary_address_balances(conn, &accounts, &account_balances)?;
            assert_current_balance(
                wallet_balances
                    .get(missing_address)
                    .expect("wallet should retain the unknown address"),
                crate::transactions::NativeBalanceState::Unknown,
            );
            Ok::<(), DbError>(())
        })
        .expect("unknown loaders should succeed");
    }

    #[test]
    fn current_bitcoin_provider_balance_zero_is_known_zero() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (account_id, address_id) =
            seed_account_balances_fixture(user_id).expect("fixture should insert");
        record_current_bitcoin_provider_balance(user_id, address_id, 0);

        with_user_db(user_id, |conn| {
            assert_current_balance(
                load_grouped_account_ledger_balances(conn)?
                    .get(&account_id)
                    .expect("account should be grouped"),
                crate::transactions::NativeBalanceState::KnownAmount(UnsignedAmount::zero()),
            );
            Ok::<(), DbError>(())
        })
        .expect("zero provider balance should load");
    }

    #[test]
    fn current_bitcoin_provider_balance_rejects_incomplete_or_invalid_observations() {
        assert_eq!(
            provider_balance_state(Some(0), None, Some("2026-05-03T18:02:05Z")),
            NativeBalanceState::Unknown
        );
        assert_eq!(
            provider_balance_state(Some(-1), Some(0), Some("2026-05-03T18:02:05Z")),
            NativeBalanceState::Unknown
        );
        assert_eq!(
            provider_balance_state(Some(0), Some(0), Some("not-a-date")),
            NativeBalanceState::Unknown
        );
    }

    #[test]
    fn current_bitcoin_provider_balance_unknown_before_overflow_is_unknown() {
        let result = checked_sum_native_balances(&[
            NativeBalanceState::Unknown,
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(u128::MAX)),
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(1)),
        ])
        .expect("unknown should dominate overflow");

        assert_eq!(result, NativeBalanceState::Unknown);
    }

    #[test]
    fn current_bitcoin_provider_balance_unknown_after_overflow_is_unknown() {
        let result = checked_sum_native_balances(&[
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(u128::MAX)),
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(1)),
            NativeBalanceState::Unknown,
        ])
        .expect("unknown should dominate overflow");

        assert_eq!(result, NativeBalanceState::Unknown);
    }

    #[test]
    fn current_bitcoin_provider_balance_all_known_overflow_is_error() {
        let result = checked_sum_native_balances(&[
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(u128::MAX)),
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(1)),
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn current_bitcoin_provider_balance_canonical_zero_before_unknown_is_error() {
        let result = checked_sum_native_balances(&[
            NativeBalanceState::CanonicalZero,
            NativeBalanceState::Unknown,
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn current_bitcoin_provider_balance_unknown_before_canonical_zero_is_error() {
        let result = checked_sum_native_balances(&[
            NativeBalanceState::Unknown,
            NativeBalanceState::CanonicalZero,
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn current_bitcoin_provider_balance_mixed_half_null_address_is_unknown() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (account_id, address_id) =
            seed_account_balances_fixture(user_id).expect("fixture should insert");
        record_current_bitcoin_provider_balance(user_id, address_id, 75_000);
        let half_null_address_id = DigitalAssetAddressId::new();
        let half_null_address = "bc1qhalfnullprovider0000000000000000000000000";

        with_user_db_mut(user_id, |conn| {
            let observed_at = "2026-05-03T18:02:05Z";
            conn.execute(
                "INSERT INTO digital_asset_addresses
                 (id, account_id, asset_id, network, address, address_normalized, address_scheme,
                  derivation_change, derivation_index, source_type, created_at, updated_at)
                 VALUES (?1, ?2, 'bitcoin', 'mainnet', ?3, ?3, 'native_segwit',
                         0, 1, 'derived', ?4, ?4)",
                params![
                    half_null_address_id.to_string(),
                    account_id.to_string(),
                    half_null_address,
                    observed_at,
                ],
            )
            .map_err(|err| DbError::new(format!("half-null address fixture failed: {err}")))?;
            conn.execute(
                "INSERT INTO transaction_sync_state
                 (id, scope, address_id, last_run_id, last_started_at, last_completed_at,
                  last_result, last_error, last_tip_height, new_tx_count, updated_tx_count,
                  api_confirmed_balance_hi, api_confirmed_balance_lo, created_at, updated_at)
                 VALUES (?1, 'address', ?2, ?3, ?4, ?4, 'success', NULL, 1, 0, 0,
                         0, NULL, ?4, ?4)",
                params![
                    Ulid::new().to_string(),
                    half_null_address_id.to_string(),
                    crate::transactions::TransactionSyncRunId::new().to_string(),
                    observed_at,
                ],
            )
            .map(|_| ())
            .map_err(|err| DbError::new(format!("half-null sync fixture failed: {err}")))
        })
        .expect("mixed provider fixture should persist");

        let accounts = crate::db::list_wallets(user_id)
            .expect("wallets should load")
            .into_iter()
            .flat_map(|wallet| wallet.accounts)
            .collect::<Vec<_>>();
        with_user_db(user_id, |conn| {
            assert_current_balance(
                load_grouped_utxo_address_balances(conn)?
                    .get(&half_null_address_id)
                    .expect("half-null address should be grouped"),
                NativeBalanceState::Unknown,
            );
            let account_balances = load_grouped_account_ledger_balances(conn)?;
            assert_current_balance(
                account_balances
                    .get(&account_id)
                    .expect("account should be grouped"),
                NativeBalanceState::Unknown,
            );
            let wallet_balances =
                load_wallet_summary_address_balances(conn, &accounts, &account_balances)?;
            assert_current_balance(
                wallet_balances
                    .get(half_null_address)
                    .expect("wallet should retain half-null address"),
                NativeBalanceState::Unknown,
            );
            Ok::<(), DbError>(())
        })
        .expect("mixed half-null observation should remain unknown");
    }

    #[test]
    fn load_all_account_balances_uses_provider_when_all_utxos_are_spent() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (account_id, address_id) =
            seed_account_balances_fixture(user_id).expect("fixture should insert");
        let run_id = crate::transactions::TransactionSyncRunId::new();
        let started = parse_datetime("2026-05-03T18:00:00Z").expect("valid started_at");
        let completed = parse_datetime("2026-05-03T18:02:05Z").expect("valid completed_at");
        let stale_api_balance =
            ApiConfirmedBalance::from_amount(UnsignedAmount::from_u128(2_441_190_093_160));

        with_user_db_mut(user_id, |conn| {
            conn.execute(
                "UPDATE utxos
                 SET spent_by_tx_hash = ?1,
                     spent_at = ?2,
                     updated_at = ?2
                 WHERE address_id = ?3",
                params![
                    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                    completed.to_rfc3339(),
                    address_id.to_string(),
                ],
            )
            .map(|_| ())
            .map_err(|err| DbError::new(format!("Failed to mark fixture UTXOs spent: {err}")))
        })
        .expect("spent UTXO fixture should persist");
        crate::db::mark_address_sync_started(user_id, address_id, run_id, started)
            .expect("sync start should persist");
        crate::db::mark_address_sync_completed_success(
            user_id,
            &crate::db::AddressSyncSuccess {
                address_id,
                run_id,
                started_at: started,
                completed_at: completed,
                last_tip_height: crate::transactions::ChainTipHeight::try_new(1)
                    .expect("tip should be valid"),
                new_tx_count: crate::transactions::TransactionCount::zero(),
                updated_tx_count: crate::transactions::TransactionCount::zero(),
                api_confirmed_balance: Some(stale_api_balance),
            },
        )
        .expect("sync success should persist");

        let result = load_all_account_balances(user_id).expect("query should succeed");
        let account = result
            .accounts
            .iter()
            .find(|account| account.account_id == account_id)
            .expect("account should be present");

        let expected =
            NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(2_441_190_093_160));
        assert_eq!(account.account_balance.confirmed, expected);
        assert_eq!(account.addresses[0].balance.confirmed, expected);
        assert_eq!(result.totals[0].confirmed, expected);
    }

    #[test]
    fn load_account_transaction_history_returns_expected_ethereum_entries() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (account_id, _address_id) =
            seed_ethereum_account_balances_fixture(user_id).expect("fixture should insert");

        let history = load_account_transaction_history(user_id).expect("history should load");
        let entries = history
            .get(&account_id)
            .expect("ethereum account should have a history row");
        assert_eq!(entries.len(), 3);

        let outgoing = entries
            .iter()
            .find(|entry| {
                entry.tx_hash == "2222222222222222222222222222222222222222222222222222222222222222"
            })
            .expect("outgoing entry should exist");
        assert_eq!(outgoing.status, ChainTransactionStatus::Confirmed);
        assert_eq!(outgoing.direction, AccountTransactionDirection::Outgoing);
        // The ledger does not track per-transfer kind (it aggregates transfers per tx).
        assert_eq!(outgoing.transfer_kind, None);
        assert_eq!(outgoing.value.value(), 1_000_000_000_000_000_000_u128);
        assert_eq!(
            outgoing
                .fee
                .expect("outgoing fee should be present")
                .value(),
            10_000_000_000_000_000_u128
        );

        let pending = entries
            .iter()
            .find(|entry| {
                entry.tx_hash == "3333333333333333333333333333333333333333333333333333333333333333"
            })
            .expect("pending entry should exist");
        assert_eq!(pending.status, ChainTransactionStatus::Pending);
        assert_eq!(pending.direction, AccountTransactionDirection::Incoming);
        assert_eq!(pending.transfer_kind, None);
        assert_eq!(pending.value.value(), 500_000_000_000_000_000_u128);
        assert!(pending.fee.is_none());
    }

    #[test]
    fn load_account_transaction_history_returns_expected_bitcoin_entries() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let account_id =
            seed_bitcoin_transaction_history_fixture(user_id).expect("fixture should insert");

        let history = load_account_transaction_history(user_id).expect("history should load");
        let entries = history
            .get(&account_id)
            .expect("bitcoin account should have a history row");
        assert_eq!(entries.len(), 2);

        let confirmed = entries
            .iter()
            .find(|entry| {
                entry.tx_hash == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            })
            .expect("confirmed entry should exist");
        assert_eq!(confirmed.status, ChainTransactionStatus::Confirmed);
        assert_eq!(confirmed.direction, AccountTransactionDirection::Incoming);
        assert_eq!(confirmed.value.value(), 50_000_u128);
        assert!(confirmed.fee.is_none());

        let pending = entries
            .iter()
            .find(|entry| {
                entry.tx_hash == "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            })
            .expect("pending entry should exist");
        assert_eq!(pending.status, ChainTransactionStatus::Pending);
        assert_eq!(pending.direction, AccountTransactionDirection::Outgoing);
        assert_eq!(pending.value.value(), 49_750_u128);
        assert_eq!(
            pending.fee.expect("pending fee should be present").value(),
            250
        );
    }

    #[test]
    fn load_account_transaction_counts_returns_expected_bitcoin_counts() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let account_id =
            seed_bitcoin_transaction_history_fixture(user_id).expect("fixture should insert");

        let counts = load_account_transaction_counts(user_id).expect("counts should load");
        let account_counts = counts
            .get(&account_id)
            .expect("bitcoin account should have transaction counts");
        assert_eq!(account_counts.pending, 1);
        assert_eq!(account_counts.confirmed, 1);
        assert_eq!(account_counts.dropped, 0);
        assert_eq!(account_counts.failed, 0);
    }

    #[test]
    fn utxo_value_amount_rejects_negative_insert() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (_account_id, address_id) =
            seed_account_balances_fixture(user_id).expect("fixture should insert");

        let result = with_user_db_mut(user_id, |conn| {
            conn.execute(
                "INSERT INTO utxos
                 (id, asset_id, network, tx_hash, output_index, address_id, value_amount_hi, value_amount_lo, status, replaced_by_tx_hash, spent_by_tx_hash, spent_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    Ulid::new().to_string(),
                    SyncedAssetId::Bitcoin.as_str(),
                    Network::Mainnet.as_str(),
                    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                    1_i64,
                    address_id.to_string(),
                    -1_i64,
                    0_i64,
                    "pending",
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    "2026-02-12T10:00:00Z",
                    "2026-02-12T10:00:00Z",
                ],
            )
            .map(|_| ())
            .map_err(|err| DbError::new(format!("failed to insert negative utxo value: {err}")))
        });

        assert!(
            result.is_err(),
            "negative value_amount_hi should violate CHECK"
        );
    }
}
