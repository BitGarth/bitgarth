mod address_loading;
mod chain_tip;
mod hd_chain;
mod mempool_history;
mod parsers;
mod reconciliation;
mod snapshots;
mod sync_state;
mod types;

pub(super) const ADDRESS_SYNC_SCOPE: &str = "address";
/// Number of records to reconcile while holding the per-user DB mutex before
/// releasing it so request handlers can interleave reads during very large syncs.
pub(super) const RECONCILE_LOCK_BATCH_SIZE: usize = 250;

// Re-export all pub(crate) items needed by src/db/mod.rs
pub(in crate::db) use address_loading::load_canonical_account_transaction_count_bounded_conn;
pub(crate) use address_loading::{
    account_has_incomplete_mempool_history_with_conn, address_has_pending_txs,
    get_non_hd_sync_addresses, get_sync_addresses_for_account, load_account_ids_with_pending_txs,
    load_account_labels, load_account_mempool_expected_tx_count, load_account_reported_tx_counts,
    load_address_ids_with_activity, load_address_ids_with_pending_txs,
    load_api_confirmed_balances_for_account_conn, load_canonical_account_transaction_count_bounded,
    load_canonical_confirmed_account_transaction_count, load_confirmed_tx_hashes_for_address,
    load_known_tx_hashes_for_address,
};
pub(crate) use chain_tip::{load_chain_tip_state, upsert_chain_tip_state};
pub(in crate::db) use hd_chain::complete_hd_account_discovery_conn;
pub(crate) use hd_chain::{
    complete_hd_account_discovery, delete_hd_account_chain_sync_state, get_hd_account_sync_bundles,
    load_hd_account_chain_sync_state, upsert_hd_account_chain_sync_state,
};
pub(crate) use mempool_history::{
    BitcoinAccountHistoryCoverage, StrictMempoolScanValidation,
    restart_strict_mempool_history_scan, validate_strict_mempool_history_scan,
};
#[cfg(any(
    all(feature = "server", feature = "dev-config", not(test)),
    all(test, feature = "db-tests")
))]
pub(crate) use reconciliation::reconcile_address_transactions;
pub(crate) use reconciliation::{
    reconcile_account_transactions, reconcile_account_transactions_conn,
    reconcile_address_transactions_preserving_invalidation,
};
pub(crate) use snapshots::{load_account_sync_snapshots, load_aggregate_sync_snapshot};
pub(crate) use sync_state::{
    MempoolAddressObservationSuccess, begin_mempool_history_scan, commit_mempool_history_page_work,
    invalidate_mempool_account_history_coverage, invalidate_mempool_history_coverage,
    invalidate_mempool_history_proof, mark_account_integration_sync_started,
    mark_address_sync_completed_failure, mark_address_sync_completed_success,
    mark_address_sync_started, persist_mempool_address_observation_success,
    publish_mempool_history_proof, publish_strict_mempool_history_proof,
    refresh_account_integration_sync_state, update_address_etherscan_backfill_cursor,
    update_address_etherscan_history_status, update_address_mempool_backfill_cursor,
    update_address_mempool_expected_tx_count, upsert_account_sync_state,
};
pub(in crate::db) use sync_state::{
    publish_mempool_history_proof_conn, publish_strict_mempool_history_proof_conn,
};
#[cfg(all(test, feature = "db-tests"))]
pub(crate) use types::AccountSyncStateRow;
pub(crate) use types::{
    AccountIntegrationSyncStart, AccountSyncBundle, AddressApiConfirmedBalanceRow,
    AddressSyncSuccess, CoverageInvalidationTargets, HdAccountChainFrontierPhase,
    HdAccountChainSyncState, HdMempoolHistoryFrontierUpdate, MempoolHistoryPageWorkUpdate,
    MempoolHistoryProof, ProviderTransferKey, SyncAccountTransactionRecord,
    SyncAccountTransferRecord, SyncAddress, SyncTransactionInputRecord,
    SyncTransactionOutputRecord, SyncTransactionRecord, TransactionSyncReconcileSummary,
};

#[cfg(all(test, feature = "db-tests"))]
use super::raw_ingestion::{
    deactivate_source_connection_for_address_tx, ensure_source_connection_for_address_tx,
};

