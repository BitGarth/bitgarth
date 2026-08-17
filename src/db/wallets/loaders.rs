use super::parsers::{
    parse_accessor_row, parse_account_row, parse_address_row, parse_hd_key_row,
    parse_summary_accessor_row, parse_summary_account_row, parse_summary_address_row,
    parse_summary_hd_key_row, parse_wallet_row,
};
use crate::account_model::AccountModel;
use crate::asset_capabilities::account_model_for;
use crate::balance_reliability::BalanceReliability;
use crate::db::balance_reliability::load_account_balance_reliability_contexts;
use crate::db::error::DbError;
use crate::db::transactions::{
    load_grouped_account_ledger_balances, load_wallet_summary_address_balances,
    load_wallet_summary_transaction_counts,
};
use crate::db::user_db::with_user_db;
use crate::models::{UserId, parse_datetime};
use crate::transactions::{
    AccountAddressSyncStatus, AccountTransactionCounts, AddressBalanceSummary, SyncErrorMessage,
    TransactionCount, TransactionSyncResult, derive_account_address_sync_status,
};
use crate::wallets::{
    ACCOUNT_LABEL_MAX_LENGTH, AccountWithHdKeys, AddressScheme, DigitalAssetAccountId,
    DigitalAssetAddressId, DigitalAssetAddressRecord, HdKeyRecord, Label, ManualAssetDisplayScale,
    SyncedAssetId, ValidatedManualAssetUnitCode, ValidatedMasterFingerprint, WalletAccessorSummary,
    WalletId, WalletWithDetails,
};
use rusqlite::{OptionalExtension, params};
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountAddressesPage {
    pub page: u32,
    pub page_size: u32,
    pub total: u32,
    pub rows: Vec<AccountAddressesPageRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountAddressesPageRow {
    pub address: String,
    pub derivation_change: Option<u32>,
    pub derivation_index: Option<u32>,
    pub sync_status: AccountAddressSyncStatus,
    pub sync_last_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub sync_last_error: Option<SyncErrorMessage>,
    pub transaction_count: TransactionCount,
    pub reported_transaction_count: Option<TransactionCount>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WalletSummaryBundle {
    pub wallets: Vec<WalletWithDetails>,
    pub manual_asset_accounts: Vec<ManualAssetAccountRow>,
    pub address_balances: HashMap<String, AddressBalanceSummary>,
    pub account_balances: HashMap<crate::wallets::DigitalAssetAccountId, AddressBalanceSummary>,
    pub account_balance_reliabilities:
        HashMap<crate::wallets::DigitalAssetAccountId, BalanceReliability>,
    pub account_tx_counts: HashMap<DigitalAssetAccountId, AccountTransactionCounts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualAssetAccountRow {
    pub account_id: crate::wallets::WalletAccountId,
    pub wallet_id: WalletId,
    pub label: Label,
    pub asset_id: crate::asset_capabilities::AssetId,
    pub network_id: crate::asset_capabilities::unsynced::UnsyncedNetworkId,
    pub unit_code: ValidatedManualAssetUnitCode,
    pub decimal_precision: ManualAssetDisplayScale,
    pub symbol: Option<String>,
    pub asset_name: String,
    pub network_name: String,
    pub coingecko_id: crate::asset_capabilities::unsynced::CoingeckoAssetId,
    pub asset_source: String,
    pub precision_source: String,
    pub coingecko_platform_id: Option<String>,
    pub provider_platform_asset_ref: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub(crate) fn account_exists(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
) -> Result<bool, DbError> {
    with_user_db(user_id, |conn| {
        let row = conn
            .query_row(
                "SELECT 1 FROM digital_asset_accounts WHERE id = ?1 LIMIT 1",
                [account_id.to_string()],
                |_| Ok(()),
            )
            .optional()
            .map_err(|err| DbError::new(format!("Failed to check account existence: {err}")))?;
        Ok(row.is_some())
    })
}

pub(crate) fn address_exists(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
) -> Result<bool, DbError> {
    with_user_db(user_id, |conn| {
        let row = conn
            .query_row(
                "SELECT 1 FROM digital_asset_addresses WHERE id = ?1 LIMIT 1",
                [address_id.to_string()],
                |_| Ok(()),
            )
            .optional()
            .map_err(|err| DbError::new(format!("Failed to check address existence: {err}")))?;
        Ok(row.is_some())
    })
}

fn load_address_transaction_count(
    conn: &rusqlite::Connection,
    account_model: AccountModel,
    address_id: DigitalAssetAddressId,
) -> Result<u32, DbError> {
    let address_id_raw = address_id.to_string();

    let count_i64 = match account_model {
        AccountModel::Utxo => conn
            .query_row(
                "SELECT COUNT(*) FROM (
                    SELECT tx_hash AS tx_ref
                    FROM utxos
                    WHERE address_id = ?1 AND tx_hash IS NOT NULL
                    UNION
                    SELECT spent_by_tx_hash AS tx_ref
                    FROM utxos
                    WHERE address_id = ?1 AND spent_by_tx_hash IS NOT NULL
                )",
                params![address_id_raw],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| DbError::new(format!("Failed to query UTXO transaction count: {e}")))?,
        AccountModel::Account => conn
            .query_row(
                "SELECT COUNT(DISTINCT chain_transaction_id)
                 FROM account_transfers
                 WHERE from_address_id = ?1 OR to_address_id = ?1",
                params![address_id_raw],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| DbError::new(format!("Failed to query account transaction count: {e}")))?,
    };

    u32::try_from(count_i64).map_err(|_| {
        DbError::new(format!(
            "Transaction count out of range for address {address_id}"
        ))
    })
}

pub(crate) fn load_account_addresses_page(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    address_scheme: AddressScheme,
    page: u32,
    page_size: u32,
) -> Result<AccountAddressesPage, DbError> {
    with_user_db(user_id, |conn| {
        let asset_id_raw = conn
            .query_row(
                "SELECT asset_id
                 FROM digital_asset_accounts
                 WHERE id = ?1
                 LIMIT 1",
                params![account_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| DbError::new(format!("Failed to load account asset for paging: {e}")))?
            .ok_or_else(|| DbError::new("Account not found for address paging"))?;

        let asset_id = SyncedAssetId::from_str(&asset_id_raw).ok_or_else(|| {
            DbError::new(format!("Invalid account asset_id in DB: {asset_id_raw}"))
        })?;
        let account_model = account_model_for(asset_id);

        let total_i64: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM digital_asset_addresses
                 WHERE account_id = ?1 AND address_scheme = ?2",
                params![account_id.to_string(), address_scheme.as_str()],
                |row| row.get(0),
            )
            .map_err(|e| DbError::new(format!("Failed to count account addresses: {e}")))?;
        let total = u32::try_from(total_i64)
            .map_err(|_| DbError::new("Address count exceeds supported range"))?;

        let offset_u64 = u64::from(page.saturating_sub(1)).saturating_mul(u64::from(page_size));
        let offset = i64::try_from(offset_u64)
            .map_err(|_| DbError::new("Address paging offset exceeds supported range"))?;
        let limit = i64::from(page_size);

        let mut stmt = conn
            .prepare(
                "SELECT
                    da.id,
                    da.address,
                    da.derivation_change,
                    da.derivation_index,
                    tss.last_started_at,
                    tss.last_completed_at,
                    tss.last_result,
                    tss.last_error,
                    tss.mempool_backfill_cursor_txid,
                    tss.etherscan_backfill_end_block,
                    tss.reported_tx_count
                 FROM digital_asset_addresses da
                 LEFT JOIN transaction_sync_state tss
                   ON tss.scope = ?2
                  AND tss.address_id = da.id
                 WHERE da.account_id = ?1 AND da.address_scheme = ?3
                 ORDER BY
                    COALESCE(da.derivation_change, 0) ASC,
                    COALESCE(da.derivation_index, 0) ASC,
                    da.id ASC
                 LIMIT ?4 OFFSET ?5",
            )
            .map_err(|e| {
                DbError::new(format!(
                    "Failed to prepare account addresses page query: {e}"
                ))
            })?;

        let rows_iter = stmt
            .query_map(
                params![
                    account_id.to_string(),
                    "address",
                    address_scheme.as_str(),
                    limit,
                    offset
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<i64>>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                    ))
                },
            )
            .map_err(|e| DbError::new(format!("Failed to query account addresses page: {e}")))?;

        let mut rows = Vec::new();
        for row_result in rows_iter {
            let (
                address_id_raw,
                address,
                derivation_change_raw,
                derivation_index_raw,
                last_started_at_raw,
                sync_last_completed_at_raw,
                last_result_raw,
                last_error_raw,
                mempool_backfill_cursor_txid_raw,
                etherscan_backfill_end_block_raw,
                reported_tx_count_raw,
            ) = row_result
                .map_err(|e| DbError::new(format!("Failed to map account address row: {e}")))?;

            let address_id = DigitalAssetAddressId::from_str(&address_id_raw)
                .map_err(|e| DbError::new(format!("Invalid address id in DB: {e}")))?;

            let derivation_change = derivation_change_raw
                .map(u32::try_from)
                .transpose()
                .map_err(|e| DbError::new(format!("Invalid derivation_change in DB: {e}")))?;
            let derivation_index = derivation_index_raw
                .map(u32::try_from)
                .transpose()
                .map_err(|e| DbError::new(format!("Invalid derivation_index in DB: {e}")))?;
            let last_started_at = last_started_at_raw
                .as_deref()
                .map(parse_datetime)
                .transpose()
                .map_err(|e| {
                    DbError::new(format!(
                        "Invalid last_started_at in transaction_sync_state: {e}"
                    ))
                })?;
            let sync_last_completed_at = sync_last_completed_at_raw
                .as_deref()
                .map(parse_datetime)
                .transpose()
                .map_err(|e| {
                    DbError::new(format!(
                        "Invalid last_completed_at in transaction_sync_state: {e}"
                    ))
                })?;
            let last_result = last_result_raw
                .as_deref()
                .map(|value| {
                    TransactionSyncResult::from_db_value(value).ok_or_else(|| {
                        DbError::new(format!(
                            "Invalid last_result in transaction_sync_state: {value}"
                        ))
                    })
                })
                .transpose()?;
            let sync_last_error = last_error_raw
                .as_deref()
                .map(str::trim)
                .filter(|raw| !raw.is_empty())
                .map(SyncErrorMessage::sanitize);
            let sync_status = derive_account_address_sync_status(
                last_started_at,
                sync_last_completed_at,
                last_result,
                mempool_backfill_cursor_txid_raw.is_some(),
                etherscan_backfill_end_block_raw.is_some(),
            );

            let transaction_count = TransactionCount::from_u32(load_address_transaction_count(
                conn,
                account_model,
                address_id,
            )?);

            let reported_transaction_count = reported_tx_count_raw
                .map(|raw| {
                    TransactionCount::try_new(raw)
                        .map_err(|e| DbError::new(format!("Invalid reported_tx_count in DB: {e}")))
                })
                .transpose()?;

            rows.push(AccountAddressesPageRow {
                address,
                derivation_change,
                derivation_index,
                sync_status,
                sync_last_completed_at,
                sync_last_error,
                transaction_count,
                reported_transaction_count,
            });
        }

        Ok(AccountAddressesPage {
            page,
            page_size,
            total,
            rows,
        })
    })
}

fn load_wallets_with_details_batched(
    conn: &rusqlite::Connection,
) -> Result<Vec<WalletWithDetails>, DbError> {
    let mut wallets = Vec::new();
    let mut wallet_positions = HashMap::new();
    let mut wallet_stmt = conn
        .prepare(
            "SELECT id, master_fingerprint, identity_source, verified_at, label, created_at, updated_at
             FROM wallets
             ORDER BY created_at",
        )
        .map_err(|e| DbError::new(format!("Failed to prepare wallets query: {e}")))?;
    let wallet_rows = wallet_stmt
        .query_map([], parse_wallet_row)
        .map_err(|e| DbError::new(format!("Failed to query wallets: {e}")))?;

    for wallet_result in wallet_rows {
        let wallet = wallet_result.map_err(|e| DbError::new(e.to_string()))?;
        let wallet_position = wallets.len();
        wallet_positions.insert(wallet.id, wallet_position);
        wallets.push(WalletWithDetails {
            wallet,
            accessors: Vec::new(),
            accounts: Vec::new(),
        });
    }

    let mut accessor_stmt = conn
        .prepare(
            "SELECT wallet_id, id, accessor_kind, accessor_label, device_id_hash, device_model,
                    accessor_version, firmware_version, created_at, updated_at
             FROM wallet_accessors
             ORDER BY created_at",
        )
        .map_err(|e| DbError::new(format!("Failed to prepare wallet accessor query: {e}")))?;
    let accessor_rows = accessor_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, parse_summary_accessor_row(row)?))
        })
        .map_err(|e| DbError::new(format!("Failed to query wallet accessors: {e}")))?;
    for row_result in accessor_rows {
        let (wallet_id_raw, accessor) = row_result.map_err(|e| DbError::new(e.to_string()))?;
        let wallet_id = WalletId::from_str(&wallet_id_raw)
            .map_err(|e| DbError::new(format!("Invalid wallet id in DB: {e}")))?;
        let wallet_position = wallet_positions.get(&wallet_id).copied().ok_or_else(|| {
            DbError::new(format!(
                "Wallet accessor references unknown wallet id {wallet_id}"
            ))
        })?;
        wallets[wallet_position].accessors.push(accessor);
    }

    let mut account_positions = HashMap::new();
    let mut account_stmt = conn
        .prepare(
            "SELECT wallet_id, id, asset_id, network, account_kind, label, created_at, updated_at
             FROM digital_asset_accounts
             WHERE wallet_id IS NOT NULL
             ORDER BY created_at",
        )
        .map_err(|e| DbError::new(format!("Failed to prepare account query: {e}")))?;
    let account_rows = account_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, parse_summary_account_row(row)?))
        })
        .map_err(|e| DbError::new(format!("Failed to query accounts: {e}")))?;
    for row_result in account_rows {
        let (wallet_id_raw, account) = row_result.map_err(|e| DbError::new(e.to_string()))?;
        let wallet_id = WalletId::from_str(&wallet_id_raw)
            .map_err(|e| DbError::new(format!("Invalid wallet id in DB: {e}")))?;
        let wallet_position = wallet_positions.get(&wallet_id).copied().ok_or_else(|| {
            DbError::new(format!("Account references unknown wallet id {wallet_id}"))
        })?;
        let account_position = wallets[wallet_position].accounts.len();
        account_positions.insert(account.id, (wallet_position, account_position));
        wallets[wallet_position].accounts.push(account);
    }

    let mut hd_key_stmt = conn
        .prepare(
            "SELECT account_id, id, key_role, key_source, verified_by_accessor_id, address_scheme,
                    extended_pubkey, derivation_purpose, derivation_coin_type, derivation_account,
                    created_at, updated_at
             FROM digital_asset_account_hd_keys
             ORDER BY created_at",
        )
        .map_err(|e| DbError::new(format!("Failed to prepare hd key query: {e}")))?;
    let hd_key_rows = hd_key_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, parse_summary_hd_key_row(row)?))
        })
        .map_err(|e| DbError::new(format!("Failed to query hd keys: {e}")))?;
    for row_result in hd_key_rows {
        let (account_id_raw, hd_key) = row_result.map_err(|e| DbError::new(e.to_string()))?;
        let account_id = DigitalAssetAccountId::from_str(&account_id_raw)
            .map_err(|e| DbError::new(format!("Invalid account id in DB: {e}")))?;
        let (wallet_position, account_position) =
            account_positions.get(&account_id).copied().ok_or_else(|| {
                DbError::new(format!("HD key references unknown account id {account_id}"))
            })?;
        wallets[wallet_position].accounts[account_position]
            .hd_keys
            .push(hd_key);
    }

    let mut address_stmt = conn
        .prepare(
            "SELECT account_id, id, asset_id, network, address, address_scheme, derivation_change,
                    derivation_index, source_type, created_at, updated_at
             FROM digital_asset_addresses
             ORDER BY derivation_change ASC, derivation_index ASC, created_at ASC",
        )
        .map_err(|e| DbError::new(format!("Failed to prepare address query: {e}")))?;
    let address_rows = address_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, parse_summary_address_row(row)?))
        })
        .map_err(|e| DbError::new(format!("Failed to query addresses: {e}")))?;
    for row_result in address_rows {
        let (account_id_raw, address) = row_result.map_err(|e| DbError::new(e.to_string()))?;
        let account_id = DigitalAssetAccountId::from_str(&account_id_raw)
            .map_err(|e| DbError::new(format!("Invalid account id in DB: {e}")))?;
        let (wallet_position, account_position) =
            account_positions.get(&account_id).copied().ok_or_else(|| {
                DbError::new(format!(
                    "Address references unknown account id {account_id}"
                ))
            })?;
        wallets[wallet_position].accounts[account_position]
            .addresses
            .push(address);
    }

    Ok(wallets)
}

