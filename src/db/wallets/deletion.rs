use super::errors::db_error_from_sqlite;
use super::moves::{WalletAccountStorageKind, load_wallet_account_context_in_tx};
use crate::db::chain_cleanup::{
    begin_chain_cleanup_scope, execute_chain_cleanup_for_marked_candidates,
    mark_chain_cleanup_candidates_for_account, mark_chain_cleanup_candidates_for_wallet,
};
use crate::db::error::DbError;
use crate::db::raw_ingestion::deactivate_source_connection_for_address_tx;
use crate::db::user_db::with_user_db_mut;
use crate::models::UserId;
use crate::wallets::{DigitalAssetAccountId, DigitalAssetAddressId, WalletAccountId, WalletId};
use chrono::Utc;
use dioxus::logger::tracing;
use rusqlite::params;
use std::str::FromStr;
use std::time::Instant;

fn delete_native_account(
    tx: &rusqlite::Transaction<'_>,
    account_id: DigitalAssetAccountId,
) -> Result<(), DbError> {
    begin_chain_cleanup_scope(tx)?;
    mark_chain_cleanup_candidates_for_account(tx, account_id)?;

    let address_ids_to_remove = {
        let mut stmt = tx
            .prepare(
                "SELECT id
                 FROM digital_asset_addresses
                 WHERE account_id = ?1",
            )
            .map_err(|e| {
                DbError::new(format!(
                    "Failed to prepare account addresses lookup before delete: {e}"
                ))
            })?;
        let rows = stmt
            .query_map(params![account_id.to_string()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| {
                DbError::new(format!(
                    "Failed to query account addresses before delete: {e}"
                ))
            })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| {
            DbError::new(format!(
                "Failed to read account addresses before delete: {e}"
            ))
        })?
    };

    for address_id_raw in &address_ids_to_remove {
        let address_id = DigitalAssetAddressId::from_str(address_id_raw)
            .map_err(|e| DbError::new(format!("Invalid account address id before delete: {e}")))?;
        deactivate_source_connection_for_address_tx(tx, address_id, Utc::now())?;
    }

    tx.execute(
        "DELETE FROM digital_asset_accounts WHERE id = ?1",
        params![account_id.to_string()],
    )
    .map_err(|e| db_error_from_sqlite("Failed to delete native account", e))?;

    let _ = execute_chain_cleanup_for_marked_candidates(tx)?;
    Ok(())
}

fn delete_manual_account(
    tx: &rusqlite::Transaction<'_>,
    account_id: WalletAccountId,
) -> Result<(), DbError> {
    tx.execute(
        "DELETE FROM manual_asset_accounts WHERE id = ?1",
        params![account_id.to_string()],
    )
    .map_err(|e| db_error_from_sqlite("Failed to delete manual asset account", e))?;
    Ok(())
}

pub(crate) fn delete_account(
    user_id: UserId,
    account_id: impl Into<WalletAccountId>,
) -> Result<(), DbError> {
    let account_id = account_id.into();
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|e| {
            DbError::new(format!("Failed to start delete account transaction: {e}"))
        })?;

        let context = load_wallet_account_context_in_tx(&tx, account_id)
            .map_err(|e| DbError::new(e.to_string()))?;
        match context.kind {
            WalletAccountStorageKind::Native => {
                let native_account_id = DigitalAssetAccountId::from_str(&account_id.to_string())
                    .map_err(|e| {
                        DbError::new(format!("Invalid native account id before delete: {e}"))
                    })?;
                delete_native_account(&tx, native_account_id)?;
            }
            WalletAccountStorageKind::Manual => {
                delete_manual_account(&tx, account_id)?;
            }
        }

        tx.commit()
            .map_err(|e| DbError::new(format!("Failed to commit delete account transaction: {e}")))
    })
}