#[cfg(all(test, feature = "db-tests"))]
fn parse_tracked_address(value: &str) -> crate::transactions::TrackedAddress {
    crate::transactions::TrackedAddress::parse(value).expect("test address should parse")
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::super::error::DbError;
    use super::super::user_db::{with_user_db, with_user_db_mut};
    use super::*;
    use crate::amounts::UnsignedAmount;
    use crate::db::raw_ingestion::{
        ExactPayloadBytes, InsertRawMempoolTransactionVersionRequest, IntegrationKind,
        MempoolPageKind, MempoolPageObservationMetadata, PayloadSha256Hex,
        RecordRawMempoolPageObservationRequest, SourceConnectionId, StartSyncRunRequest, SyncRunId,
        SyncRunScopeKind, SyncRunTriggerKind, insert_raw_mempool_tx_version,
        record_raw_mempool_page_observation, start_sync_run,
    };
    use crate::db::{
        acquire_test_runtime, add_bitcoin_address, create_eth_wallet_account_fixture,
        initialize_user_db_for_test,
    };
    use crate::ethereum::{EthAddress, RawEthAddress, TransferKind};
    use crate::models::UserId;
    use crate::transactions::{
        AccountSyncResult, AddressBackfillCursor, AggregateSyncResult, ApiConfirmedBalance,
        ChainTipHeight, ChainTransactionStatus, EthereumBlockNumber, EtherscanHistoryStatus,
        MempoolCursorTxid, SyncErrorMessage, SyncIntegrationId, TrackedAddress, TransactionCount,
        TransactionSyncRunId, TxHash,
    };
    use crate::wallets::{
        AddressScheme, BtcAddress, DigitalAssetAccountId, DigitalAssetAddressId, Label, Network,
        RawBtcAddress, SyncedAssetId, WALLET_LABEL_MAX_LENGTH,
    };
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use rusqlite::{OptionalExtension, params};
    use std::collections::HashSet;
    use ulid::Ulid;

    fn test_now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("valid timestamp")
    }

    fn parse_eth_address(value: &str) -> EthAddress {
        let raw = RawEthAddress::new(value.to_string());
        EthAddress::parse(&raw).expect("test eth address should parse")
    }

    fn parse_btc_address(value: &str) -> BtcAddress {
        let raw = RawBtcAddress::new(value.to_string());
        BtcAddress::parse(&raw, Network::Mainnet).expect("test btc address should parse")
    }

    fn parse_wallet_label(value: &str) -> Label {
        Label::parse_with_limit(value, WALLET_LABEL_MAX_LENGTH).expect("test label should parse")
    }

    fn parse_tx_hash(value: &str) -> TxHash {
        TxHash::parse(value).expect("test tx hash should parse")
    }

    fn load_api_confirmed_balances_for_account(
        user_id: UserId,
        account_id: DigitalAssetAccountId,
    ) -> Result<Vec<AddressApiConfirmedBalanceRow>, DbError> {
        with_user_db(user_id, |conn| {
            load_api_confirmed_balances_for_account_conn(conn, account_id)
        })
    }

    #[test]
    fn account_transfer_merge_uses_provider_identity_across_watched_addresses() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user DB should initialize");
        let now = test_now();
        let address_a = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let address_b = parse_eth_address("0x8617E340B3D01FA5F11F306F4090FD50E238070D");
        create_eth_wallet_account_fixture(user_id, &address_a, "Provider A", now);
        create_eth_wallet_account_fixture(user_id, &address_b, "Provider B", now);
        let tx_hash =
            parse_tx_hash("1212121212121212121212121212121212121212121212121212121212121212");

        let record = |key: ProviderTransferKey, index: i64, to: &EthAddress, value: u128| {
            SyncAccountTransactionRecord {
                tx_hash: tx_hash.clone(),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(10),
                block_hash: None,
                block_time: Some(now),
                fee_amount: Some(UnsignedAmount::zero()),
                nonce: Some(1),
                transfers: vec![SyncAccountTransferRecord {
                    provider_transfer_key: key,
                    transfer_index: index,
                    transfer_kind: TransferKind::Internal,
                    from_address: None,
                    to_address: Some(parse_tracked_address(&to.checksummed())),
                    value_amount: UnsignedAmount::from_u128(value),
                }],
            }
        };

        let a = record(
            ProviderTransferKey::from_internal_trace_id("0_1").unwrap(),
            1,
            &address_a,
            11,
        );
        let b = record(
            ProviderTransferKey::from_internal_trace_id("1").unwrap(),
            1,
            &address_b,
            22,
        );
        reconcile_account_transactions(
            user_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            std::slice::from_ref(&a),
            now,
        )
        .expect("A should reconcile");
        reconcile_account_transactions(
            user_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            std::slice::from_ref(&b),
            now,
        )
        .expect("B should reconcile");
        reconcile_account_transactions(
            user_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            std::slice::from_ref(&a),
            now,
        )
        .expect("re-syncing A should reconcile");

        let keys = with_user_db(user_id, |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT provider_transfer_key FROM account_transfers ORDER BY provider_transfer_key",
                )
                .map_err(|err| DbError::new(format!("Failed to prepare key query: {err}")))?;
            stmt.query_map([], |row| row.get::<_, String>(0))
                .map_err(|err| DbError::new(format!("Failed to query keys: {err}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| DbError::new(format!("Failed to read keys: {err}")))
        })
        .expect("keys should load");
        assert_eq!(keys, vec!["internal:0_1", "internal:1"]);
    }

    #[test]
    fn observed_transfer_retires_only_its_matching_legacy_identity() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user DB should initialize");
        let now = test_now();
        let address_a = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let address_b = parse_eth_address("0x8617E340B3D01FA5F11F306F4090FD50E238070D");
        let account_a = create_eth_wallet_account_fixture(user_id, &address_a, "Legacy A", now);
        let account_b = create_eth_wallet_account_fixture(user_id, &address_b, "Legacy B", now);
        let tx_hash =
            parse_tx_hash("2323232323232323232323232323232323232323232323232323232323232323");
        let chain_transaction_id = ulid::Ulid::new().to_string();

        with_user_db_mut(user_id, |conn| {
            conn.execute(
                "INSERT INTO chain_transactions
                 (id, asset_id, network, tx_hash, status, block_height, created_at, updated_at)
             VALUES (?1, 'ethereum', 'mainnet', ?2, 'confirmed', 10, ?3, ?3)",
                params![chain_transaction_id, tx_hash.as_str(), now.to_rfc3339()],
            )
            .map_err(|err| DbError::new(format!("Failed to seed chain transaction: {err}")))?;
            conn.execute(
                "INSERT INTO account_transfers
                 (id, chain_transaction_id, asset_id, network, tx_hash,
                  provider_transfer_key, transfer_index, transfer_kind,
                  to_address, to_address_id, value_amount_hi, value_amount_lo,
                  created_at, updated_at)
             VALUES (?1, ?2, 'ethereum', 'mainnet', ?3, 'legacy:2', 2,
                     'internal', ?4, ?5, 0, 11, ?6, ?6),
                    (?7, ?2, 'ethereum', 'mainnet', ?3, 'legacy:3', 3,
                     'internal', ?8, ?9, 0, 22, ?6, ?6)",
                params![
                    ulid::Ulid::new().to_string(),
                    chain_transaction_id,
                    tx_hash.as_str(),
                    address_a.checksummed(),
                    account_a.address_id.to_string(),
                    now.to_rfc3339(),
                    ulid::Ulid::new().to_string(),
                    address_b.checksummed(),
                    account_b.address_id.to_string(),
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to seed legacy transfers: {err}")))?;
            Ok::<(), DbError>(())
        })
        .expect("legacy fixtures should seed");

        let record = |trace_id: &str, to: &EthAddress, value: u128| SyncAccountTransactionRecord {
            tx_hash: tx_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(10),
            block_hash: None,
            block_time: Some(now),
            fee_amount: Some(UnsignedAmount::zero()),
            nonce: Some(1),
            transfers: vec![SyncAccountTransferRecord {
                provider_transfer_key: ProviderTransferKey::from_internal_trace_id(trace_id)
                    .unwrap(),
                transfer_index: 1,
                transfer_kind: TransferKind::Internal,
                from_address: None,
                to_address: Some(parse_tracked_address(&to.checksummed())),
                value_amount: UnsignedAmount::from_u128(value),
            }],
        };
        let load_keys = || {
            with_user_db(user_id, |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT provider_transfer_key
                     FROM account_transfers
                     ORDER BY provider_transfer_key",
                    )
                    .map_err(|err| DbError::new(format!("Failed to prepare key query: {err}")))?;
                stmt.query_map([], |row| row.get::<_, String>(0))
                    .map_err(|err| DbError::new(format!("Failed to query keys: {err}")))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|err| DbError::new(format!("Failed to read keys: {err}")))
            })
            .expect("provider keys should load")
        };

        reconcile_account_transactions(
            user_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            &[record("0_1", &address_a, 11)],
            now,
        )
        .expect("A should reconcile");
        assert_eq!(load_keys(), vec!["internal:0_1", "legacy:3"]);

        reconcile_account_transactions(
            user_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            &[record("2", &address_b, 22)],
            now,
        )
        .expect("B should reconcile");
        assert_eq!(load_keys(), vec!["internal:0_1", "internal:2"]);
    }

    fn insert_extra_eth_address(
        user_id: UserId,
        account_id: DigitalAssetAccountId,
        address: &str,
        now: DateTime<Utc>,
    ) -> DigitalAssetAddressId {
        let address_id = DigitalAssetAddressId::new();
        with_user_db_mut(user_id, |conn| {
            let tx = conn
                .transaction()
                .map_err(|err| DbError::new(format!("Failed to start test address tx: {err}")))?;
            tx.execute(
                "INSERT INTO digital_asset_addresses
                 (id, account_id, asset_id, network, address, address_normalized, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, ?8, ?9, ?10)",
                params![
                    address_id.to_string(),
                    account_id.to_string(),
                    SyncedAssetId::Ethereum.as_str(),
                    Network::Mainnet.as_str(),
                    address,
                    address.to_ascii_lowercase(),
                    AddressScheme::Standard.as_str(),
                    "imported",
                    now.to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to insert test address: {err}")))?;
            ensure_source_connection_for_address_tx(
                &tx,
                address_id,
                SyncedAssetId::Ethereum,
                Network::Mainnet,
                &address.to_ascii_lowercase(),
                now,
            )?;
            tx.commit()
                .map_err(|err| DbError::new(format!("Failed to commit test address tx: {err}")))?;
            Ok::<(), DbError>(())
        })
        .expect("test address insert should succeed");
        address_id
    }

    #[test]
    fn get_non_hd_sync_addresses_excludes_inactive_source_connections() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let add_result =
            create_eth_wallet_account_fixture(user_id, &address, "Inactive Source", now);

        let active_addresses =
            get_non_hd_sync_addresses(user_id).expect("active sync addresses should load");
        assert_eq!(active_addresses.len(), 1);
        assert_eq!(active_addresses[0].address_id, add_result.address_id);

        with_user_db_mut(user_id, |conn| {
            let tx = conn.transaction().map_err(|err| {
                DbError::new(format!(
                    "Failed to start source deactivation test tx: {err}"
                ))
            })?;
            deactivate_source_connection_for_address_tx(&tx, add_result.address_id, now)?;
            tx.commit().map_err(|err| {
                DbError::new(format!(
                    "Failed to commit source deactivation test tx: {err}"
                ))
            })?;
            Ok::<(), DbError>(())
        })
        .expect("source connection deactivation should succeed");

        let inactive_addresses =
            get_non_hd_sync_addresses(user_id).expect("inactive sync addresses should load");
        assert!(inactive_addresses.is_empty());
    }

    #[test]
    fn address_failure_count_increments_skips_rate_limits_and_resets_on_success() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let fixture = create_eth_wallet_account_fixture(user_id, &address, "Failure Count", now);

        let first_run = TransactionSyncRunId::new();
        mark_address_sync_started(user_id, fixture.address_id, first_run, now)
            .expect("first start should persist");
        mark_address_sync_completed_failure(
            user_id,
            fixture.address_id,
            first_run,
            now,
            now + Duration::seconds(1),
            &SyncErrorMessage::sanitize("address parse failure"),
            true,
        )
        .expect("first failure should persist");

        let rate_limit_run = TransactionSyncRunId::new();
        mark_address_sync_completed_failure(
            user_id,
            fixture.address_id,
            rate_limit_run,
            now + Duration::seconds(2),
            now + Duration::seconds(3),
            &SyncErrorMessage::sanitize("rate limited"),
            false,
        )
        .expect("rate limit failure should persist without incrementing");

        let second_run = TransactionSyncRunId::new();
        mark_address_sync_completed_failure(
            user_id,
            fixture.address_id,
            second_run,
            now + Duration::seconds(4),
            now + Duration::seconds(5),
            &SyncErrorMessage::sanitize("provider parse failure"),
            true,
        )
        .expect("second failure should persist");

        let loaded = get_sync_addresses_for_account(user_id, fixture.account_id)
            .expect("addresses should load");
        assert_eq!(loaded[0].consecutive_failure_count.value(), 2);

        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: fixture.address_id,
                run_id: TransactionSyncRunId::new(),
                started_at: now + Duration::seconds(6),
                completed_at: now + Duration::seconds(7),
                last_tip_height: ChainTipHeight::try_new(1).expect("tip should parse"),
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: None,
            },
        )
        .expect("success should reset failure count");

        let reloaded = get_sync_addresses_for_account(user_id, fixture.account_id)
            .expect("addresses should reload");
        assert_eq!(reloaded[0].consecutive_failure_count.value(), 0);
    }

    #[test]
    fn canonical_account_transaction_count_ignores_stale_ledger_projection() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let now_raw = now.to_rfc3339();
        let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let fixture = create_eth_wallet_account_fixture(user_id, &address, "Canonical Count", now);

        with_user_db_mut(user_id, |conn| {
            let linked_tx_id = Ulid::new().to_string();
            let stale_projection_tx_id = Ulid::new().to_string();
            for (tx_id, tx_hash) in [
                (
                    linked_tx_id.as_str(),
                    "0x1111111111111111111111111111111111111111111111111111111111111111",
                ),
                (
                    stale_projection_tx_id.as_str(),
                    "0x2222222222222222222222222222222222222222222222222222222222222222",
                ),
            ] {
                conn.execute(
                    "INSERT INTO chain_transactions
                     (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'confirmed', ?5, ?6, ?7, 0, 0, NULL, ?8, ?9)",
                    params![
                        tx_id,
                        SyncedAssetId::Ethereum.as_str(),
                        Network::Mainnet.as_str(),
                        tx_hash,
                        1_i64,
                        "block",
                        now_raw,
                        now_raw,
                        now_raw,
                    ],
                )
                .map_err(|err| DbError::new(format!("Failed to insert chain tx: {err}")))?;
            }

            conn.execute(
                "INSERT INTO account_transfers
                 (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index, provider_transfer_key, transfer_kind, from_address, from_address_id, to_address, to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, 'legacy:0', 'normal', NULL, NULL, ?6, ?7, 0, 1, ?8, ?9)",
                params![
                    Ulid::new().to_string(),
                    linked_tx_id,
                    SyncedAssetId::Ethereum.as_str(),
                    Network::Mainnet.as_str(),
                    "0x1111111111111111111111111111111111111111111111111111111111111111",
                    address.checksummed(),
                    fixture.address_id.to_string(),
                    now_raw,
                    now_raw,
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to insert account transfer: {err}")))?;

            conn.execute(
                "INSERT INTO account_transaction_ledger
                 (id, account_id, chain_transaction_id, asset_id, network, tx_hash, status, occurred_at, first_seen_at, block_height, nonce, min_transfer_index, tx_type, from_addresses_json, to_addresses_json, value_amount_hi, value_amount_lo, fee_amount_hi, fee_amount_lo, closing_balance_hi, closing_balance_lo, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'confirmed', ?7, ?8, 1, NULL, 0, 'receive', '[]', '[]', 0, 1, 0, 0, 0, 1, ?9, ?10)",
                params![
                    Ulid::new().to_string(),
                    fixture.account_id.to_string(),
                    stale_projection_tx_id,
                    SyncedAssetId::Ethereum.as_str(),
                    Network::Mainnet.as_str(),
                    "0x2222222222222222222222222222222222222222222222222222222222222222",
                    now_raw,
                    now_raw,
                    now_raw,
                    now_raw,
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to insert stale ledger row: {err}")))?;

            Ok::<(), DbError>(())
        })
        .expect("canonical count fixture should persist");

        let count = load_canonical_confirmed_account_transaction_count(user_id, fixture.account_id)
            .expect("canonical confirmed count should load");
        assert_eq!(count.value(), 1);
    }

    #[test]
    fn breadth_cap_boundary_count_query_stops_at_requested_limit() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_btc_address("1BoatSLRHtKNngkdXEeobR76b53LETtpyT");
        let label = parse_wallet_label("Bounded count");
        let fixture =
            add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
                .expect("bitcoin fixture should insert");

        for index in 0..3_u32 {
            let record = SyncTransactionRecord {
                tx_hash: parse_tx_hash(&format!("{:064x}", index + 1)),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(i64::from(index) + 1),
                block_hash: Some(format!("block-{index}")),
                block_time: Some(now),
                fee_amount: Some(0),
                inputs: Vec::new(),
                outputs: vec![SyncTransactionOutputRecord {
                    output_index: 0,
                    raw_address: Some(parse_tracked_address(address.canonical())),
                    script_pubkey_hex: "0014deadbeef".to_string(),
                    value_amount: 1,
                }],
            };
            reconcile_address_transactions(
                user_id,
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                &[record],
                now,
            )
            .expect("canonical fixture should reconcile");
        }

        assert_eq!(
            load_canonical_account_transaction_count_bounded(
                user_id,
                fixture.account_id,
                TransactionCount::from_u32(2),
            )
            .expect("bounded count should load"),
            TransactionCount::from_u32(2),
        );
        assert_eq!(
            load_canonical_account_transaction_count_bounded(
                user_id,
                fixture.account_id,
                TransactionCount::from_u32(10),
            )
            .expect("full bounded count should load"),
            TransactionCount::from_u32(3),
        );
    }

    #[test]
    fn account_reported_tx_counts_use_latest_confirmed_per_address_observations() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_btc_address("1BoatSLRHtKNngkdXEeobR76b53LETtpyT");
        let label = parse_wallet_label("Reported count");
        let fixture =
            add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
                .expect("bitcoin fixture should insert");
        mark_address_sync_started(
            user_id,
            fixture.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("address sync state should insert");

        persist_mempool_address_observation_success(
            user_id,
            MempoolAddressObservationSuccess {
                address_id: fixture.address_id,
                confirmed_tx_count: TransactionCount::from_u32(2),
                confirmed_balance: None,
                tip_height: ChainTipHeight::try_new(800_000).expect("height should parse"),
                observed_at: now,
            },
        )
        .expect("mempool observation should persist");

        assert_eq!(
            load_account_reported_tx_counts(user_id, fixture.account_id)
                .expect("reported counts should load"),
            vec![TransactionCount::from_u32(2)]
        );
    }

    #[test]
    fn coverage_invalidation_clears_proof_and_both_bitcoin_closing_limbs() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let now_raw = now.to_rfc3339();
        let address = parse_btc_address("1BoatSLRHtKNngkdXEeobR76b53LETtpyT");
        let label = parse_wallet_label("Invalidated proof");
        let fixture =
            add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
                .expect("bitcoin fixture should insert");
        mark_address_sync_started(
            user_id,
            fixture.address_id,
            TransactionSyncRunId::new(),
            now,
        )
        .expect("address sync state should insert");
        publish_mempool_history_proof(
            user_id,
            fixture.address_id,
            MempoolHistoryProof {
                confirmed_tx_count: TransactionCount::from_u32(1),
                complete_height: ChainTipHeight::try_new(800_000).expect("height should parse"),
            },
        )
        .expect("proof should publish");

        with_user_db_mut(user_id, |conn| {
            let chain_transaction_id = Ulid::new().to_string();
            let tx_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
            conn.execute(
                "INSERT INTO chain_transactions
                 (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time,
                  fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'confirmed', 1, 'block', ?5, 0, 0, NULL, ?5, ?5)",
                params![
                    chain_transaction_id,
                    SyncedAssetId::Bitcoin.as_str(),
                    Network::Mainnet.as_str(),
                    tx_hash,
                    now_raw,
                ],
            )
            .map_err(|err| DbError::new(format!("chain transaction insert failed: {err}")))?;
            conn.execute(
                "INSERT INTO account_transaction_ledger
                 (id, account_id, chain_transaction_id, asset_id, network, tx_hash, status,
                  occurred_at, first_seen_at, block_height, nonce, min_transfer_index, tx_type,
                  from_addresses_json, to_addresses_json, value_amount_hi, value_amount_lo,
                  fee_amount_hi, fee_amount_lo, closing_balance_hi, closing_balance_lo,
                  created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'confirmed', ?7, ?7, 1, NULL, 0, 'receive',
                         '[]', '[]', 0, 1, 0, 0, 0, 1, ?7, ?7)",
                params![
                    Ulid::new().to_string(),
                    fixture.account_id.to_string(),
                    chain_transaction_id,
                    SyncedAssetId::Bitcoin.as_str(),
                    Network::Mainnet.as_str(),
                    tx_hash,
                    now_raw,
                ],
            )
            .map_err(|err| DbError::new(format!("ledger row insert failed: {err}")))?;
            Ok::<(), DbError>(())
        })
        .expect("closing-balance fixture should persist");

        invalidate_mempool_history_coverage(
            user_id,
            &CoverageInvalidationTargets {
                address_ids: HashSet::from([fixture.address_id]),
                account_ids: HashSet::from([fixture.account_id]),
            },
        )
        .expect("coverage should invalidate");

        assert_eq!(
            get_sync_addresses_for_account(user_id, fixture.account_id)
                .expect("address should load")[0]
                .mempool_history_proof,
            None
        );
        let closing = with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT closing_balance_hi, closing_balance_lo
                 FROM account_transaction_ledger
                 WHERE account_id = ?1",
                [fixture.account_id.to_string()],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .map_err(|err| DbError::new(format!("closing balance query failed: {err}")))
        })
        .expect("closing balance should load");
        assert_eq!(closing, (None, None));
    }

    #[test]
    fn interrupted_account_recovery_is_reported_before_new_start_overwrites_it() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_btc_address("1BoatSLRHtKNngkdXEeobR76b53LETtpyT");
        let fixture = add_bitcoin_address(
            user_id,
            &address,
            Network::Mainnet,
            None,
            Some(&parse_wallet_label("Interrupted account")),
            now,
        )
        .expect("bitcoin fixture should insert");

        let initial = mark_account_integration_sync_started(
            user_id,
            fixture.account_id,
            SyncIntegrationId::Mempool,
            now,
        )
        .expect("initial start should persist");
        assert!(!initial.was_interrupted);

        let recovery = mark_account_integration_sync_started(
            user_id,
            fixture.account_id,
            SyncIntegrationId::Mempool,
            now + Duration::seconds(1),
        )
        .expect("recovery start should persist");
        assert!(recovery.was_interrupted);
    }

    #[test]
    fn hd_account_chain_sync_state_roundtrips_and_deletes() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let fixture = create_eth_wallet_account_fixture(user_id, &address, "HD Frontier", now);
        let account_id = fixture.account_id;

        let frontier_state = HdAccountChainSyncState::DerivedAddresses {
            next_index_to_scan: 7,
            consecutive_unused: 3,
        };
        upsert_hd_account_chain_sync_state(user_id, account_id, 0, &frontier_state, now)
            .expect("frontier state should persist");

        let loaded = load_hd_account_chain_sync_state(user_id, account_id, 0)
            .expect("frontier state should load")
            .expect("frontier state should exist");
        assert_eq!(loaded.account_id, account_id);
        assert_eq!(loaded.derivation_change, 0);
        assert_eq!(loaded.frontier_state, frontier_state);
        assert_eq!(loaded.updated_at, now);

        delete_hd_account_chain_sync_state(user_id, account_id, 0)
            .expect("frontier state should delete");
        assert!(
            load_hd_account_chain_sync_state(user_id, account_id, 0)
                .expect("frontier state should reload")
                .is_none()
        );
    }

    #[test]
    fn load_account_sync_snapshots_marks_in_progress_and_never_synced() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let first_address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let add_result =
            create_eth_wallet_account_fixture(user_id, &first_address, "Sync Snapshot", now);
        let in_progress_address = insert_extra_eth_address(
            user_id,
            add_result.account_id,
            "0x8617E340B3D01FA5F11F306F4090FD50E238070D",
            now,
        );

        let run_id = TransactionSyncRunId::new();
        mark_address_sync_started(user_id, in_progress_address, run_id, now)
            .expect("mark start should succeed");

        let snapshots = load_account_sync_snapshots(user_id).expect("snapshots should load");
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.account_id, add_result.account_id);
        assert_eq!(snapshot.addresses_total.value(), 2);
        assert_eq!(snapshot.addresses_never_synced.value(), 1);
        assert_eq!(snapshot.addresses_in_progress.value(), 1);
        assert_eq!(snapshot.addresses_synced.value(), 0);
        assert_eq!(snapshot.addresses_failed.value(), 0);
        assert_eq!(
            snapshot.last_result,
            Some(crate::transactions::AccountSyncResult::InProgress)
        );
        assert!(snapshot.backfill_progress.is_none());
    }

    #[test]
    fn load_account_sync_snapshots_exposes_max_consecutive_failures() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let first_address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let add_result =
            create_eth_wallet_account_fixture(user_id, &first_address, "Failure Streak", now);
        let second_address_id = insert_extra_eth_address(
            user_id,
            add_result.account_id,
            "0x8617E340B3D01FA5F11F306F4090FD50E238070D",
            now,
        );

        let run_id = TransactionSyncRunId::new();
        mark_address_sync_started(user_id, add_result.address_id, run_id, now)
            .expect("mark start should succeed");
        mark_address_sync_started(user_id, second_address_id, run_id, now)
            .expect("mark start should succeed");
        with_user_db_mut(user_id, |conn| {
            conn.execute(
                "UPDATE transaction_sync_state SET consecutive_failure_count = 1
                 WHERE address_id = ?1",
                params![add_result.address_id.to_string()],
            )
            .map_err(|err| DbError::new(format!("test update failed: {err}")))?;
            conn.execute(
                "UPDATE transaction_sync_state SET consecutive_failure_count = 3
                 WHERE address_id = ?1",
                params![second_address_id.to_string()],
            )
            .map_err(|err| DbError::new(format!("test update failed: {err}")))?;
            Ok::<(), DbError>(())
        })
        .expect("test updates should succeed");

        let snapshots = load_account_sync_snapshots(user_id).expect("snapshots should load");
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].max_consecutive_failures.value(), 3);
    }

    #[test]
    fn load_account_sync_snapshots_keeps_completed_mempool_cursor_as_inactive_progress() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let label = parse_wallet_label("BTC Backfill");
        let add_result =
            add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
                .expect("bitcoin fixture should insert");
        let run_id = TransactionSyncRunId::new();
        let completed_at = now + Duration::seconds(1);
        let cursor = MempoolCursorTxid::parse(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("cursor should parse");

        mark_account_integration_sync_started(
            user_id,
            add_result.account_id,
            SyncIntegrationId::Mempool,
            now,
        )
        .expect("account integration start should persist");
        mark_address_sync_started(user_id, add_result.address_id, run_id, now)
            .expect("mark start should succeed");
        update_address_mempool_backfill_cursor(user_id, add_result.address_id, Some(&cursor))
            .expect("mempool cursor update should succeed");
        update_address_mempool_expected_tx_count(
            user_id,
            add_result.address_id,
            Some(TransactionCount::from_u32(321)),
        )
        .expect("mempool expected count update should succeed");
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: add_result.address_id,
                run_id,
                started_at: now,
                completed_at,
                last_tip_height: ChainTipHeight::try_new(100).expect("tip should parse"),
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: None,
            },
        )
        .expect("address completion should persist");
        refresh_account_integration_sync_state(
            user_id,
            add_result.account_id,
            SyncIntegrationId::Mempool,
            completed_at,
        )
        .expect("account integration completion should refresh");

        let snapshots = load_account_sync_snapshots(user_id).expect("snapshots should load");
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        let backfill = snapshot
            .backfill_progress
            .as_ref()
            .expect("mempool backfill progress should exist");

        assert_eq!(
            backfill.state.cursor,
            AddressBackfillCursor::Mempool {
                cursor_txid: cursor.clone()
            }
        );
        assert_eq!(
            backfill.expected_tx_count(),
            Some(TransactionCount::from_u32(321))
        );
        assert_eq!(backfill.fetched_tx_count, Some(TransactionCount::zero()));
        assert!(!backfill.expected_tx_count_is_lower_bound);
        assert_eq!(snapshot.addresses_in_progress.value(), 0);
        assert_eq!(snapshot.last_result, Some(AccountSyncResult::Success));
        let integration = snapshot
            .integration_states
            .first()
            .expect("mempool integration should exist");
        assert!(!integration.is_active);
        assert_eq!(integration.last_result, Some(AggregateSyncResult::Success));
        assert_eq!(
            integration
                .backfill_progress
                .as_ref()
                .expect("integration cursor progress should remain")
                .state
                .cursor,
            AddressBackfillCursor::Mempool {
                cursor_txid: cursor
            },
        );
    }

    #[test]
    fn v49_bitcoin_history_proof_and_work_state_are_typed_and_atomic() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let first_address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let first_label = parse_wallet_label("BTC History");
        let first = add_bitcoin_address(
            user_id,
            &first_address,
            Network::Mainnet,
            None,
            Some(&first_label),
            now,
        )
        .expect("first bitcoin fixture should insert");
        let second_address = parse_btc_address("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa");
        let second_label = parse_wallet_label("Other BTC History");
        let second = add_bitcoin_address(
            user_id,
            &second_address,
            Network::Mainnet,
            None,
            Some(&second_label),
            now,
        )
        .expect("second bitcoin fixture should insert");

        mark_address_sync_started(user_id, first.address_id, TransactionSyncRunId::new(), now)
            .expect("address sync state should insert");
        with_user_db_mut(user_id, |conn| {
            conn.execute(
                "INSERT INTO account_sync_state
                 (id, account_id, last_scanned_height, last_scanned_time, gap_limit,
                  last_derived_external_index, last_derived_internal_index, created_at, updated_at)
                 VALUES (?1, ?2, NULL, NULL, 20, NULL, NULL, ?3, ?3)",
                params![
                    Ulid::new().to_string(),
                    first.account_id.to_string(),
                    now.to_rfc3339(),
                ],
            )
            .map_err(|err| DbError::new(format!("test account sync state insert failed: {err}")))?;
            Ok::<(), DbError>(())
        })
        .expect("account sync state should insert");

        let loaded = get_sync_addresses_for_account(user_id, first.account_id)
            .expect("sync address should load");
        assert_eq!(loaded[0].mempool_history_proof, None);
        assert_eq!(loaded[0].mempool_history_scan_start_run_id, None);

        let normal_proof = MempoolHistoryProof {
            confirmed_tx_count: TransactionCount::from_u32(3),
            complete_height: ChainTipHeight::try_new(800_000).expect("height should parse"),
        };
        publish_mempool_history_proof(user_id, first.address_id, normal_proof)
            .expect("normal proof should publish");
        assert_eq!(
            get_sync_addresses_for_account(user_id, first.account_id).expect("proof should load")
                [0]
            .mempool_history_proof,
            Some(normal_proof)
        );

        let cursor = MempoolCursorTxid::parse(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("cursor should parse");
        update_address_mempool_backfill_cursor(user_id, first.address_id, Some(&cursor))
            .expect("cursor should seed");
        let strict_run = start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: IntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: first.address_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                trigger_kind: SyncRunTriggerKind::Backfill,
                started_at: now,
                summary_json: None,
            },
        )
        .expect("strict raw sync run should insert")
        .sync_run_id;
        begin_mempool_history_scan(user_id, first.address_id, strict_run)
            .expect("strict scan should begin");

        let begun = get_sync_addresses_for_account(user_id, first.account_id)
            .expect("begun state should load");
        assert_eq!(begun[0].mempool_backfill_cursor_txid, None);
        assert_eq!(begun[0].mempool_history_scan_start_run_id, Some(strict_run));
        assert_eq!(begun[0].mempool_history_proof, Some(normal_proof));

        let replacement_normal_proof = MempoolHistoryProof {
            confirmed_tx_count: TransactionCount::from_u32(4),
            complete_height: ChainTipHeight::try_new(800_001).expect("height should parse"),
        };
        publish_mempool_history_proof(user_id, first.address_id, replacement_normal_proof)
            .expect("normal proof should republish");
        let after_normal = get_sync_addresses_for_account(user_id, first.account_id)
            .expect("normal proof state should load");
        assert_eq!(
            after_normal[0].mempool_history_scan_start_run_id,
            Some(strict_run)
        );
        assert_eq!(
            after_normal[0].mempool_history_proof,
            Some(replacement_normal_proof)
        );

        invalidate_mempool_history_proof(user_id, first.address_id)
            .expect("proof should invalidate");
        let invalidated = get_sync_addresses_for_account(user_id, first.account_id)
            .expect("invalidated state should load");
        assert_eq!(invalidated[0].mempool_history_proof, None);
        assert_eq!(
            invalidated[0].mempool_history_scan_start_run_id,
            Some(strict_run)
        );

        let wrong_run = start_sync_run(
            user_id,
            StartSyncRunRequest {
                integration: IntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: first.address_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                trigger_kind: SyncRunTriggerKind::Backfill,
                started_at: now,
                summary_json: None,
            },
        )
        .expect("second raw sync run should insert")
        .sync_run_id;
        let strict_proof = MempoolHistoryProof {
            confirmed_tx_count: TransactionCount::from_u32(4),
            complete_height: ChainTipHeight::try_new(800_002).expect("height should parse"),
        };
        assert!(
            publish_strict_mempool_history_proof(
                user_id,
                first.address_id,
                wrong_run,
                strict_proof,
            )
            .is_err(),
            "mismatched strict scan run should be rejected"
        );

        publish_strict_mempool_history_proof(user_id, first.address_id, strict_run, strict_proof)
            .expect("matching strict proof should publish");
        let completed = get_sync_addresses_for_account(user_id, first.account_id)
            .expect("strict proof state should load");
        assert_eq!(completed[0].mempool_history_proof, Some(strict_proof));
        assert_eq!(completed[0].mempool_history_scan_start_run_id, None);

        let first_page_cursor = MempoolCursorTxid::parse(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .expect("page cursor should parse");
        assert!(
            commit_mempool_history_page_work(
                user_id,
                MempoolHistoryPageWorkUpdate {
                    address_id: first.address_id,
                    next_cursor: Some(first_page_cursor.clone()),
                    hd_frontier: Some(HdMempoolHistoryFrontierUpdate {
                        account_id: first.account_id,
                        next_address_id: Some(second.address_id),
                    }),
                },
            )
            .is_err(),
            "frontier address owned by another account should be rejected"
        );

        let state_after_rejected_page = get_sync_addresses_for_account(user_id, first.account_id)
            .expect("rejected page state should load");
        assert_eq!(
            state_after_rejected_page[0].mempool_backfill_cursor_txid,
            None
        );
        let frontier_after_rejected_page: Option<String> = with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT mempool_history_next_address_id
                     FROM account_sync_state WHERE account_id = ?1",
                params![first.account_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|err| DbError::new(format!("test frontier load failed: {err}")))
        })
        .expect("frontier should load");
        assert_eq!(frontier_after_rejected_page, None);

        commit_mempool_history_page_work(
            user_id,
            MempoolHistoryPageWorkUpdate {
                address_id: first.address_id,
                next_cursor: Some(first_page_cursor.clone()),
                hd_frontier: Some(HdMempoolHistoryFrontierUpdate {
                    account_id: first.account_id,
                    next_address_id: Some(first.address_id),
                }),
            },
        )
        .expect("same-account page work should commit");
        let committed = get_sync_addresses_for_account(user_id, first.account_id)
            .expect("committed page state should load");
        assert_eq!(
            committed[0].mempool_backfill_cursor_txid,
            Some(first_page_cursor)
        );
        let account_state = with_user_db(user_id, |conn| {
            address_loading::load_account_sync_state_row(conn, first.account_id)
        })
        .expect("account state should load")
        .expect("account state should exist");
        assert_eq!(
            account_state.mempool_history_next_address_id,
            Some(first.address_id)
        );
    }

    #[test]
    fn load_account_mempool_expected_tx_count_sums_known_address_estimates() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let label = parse_wallet_label("BTC Estimate");
        let add_result =
            add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
                .expect("bitcoin fixture should insert");

        assert_eq!(
            load_account_mempool_expected_tx_count(user_id, add_result.account_id)
                .expect("missing estimate should load"),
            None
        );

        let run_id = TransactionSyncRunId::new();
        mark_address_sync_started(user_id, add_result.address_id, run_id, now)
            .expect("mark start should succeed");
        update_address_mempool_expected_tx_count(
            user_id,
            add_result.address_id,
            Some(TransactionCount::from_u32(123)),
        )
        .expect("mempool expected count update should succeed");

        assert_eq!(
            load_account_mempool_expected_tx_count(user_id, add_result.account_id)
                .expect("estimate should load"),
            Some(TransactionCount::from_u32(123))
        );
    }

    #[test]
    fn load_account_sync_snapshots_exposes_etherscan_backfill_progress_for_single_address_account()
    {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let first_address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let add_result =
            create_eth_wallet_account_fixture(user_id, &first_address, "ETH Backfill", now);
        let run_id = TransactionSyncRunId::new();
        let completed_at = now + Duration::seconds(1);
        let end_block = EthereumBlockNumber::try_new(456_789).expect("block should be valid");

        mark_address_sync_started(user_id, add_result.address_id, run_id, now)
            .expect("mark start should succeed");
        update_address_etherscan_backfill_cursor(user_id, add_result.address_id, Some(end_block))
            .expect("etherscan cursor update should succeed");
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: add_result.address_id,
                run_id,
                started_at: now,
                completed_at,
                last_tip_height: ChainTipHeight::try_new(100).expect("tip should parse"),
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: None,
            },
        )
        .expect("etherscan completion should persist");

        let snapshots = load_account_sync_snapshots(user_id).expect("snapshots should load");
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        let backfill = snapshot
            .backfill_progress
            .as_ref()
            .expect("etherscan backfill progress should exist");

        assert_eq!(
            backfill.state.cursor,
            AddressBackfillCursor::Etherscan { end_block }
        );
        assert_eq!(backfill.expected_tx_count(), None);
        assert_eq!(backfill.fetched_tx_count, Some(TransactionCount::zero()));
        assert!(!backfill.expected_tx_count_is_lower_bound);
        assert_eq!(snapshot.addresses_in_progress.value(), 0);
        assert_ne!(snapshot.last_result, Some(AccountSyncResult::InProgress));
        assert!(
            snapshot
                .integration_states
                .iter()
                .all(|state| !state.is_active)
        );
    }

    #[test]
    fn load_account_sync_snapshots_exposes_etherscan_history_gap_status() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let first_address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let add_result = create_eth_wallet_account_fixture(user_id, &first_address, "ETH Gap", now);
        let run_id = TransactionSyncRunId::new();

        mark_address_sync_started(user_id, add_result.address_id, run_id, now)
            .expect("mark start should succeed");
        update_address_etherscan_history_status(
            user_id,
            add_result.address_id,
            EtherscanHistoryStatus::Gap,
        )
        .expect("history status update should succeed");

        let snapshots = load_account_sync_snapshots(user_id).expect("snapshots should load");
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(
            snapshot.etherscan_history_status,
            Some(EtherscanHistoryStatus::Gap)
        );
        assert_eq!(snapshot.integration_states.len(), 1);
        assert_eq!(
            snapshot.integration_states[0].etherscan_history_status,
            Some(EtherscanHistoryStatus::Gap)
        );
    }

    #[test]
    fn load_account_sync_snapshots_clears_in_progress_after_mempool_cursor_recovery() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let label = parse_wallet_label("BTC Cursor Recovery");
        let add_result =
            add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
                .expect("bitcoin fixture should insert");
        let run_id = TransactionSyncRunId::new();
        let started_at = now;
        let completed_at = now + Duration::seconds(15);
        let cursor = MempoolCursorTxid::parse(
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        )
        .expect("cursor should parse");

        mark_address_sync_started(user_id, add_result.address_id, run_id, started_at)
            .expect("mark start should succeed");
        update_address_mempool_backfill_cursor(user_id, add_result.address_id, Some(&cursor))
            .expect("mempool cursor update should succeed");
        update_address_mempool_expected_tx_count(
            user_id,
            add_result.address_id,
            Some(TransactionCount::from_u32(2)),
        )
        .expect("mempool expected count update should succeed");
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: add_result.address_id,
                run_id,
                started_at,
                completed_at,
                last_tip_height: ChainTipHeight::try_new(100).expect("tip should be valid"),
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: None,
            },
        )
        .expect("mark success should persist");
        update_address_mempool_backfill_cursor(user_id, add_result.address_id, None)
            .expect("mempool cursor clear should succeed");
        update_address_mempool_expected_tx_count(user_id, add_result.address_id, None)
            .expect("mempool expected count clear should succeed");

        let snapshots = load_account_sync_snapshots(user_id).expect("snapshots should load");
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.addresses_in_progress.value(), 0);
        assert_eq!(
            snapshot.last_result,
            Some(crate::transactions::AccountSyncResult::Success)
        );
        assert!(snapshot.backfill_progress.is_none());
        assert_eq!(snapshot.integration_states.len(), 1);
        let integration_state = &snapshot.integration_states[0];
        assert!(!integration_state.is_active);
        assert_eq!(
            integration_state.last_result,
            Some(AggregateSyncResult::Success)
        );
        assert!(integration_state.backfill_progress.is_none());
    }

    #[test]
    fn load_account_sync_snapshots_uses_latest_failure_error_by_timestamp() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let first_address = parse_eth_address("0xde709f2102306220921060314715629080e2fb77");
        let add_result =
            create_eth_wallet_account_fixture(user_id, &first_address, "Sync Errors", now);

        let older_failure_address = insert_extra_eth_address(
            user_id,
            add_result.account_id,
            "0x27b1fdb04752bbc536007a920d24acb045561c26",
            now,
        );
        let newer_failure_address = insert_extra_eth_address(
            user_id,
            add_result.account_id,
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
            now,
        );

        let older_started = now + Duration::seconds(10);
        let older_completed = now + Duration::seconds(20);
        let newer_started = now + Duration::seconds(30);
        let newer_completed = now + Duration::seconds(40);
        let success_started = now + Duration::seconds(50);
        let success_completed = now + Duration::seconds(60);

        let older_run = TransactionSyncRunId::new();
        mark_address_sync_started(user_id, older_failure_address, older_run, older_started)
            .expect("mark older start should succeed");
        mark_address_sync_completed_failure(
            user_id,
            older_failure_address,
            older_run,
            older_started,
            older_completed,
            &SyncErrorMessage::sanitize("older failure"),
            true,
        )
        .expect("mark older failure should succeed");

        let newer_run = TransactionSyncRunId::new();
        mark_address_sync_started(user_id, newer_failure_address, newer_run, newer_started)
            .expect("mark newer start should succeed");
        mark_address_sync_completed_failure(
            user_id,
            newer_failure_address,
            newer_run,
            newer_started,
            newer_completed,
            &SyncErrorMessage::sanitize("newer failure"),
            true,
        )
        .expect("mark newer failure should succeed");

        let success_run = TransactionSyncRunId::new();
        mark_address_sync_started(user_id, add_result.address_id, success_run, success_started)
            .expect("mark success start should succeed");
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: add_result.address_id,
                run_id: success_run,
                started_at: success_started,
                completed_at: success_completed,
                last_tip_height: ChainTipHeight::try_new(123).expect("valid tip"),
                new_tx_count: TransactionCount::from_u32(2),
                updated_tx_count: TransactionCount::from_u32(1),
                api_confirmed_balance: None,
            },
        )
        .expect("mark success completion should succeed");

        let snapshots = load_account_sync_snapshots(user_id).expect("snapshots should load");
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.account_id, add_result.account_id);
        assert_eq!(snapshot.addresses_total.value(), 3);
        assert_eq!(snapshot.addresses_never_synced.value(), 0);
        assert_eq!(snapshot.addresses_synced.value(), 1);
        assert_eq!(snapshot.addresses_failed.value(), 2);
        assert_eq!(
            snapshot.last_result,
            Some(crate::transactions::AccountSyncResult::Partial)
        );
        assert_eq!(
            snapshot.last_error.as_ref().map(SyncErrorMessage::as_str),
            Some("newer failure")
        );
        assert_eq!(snapshot.last_success_at, Some(success_completed));
        assert_eq!(snapshot.last_completed_at, Some(success_completed));
        assert_eq!(snapshot.integration_states.len(), 1);
        let integration_state = &snapshot.integration_states[0];
        assert_eq!(
            integration_state.integration_id,
            SyncIntegrationId::Etherscan
        );
        assert!(!integration_state.is_active);
        assert_eq!(
            integration_state.last_result,
            Some(AggregateSyncResult::Partial)
        );
        assert_eq!(
            integration_state
                .last_error
                .as_ref()
                .map(SyncErrorMessage::as_str),
            Some("newer failure")
        );
        assert_eq!(integration_state.last_completed_at, Some(success_completed));
    }

    #[test]
    fn refresh_account_integration_sync_state_persists_latest_account_summary() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let first_address = parse_eth_address("0xde709f2102306220921060314715629080e2fb77");
        let add_result =
            create_eth_wallet_account_fixture(user_id, &first_address, "Integration State", now);
        let second_address = insert_extra_eth_address(
            user_id,
            add_result.account_id,
            "0x27b1fdb04752bbc536007a920d24acb045561c26",
            now,
        );

        let failure_started = now + Duration::seconds(10);
        let failure_completed = now + Duration::seconds(20);
        let success_started = now + Duration::seconds(30);
        let success_completed = now + Duration::seconds(40);

        let failure_run = TransactionSyncRunId::new();
        mark_account_integration_sync_started(
            user_id,
            add_result.account_id,
            SyncIntegrationId::Etherscan,
            failure_started,
        )
        .expect("integration start should persist");
        mark_address_sync_started(user_id, second_address, failure_run, failure_started)
            .expect("mark failure start should succeed");
        mark_address_sync_completed_failure(
            user_id,
            second_address,
            failure_run,
            failure_started,
            failure_completed,
            &SyncErrorMessage::sanitize("durable failure"),
            true,
        )
        .expect("mark failure should succeed");
        refresh_account_integration_sync_state(
            user_id,
            add_result.account_id,
            SyncIntegrationId::Etherscan,
            failure_completed,
        )
        .expect("integration state should refresh after failure");

        let success_run = TransactionSyncRunId::new();
        mark_account_integration_sync_started(
            user_id,
            add_result.account_id,
            SyncIntegrationId::Etherscan,
            success_started,
        )
        .expect("integration start should update");
        mark_address_sync_started(user_id, add_result.address_id, success_run, success_started)
            .expect("mark success start should succeed");
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: add_result.address_id,
                run_id: success_run,
                started_at: success_started,
                completed_at: success_completed,
                last_tip_height: ChainTipHeight::try_new(777).expect("valid tip"),
                new_tx_count: TransactionCount::from_u32(1),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: None,
            },
        )
        .expect("mark success completion should succeed");
        refresh_account_integration_sync_state(
            user_id,
            add_result.account_id,
            SyncIntegrationId::Etherscan,
            success_completed,
        )
        .expect("integration state should refresh after success");

        let snapshots = load_account_sync_snapshots(user_id).expect("snapshots should load");
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        let integration_state = &snapshot.integration_states[0];
        assert_eq!(
            integration_state.integration_id,
            SyncIntegrationId::Etherscan
        );
        assert_eq!(integration_state.last_started_at, Some(success_started));
        assert_eq!(integration_state.last_completed_at, Some(success_completed));
        assert_eq!(
            integration_state.last_result,
            Some(AggregateSyncResult::Partial)
        );
        assert_eq!(
            integration_state
                .last_error
                .as_ref()
                .map(SyncErrorMessage::as_str),
            Some("durable failure")
        );
    }

    #[test]
    fn refresh_account_integration_sync_state_does_not_persist_success_until_all_addresses_sync() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let first_address = parse_eth_address("0xde709f2102306220921060314715629080e2fb77");
        let add_result =
            create_eth_wallet_account_fixture(user_id, &first_address, "Partial Initial", now);
        let _never_synced_address = insert_extra_eth_address(
            user_id,
            add_result.account_id,
            "0x27b1fdb04752bbc536007a920d24acb045561c26",
            now,
        );
        let started_at = now + Duration::seconds(10);
        let completed_at = now + Duration::seconds(20);
        let run_id = TransactionSyncRunId::new();

        mark_account_integration_sync_started(
            user_id,
            add_result.account_id,
            SyncIntegrationId::Etherscan,
            started_at,
        )
        .expect("integration start should persist");
        mark_address_sync_started(user_id, add_result.address_id, run_id, started_at)
            .expect("address start should persist");
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: add_result.address_id,
                run_id,
                started_at,
                completed_at,
                last_tip_height: ChainTipHeight::try_new(777).expect("valid tip"),
                new_tx_count: TransactionCount::from_u32(1),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: None,
            },
        )
        .expect("address success should persist");
        refresh_account_integration_sync_state(
            user_id,
            add_result.account_id,
            SyncIntegrationId::Etherscan,
            completed_at,
        )
        .expect("integration state should refresh");

        let stored_last_result = with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT last_result FROM account_integration_sync_state WHERE account_id = ?1",
                params![add_result.account_id.to_string()],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|err| {
                DbError::new(format!(
                    "Failed to query account integration sync state: {err}"
                ))
            })
        })
        .expect("stored integration state should load");
        assert_eq!(stored_last_result, None);

        let snapshots = load_account_sync_snapshots(user_id).expect("snapshots should load");
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.addresses_never_synced.value(), 1);
        assert_eq!(snapshot.addresses_synced.value(), 1);
        assert_eq!(snapshot.last_result, Some(AccountSyncResult::InProgress));
        assert_eq!(snapshot.integration_states[0].last_result, None);
    }

    #[test]
    fn reconcile_account_transactions_prunes_chain_tx_when_transfer_becomes_unowned() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();

        let record = SyncAccountTransactionRecord {
            tx_hash: parse_tx_hash(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(10),
            block_hash: Some(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            ),
            block_time: Some(now),
            fee_amount: Some(UnsignedAmount::zero()),
            nonce: Some(1),
            transfers: vec![SyncAccountTransferRecord {
                provider_transfer_key: ProviderTransferKey::normal(),
                transfer_index: 0,
                transfer_kind: TransferKind::Normal,
                from_address: Some(parse_tracked_address(
                    "0x1111111111111111111111111111111111111111",
                )),
                to_address: Some(parse_tracked_address(
                    "0x2222222222222222222222222222222222222222",
                )),
                value_amount: UnsignedAmount::from_u128(42),
            }],
        };

        let summary = reconcile_account_transactions(
            user_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            &[record],
            now,
        )
        .expect("reconcile should succeed");

        assert_eq!(summary.new_tx_count.value(), 0);
        assert_eq!(summary.updated_tx_count.value(), 0);

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let transfer_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM account_transfers", [], |row| {
                    row.get(0)
                })
                .map_err(|err| DbError::new(format!("Failed to count account transfers: {err}")))?;
            let chain_tx_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM chain_transactions", [], |row| {
                    row.get(0)
                })
                .map_err(|err| {
                    DbError::new(format!("Failed to count chain transactions: {err}"))
                })?;

            assert_eq!(transfer_count, 0);
            assert_eq!(chain_tx_count, 0);
            Ok(())
        })
        .expect("post-reconcile assertions should succeed");
    }

    #[test]
    fn load_known_tx_hashes_for_address_includes_account_transfers() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
        let add_result = create_eth_wallet_account_fixture(user_id, &address, "ETH Known", now);
        let tx_hash =
            parse_tx_hash("abababababababababababababababababababababababababababababababab");

        let record = SyncAccountTransactionRecord {
            tx_hash: tx_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(10),
            block_hash: Some(
                "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd".to_string(),
            ),
            block_time: Some(now),
            fee_amount: Some(UnsignedAmount::zero()),
            nonce: Some(1),
            transfers: vec![SyncAccountTransferRecord {
                provider_transfer_key: ProviderTransferKey::normal(),
                transfer_index: 0,
                transfer_kind: TransferKind::Normal,
                from_address: Some(parse_tracked_address(
                    "0x1111111111111111111111111111111111111111",
                )),
                to_address: Some(parse_tracked_address(&address.checksummed())),
                value_amount: UnsignedAmount::from_u128(42),
            }],
        };

        reconcile_account_transactions(
            user_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            &[record],
            now,
        )
        .expect("reconcile should succeed");

        let hashes = load_known_tx_hashes_for_address(
            user_id,
            add_result.address_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
        )
        .expect("known hashes should load");

        assert_eq!(hashes, HashSet::from([tx_hash]));
    }

    #[test]
    fn reconcile_address_transactions_prunes_chain_tx_when_no_owned_edges_exist() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();

        let record = SyncTransactionRecord {
            tx_hash: parse_tx_hash(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            ),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(20),
            block_hash: Some(
                "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_string(),
            ),
            block_time: Some(now),
            fee_amount: Some(0),
            inputs: vec![SyncTransactionInputRecord {
                input_index: 0,
                prev_tx_hash: parse_tx_hash(
                    "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                ),
                prev_output_index: 0,
                prev_address: Some(parse_tracked_address(
                    "0x3333333333333333333333333333333333333333",
                )),
                value_amount: Some(12),
            }],
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(parse_tracked_address(
                    "0x4444444444444444444444444444444444444444",
                )),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 12,
            }],
        };

        let summary = reconcile_address_transactions(
            user_id,
            SyncedAssetId::Ethereum,
            Network::Mainnet,
            &[record],
            now,
        )
        .expect("reconcile should succeed");

        assert_eq!(summary.new_tx_count.value(), 0);
        assert_eq!(summary.updated_tx_count.value(), 0);

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let input_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM transaction_inputs", [], |row| {
                    row.get(0)
                })
                .map_err(|err| DbError::new(format!("Failed to count inputs: {err}")))?;
            let output_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM transaction_outputs", [], |row| {
                    row.get(0)
                })
                .map_err(|err| DbError::new(format!("Failed to count outputs: {err}")))?;
            let chain_tx_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM chain_transactions", [], |row| {
                    row.get(0)
                })
                .map_err(|err| {
                    DbError::new(format!("Failed to count chain transactions: {err}"))
                })?;

            assert_eq!(input_count, 0);
            assert_eq!(output_count, 0);
            assert_eq!(chain_tx_count, 0);
            Ok(())
        })
        .expect("post-reconcile assertions should succeed");
    }

    #[test]
    fn coverage_invalidation_detects_confirmed_canonical_contradictions() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let fixture = add_bitcoin_address(
            user_id,
            &address,
            Network::Mainnet,
            None,
            Some(&parse_wallet_label("Coverage invalidation")),
            now,
        )
        .expect("bitcoin fixture should insert");
        let tracked = parse_tracked_address(address.canonical());
        let base_record = |index: u32| SyncTransactionRecord {
            tx_hash: parse_tx_hash(&format!("{index:064x}")),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(100),
            block_hash: Some("block-100".to_string()),
            block_time: Some(now),
            fee_amount: Some(10),
            inputs: vec![SyncTransactionInputRecord {
                input_index: 0,
                prev_tx_hash: parse_tx_hash(&format!("{:064x}", index + 100)),
                prev_output_index: 0,
                prev_address: Some(tracked.clone()),
                value_amount: Some(100),
            }],
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked.clone()),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 90,
            }],
        };

        let mutations: [fn(&mut SyncTransactionRecord); 12] = [
            |record| record.status = ChainTransactionStatus::Pending,
            |record| record.status = ChainTransactionStatus::Dropped,
            |record| {
                record.status = ChainTransactionStatus::Failed;
                record.outputs.clear();
            },
            |record| record.block_height = None,
            |record| record.block_height = Some(101),
            |record| record.block_hash = Some("replacement-block".to_string()),
            |record| record.block_time = record.block_time.map(|time| time + Duration::seconds(1)),
            |record| record.fee_amount = Some(11),
            |record| record.inputs[0].value_amount = Some(101),
            |record| record.outputs[0].value_amount = 89,
            |record| record.inputs.clear(),
            |record| record.outputs.clear(),
        ];

        for (offset, mutate) in mutations.into_iter().enumerate() {
            let original = base_record(u32::try_from(offset).expect("small test offset") + 1);
            reconcile_address_transactions(
                user_id,
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                std::slice::from_ref(&original),
                now,
            )
            .expect("original should reconcile");
            let mut changed = original;
            mutate(&mut changed);
            let summary = reconcile_address_transactions(
                user_id,
                SyncedAssetId::Bitcoin,
                Network::Mainnet,
                &[changed],
                now + Duration::seconds(1),
            )
            .expect("contradiction should reconcile");

            assert_eq!(
                summary.coverage_invalidation.address_ids,
                HashSet::from([fixture.address_id]),
                "mutation {offset}"
            );
            assert_eq!(
                summary.coverage_invalidation.account_ids,
                HashSet::from([fixture.account_id]),
                "mutation {offset}"
            );
        }
    }

    #[test]
    fn reconciliation_failure_preserves_targets_from_prior_committed_records() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let fixture = add_bitcoin_address(
            user_id,
            &address,
            Network::Mainnet,
            None,
            Some(&parse_wallet_label("Partial reconciliation")),
            now,
        )
        .expect("bitcoin fixture should insert");
        let tracked = parse_tracked_address(address.canonical());
        let original = SyncTransactionRecord {
            tx_hash: parse_tx_hash(&format!("{:064x}", 700)),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(100),
            block_hash: Some("block-100".to_string()),
            block_time: Some(now),
            fee_amount: Some(10),
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked.clone()),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 90,
            }],
        };
        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            std::slice::from_ref(&original),
            now,
        )
        .expect("original should reconcile");

        let mut contradicted = original;
        contradicted.block_hash = Some("replacement-block".to_string());
        let invalid = SyncTransactionRecord {
            tx_hash: parse_tx_hash(&format!("{:064x}", 701)),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(101),
            block_hash: Some("block-101".to_string()),
            block_time: Some(now),
            fee_amount: Some(10),
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked),
                script_pubkey_hex: "0014bad".to_string(),
                value_amount: -1,
            }],
        };

        let failure = reconcile_address_transactions_preserving_invalidation(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[contradicted, invalid],
            now + Duration::seconds(1),
        )
        .expect_err("second record should fail after the contradiction commits");

        assert_eq!(
            failure.summary.coverage_invalidation.address_ids,
            HashSet::from([fixture.address_id])
        );
        assert_eq!(
            failure.summary.coverage_invalidation.account_ids,
            HashSet::from([fixture.account_id])
        );
        let stored_block_hash = with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT block_hash FROM chain_transactions WHERE tx_hash = ?1",
                [format!("{:064x}", 700)],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|err| DbError::new(format!("Failed to load reconciled block hash: {err}")))
        })
        .expect("reconciled block hash should load");
        assert_eq!(stored_block_hash.as_deref(), Some("replacement-block"));
    }

    #[test]
    fn coverage_invalidation_treats_pending_confirmation_as_advancement_and_deduplicates_targets() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let fixture = add_bitcoin_address(
            user_id,
            &address,
            Network::Mainnet,
            None,
            Some(&parse_wallet_label("Coverage target union")),
            now,
        )
        .expect("bitcoin fixture should insert");
        let tracked = parse_tracked_address(address.canonical());
        let record = |index: u32, status| SyncTransactionRecord {
            tx_hash: parse_tx_hash(&format!("{index:064x}")),
            status,
            block_height: (status == ChainTransactionStatus::Confirmed).then_some(100),
            block_hash: (status == ChainTransactionStatus::Confirmed)
                .then(|| "block-100".to_string()),
            block_time: (status == ChainTransactionStatus::Confirmed).then_some(now),
            fee_amount: Some(0),
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked.clone()),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 1,
            }],
        };

        let pending = record(1000, ChainTransactionStatus::Pending);
        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            std::slice::from_ref(&pending),
            now,
        )
        .expect("pending transaction should reconcile");
        let advanced = record(1000, ChainTransactionStatus::Confirmed);
        let advancement = reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[advanced],
            now + Duration::seconds(1),
        )
        .expect("confirmation should reconcile");
        assert_eq!(advancement.coverage_invalidation, Default::default());

        let first = record(1001, ChainTransactionStatus::Confirmed);
        let second = record(1002, ChainTransactionStatus::Confirmed);
        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[first.clone(), second.clone()],
            now,
        )
        .expect("confirmed transactions should reconcile");
        let mut first_changed = first;
        first_changed.block_hash = Some("replacement-1".to_string());
        let mut second_changed = second;
        second_changed.block_hash = Some("replacement-2".to_string());
        let contradictions = reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[first_changed, second_changed],
            now + Duration::seconds(1),
        )
        .expect("contradictions should reconcile");

        assert_eq!(
            contradictions.coverage_invalidation.address_ids,
            HashSet::from([fixture.address_id])
        );
        assert_eq!(
            contradictions.coverage_invalidation.account_ids,
            HashSet::from([fixture.account_id])
        );
    }

    /// Helper: create a Bitcoin address and return the IDs + tracked address.
    fn setup_btc_address_fixture(
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> (DigitalAssetAccountId, TrackedAddress) {
        let wallet_label = parse_wallet_label("UTXO Linkage Wallet");
        let btc_address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let account = crate::db::add_bitcoin_address(
            user_id,
            &btc_address,
            Network::Mainnet,
            None,
            Some(&wallet_label),
            now,
        )
        .expect("btc address should be added");
        let tracked =
            TrackedAddress::parse(btc_address.canonical()).expect("tracked address should parse");
        (account.account_id, tracked)
    }

    /// Helper: query spent_by_tx_hash for a specific UTXO.
    fn query_utxo_spent_by(user_id: UserId, tx_hash: &str, output_index: i64) -> Option<String> {
        with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT spent_by_tx_hash FROM utxos
                 WHERE tx_hash = ?1 AND output_index = ?2",
                params![tx_hash, output_index],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|err| DbError::new(format!("Failed to query UTXO: {err}")))
            .map(|opt| opt.flatten())
        })
        .expect("query should succeed")
    }

    #[test]
    fn utxo_spent_linkage_works_when_spending_tx_synced_before_producing_tx() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let (_account_id, tracked) = setup_btc_address_fixture(user_id, now);

        let producing_tx_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let spending_tx_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        // Sync spending tx FIRST (reverse order, like mempool API newest-first).
        let spending_record = SyncTransactionRecord {
            tx_hash: parse_tx_hash(spending_tx_hash),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(200),
            block_hash: Some("block200".to_string()),
            block_time: Some(now + Duration::seconds(100)),
            fee_amount: Some(500),
            inputs: vec![SyncTransactionInputRecord {
                input_index: 0,
                prev_tx_hash: parse_tx_hash(producing_tx_hash),
                prev_output_index: 0,
                prev_address: Some(tracked.clone()),
                value_amount: Some(50_000),
            }],
            outputs: Vec::new(),
        };

        // Sync producing tx SECOND.
        let producing_record = SyncTransactionRecord {
            tx_hash: parse_tx_hash(producing_tx_hash),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(100),
            block_hash: Some("block100".to_string()),
            block_time: Some(now),
            fee_amount: Some(200),
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 50_000,
            }],
        };

        // Reconcile in reverse order: spending first, producing second.
        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[spending_record, producing_record],
            now,
        )
        .expect("reconcile should succeed");

        // The producing tx's UTXO should be linked to the spending tx.
        let spent_by = query_utxo_spent_by(user_id, producing_tx_hash, 0);
        assert_eq!(
            spent_by.as_deref(),
            Some(spending_tx_hash),
            "UTXO should be linked to spending tx even when synced in reverse order"
        );
    }

    #[test]
    fn utxo_spent_linkage_works_when_producing_tx_synced_before_spending_tx() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let (_account_id, tracked) = setup_btc_address_fixture(user_id, now);

        let producing_tx_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let spending_tx_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        // Normal order: producing first, spending second.
        let producing_record = SyncTransactionRecord {
            tx_hash: parse_tx_hash(producing_tx_hash),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(100),
            block_hash: Some("block100".to_string()),
            block_time: Some(now),
            fee_amount: Some(200),
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked.clone()),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 50_000,
            }],
        };

        let spending_record = SyncTransactionRecord {
            tx_hash: parse_tx_hash(spending_tx_hash),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(200),
            block_hash: Some("block200".to_string()),
            block_time: Some(now + Duration::seconds(100)),
            fee_amount: Some(500),
            inputs: vec![SyncTransactionInputRecord {
                input_index: 0,
                prev_tx_hash: parse_tx_hash(producing_tx_hash),
                prev_output_index: 0,
                prev_address: Some(tracked),
                value_amount: Some(50_000),
            }],
            outputs: Vec::new(),
        };

        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[producing_record, spending_record],
            now,
        )
        .expect("reconcile should succeed");

        let spent_by = query_utxo_spent_by(user_id, producing_tx_hash, 0);
        assert_eq!(
            spent_by.as_deref(),
            Some(spending_tx_hash),
            "UTXO should be linked to spending tx in normal order"
        );
    }

    #[test]
    fn utxo_resync_does_not_clear_existing_spent_linkage() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();
        let (_account_id, tracked) = setup_btc_address_fixture(user_id, now);

        let producing_tx_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let spending_tx_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        let producing_record = SyncTransactionRecord {
            tx_hash: parse_tx_hash(producing_tx_hash),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(100),
            block_hash: Some("block100".to_string()),
            block_time: Some(now),
            fee_amount: Some(200),
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked.clone()),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 50_000,
            }],
        };

        let spending_record = SyncTransactionRecord {
            tx_hash: parse_tx_hash(spending_tx_hash),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(200),
            block_hash: Some("block200".to_string()),
            block_time: Some(now + Duration::seconds(100)),
            fee_amount: Some(500),
            inputs: vec![SyncTransactionInputRecord {
                input_index: 0,
                prev_tx_hash: parse_tx_hash(producing_tx_hash),
                prev_output_index: 0,
                prev_address: Some(tracked.clone()),
                value_amount: Some(50_000),
            }],
            outputs: Vec::new(),
        };

        // First reconcile: establish the linkage.
        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[producing_record, spending_record],
            now,
        )
        .expect("first reconcile should succeed");

        let spent_by = query_utxo_spent_by(user_id, producing_tx_hash, 0);
        assert_eq!(spent_by.as_deref(), Some(spending_tx_hash));

        // Re-sync the producing tx (e.g., status update or data refresh).
        let resync_record = SyncTransactionRecord {
            tx_hash: parse_tx_hash(producing_tx_hash),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(100),
            block_hash: Some("block100".to_string()),
            block_time: Some(now),
            fee_amount: Some(200),
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 50_000,
            }],
        };

        reconcile_address_transactions(
            user_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            &[resync_record],
            now,
        )
        .expect("resync should succeed");

        // spent_by_tx_hash must still be set (not cleared by the upsert).
        let spent_by_after = query_utxo_spent_by(user_id, producing_tx_hash, 0);
        assert_eq!(
            spent_by_after.as_deref(),
            Some(spending_tx_hash),
            "Re-syncing the producing tx must not clear existing spent_by_tx_hash"
        );
    }

    #[test]
    fn api_confirmed_balance_is_persisted_and_loadable_per_account() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();

        let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let label = parse_wallet_label("Balance Persist Test");
        let add_result =
            add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
                .expect("bitcoin fixture should insert");

        // Mark a successful sync with an API confirmed balance.
        let run_id = TransactionSyncRunId::new();
        let started = now;
        let completed = now + Duration::seconds(30);
        let balance = ApiConfirmedBalance::from_smallest_unit_i64(1_500_000)
            .expect("test balance should be valid");

        mark_address_sync_started(user_id, add_result.address_id, run_id, started)
            .expect("mark start should succeed");
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: add_result.address_id,
                run_id,
                started_at: started,
                completed_at: completed,
                last_tip_height: ChainTipHeight::try_new(888).expect("tip should be valid"),
                new_tx_count: TransactionCount::from_u32(5),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: Some(balance),
            },
        )
        .expect("mark success should persist");

        // Load balances for the account.
        let balances = load_api_confirmed_balances_for_account(user_id, add_result.account_id)
            .expect("load balances should succeed");
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].address_id, add_result.address_id);
        assert_eq!(balances[0].last_completed_at, Some(completed));
        let loaded_balance = balances[0]
            .api_confirmed_balance
            .expect("balance should be present");
        assert_eq!(loaded_balance.amount().value(), 1_500_000_u128);
    }

    #[test]
    fn api_confirmed_balance_none_preserves_existing_stored_value() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();

        let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let label = parse_wallet_label("Balance Preserve Test");
        let add_result =
            add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
                .expect("bitcoin fixture should insert");

        let first_balance = ApiConfirmedBalance::from_smallest_unit_i64(2_000_000)
            .expect("first balance should be valid");

        // First sync: store a balance.
        let run1 = TransactionSyncRunId::new();
        mark_address_sync_started(user_id, add_result.address_id, run1, now)
            .expect("mark start 1 should succeed");
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: add_result.address_id,
                run_id: run1,
                started_at: now,
                completed_at: now + Duration::seconds(10),
                last_tip_height: ChainTipHeight::try_new(100).expect("tip"),
                new_tx_count: TransactionCount::from_u32(3),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: Some(first_balance),
            },
        )
        .expect("first success should persist");

        // Second sync: stats call fails, balance is None — should preserve existing.
        let run2 = TransactionSyncRunId::new();
        mark_address_sync_started(
            user_id,
            add_result.address_id,
            run2,
            now + Duration::seconds(60),
        )
        .expect("mark start 2 should succeed");
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: add_result.address_id,
                run_id: run2,
                started_at: now + Duration::seconds(60),
                completed_at: now + Duration::seconds(70),
                last_tip_height: ChainTipHeight::try_new(101).expect("tip"),
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: None,
            },
        )
        .expect("second success should persist");

        let balances = load_api_confirmed_balances_for_account(user_id, add_result.account_id)
            .expect("load should succeed");
        assert_eq!(balances.len(), 1);
        let loaded = balances[0]
            .api_confirmed_balance
            .expect("previous balance should be preserved");
        assert_eq!(loaded.amount().value(), 2_000_000_u128);
    }

    #[test]
    fn load_api_confirmed_balances_includes_account_address_without_sync_state() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();

        let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let label = parse_wallet_label("No Sync State");
        let add_result =
            add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
                .expect("bitcoin fixture should insert");

        let balances = load_api_confirmed_balances_for_account(user_id, add_result.account_id)
            .expect("load should succeed");
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].address_id, add_result.address_id);
        assert_eq!(balances[0].api_confirmed_balance, None);
    }

    #[test]
    fn api_confirmed_balance_supports_split_storage_beyond_i64() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let now = test_now();

        let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        let label = parse_wallet_label("Balance Split Test");
        let add_result =
            add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
                .expect("bitcoin fixture should insert");

        let large_balance =
            ApiConfirmedBalance::from_amount(UnsignedAmount::from_u128((i64::MAX as u128) + 42));

        let run_id = TransactionSyncRunId::new();
        mark_address_sync_started(user_id, add_result.address_id, run_id, now)
            .expect("mark start should succeed");
        mark_address_sync_completed_success(
            user_id,
            &AddressSyncSuccess {
                address_id: add_result.address_id,
                run_id,
                started_at: now,
                completed_at: now + Duration::seconds(5),
                last_tip_height: ChainTipHeight::try_new(1_000).expect("tip should be valid"),
                new_tx_count: TransactionCount::zero(),
                updated_tx_count: TransactionCount::zero(),
                api_confirmed_balance: Some(large_balance),
            },
        )
        .expect("mark success should persist");

        let balances = load_api_confirmed_balances_for_account(user_id, add_result.account_id)
            .expect("load balances should succeed");
        let loaded = balances[0]
            .api_confirmed_balance
            .expect("balance should be present");
        assert_eq!(loaded.amount().value(), (i64::MAX as u128) + 42);
    }

    struct StrictMempoolHistoryFixture {
        user_id: UserId,
        address_id: DigitalAssetAddressId,
        start_run_id: SyncRunId,
        source_connection_id: SourceConnectionId,
        now: DateTime<Utc>,
    }

    impl StrictMempoolHistoryFixture {
        fn new() -> Self {
            let user_id = UserId::new();
            initialize_user_db_for_test(user_id).expect("user db should initialize");
            let now = test_now();
            let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
            let label = parse_wallet_label("Strict History");
            let address_id =
                add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
                    .expect("bitcoin fixture should insert")
                    .address_id;
            let run = start_sync_run(
                user_id,
                StartSyncRunRequest {
                    integration: IntegrationKind::Mempool,
                    scope_kind: SyncRunScopeKind::Address,
                    scope_address_id: address_id,
                    asset_id: SyncedAssetId::Bitcoin,
                    network: Network::Mainnet,
                    trigger_kind: SyncRunTriggerKind::Manual,
                    started_at: now,
                    summary_json: None,
                },
            )
            .expect("sync run should insert");
            Self {
                user_id,
                address_id,
                start_run_id: run.sync_run_id,
                source_connection_id: run.source_connection_id,
                now,
            }
        }

        fn record_page(
            &self,
            scan_start_run_id: Option<SyncRunId>,
            page_kind: MempoolPageKind,
            requested_cursor: Option<&TxHash>,
            returned_cursor: Option<&TxHash>,
            transactions: &[(&TxHash, bool)],
        ) {
            self.record_page_for_run(
                self.start_run_id,
                scan_start_run_id,
                page_kind,
                requested_cursor,
                returned_cursor,
                transactions,
            );
        }

        fn record_page_for_run(
            &self,
            sync_run_id: SyncRunId,
            scan_start_run_id: Option<SyncRunId>,
            page_kind: MempoolPageKind,
            requested_cursor: Option<&TxHash>,
            returned_cursor: Option<&TxHash>,
            transactions: &[(&TxHash, bool)],
        ) {
            let raw_version_ids = transactions
                .iter()
                .map(|(txid, confirmed)| {
                    let payload = ExactPayloadBytes::try_new(
                        format!(
                            r#"{{"txid":"{}","vin":[],"vout":[],"status":{{"confirmed":{}}}}}"#,
                            txid.as_str(),
                            confirmed
                        )
                        .into_bytes(),
                    )
                    .expect("payload should parse");
                    insert_raw_mempool_tx_version(
                        self.user_id,
                        InsertRawMempoolTransactionVersionRequest {
                            source_connection_id: self.source_connection_id.clone(),
                            network: Network::Mainnet,
                            txid: (*txid).clone(),
                            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload),
                            payload_bytes: payload,
                            first_observed_at: self.now,
                        },
                    )
                    .expect("raw version should insert")
                    .raw_version_id
                })
                .collect();
            record_raw_mempool_page_observation(
                self.user_id,
                RecordRawMempoolPageObservationRequest {
                    sync_run_id,
                    source_connection_id: self.source_connection_id.clone(),
                    metadata: MempoolPageObservationMetadata {
                        address_id: self.address_id,
                        scan_start_run_id,
                        page_kind,
                        requested_cursor: requested_cursor
                            .map(|cursor| cursor.as_str().to_string()),
                        returned_last_confirmed_cursor: returned_cursor
                            .map(|cursor| cursor.as_str().to_string()),
                        item_count: u32::try_from(transactions.len())
                            .expect("test page length should fit"),
                    },
                    raw_version_ids,
                    observed_at: self.now,
                },
            )
            .expect("page observation should insert");
        }

        fn seed_canonical(&self, txids: &[&TxHash]) {
            with_user_db_mut(self.user_id, |conn| {
                for txid in txids {
                    let chain_transaction_id = Ulid::new().to_string();
                    conn.execute(
                        "INSERT INTO chain_transactions
                         (id, asset_id, network, tx_hash, status, block_height,
                          created_at, updated_at)
                         VALUES (?1, 'bitcoin', 'mainnet', ?2, 'confirmed', 1, ?3, ?3)",
                        params![chain_transaction_id, txid.as_str(), self.now.to_rfc3339()],
                    )
                    .map_err(|err| DbError::new(format!("Failed to seed canonical tx: {err}")))?;
                    conn.execute(
                        "INSERT INTO transaction_outputs
                         (id, tx_id, output_index, address_id, raw_address, script_pubkey_hex,
                          value_amount_hi, value_amount_lo, created_at, updated_at)
                         VALUES (?1, ?2, 0, ?3, NULL, '00', 0, 1, ?4, ?4)",
                        params![
                            Ulid::new().to_string(),
                            chain_transaction_id,
                            self.address_id.to_string(),
                            self.now.to_rfc3339(),
                        ],
                    )
                    .map_err(|err| {
                        DbError::new(format!("Failed to seed canonical output: {err}"))
                    })?;
                }
                Ok::<(), DbError>(())
            })
            .expect("canonical fixture should persist");
        }

        fn seed_exact_chain(&self, first: &TxHash, second: &TxHash) {
            let pending =
                parse_tx_hash("fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0");
            self.record_page(
                Some(self.start_run_id),
                MempoolPageKind::FirstPage,
                None,
                Some(first),
                &[(&pending, false), (first, true)],
            );
            self.record_page(
                Some(self.start_run_id),
                MempoolPageKind::PaginatedAfterConfirmed,
                Some(first),
                Some(second),
                &[(second, true)],
            );
            self.record_page(
                Some(self.start_run_id),
                MempoolPageKind::PaginatedAfterConfirmed,
                Some(second),
                None,
                &[],
            );
            self.seed_canonical(&[first, second]);
        }
    }

    fn assert_strict_mempool_history_restart(
        fixture: &StrictMempoolHistoryFixture,
        expected_count: u32,
    ) {
        let validation = validate_strict_mempool_history_scan(
            fixture.user_id,
            fixture.address_id,
            fixture.start_run_id,
            TransactionCount::from_u32(expected_count),
        )
        .expect("validation");
        let StrictMempoolScanValidation::Restart { reason } = validation else {
            panic!("invalid evidence should require restart");
        };
        assert!(!reason.trim().is_empty());
        assert!(reason.len() <= 512);
    }

    #[test]
    fn strict_mempool_history_scan_accepts_exact_advancing_chain() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let fixture = StrictMempoolHistoryFixture::new();
        let first =
            parse_tx_hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1");
        let second =
            parse_tx_hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa2");
        fixture.seed_exact_chain(&first, &second);

        assert_eq!(
            validate_strict_mempool_history_scan(
                fixture.user_id,
                fixture.address_id,
                fixture.start_run_id,
                TransactionCount::from_u32(2),
            )
            .expect("validation"),
            StrictMempoolScanValidation::Exact
        );
    }

    #[test]
    fn strict_mempool_history_scan_restarts_for_missing_or_duplicate_cursor_links() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let first =
            parse_tx_hash("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb1");
        let second =
            parse_tx_hash("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb2");
        let third =
            parse_tx_hash("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb3");

        let missing = StrictMempoolHistoryFixture::new();
        missing.record_page(
            Some(missing.start_run_id),
            MempoolPageKind::FirstPage,
            None,
            Some(&first),
            &[(&first, true)],
        );
        missing.seed_canonical(&[&first]);
        assert_strict_mempool_history_restart(&missing, 1);

        let duplicate = StrictMempoolHistoryFixture::new();
        duplicate.record_page(
            Some(duplicate.start_run_id),
            MempoolPageKind::FirstPage,
            None,
            Some(&first),
            &[(&first, true)],
        );
        for returned in [&second, &third] {
            duplicate.record_page(
                Some(duplicate.start_run_id),
                MempoolPageKind::PaginatedAfterConfirmed,
                Some(&first),
                Some(returned),
                &[(returned, true)],
            );
        }
        duplicate.seed_canonical(&[&first, &second, &third]);
        assert_strict_mempool_history_restart(&duplicate, 3);

        let nonadvancing = StrictMempoolHistoryFixture::new();
        nonadvancing.record_page(
            Some(nonadvancing.start_run_id),
            MempoolPageKind::FirstPage,
            None,
            Some(&first),
            &[(&first, true)],
        );
        nonadvancing.record_page(
            Some(nonadvancing.start_run_id),
            MempoolPageKind::PaginatedAfterConfirmed,
            Some(&first),
            Some(&first),
            &[(&first, true)],
        );
        nonadvancing.seed_canonical(&[&first]);
        assert_strict_mempool_history_restart(&nonadvancing, 1);
    }

    #[test]
    fn strict_mempool_history_scan_restarts_for_provider_count_mismatch() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let fixture = StrictMempoolHistoryFixture::new();
        let first =
            parse_tx_hash("ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc1");
        let second =
            parse_tx_hash("ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc2");
        fixture.seed_exact_chain(&first, &second);

        assert_strict_mempool_history_restart(&fixture, 1);
        assert_strict_mempool_history_restart(&fixture, 3);
    }

    #[test]
    fn strict_mempool_history_scan_restarts_for_canonical_set_mismatch() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let first =
            parse_tx_hash("ddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd1");
        let second =
            parse_tx_hash("ddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd2");

        let canonical_extra = StrictMempoolHistoryFixture::new();
        canonical_extra.record_page(
            Some(canonical_extra.start_run_id),
            MempoolPageKind::FirstPage,
            None,
            Some(&first),
            &[(&first, true)],
        );
        canonical_extra.record_page(
            Some(canonical_extra.start_run_id),
            MempoolPageKind::PaginatedAfterConfirmed,
            Some(&first),
            None,
            &[],
        );
        canonical_extra.seed_canonical(&[&first, &second]);
        assert_strict_mempool_history_restart(&canonical_extra, 1);

        let canonical_missing = StrictMempoolHistoryFixture::new();
        canonical_missing.record_page(
            Some(canonical_missing.start_run_id),
            MempoolPageKind::FirstPage,
            None,
            Some(&first),
            &[(&first, true)],
        );
        canonical_missing.record_page(
            Some(canonical_missing.start_run_id),
            MempoolPageKind::PaginatedAfterConfirmed,
            Some(&first),
            Some(&second),
            &[(&second, true)],
        );
        canonical_missing.record_page(
            Some(canonical_missing.start_run_id),
            MempoolPageKind::PaginatedAfterConfirmed,
            Some(&second),
            None,
            &[],
        );
        canonical_missing.seed_canonical(&[&first]);
        assert_strict_mempool_history_restart(&canonical_missing, 2);
    }

    #[test]
    fn strict_mempool_history_scan_ignores_deleted_legacy_or_differently_tagged_evidence() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let first =
            parse_tx_hash("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee1");
        let second =
            parse_tx_hash("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee2");

        let deleted = StrictMempoolHistoryFixture::new();
        deleted.seed_exact_chain(&first, &second);
        with_user_db_mut(deleted.user_id, |conn| {
            conn.execute(
                "DELETE FROM sync_runs WHERE id = ?1",
                [deleted.start_run_id.to_string()],
            )
            .map_err(|err| DbError::new(format!("Failed to delete start run: {err}")))?;
            Ok::<(), DbError>(())
        })
        .expect("start run should delete");
        assert_strict_mempool_history_restart(&deleted, 2);

        let untagged = StrictMempoolHistoryFixture::new();
        untagged.record_page(
            None,
            MempoolPageKind::FirstPage,
            None,
            Some(&first),
            &[(&first, true)],
        );
        untagged.record_page(
            None,
            MempoolPageKind::PaginatedAfterConfirmed,
            Some(&first),
            None,
            &[],
        );
        untagged.seed_canonical(&[&first]);
        assert_strict_mempool_history_restart(&untagged, 1);

        let differently_tagged = StrictMempoolHistoryFixture::new();
        let other_start_run_id = SyncRunId::new();
        differently_tagged.record_page(
            Some(other_start_run_id),
            MempoolPageKind::FirstPage,
            None,
            Some(&first),
            &[(&first, true)],
        );
        differently_tagged.record_page(
            Some(other_start_run_id),
            MempoolPageKind::PaginatedAfterConfirmed,
            Some(&first),
            None,
            &[],
        );
        differently_tagged.seed_canonical(&[&first]);
        assert_strict_mempool_history_restart(&differently_tagged, 1);
    }

    #[test]
    fn strict_mempool_history_scan_restarts_when_deleted_start_run_has_surviving_resume_pages() {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let fixture = StrictMempoolHistoryFixture::new();
        let first =
            parse_tx_hash("abababababababababababababababababababababababababababababababa1");
        let resume_run = start_sync_run(
            fixture.user_id,
            StartSyncRunRequest {
                integration: IntegrationKind::Mempool,
                scope_kind: SyncRunScopeKind::Address,
                scope_address_id: fixture.address_id,
                asset_id: SyncedAssetId::Bitcoin,
                network: Network::Mainnet,
                trigger_kind: SyncRunTriggerKind::Manual,
                started_at: fixture.now + Duration::seconds(1),
                summary_json: None,
            },
        )
        .expect("resume run should insert");
        fixture.record_page_for_run(
            resume_run.sync_run_id,
            Some(fixture.start_run_id),
            MempoolPageKind::FirstPage,
            None,
            Some(&first),
            &[(&first, true)],
        );
        fixture.record_page_for_run(
            resume_run.sync_run_id,
            Some(fixture.start_run_id),
            MempoolPageKind::PaginatedAfterConfirmed,
            Some(&first),
            None,
            &[],
        );
        fixture.seed_canonical(&[&first]);
        with_user_db_mut(fixture.user_id, |conn| {
            conn.execute(
                "DELETE FROM sync_runs WHERE id = ?1",
                [fixture.start_run_id.to_string()],
            )
            .map_err(|err| DbError::new(format!("Failed to delete start run: {err}")))?;
            Ok::<(), DbError>(())
        })
        .expect("start run should delete");

        assert_strict_mempool_history_restart(&fixture, 1);
    }
}