fn load_manual_asset_accounts_batched(
    conn: &rusqlite::Connection,
) -> Result<Vec<ManualAssetAccountRow>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, wallet_id, label, asset_id, network_id, unit_code, decimal_precision,
                    symbol, asset_name, network_name, coingecko_id, asset_source,
                    precision_source, coingecko_platform_id, provider_platform_asset_ref,
                    created_at, updated_at
             FROM manual_asset_accounts
             ORDER BY created_at",
        )
        .map_err(|e| DbError::new(format!("Failed to prepare manual asset account query: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, String>(15)?,
                row.get::<_, String>(16)?,
            ))
        })
        .map_err(|e| DbError::new(format!("Failed to query manual asset accounts: {e}")))?;

    rows.map(|row_result| {
        let (
            id_raw,
            wallet_id_raw,
            label_raw,
            asset_id_raw,
            network_id_raw,
            unit_code_raw,
            decimal_precision_raw,
            symbol,
            asset_name,
            network_name,
            coingecko_id_raw,
            asset_source,
            precision_source,
            coingecko_platform_id,
            provider_platform_asset_ref,
            created_at_raw,
            updated_at_raw,
        ) = row_result
            .map_err(|e| DbError::new(format!("Failed to read manual asset account row: {e}")))?;

        let account_id = crate::wallets::WalletAccountId::from_str(&id_raw)
            .map_err(|e| DbError::new(format!("Invalid manual asset account id in DB: {e}")))?;
        let wallet_id = WalletId::from_str(&wallet_id_raw)
            .map_err(|e| DbError::new(format!("Invalid manual asset wallet id in DB: {e}")))?;
        let label = Label::parse_with_limit(&label_raw, ACCOUNT_LABEL_MAX_LENGTH)
            .map_err(|e| DbError::new(format!("Invalid manual asset label in DB: {e}")))?;
        let asset_id = crate::asset_capabilities::AssetId::owned(asset_id_raw)
            .map_err(|e| DbError::new(format!("Invalid manual asset id in DB: {e}")))?;
        let network_id =
            crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(&network_id_raw)
                .map_err(|e| DbError::new(format!("Invalid manual asset network_id in DB: {e}")))?;
        let unit_code = ValidatedManualAssetUnitCode::parse(&unit_code_raw)
            .map_err(|e| DbError::new(format!("Invalid manual asset unit_code in DB: {e}")))?;
        let decimal_precision =
            ManualAssetDisplayScale::try_from(decimal_precision_raw).map_err(|e| {
                DbError::new(format!("Invalid manual asset decimal_precision in DB: {e}"))
            })?;
        let coingecko_id =
            crate::asset_capabilities::unsynced::CoingeckoAssetId::parse(&coingecko_id_raw)
                .map_err(|e| {
                    DbError::new(format!("Invalid manual asset coingecko_id in DB: {e}"))
                })?;
        let created_at = parse_datetime(&created_at_raw)
            .map_err(|e| DbError::new(format!("Invalid manual asset created_at in DB: {e}")))?;
        let updated_at = parse_datetime(&updated_at_raw)
            .map_err(|e| DbError::new(format!("Invalid manual asset updated_at in DB: {e}")))?;

        Ok(ManualAssetAccountRow {
            account_id,
            wallet_id,
            label,
            asset_id,
            network_id,
            unit_code,
            decimal_precision,
            symbol,
            asset_name,
            network_name,
            coingecko_id,
            asset_source,
            precision_source,
            coingecko_platform_id,
            provider_platform_asset_ref,
            created_at,
            updated_at,
        })
    })
    .collect()
}