pub(crate) fn delete_wallet(user_id: UserId, wallet_id: WalletId) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let transaction_started = Instant::now();
        let tx = conn
            .transaction()
            .map_err(|e| DbError::new(format!("Failed to start delete wallet transaction: {e}")))?;

        let candidate_scan_started = Instant::now();
        begin_chain_cleanup_scope(&tx)?;
        mark_chain_cleanup_candidates_for_wallet(&tx, wallet_id)?;
        let candidate_scan_ms = candidate_scan_started.elapsed().as_millis() as u64;

        let delete_started = Instant::now();
        let address_ids_to_remove = {
            let mut stmt = tx
                .prepare(
                    "SELECT da.id
                     FROM digital_asset_addresses da
                     INNER JOIN digital_asset_accounts daa ON daa.id = da.account_id
                     WHERE daa.wallet_id = ?1",
                )
                .map_err(|e| {
                    DbError::new(format!(
                        "Failed to prepare wallet addresses lookup before delete: {e}"
                    ))
                })?;
            let rows = stmt
                .query_map(params![wallet_id.to_string()], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| {
                    DbError::new(format!(
                        "Failed to query wallet addresses before delete: {e}"
                    ))
                })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| {
                DbError::new(format!(
                    "Failed to read wallet addresses before delete: {e}"
                ))
            })?
        };
        for address_id_raw in &address_ids_to_remove {
            let address_id = DigitalAssetAddressId::from_str(address_id_raw).map_err(|e| {
                DbError::new(format!("Invalid wallet address id before delete: {e}"))
            })?;
            deactivate_source_connection_for_address_tx(&tx, address_id, Utc::now())?;
        }
        let deleted_accounts = tx
            .execute(
                "DELETE FROM digital_asset_accounts WHERE wallet_id = ?1",
                params![wallet_id.to_string()],
            )
            .map_err(|e| DbError::new(format!("Failed to delete wallet accounts: {e}")))?;

        let deleted_wallets = tx
            .execute(
                "DELETE FROM wallets WHERE id = ?1",
                params![wallet_id.to_string()],
            )
            .map_err(|e| DbError::new(format!("Failed to delete wallet: {e}")))?;
        let delete_ms = delete_started.elapsed().as_millis() as u64;

        let cleanup_started = Instant::now();
        let cleanup_stats = execute_chain_cleanup_for_marked_candidates(&tx)?;
        let cleanup_ms = cleanup_started.elapsed().as_millis() as u64;

        tx.commit().map_err(|e| {
            DbError::new(format!("Failed to commit wallet delete transaction: {e}"))
        })?;

        let total_ms = transaction_started.elapsed().as_millis() as u64;
        tracing::info!(
            wallet_id = %wallet_id,
            deleted_accounts,
            deleted_wallets,
            candidate_chain_tx_count = cleanup_stats.candidate_chain_tx_count,
            deleted_unowned_transfers = cleanup_stats.deleted_unowned_transfers,
            deleted_orphan_chain_transactions = cleanup_stats.deleted_orphan_chain_transactions,
            candidate_scan_ms,
            delete_ms,
            cleanup_ms,
            total_ms,
            "wallets db: delete wallet completed"
        );

        Ok(())
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::user_db::{with_user_db, with_user_db_mut};
    use crate::db::wallets::manual_assets::add_manual_asset_account;
    use crate::db::wallets::single_address::add_ethereum_address;
    use crate::db::{setup_test_user, unique_user_id, wallet_label};
    use crate::ethereum::EthAddress;
    use crate::ethereum::RawEthAddress;
    use crate::wallets::{
        Label, Network, SyncedAssetId, ValidatedAddManualAssetAccountRequest,
        WALLET_LABEL_MAX_LENGTH,
    };
    use chrono::Utc;
    use rusqlite::params;
    use ulid::Ulid;

    fn test_eth_address() -> EthAddress {
        let raw = RawEthAddress::new("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".to_string());
        EthAddress::parse(&raw).expect("test address should be valid")
    }

    fn second_eth_address() -> EthAddress {
        let raw = RawEthAddress::new("0xde0b295669a9fd93d5f28d9ec85e40f4cb697bae".to_string());
        EthAddress::parse(&raw).expect("second test address should be valid")
    }

    fn insert_chain_transaction_fixture(
        conn: &rusqlite::Connection,
        chain_tx_id: &str,
        tx_hash: &str,
        now: &str,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO chain_transactions
             (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                chain_tx_id,
                SyncedAssetId::Ethereum.as_str(),
                Network::Mainnet.as_str(),
                tx_hash,
                "confirmed",
                Some(100_i64),
                Some("block-hash-100".to_string()),
                Some(now.to_string()),
                Some(0_i64),
                Some(0_i64),
                Option::<i64>::None,
                now,
                now,
            ],
        )
        .map_err(|e| DbError::new(format!("Failed to insert chain transaction fixture: {e}")))?;
        Ok(())
    }

    fn tx_hash_for_index(index: u32) -> String {
        format!("{index:064x}")
    }

    #[test]
    fn delete_account_removes_owned_transaction_records_and_prunes_chain_transaction() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let account = add_ethereum_address(
            user_id,
            &test_eth_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Delete Account Wallet")),
            Utc::now(),
        )
        .expect("account should be created");

        let chain_tx_id = Ulid::new().to_string();
        let tx_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let now = "2026-02-22T17:00:00Z";

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            insert_chain_transaction_fixture(conn, &chain_tx_id, tx_hash, now)?;

            conn.execute(
                "INSERT INTO transaction_inputs
                 (id, tx_id, input_index, prev_tx_hash, prev_output_index, address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    Ulid::new().to_string(),
                    chain_tx_id,
                    0_i64,
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    0_i64,
                    account.address_id.to_string(),
                    0_i64,
                    42_i64,
                    now,
                    now,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert transaction input fixture: {e}")))?;

            conn.execute(
                "INSERT INTO transaction_outputs
                 (id, tx_id, output_index, address_id, raw_address, script_pubkey_hex, value_amount_hi, value_amount_lo, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    Ulid::new().to_string(),
                    chain_tx_id,
                    0_i64,
                    account.address_id.to_string(),
                    Option::<String>::None,
                    "0014deadbeef",
                    0_i64,
                    42_i64,
                    now,
                    now,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert transaction output fixture: {e}")))?;

            conn.execute(
                "INSERT INTO utxos
                 (id, asset_id, network, tx_hash, output_index, address_id, value_amount_hi, value_amount_lo, status, replaced_by_tx_hash, spent_by_tx_hash, spent_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    Ulid::new().to_string(),
                    SyncedAssetId::Ethereum.as_str(),
                    Network::Mainnet.as_str(),
                    tx_hash,
                    0_i64,
                    account.address_id.to_string(),
                    0_i64,
                    42_i64,
                    "confirmed",
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    now,
                    now,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert UTXO fixture: {e}")))?;

            conn.execute(
                "INSERT INTO account_transfers
                 (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, from_address, from_address_id, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'legacy:' || ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    Ulid::new().to_string(),
                    chain_tx_id,
                    SyncedAssetId::Ethereum.as_str(),
                    Network::Mainnet.as_str(),
                    tx_hash,
                    0_i64,
                    "normal",
                    "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed",
                    Some(account.address_id.to_string()),
                    Option::<String>::None,
                    Option::<String>::None,
                    0_i64,
                    42_i64,
                    now,
                    now,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert transfer fixture: {e}")))?;

            Ok(())
        })
        .expect("fixture rows should insert");

        delete_account(user_id, account.account_id).expect("delete account should succeed");

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let remaining_addresses: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM digital_asset_addresses WHERE id = ?1",
                    params![account.address_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count addresses: {e}")))?;
            let remaining_inputs: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM transaction_inputs WHERE tx_id = ?1",
                    params![chain_tx_id],
                    |row| row.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count inputs: {e}")))?;
            let remaining_outputs: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM transaction_outputs WHERE tx_id = ?1",
                    params![chain_tx_id],
                    |row| row.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count outputs: {e}")))?;
            let remaining_utxos: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM utxos WHERE address_id = ?1",
                    params![account.address_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count UTXOs: {e}")))?;
            let remaining_transfers: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM account_transfers WHERE chain_transaction_id = ?1",
                    params![chain_tx_id],
                    |row| row.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count transfers: {e}")))?;
            let remaining_chain_txs: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM chain_transactions WHERE id = ?1",
                    params![chain_tx_id],
                    |row| row.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count chain transactions: {e}")))?;

            assert_eq!(remaining_addresses, 0);
            assert_eq!(remaining_inputs, 0);
            assert_eq!(remaining_outputs, 0);
            assert_eq!(remaining_utxos, 0);
            assert_eq!(remaining_transfers, 0);
            assert_eq!(remaining_chain_txs, 0);
            Ok(())
        })
        .expect("post-delete assertions should succeed");
    }

    #[test]
    fn delete_account_large_transfer_fixture_prunes_without_row_level_trigger_work() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let account = add_ethereum_address(
            user_id,
            &test_eth_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Large Delete Wallet")),
            Utc::now(),
        )
        .expect("account should be created");

        let now = "2026-02-22T18:00:00Z";
        let fixture_rows = 5_000_u32;

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            let mut insert_chain = conn
                .prepare(
                    "INSERT INTO chain_transactions
                     (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                )
                .map_err(|e| DbError::new(format!("Failed to prepare chain tx fixture insert: {e}")))?;
            let mut insert_transfer = conn
                .prepare(
                    "INSERT INTO account_transfers
                     (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, from_address, from_address_id, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'legacy:' || ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                )
                .map_err(|e| DbError::new(format!("Failed to prepare transfer fixture insert: {e}")))?;

            for index in 0..fixture_rows {
                let chain_tx_id = Ulid::new().to_string();
                let tx_hash = tx_hash_for_index(index);
                insert_chain
                    .execute(params![
                        chain_tx_id,
                        SyncedAssetId::Ethereum.as_str(),
                        Network::Mainnet.as_str(),
                        tx_hash,
                        "confirmed",
                        Some(100_i64),
                        Some("block-hash-100".to_string()),
                        Some(now.to_string()),
                        Some(0_i64),
                        Some(0_i64),
                        Option::<i64>::None,
                        now,
                        now,
                    ])
                    .map_err(|e| {
                        DbError::new(format!("Failed to insert chain transaction fixture row: {e}"))
                    })?;

                insert_transfer
                    .execute(params![
                        Ulid::new().to_string(),
                        chain_tx_id,
                        SyncedAssetId::Ethereum.as_str(),
                        Network::Mainnet.as_str(),
                        tx_hash,
                        0_i64,
                        "normal",
                        "0x5555555555555555555555555555555555555555",
                        Some(account.address_id.to_string()),
                        Option::<String>::None,
                        Option::<String>::None,
                        0_i64,
                        i64::from(index),
                        now,
                        now,
                    ])
                    .map_err(|e| DbError::new(format!("Failed to insert transfer fixture row: {e}")))?;
            }

            Ok(())
        })
        .expect("large fixture rows should insert");

        delete_account(user_id, account.account_id).expect("delete account should succeed");

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let remaining_transfers: i64 = conn
                .query_row("SELECT COUNT(*) FROM account_transfers", [], |row| {
                    row.get(0)
                })
                .map_err(|e| DbError::new(format!("Failed to count transfers: {e}")))?;
            let remaining_chain_txs: i64 = conn
                .query_row("SELECT COUNT(*) FROM chain_transactions", [], |row| {
                    row.get(0)
                })
                .map_err(|e| DbError::new(format!("Failed to count chain tx rows: {e}")))?;

            assert_eq!(remaining_transfers, 0);
            assert_eq!(remaining_chain_txs, 0);
            Ok(())
        })
        .expect("large delete assertions should succeed");
    }

    #[test]
    fn delete_account_removes_manual_asset_account() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let native_account = add_ethereum_address(
            user_id,
            &test_eth_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Custom Delete Wallet")),
            Utc::now(),
        )
        .expect("native wallet account should be created");

        let custom_account = add_manual_asset_account(
            user_id,
            ValidatedAddManualAssetAccountRequest {
                wallet_id: Some(native_account.wallet_id),
                wallet_label: None,
                account_label: None,
                asset: crate::wallets::ValidatedAddManualAssetAccountAsset::BitGarthCatalog {
                    candidate_id:
                        crate::asset_capabilities::ManualAssetCatalogCandidateId::Unsynced(
                            crate::asset_capabilities::unsynced::UnsyncedAssetInstanceId {
                                asset_id:
                                    crate::asset_capabilities::unsynced::UnsyncedAssetId::parse(
                                        "cardano",
                                    )
                                    .expect("asset id"),
                                network_id:
                                    crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(
                                        "cardano-mainnet",
                                    )
                                    .expect("network id"),
                            },
                        ),
                },
            },
            Utc::now(),
        )
        .expect("manual asset account should be created");

        delete_account(user_id, custom_account.account_id)
            .expect("custom account delete should succeed");

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let remaining: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM manual_asset_accounts WHERE id = ?1",
                    params![custom_account.account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    DbError::new(format!("Failed to count remaining manual accounts: {e}"))
                })?;
            assert_eq!(remaining, 0);
            Ok(())
        })
        .expect("custom account should be gone after delete");
    }

    #[test]
    fn delete_account_preserves_shared_transfer_and_chain_transaction_for_other_account() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let account_a = add_ethereum_address(
            user_id,
            &test_eth_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Shared A")),
            Utc::now(),
        )
        .expect("account A should be created");
        let account_b = add_ethereum_address(
            user_id,
            &second_eth_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Shared B")),
            Utc::now(),
        )
        .expect("account B should be created");

        let chain_tx_id = Ulid::new().to_string();
        let tx_hash = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let now = "2026-02-22T17:05:00Z";

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            insert_chain_transaction_fixture(conn, &chain_tx_id, tx_hash, now)?;

            conn.execute(
                "INSERT INTO account_transfers
                 (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, from_address, from_address_id, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'legacy:' || ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    Ulid::new().to_string(),
                    chain_tx_id,
                    SyncedAssetId::Ethereum.as_str(),
                    Network::Mainnet.as_str(),
                    tx_hash,
                    0_i64,
                    "normal",
                    "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed",
                    Some(account_a.address_id.to_string()),
                    "0xde0b295669a9fd93d5f28d9ec85e40f4cb697bae",
                    Some(account_b.address_id.to_string()),
                    0_i64,
                    50_i64,
                    now,
                    now,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert shared transfer fixture: {e}")))?;
            Ok(())
        })
        .expect("fixture rows should insert");

        delete_account(user_id, account_a.account_id).expect("delete account A should succeed");

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let row = conn
                .query_row(
                    "SELECT from_address_id, to_address_id
                     FROM account_transfers
                     WHERE chain_transaction_id = ?1",
                    params![chain_tx_id],
                    |r| {
                        Ok((
                            r.get::<_, Option<String>>(0)?,
                            r.get::<_, Option<String>>(1)?,
                        ))
                    },
                )
                .map_err(|e| DbError::new(format!("Failed to load transfer row: {e}")))?;

            assert_eq!(row.0, None);
            assert_eq!(row.1, Some(account_b.address_id.to_string()));

            let chain_tx_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM chain_transactions WHERE id = ?1",
                    params![chain_tx_id],
                    |r| r.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count chain tx rows: {e}")))?;
            assert_eq!(chain_tx_count, 1);

            Ok(())
        })
        .expect("shared transfer assertions should succeed");
    }

    #[test]
    fn deleting_last_shared_account_removes_transfer_and_chain_transaction() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let account_a = add_ethereum_address(
            user_id,
            &test_eth_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Last Shared A")),
            Utc::now(),
        )
        .expect("account A should be created");
        let account_b = add_ethereum_address(
            user_id,
            &second_eth_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Last Shared B")),
            Utc::now(),
        )
        .expect("account B should be created");

        let chain_tx_id = Ulid::new().to_string();
        let tx_hash = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
        let now = "2026-02-22T17:10:00Z";

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            insert_chain_transaction_fixture(conn, &chain_tx_id, tx_hash, now)?;
            conn.execute(
                "INSERT INTO account_transfers
                 (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, from_address, from_address_id, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'legacy:' || ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    Ulid::new().to_string(),
                    chain_tx_id,
                    SyncedAssetId::Ethereum.as_str(),
                    Network::Mainnet.as_str(),
                    tx_hash,
                    0_i64,
                    "normal",
                    "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed",
                    Some(account_a.address_id.to_string()),
                    "0xde0b295669a9fd93d5f28d9ec85e40f4cb697bae",
                    Some(account_b.address_id.to_string()),
                    0_i64,
                    75_i64,
                    now,
                    now,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert shared transfer fixture: {e}")))?;
            Ok(())
        })
        .expect("fixture rows should insert");

        delete_account(user_id, account_a.account_id).expect("delete account A should succeed");
        delete_account(user_id, account_b.account_id).expect("delete account B should succeed");

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let transfer_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM account_transfers WHERE chain_transaction_id = ?1",
                    params![chain_tx_id],
                    |r| r.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count transfer rows: {e}")))?;
            let chain_tx_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM chain_transactions WHERE id = ?1",
                    params![chain_tx_id],
                    |r| r.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count chain tx rows: {e}")))?;
            assert_eq!(transfer_count, 0);
            assert_eq!(chain_tx_count, 0);
            Ok(())
        })
        .expect("cleanup assertions should succeed");
    }

    #[test]
    fn delete_wallet_removes_shared_wallet_transfer_data() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let first = add_ethereum_address(
            user_id,
            &test_eth_address(),
            Network::Mainnet,
            None,
            Some(
                &Label::parse_with_limit("Delete Wallet", WALLET_LABEL_MAX_LENGTH).expect("valid"),
            ),
            Utc::now(),
        )
        .expect("first account should be created");
        let second = add_ethereum_address(
            user_id,
            &second_eth_address(),
            Network::Mainnet,
            Some(&first.wallet_id),
            None,
            Utc::now(),
        )
        .expect("second account should be created in same wallet");

        let chain_tx_id = Ulid::new().to_string();
        let tx_hash = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        let now = "2026-02-22T17:15:00Z";

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            insert_chain_transaction_fixture(conn, &chain_tx_id, tx_hash, now)?;
            conn.execute(
                "INSERT INTO account_transfers
                 (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, from_address, from_address_id, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'legacy:' || ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    Ulid::new().to_string(),
                    chain_tx_id,
                    SyncedAssetId::Ethereum.as_str(),
                    Network::Mainnet.as_str(),
                    tx_hash,
                    0_i64,
                    "normal",
                    "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed",
                    Some(first.address_id.to_string()),
                    "0xde0b295669a9fd93d5f28d9ec85e40f4cb697bae",
                    Some(second.address_id.to_string()),
                    0_i64,
                    99_i64,
                    now,
                    now,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert wallet transfer fixture: {e}")))?;
            Ok(())
        })
        .expect("fixture rows should insert");

        delete_wallet(user_id, first.wallet_id).expect("wallet delete should succeed");

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let wallet_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM wallets WHERE id = ?1",
                    params![first.wallet_id.to_string()],
                    |r| r.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count wallet rows: {e}")))?;
            let account_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM digital_asset_accounts WHERE id IN (?1, ?2)",
                    params![first.account_id.to_string(), second.account_id.to_string()],
                    |r| r.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count account rows: {e}")))?;
            let transfer_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM account_transfers WHERE chain_transaction_id = ?1",
                    params![chain_tx_id],
                    |r| r.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count transfer rows: {e}")))?;
            let chain_tx_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM chain_transactions WHERE id = ?1",
                    params![chain_tx_id],
                    |r| r.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to count chain tx rows: {e}")))?;

            assert_eq!(wallet_count, 0);
            assert_eq!(account_count, 0);
            assert_eq!(transfer_count, 0);
            assert_eq!(chain_tx_count, 0);
            Ok(())
        })
        .expect("wallet cleanup assertions should succeed");
    }
}