pub(crate) fn load_wallet_summary_bundle(user_id: UserId) -> Result<WalletSummaryBundle, DbError> {
    with_user_db(user_id, |conn| {
        let wallets = load_wallets_with_details_batched(conn)?;
        let manual_asset_accounts = load_manual_asset_accounts_batched(conn)?;
        let accounts = wallets
            .iter()
            .flat_map(|wallet| wallet.accounts.iter().cloned())
            .collect::<Vec<_>>();
        let account_ids = accounts
            .iter()
            .map(|account| account.id)
            .collect::<Vec<_>>();
        let account_balances = load_grouped_account_ledger_balances(conn)?;
        let address_balances =
            load_wallet_summary_address_balances(conn, &accounts, &account_balances)?;
        let account_balance_reliabilities =
            load_account_balance_reliability_contexts(conn, &account_ids)?
                .into_iter()
                .map(|(account_id, context)| (account_id, context.balance_reliability))
                .collect::<HashMap<_, _>>();
        let account_tx_counts = load_wallet_summary_transaction_counts(conn, &account_ids)?;

        Ok(WalletSummaryBundle {
            wallets,
            manual_asset_accounts,
            address_balances,
            account_balances,
            account_balance_reliabilities,
            account_tx_counts,
        })
    })
}

pub(crate) fn list_wallets(user_id: UserId) -> Result<Vec<WalletWithDetails>, DbError> {
    with_user_db(user_id, load_wallets_with_details_batched)
}

pub(crate) fn get_wallet_by_fingerprint(
    user_id: UserId,
    fingerprint: &ValidatedMasterFingerprint,
) -> Result<Option<WalletWithDetails>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, master_fingerprint, identity_source, verified_at, label, created_at, updated_at \
                 FROM wallets WHERE master_fingerprint = ?1",
            )
            .map_err(|e| DbError::new(format!("Failed to prepare wallet lookup: {e}")))?;

        let wallet = stmt
            .query_row([fingerprint.as_str()], parse_wallet_row)
            .optional()
            .map_err(|e| DbError::new(format!("Failed to lookup wallet: {e}")))?;

        match wallet {
            Some(wallet) => {
                let accessors = load_wallet_accessors(conn, wallet.id)?;
                let accounts = load_wallet_accounts(conn, wallet.id)?;
                Ok(Some(WalletWithDetails {
                    wallet,
                    accessors,
                    accounts,
                }))
            }
            None => Ok(None),
        }
    })
}

fn load_wallet_accessors(
    conn: &rusqlite::Connection,
    wallet_id: WalletId,
) -> Result<Vec<WalletAccessorSummary>, DbError> {
    let mut accessors = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT id, accessor_kind, accessor_label, device_id_hash, device_model, accessor_version, firmware_version, created_at, updated_at \
             FROM wallet_accessors WHERE wallet_id = ?1 ORDER BY created_at",
        )
        .map_err(|e| DbError::new(format!("Failed to prepare wallet accessor query: {e}")))?;

    let rows = stmt
        .query_map([wallet_id.to_string()], parse_accessor_row)
        .map_err(|e| DbError::new(format!("Failed to query wallet accessors: {e}")))?;

    for row in rows {
        let accessor = row.map_err(|e| DbError::new(e.to_string()))?;
        accessors.push(accessor);
    }

    Ok(accessors)
}

fn load_wallet_accounts(
    conn: &rusqlite::Connection,
    wallet_id: WalletId,
) -> Result<Vec<AccountWithHdKeys>, DbError> {
    let mut accounts = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT id, asset_id, network, account_kind, label, created_at, updated_at \
             FROM digital_asset_accounts WHERE wallet_id = ?1 ORDER BY created_at",
        )
        .map_err(|e| DbError::new(format!("Failed to prepare account query: {e}")))?;

    let rows = stmt
        .query_map([wallet_id.to_string()], parse_account_row)
        .map_err(|e| DbError::new(format!("Failed to query accounts: {e}")))?;

    for row in rows {
        let mut account = row.map_err(|e| DbError::new(e.to_string()))?;
        account.hd_keys = load_account_hd_keys(conn, account.id)?;
        account.addresses = load_account_addresses(conn, account.id)?;
        accounts.push(account);
    }

    Ok(accounts)
}

fn load_account_hd_keys(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Vec<HdKeyRecord>, DbError> {
    let mut hd_keys = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT id, key_role, key_source, verified_by_accessor_id, address_scheme, extended_pubkey, derivation_purpose, derivation_coin_type, derivation_account, created_at, updated_at \
             FROM digital_asset_account_hd_keys WHERE account_id = ?1 ORDER BY created_at",
        )
        .map_err(|e| DbError::new(format!("Failed to prepare hd key query: {e}")))?;

    let rows = stmt
        .query_map([account_id.to_string()], parse_hd_key_row)
        .map_err(|e| DbError::new(format!("Failed to query hd keys: {e}")))?;

    for row in rows {
        let hd_key = row.map_err(|e| DbError::new(e.to_string()))?;
        hd_keys.push(hd_key);
    }

    Ok(hd_keys)
}

fn load_account_addresses(
    conn: &rusqlite::Connection,
    account_id: DigitalAssetAccountId,
) -> Result<Vec<DigitalAssetAddressRecord>, DbError> {
    let mut addresses = Vec::new();
    let mut stmt = conn
        .prepare(
            "SELECT id, asset_id, network, address, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at \
             FROM digital_asset_addresses \
             WHERE account_id = ?1 \
             ORDER BY derivation_change ASC, derivation_index ASC, created_at ASC",
        )
        .map_err(|e| DbError::new(format!("Failed to prepare address query: {e}")))?;

    let rows = stmt
        .query_map([account_id.to_string()], parse_address_row)
        .map_err(|e| DbError::new(format!("Failed to query addresses: {e}")))?;

    for row in rows {
        let address = row.map_err(|e| DbError::new(e.to_string()))?;
        addresses.push(address);
    }

    Ok(addresses)
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::manual_asset_assertions::load_manual_asset_wallet_report_rows;
    use crate::db::test_fixtures::{setup_test_user, unique_user_id, wallet_label};
    use crate::db::transactions::seed_ethereum_account_balances_fixture;
    use crate::db::user_db::with_user_db_mut;
    use crate::db::wallets::manual_assets::add_manual_asset_account;
    use crate::ethereum::{EthAddress, RawEthAddress};
    use crate::wallets::{
        Network, TransactionSortDirection, ValidatedAddManualAssetAccountRequest,
        ValidatedManualAssetBalanceLiteral,
    };

    fn ada_cardano_mainnet_instance_id()
    -> crate::asset_capabilities::unsynced::UnsyncedAssetInstanceId {
        crate::asset_capabilities::unsynced::UnsyncedAssetInstanceId {
            asset_id: crate::asset_capabilities::unsynced::UnsyncedAssetId::parse("cardano")
                .expect("asset id"),
            network_id: crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(
                "cardano-mainnet",
            )
            .expect("network id"),
        }
    }
    use chrono::{NaiveDate, TimeZone};

    fn seed_test_eth_address() -> EthAddress {
        let raw = RawEthAddress::new("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".to_string());
        EthAddress::parse(&raw).expect("test address should validate")
    }

    struct ManualAssetAccountFixture<'a> {
        asset_id: &'a str,
        network_id: &'a str,
        unit_code: &'a str,
        asset_name: &'a str,
        network_name: &'a str,
        coingecko_id: &'a str,
        label: &'a str,
    }

    fn insert_manual_asset_account_row(
        user_id: UserId,
        wallet_id: WalletId,
        fixture: ManualAssetAccountFixture<'_>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> crate::wallets::WalletAccountId {
        let account_id = crate::wallets::WalletAccountId::new();
        let label_key = fixture.label.to_ascii_lowercase();
        let timestamp = now.to_rfc3339();

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO manual_asset_accounts \
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, coingecko_platform_id, provider_platform_asset_ref,
                  created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 6, ?7, NULL, ?8, ?9, ?10,
                         'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, ?11, ?12)",
                rusqlite::params![
                    account_id.to_string(),
                    wallet_id.to_string(),
                    fixture.label,
                    label_key,
                    fixture.asset_id,
                    fixture.network_id,
                    fixture.unit_code,
                    fixture.asset_name,
                    fixture.network_name,
                    fixture.coingecko_id,
                    &timestamp,
                    &timestamp,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert manual asset account: {e}")))?;
            Ok(())
        })
        .expect("manual asset account fixture should insert");

        account_id
    }

    fn setup_user_with_wallet_manual_ada() -> (UserId, WalletId) {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = chrono::Utc.with_ymd_and_hms(2026, 2, 1, 12, 0, 0).unwrap();

        let eth_response = crate::db::wallets::single_address::add_ethereum_address(
            user_id,
            &seed_test_eth_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Manual Wallet")),
            now,
        )
        .expect("eth wallet fixture should insert");
        let wallet_id = eth_response.wallet_id;

        add_manual_asset_account(
            user_id,
            ValidatedAddManualAssetAccountRequest {
                wallet_id: Some(wallet_id),
                wallet_label: None,
                account_label: None,
                asset: crate::wallets::ValidatedAddManualAssetAccountAsset::BitGarthCatalog {
                    candidate_id:
                        crate::asset_capabilities::ManualAssetCatalogCandidateId::Unsynced(
                            ada_cardano_mainnet_instance_id(),
                        ),
                },
            },
            now,
        )
        .expect("manual ADA account should insert");

        (user_id, wallet_id)
    }

    fn setup_user_with_wallet_manual_ada_assertion() -> (UserId, WalletId) {
        let (user_id, wallet_id) = setup_user_with_wallet_manual_ada();
        let assert_time = chrono::Utc.with_ymd_and_hms(2026, 2, 5, 12, 0, 0).unwrap();
        let asserted_on = NaiveDate::from_ymd_opt(2026, 2, 5).expect("valid date");

        // Find the manual ADA account that was inserted by setup
        let manual_ada_id = with_user_db_mut::<_, _, DbError>(user_id, |conn| {
            let id: String = conn
                .query_row(
                    "SELECT id FROM manual_asset_accounts WHERE wallet_id = ?1 LIMIT 1",
                    rusqlite::params![wallet_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|e| DbError::new(format!("failed to read manual ada account id: {e}")))?;
            crate::wallets::WalletAccountId::from_str(&id)
                .map_err(|e| DbError::new(format!("invalid manual account id: {e}")))
        })
        .expect("manual ada account id should load");

        crate::db::manual_asset_assertions::add_manual_asset_balance_assertion(
            user_id,
            crate::wallets::ValidatedAddManualAssetBalanceAssertionRequest {
                account_id: manual_ada_id,
                asserted_on,
                balance: ValidatedManualAssetBalanceLiteral::parse("1.234")
                    .expect("balance literal should validate"),
                note: None,
            },
            assert_time,
        )
        .expect("ADA assertion should insert");

        (user_id, wallet_id)
    }

    #[test]
    fn wallet_loader_returns_manual_asset_accounts() {
        let (user_id, wallet_id) = setup_user_with_wallet_manual_ada();

        let bundle = load_wallet_summary_bundle(user_id).expect("summary bundle should load");

        let manual_ada = bundle.manual_asset_accounts.iter().find(|row| {
            row.wallet_id == wallet_id
                && row.asset_id.as_str() == "cardano"
                && row.network_id.as_str() == "cardano-mainnet"
        });
        assert!(
            manual_ada.is_some(),
            "manual ADA account should be loaded from manual_asset_accounts"
        );
        let manual_ada = manual_ada.unwrap();
        assert_eq!(manual_ada.unit_code.as_str(), "ADA");
        assert_eq!(manual_ada.decimal_precision.as_u8(), 6);
    }

    #[test]
    fn wallet_loader_accepts_catalog_only_manual_asset_rows() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = chrono::Utc.with_ymd_and_hms(2026, 2, 2, 12, 0, 0).unwrap();
        let eth_response = crate::db::wallets::single_address::add_ethereum_address(
            user_id,
            &seed_test_eth_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Catalog Only Wallet")),
            now,
        )
        .expect("eth wallet fixture should insert");
        let wallet_id = eth_response.wallet_id;

        let account_id = insert_manual_asset_account_row(
            user_id,
            wallet_id,
            ManualAssetAccountFixture {
                asset_id: "algorand",
                network_id: "algorand-mainnet",
                unit_code: "ALGO",
                asset_name: "Algorand",
                network_name: "Algorand",
                coingecko_id: "algorand",
                label: "ALGO Account 1",
            },
            now,
        );

        let bundle = load_wallet_summary_bundle(user_id).expect("summary bundle should load");
        let manual_algo = bundle
            .manual_asset_accounts
            .iter()
            .find(|row| row.wallet_id == wallet_id)
            .expect("manual ALGO account should load");

        assert_eq!(manual_algo.asset_id.as_str(), "algorand");
        assert_eq!(manual_algo.network_id.as_str(), "algorand-mainnet");
        assert_eq!(manual_algo.unit_code.as_str(), "ALGO");
        assert_eq!(manual_algo.decimal_precision.as_u8(), 6);

        let history = crate::db::manual_asset_assertions::load_manual_asset_account_history(
            user_id,
            account_id,
            1,
            50,
            TransactionSortDirection::Descending,
            None,
            None,
        )
        .expect("manual ALGO history should load");
        assert_eq!(history.unit_code.as_str(), "ALGO");
        assert_eq!(history.decimal_precision.as_u8(), 6);

        let from = NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date");
        let to = NaiveDate::from_ymd_opt(2026, 12, 31).expect("valid date");
        let report_rows = load_manual_asset_wallet_report_rows(user_id, wallet_id, from, to)
            .expect("manual ALGO report rows should load");
        let algo_report = report_rows
            .iter()
            .find(|row| row.unit_code.as_str() == "ALGO")
            .expect("ALGO report row should exist");
        assert_eq!(algo_report.decimal_precision.as_u8(), 6);
    }

    #[test]
    fn report_rows_use_catalog_scale_for_manual_assets() {
        let (user_id, wallet_id) = setup_user_with_wallet_manual_ada_assertion();

        let from = NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date");
        let to = NaiveDate::from_ymd_opt(2026, 12, 31).expect("valid date");
        let rows = load_manual_asset_wallet_report_rows(user_id, wallet_id, from, to)
            .expect("report rows should load");

        let ada = rows
            .iter()
            .find(|row| row.unit_code.as_str() == "ADA")
            .expect("ADA row should exist");
        assert_eq!(ada.decimal_precision.as_u8(), 6);
    }

    #[test]
    fn load_wallet_summary_bundle_returns_expected_ethereum_balances_and_counts() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let (account_id, _address_id) =
            seed_ethereum_account_balances_fixture(user_id).expect("fixture should insert");

        let bundle = load_wallet_summary_bundle(user_id).expect("summary bundle should load");

        assert_eq!(bundle.wallets.len(), 1);
        assert_eq!(bundle.wallets[0].accounts.len(), 1);
        assert_eq!(bundle.wallets[0].accounts[0].id, account_id);

        let address_balance = bundle
            .address_balances
            .get("0x52908400098527886E0F7030069857D2E4169EE7")
            .expect("ethereum address balance should exist");
        assert_eq!(
            address_balance.confirmed,
            crate::transactions::NativeBalanceState::KnownAmount(
                crate::amounts::UnsignedAmount::from_u128(990_000_000_000_000_000_u128)
            )
        );

        let counts = bundle
            .account_tx_counts
            .get(&account_id)
            .expect("ethereum account counts should exist");
        assert_eq!(counts.pending, 1);
        assert_eq!(counts.confirmed, 2);
        assert_eq!(counts.dropped, 0);
        assert_eq!(counts.failed, 0);

        let account_balance = bundle
            .account_balances
            .get(&account_id)
            .expect("account ledger balance should exist");
        assert_eq!(
            account_balance.confirmed,
            crate::transactions::NativeBalanceState::KnownAmount(
                crate::amounts::UnsignedAmount::from_u128(990_000_000_000_000_000_u128)
            )
        );
    }
}
