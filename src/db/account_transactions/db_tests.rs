use super::balance;
use super::page_query;
use super::types::*;
use crate::amounts::UnsignedAmount;
use crate::balance_reliability::{BalanceProvisionalReason, BalanceReliability};
use crate::db::balance_reliability::AccountBalanceReliabilityContext;
use crate::db::transaction_sync::BitcoinAccountHistoryCoverage;
use crate::db::{
    AddressSyncSuccess, SyncTransactionInputRecord, SyncTransactionOutputRecord,
    SyncTransactionRecord, acquire_test_runtime, add_bitcoin_address,
    create_eth_wallet_account_fixture, initialize_user_db_for_test,
    mark_account_integration_sync_started, mark_address_sync_completed_failure,
    mark_address_sync_completed_success, mark_address_sync_started,
    publish_bitcoin_account_completion, publish_mempool_history_proof,
    reconcile_address_transactions, refresh_account_integration_sync_state,
    update_address_mempool_backfill_cursor, update_address_mempool_expected_tx_count,
    upsert_account_sync_state,
};
use crate::db::{
    BitcoinAccountCompletionPublication, BitcoinAddressProofPublication,
    BitcoinHdDiscoveryPublication, MempoolHistoryProof,
};
use crate::ethereum::{EthAddress, RawEthAddress};
use crate::models::{UserId, parse_datetime};
use crate::transactions::{
    ApiConfirmedBalance, ChainTipHeight, ChainTransactionStatus, MempoolCursorTxid,
    SyncErrorMessage, SyncIntegrationId, TrackedAddress, TransactionCount, TransactionSyncRunId,
    TxHash,
};
use crate::wallets::{
    BtcAddress, Label, RawBtcAddress, TransactionSortDirection, WALLET_LABEL_MAX_LENGTH,
};
use crate::wallets::{DigitalAssetAccountId, Network, SyncedAssetId, TransactionFilters};
use chrono::{DateTime, Utc};
use rusqlite::params;
use ulid::Ulid;

fn dt(s: &str) -> DateTime<Utc> {
    parse_datetime(s).expect("valid test datetime")
}

fn parse_eth_address(value: &str) -> EthAddress {
    let raw = RawEthAddress::new(value.to_string());
    EthAddress::parse(&raw).expect("test eth address should parse")
}

fn seed_eth_ledger_row(
    user_id: UserId,
    account_id: DigitalAssetAccountId,
    owned_address: &str,
    tx_hash: &str,
    occurred_at: &str,
    value_amount: UnsignedAmount,
    closing_balance: UnsignedAmount,
) {
    use crate::db::amount_storage::split_unsigned_amount as split_parts;
    use crate::db::error::DbError;

    let value_parts = split_parts(value_amount).expect("test eth value should encode");
    let balance_parts = split_parts(closing_balance).expect("test eth balance should encode");

    crate::db::user_db::with_user_db_mut(user_id, |conn| {
        let tx = conn
            .transaction()
            .map_err(|err| DbError::new(format!("Failed to start eth fixture tx: {err}")))?;
        let chain_transaction_id = Ulid::new().to_string();

        tx.execute(
            "INSERT INTO chain_transactions
                 (id, asset_id, network, tx_hash, status, block_height, block_hash, block_time, fee_amount_lo, fee_amount_hi, nonce, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                chain_transaction_id,
                "ethereum",
                "mainnet",
                tx_hash,
                "confirmed",
                1_i64,
                "blockhash-1",
                occurred_at,
                Option::<i64>::None,
                Option::<i64>::None,
                Option::<i64>::None,
                occurred_at,
                occurred_at,
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to insert eth chain tx fixture: {err}")))?;

        tx.execute(
            "INSERT INTO account_transaction_ledger
                 (id, account_id, chain_transaction_id, asset_id, network, tx_hash, status, occurred_at, first_seen_at, block_height, nonce, min_transfer_index, tx_type, from_addresses_json, to_addresses_json, value_amount_hi, value_amount_lo, fee_amount_hi, fee_amount_lo, closing_balance_hi, closing_balance_lo, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            params![
                Ulid::new().to_string(),
                account_id.to_string(),
                chain_transaction_id,
                "ethereum",
                "mainnet",
                tx_hash,
                "confirmed",
                occurred_at,
                occurred_at,
                1_i64,
                Option::<i64>::None,
                Option::<i64>::None,
                "receive",
                "[\"0x0000000000000000000000000000000000000001\"]",
                format!("[\"{owned_address}\"]"),
                value_parts.hi,
                value_parts.lo,
                Option::<i64>::None,
                Option::<i64>::None,
                balance_parts.hi,
                balance_parts.lo,
                occurred_at,
                occurred_at,
            ],
        )
        .map_err(|err| DbError::new(format!("Failed to insert eth ledger fixture: {err}")))?;

        tx.commit()
            .map_err(|err| DbError::new(format!("Failed to commit eth fixture tx: {err}")))?;
        Ok::<(), DbError>(())
    })
    .expect("eth ledger fixture should persist");
}

fn seed_btc_partial_backfill_fixture(
    user_id: UserId,
) -> (DigitalAssetAccountId, DateTime<Utc>, DateTime<Utc>) {
    let (account_id, _, transaction_time, sync_completed) =
        seed_btc_partial_backfill_fixture_with_balance(user_id, 150_000);
    (account_id, transaction_time, sync_completed)
}

fn seed_btc_partial_backfill_fixture_with_balance(
    user_id: UserId,
    api_confirmed_balance: u128,
) -> (
    DigitalAssetAccountId,
    crate::wallets::WalletId,
    DateTime<Utc>,
    DateTime<Utc>,
) {
    let transaction_time = dt("2026-01-10T12:00:00Z");
    let sync_started = dt("2026-01-20T09:00:00Z");
    let sync_completed = dt("2026-01-20T10:00:00Z");

    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("BTC Partial Backfill");
    let add_result = add_bitcoin_address(
        user_id,
        &address,
        Network::Mainnet,
        None,
        Some(&label),
        transaction_time,
    )
    .expect("bitcoin fixture should insert");

    let owned_tracked =
        TrackedAddress::parse(address.canonical()).expect("owned tracked address should parse");
    let tx_hash = TxHash::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        .expect("tx hash should parse");
    let records = vec![SyncTransactionRecord {
        tx_hash,
        status: ChainTransactionStatus::Confirmed,
        block_height: Some(100),
        block_hash: Some("blockhash-100".to_string()),
        block_time: Some(transaction_time),
        fee_amount: None,
        inputs: Vec::new(),
        outputs: vec![SyncTransactionOutputRecord {
            output_index: 0,
            raw_address: Some(owned_tracked),
            script_pubkey_hex: "0014deadbeef".to_string(),
            value_amount: 50_000,
        }],
    }];
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &records,
        transaction_time,
    )
    .expect("transactions should reconcile");

    let run_id = TransactionSyncRunId::new();
    mark_address_sync_started(user_id, add_result.address_id, run_id, sync_started)
        .expect("address sync start should persist");
    mark_address_sync_completed_success(
        user_id,
        &AddressSyncSuccess {
            address_id: add_result.address_id,
            run_id,
            started_at: sync_started,
            completed_at: sync_completed,
            last_tip_height: ChainTipHeight::try_new(777).expect("tip should be valid"),
            new_tx_count: TransactionCount::from_u32(1),
            updated_tx_count: TransactionCount::zero(),
            api_confirmed_balance: Some(ApiConfirmedBalance::from_amount(
                UnsignedAmount::from_u128(api_confirmed_balance),
            )),
        },
    )
    .expect("address sync success should persist");
    super::ledger_rebuild::rebuild_account_transaction_ledger(
        user_id,
        add_result.account_id,
        sync_completed,
    )
    .expect("ledger rebuild should succeed");

    (
        add_result.account_id,
        add_result.wallet_id,
        transaction_time,
        sync_completed,
    )
}

fn parse_btc_address(value: &str) -> BtcAddress {
    let raw = RawBtcAddress::new(value.to_string());
    BtcAddress::parse(&raw, Network::Mainnet).expect("test btc address should parse")
}

fn parse_wallet_label(value: &str) -> Label {
    Label::parse_with_limit(value, WALLET_LABEL_MAX_LENGTH).expect("test label should parse")
}

#[test]
fn bitcoin_ledger_rebuild_visits_each_canonical_account_row_once() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let observed_at = dt("2026-07-24T10:00:00Z");
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let account = add_bitcoin_address(
        user_id,
        &address,
        Network::Mainnet,
        None,
        Some(&parse_wallet_label("Linear rebuild")),
        observed_at,
    )
    .expect("bitcoin account should insert");
    let tracked = TrackedAddress::parse(address.canonical()).expect("tracked address should parse");
    let records = |range: std::ops::Range<u32>| {
        range
            .map(|index| SyncTransactionRecord {
                tx_hash: TxHash::parse(&format!("{:064x}", index + 1))
                    .expect("tx hash should parse"),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(i64::from(index) + 1),
                block_hash: Some(format!("block-{index}")),
                block_time: Some(observed_at + chrono::Duration::seconds(i64::from(index))),
                fee_amount: None,
                inputs: Vec::new(),
                outputs: vec![SyncTransactionOutputRecord {
                    output_index: 0,
                    raw_address: Some(tracked.clone()),
                    script_pubkey_hex: "0014deadbeef".to_string(),
                    value_amount: 1,
                }],
            })
            .collect::<Vec<_>>()
    };

    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &records(0..4),
        observed_at,
    )
    .expect("first canonical batch should reconcile");
    let first_counts = crate::db::user_db::with_user_db(user_id, |conn| {
        super::ledger_rebuild::utxo_model_entry_and_visit_counts_for_test(
            conn,
            account.account_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
        )
    })
    .expect("first visit counts should load");

    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &records(4..8),
        observed_at,
    )
    .expect("second canonical batch should reconcile");
    let second_counts = crate::db::user_db::with_user_db(user_id, |conn| {
        super::ledger_rebuild::utxo_model_entry_and_visit_counts_for_test(
            conn,
            account.account_id,
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
        )
    })
    .expect("second visit counts should load");

    assert_eq!(first_counts, (4, 4));
    assert_eq!(second_counts, (8, 8));
    assert_eq!(second_counts.1, first_counts.1 * 2);
}

#[test]
fn bitcoin_canonical_zero_rebuild_orders_same_block_parent_before_child() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let observed_at = dt("2026-07-24T10:00:00Z");
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("Canonical zero");
    let account = add_bitcoin_address(
        user_id,
        &address,
        Network::Mainnet,
        None,
        Some(&label),
        observed_at,
    )
    .expect("bitcoin account should insert");
    let tracked = TrackedAddress::parse(address.canonical()).expect("tracked address should parse");
    let parent_hash =
        TxHash::parse("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
            .expect("parent hash should parse");
    let child_hash =
        TxHash::parse("1111111111111111111111111111111111111111111111111111111111111111")
            .expect("child hash should parse");
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &[
            SyncTransactionRecord {
                tx_hash: parent_hash.clone(),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(100),
                block_hash: Some("same-block".to_string()),
                block_time: Some(dt("2026-07-24T10:02:00Z")),
                fee_amount: None,
                inputs: Vec::new(),
                outputs: vec![SyncTransactionOutputRecord {
                    output_index: 0,
                    raw_address: Some(tracked.clone()),
                    script_pubkey_hex: "00".to_string(),
                    value_amount: 10,
                }],
            },
            SyncTransactionRecord {
                tx_hash: child_hash.clone(),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(100),
                block_hash: Some("same-block".to_string()),
                block_time: Some(dt("2026-07-24T10:01:00Z")),
                fee_amount: Some(0),
                inputs: vec![SyncTransactionInputRecord {
                    input_index: 0,
                    prev_tx_hash: parent_hash.clone(),
                    prev_output_index: 0,
                    prev_address: Some(tracked.clone()),
                    value_amount: Some(10),
                }],
                outputs: vec![SyncTransactionOutputRecord {
                    output_index: 0,
                    raw_address: Some(tracked),
                    script_pubkey_hex: "00".to_string(),
                    value_amount: 6,
                }],
            },
        ],
        observed_at,
    )
    .expect("canonical transactions should reconcile");
    let run_id = TransactionSyncRunId::new();
    mark_address_sync_started(user_id, account.address_id, run_id, observed_at)
        .expect("address sync should start");
    mark_address_sync_completed_success(
        user_id,
        &AddressSyncSuccess {
            address_id: account.address_id,
            run_id,
            started_at: observed_at,
            completed_at: observed_at,
            last_tip_height: ChainTipHeight::try_new(100).expect("tip should parse"),
            new_tx_count: TransactionCount::from_u32(2),
            updated_tx_count: TransactionCount::zero(),
            api_confirmed_balance: Some(ApiConfirmedBalance::from_amount(
                UnsignedAmount::from_u128(1_000),
            )),
        },
    )
    .expect("address sync should complete");

    super::ledger_rebuild::rebuild_account_transaction_ledger(
        user_id,
        account.account_id,
        observed_at,
    )
    .expect("incomplete ledger should rebuild");
    let provisional_parent = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1 AND tx_hash = ?2",
            params![account.account_id.to_string(), parent_hash.as_str()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|err| crate::db::DbError::new(format!("provisional query failed: {err}")))
    })
    .expect("provisional parent should load");
    assert_eq!(provisional_parent, Some(1_004));

    assert!(
        publish_bitcoin_account_completion(
            user_id,
            BitcoinAccountCompletionPublication {
                account_id: account.account_id,
                final_address_proof: Some(BitcoinAddressProofPublication {
                    address_id: account.address_id,
                    proof: MempoolHistoryProof {
                        confirmed_tx_count: TransactionCount::from_u32(2),
                        complete_height: ChainTipHeight::try_new(100).expect("height should parse"),
                    },
                    scan_start_run_id: None,
                }),
                completed_hd_discovery: None,
                observed_at,
            },
        )
        .expect("proof and canonical ledger should publish"),
        "final proof should make the account candidate-complete"
    );

    let durable_proof = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT mempool_history_complete_tx_count, mempool_history_complete_height
             FROM transaction_sync_state
             WHERE address_id = ?1",
            [account.address_id.to_string()],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .map_err(|err| crate::db::DbError::new(format!("proof query failed: {err}")))
    })
    .expect("proof should load");
    assert_eq!(durable_proof, (Some(2), Some(100)));

    let rows = crate::db::with_user_db(user_id, |conn| {
        let mut statement = conn
            .prepare(
                "SELECT tx_hash, closing_balance_lo
                 FROM account_transaction_ledger
                 WHERE account_id = ?1
                 ORDER BY tx_hash ASC",
            )
            .map_err(|err| crate::db::DbError::new(format!("ledger query failed: {err}")))?;
        statement
            .query_map([account.account_id.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .map_err(|err| crate::db::DbError::new(format!("ledger rows failed: {err}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| crate::db::DbError::new(format!("ledger row failed: {err}")))
    })
    .expect("ledger rows should load");

    assert_eq!(
        rows,
        vec![
            (child_hash.as_str().to_string(), Some(6)),
            (parent_hash.as_str().to_string(), Some(10)),
        ]
    );
}

#[test]
fn bitcoin_rebuild_ignores_same_block_parent_from_another_account() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let observed_at = dt("2026-07-24T11:00:00Z");
    let receiving_address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let sending_address = parse_btc_address("1BoatSLRHtKNngkdXEeobR76b53LETtpyT");
    let receiving_account = add_bitcoin_address(
        user_id,
        &receiving_address,
        Network::Mainnet,
        None,
        Some(&parse_wallet_label("Receiving account")),
        observed_at,
    )
    .expect("receiving account should insert");
    add_bitcoin_address(
        user_id,
        &sending_address,
        Network::Mainnet,
        None,
        Some(&parse_wallet_label("Sending account")),
        observed_at,
    )
    .expect("sending account should insert");
    let receiving_tracked = TrackedAddress::parse(receiving_address.canonical())
        .expect("receiving address should parse");
    let sending_tracked =
        TrackedAddress::parse(sending_address.canonical()).expect("sending address should parse");
    let parent_hash =
        TxHash::parse("2222222222222222222222222222222222222222222222222222222222222222")
            .expect("parent hash should parse");
    let child_hash =
        TxHash::parse("3333333333333333333333333333333333333333333333333333333333333333")
            .expect("child hash should parse");

    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &[
            SyncTransactionRecord {
                tx_hash: parent_hash.clone(),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(101),
                block_hash: Some("shared-block".to_string()),
                block_time: Some(observed_at),
                fee_amount: None,
                inputs: Vec::new(),
                outputs: vec![SyncTransactionOutputRecord {
                    output_index: 0,
                    raw_address: Some(sending_tracked.clone()),
                    script_pubkey_hex: "00".to_string(),
                    value_amount: 10,
                }],
            },
            SyncTransactionRecord {
                tx_hash: child_hash.clone(),
                status: ChainTransactionStatus::Confirmed,
                block_height: Some(101),
                block_hash: Some("shared-block".to_string()),
                block_time: Some(observed_at + chrono::Duration::seconds(1)),
                fee_amount: Some(0),
                inputs: vec![SyncTransactionInputRecord {
                    input_index: 0,
                    prev_tx_hash: parent_hash,
                    prev_output_index: 0,
                    prev_address: Some(sending_tracked),
                    value_amount: Some(10),
                }],
                outputs: vec![SyncTransactionOutputRecord {
                    output_index: 0,
                    raw_address: Some(receiving_tracked),
                    script_pubkey_hex: "00".to_string(),
                    value_amount: 7,
                }],
            },
        ],
        observed_at,
    )
    .expect("cross-account transactions should reconcile");

    let run_id = TransactionSyncRunId::new();
    mark_address_sync_started(user_id, receiving_account.address_id, run_id, observed_at)
        .expect("receiving address sync should start");
    mark_address_sync_completed_success(
        user_id,
        &AddressSyncSuccess {
            address_id: receiving_account.address_id,
            run_id,
            started_at: observed_at,
            completed_at: observed_at,
            last_tip_height: ChainTipHeight::try_new(101).expect("tip should parse"),
            new_tx_count: TransactionCount::from_u32(1),
            updated_tx_count: TransactionCount::zero(),
            api_confirmed_balance: Some(ApiConfirmedBalance::from_amount(
                UnsignedAmount::from_u128(7),
            )),
        },
    )
    .expect("receiving address sync should complete");

    super::ledger_rebuild::rebuild_account_transaction_ledger(
        user_id,
        receiving_account.account_id,
        observed_at,
    )
    .expect("receiving account should ignore another account's parent dependency");

    let rows = crate::db::with_user_db(user_id, |conn| {
        let mut statement = conn
            .prepare(
                "SELECT tx_hash, tx_type, value_amount_lo, closing_balance_lo
                 FROM account_transaction_ledger
                 WHERE account_id = ?1
                 ORDER BY occurred_at ASC, tx_hash ASC",
            )
            .map_err(|err| crate::db::DbError::new(format!("ledger query failed: {err}")))?;
        statement
            .query_map([receiving_account.account_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })
            .map_err(|err| crate::db::DbError::new(format!("ledger rows failed: {err}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| crate::db::DbError::new(format!("ledger row failed: {err}")))
    })
    .expect("receiving ledger rows should load");

    assert_eq!(
        rows,
        vec![(
            child_hash.as_str().to_string(),
            "receive".to_string(),
            7,
            Some(7)
        )]
    );
}

#[test]
fn bitcoin_canonical_zero_publication_rolls_back_proof_when_ledger_swap_fails() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let observed_at = dt("2026-07-24T10:30:00Z");
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("Atomic completion");
    let account = add_bitcoin_address(
        user_id,
        &address,
        Network::Mainnet,
        None,
        Some(&label),
        observed_at,
    )
    .expect("bitcoin account should insert");
    let tracked = TrackedAddress::parse(address.canonical()).expect("tracked address should parse");
    let tx_hash = TxHash::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        .expect("hash should parse");
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &[SyncTransactionRecord {
            tx_hash: tx_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(100),
            block_hash: Some("block-100".to_string()),
            block_time: Some(observed_at),
            fee_amount: None,
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked),
                script_pubkey_hex: "00".to_string(),
                value_amount: 10,
            }],
        }],
        observed_at,
    )
    .expect("transaction should reconcile");
    let run_id = TransactionSyncRunId::new();
    mark_address_sync_started(user_id, account.address_id, run_id, observed_at)
        .expect("address sync should start");
    mark_address_sync_completed_success(
        user_id,
        &AddressSyncSuccess {
            address_id: account.address_id,
            run_id,
            started_at: observed_at,
            completed_at: observed_at,
            last_tip_height: ChainTipHeight::try_new(100).expect("tip should parse"),
            new_tx_count: TransactionCount::from_u32(1),
            updated_tx_count: TransactionCount::zero(),
            api_confirmed_balance: Some(ApiConfirmedBalance::from_amount(
                UnsignedAmount::from_u128(10),
            )),
        },
    )
    .expect("address sync should complete");
    super::ledger_rebuild::rebuild_account_transaction_ledger(
        user_id,
        account.account_id,
        observed_at,
    )
    .expect("initial ledger should rebuild");
    crate::db::with_user_db_mut(user_id, |conn| {
        conn.execute_batch(
            "CREATE TRIGGER reject_atomic_bitcoin_ledger
             BEFORE INSERT ON account_transaction_ledger
             BEGIN
               SELECT RAISE(ABORT, 'reject atomic ledger');
             END;",
        )
        .map_err(|err| crate::db::DbError::new(format!("trigger should install: {err}")))
    })
    .expect("trigger should install");

    publish_bitcoin_account_completion(
        user_id,
        BitcoinAccountCompletionPublication {
            account_id: account.account_id,
            final_address_proof: Some(BitcoinAddressProofPublication {
                address_id: account.address_id,
                proof: MempoolHistoryProof {
                    confirmed_tx_count: TransactionCount::from_u32(1),
                    complete_height: ChainTipHeight::try_new(100).expect("height should parse"),
                },
                scan_start_run_id: None,
            }),
            completed_hd_discovery: None,
            observed_at,
        },
    )
    .expect_err("ledger failure must roll back completion");

    let state = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT tss.mempool_history_complete_tx_count,
                    tss.mempool_history_complete_height,
                    atl.closing_balance_lo
             FROM transaction_sync_state tss
             JOIN account_transaction_ledger atl ON atl.account_id = ?1
             WHERE tss.address_id = ?2",
            params![
                account.account_id.to_string(),
                account.address_id.to_string()
            ],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .map_err(|err| crate::db::DbError::new(format!("atomic state query failed: {err}")))
    })
    .expect("atomic state should load");
    assert_eq!(state, (None, None, None));
}

#[test]
fn bitcoin_canonical_zero_publication_rolls_back_hd_checkpoint_when_ledger_swap_fails() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let observed_at = dt("2026-07-24T10:45:00Z");
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("Atomic HD completion");
    let account = add_bitcoin_address(
        user_id,
        &address,
        Network::Mainnet,
        None,
        Some(&label),
        observed_at,
    )
    .expect("bitcoin account should insert");
    let tracked = TrackedAddress::parse(address.canonical()).expect("tracked address should parse");
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &[SyncTransactionRecord {
            tx_hash: TxHash::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .expect("hash should parse"),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(100),
            block_hash: Some("block-100".to_string()),
            block_time: Some(observed_at),
            fee_amount: None,
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked),
                script_pubkey_hex: "00".to_string(),
                value_amount: 10,
            }],
        }],
        observed_at,
    )
    .expect("transaction should reconcile");
    let run_id = TransactionSyncRunId::new();
    mark_address_sync_started(user_id, account.address_id, run_id, observed_at)
        .expect("address sync should start");
    mark_address_sync_completed_success(
        user_id,
        &AddressSyncSuccess {
            address_id: account.address_id,
            run_id,
            started_at: observed_at,
            completed_at: observed_at,
            last_tip_height: ChainTipHeight::try_new(100).expect("tip should parse"),
            new_tx_count: TransactionCount::from_u32(1),
            updated_tx_count: TransactionCount::zero(),
            api_confirmed_balance: Some(ApiConfirmedBalance::from_amount(
                UnsignedAmount::from_u128(10),
            )),
        },
    )
    .expect("address sync should complete");
    publish_mempool_history_proof(
        user_id,
        account.address_id,
        MempoolHistoryProof {
            confirmed_tx_count: TransactionCount::from_u32(1),
            complete_height: ChainTipHeight::try_new(100).expect("height should parse"),
        },
    )
    .expect("address proof should seed");
    crate::db::with_user_db_mut(user_id, |conn| {
        conn.execute(
            "UPDATE digital_asset_accounts
             SET account_kind = 'hd_pubkey'
             WHERE id = ?1",
            [account.account_id.to_string()],
        )
        .map_err(|err| crate::db::DbError::new(format!("account kind update failed: {err}")))?;
        Ok::<(), crate::db::DbError>(())
    })
    .expect("account kind should update");
    upsert_account_sync_state(
        user_id,
        account.account_id,
        20,
        Some(0),
        Some(0),
        observed_at,
    )
    .expect("HD account sync state should seed");
    super::ledger_rebuild::rebuild_account_transaction_ledger(
        user_id,
        account.account_id,
        observed_at,
    )
    .expect("initial ledger should rebuild");
    crate::db::with_user_db_mut(user_id, |conn| {
        conn.execute_batch(
            "CREATE TRIGGER reject_atomic_hd_bitcoin_ledger
             BEFORE INSERT ON account_transaction_ledger
             BEGIN
               SELECT RAISE(ABORT, 'reject atomic HD ledger');
             END;",
        )
        .map_err(|err| crate::db::DbError::new(format!("trigger should install: {err}")))
    })
    .expect("trigger should install");

    publish_bitcoin_account_completion(
        user_id,
        BitcoinAccountCompletionPublication {
            account_id: account.account_id,
            final_address_proof: None,
            completed_hd_discovery: Some(BitcoinHdDiscoveryPublication {
                external_last_index: Some(0),
                internal_last_index: Some(0),
                completed_tip: ChainTipHeight::try_new(100).expect("height should parse"),
                completed_at: observed_at,
            }),
            observed_at,
        },
    )
    .expect_err("ledger failure must roll back checkpoint");

    let state = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT ass.last_scanned_height,
                    ass.last_scanned_time,
                    atl.closing_balance_lo
             FROM account_sync_state ass
             JOIN account_transaction_ledger atl ON atl.account_id = ass.account_id
             WHERE ass.account_id = ?1",
            [account.account_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .map_err(|err| crate::db::DbError::new(format!("HD atomic state query failed: {err}")))
    })
    .expect("HD atomic state should load");
    assert_eq!(state, (None, None, None));
}

#[test]
fn bitcoin_incomplete_rebuild_uses_known_synthetic_basis_but_not_during_repair() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let observed_at = dt("2026-07-24T11:00:00Z");
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("Synthetic basis");
    let account = add_bitcoin_address(
        user_id,
        &address,
        Network::Mainnet,
        None,
        Some(&label),
        observed_at,
    )
    .expect("bitcoin account should insert");
    let tracked = TrackedAddress::parse(address.canonical()).expect("tracked address should parse");
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &[SyncTransactionRecord {
            tx_hash: TxHash::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .expect("hash should parse"),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(100),
            block_hash: Some("block-100".to_string()),
            block_time: Some(observed_at),
            fee_amount: None,
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked),
                script_pubkey_hex: "00".to_string(),
                value_amount: 10,
            }],
        }],
        observed_at,
    )
    .expect("transaction should reconcile");
    let run_id = TransactionSyncRunId::new();
    mark_address_sync_started(user_id, account.address_id, run_id, observed_at)
        .expect("address sync should start");
    mark_address_sync_completed_success(
        user_id,
        &AddressSyncSuccess {
            address_id: account.address_id,
            run_id,
            started_at: observed_at,
            completed_at: observed_at,
            last_tip_height: ChainTipHeight::try_new(100).expect("tip should parse"),
            new_tx_count: TransactionCount::from_u32(1),
            updated_tx_count: TransactionCount::zero(),
            api_confirmed_balance: Some(ApiConfirmedBalance::from_amount(
                UnsignedAmount::from_u128(110),
            )),
        },
    )
    .expect("address sync should complete");

    super::ledger_rebuild::rebuild_account_transaction_ledger(
        user_id,
        account.account_id,
        observed_at,
    )
    .expect("incomplete ledger should rebuild");

    let closing = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1",
            [account.account_id.to_string()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|err| crate::db::DbError::new(format!("closing query failed: {err}")))
    })
    .expect("closing should load");
    assert_eq!(closing, Some(110));

    let repair_run = crate::db::raw_ingestion::start_sync_run(
        user_id,
        crate::db::raw_ingestion::StartSyncRunRequest {
            integration: crate::db::raw_ingestion::IntegrationKind::Mempool,
            scope_kind: crate::db::raw_ingestion::SyncRunScopeKind::Address,
            scope_address_id: account.address_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            trigger_kind: crate::db::raw_ingestion::SyncRunTriggerKind::Backfill,
            started_at: observed_at,
            summary_json: None,
        },
    )
    .expect("repair run should start");
    crate::db::begin_mempool_history_scan(user_id, account.address_id, repair_run.sync_run_id)
        .expect("repair scan should start");

    super::ledger_rebuild::rebuild_account_transaction_ledger(
        user_id,
        account.account_id,
        observed_at,
    )
    .expect("repair ledger should rebuild as unavailable");
    let repair_closing = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1",
            [account.account_id.to_string()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|err| crate::db::DbError::new(format!("repair closing query failed: {err}")))
    })
    .expect("repair closing should load");
    assert_eq!(repair_closing, None);
}

#[test]
fn bitcoin_incomplete_rebuild_without_provider_balance_has_null_closing() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let observed_at = dt("2026-07-24T12:00:00Z");
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("Missing basis");
    let account = add_bitcoin_address(
        user_id,
        &address,
        Network::Mainnet,
        None,
        Some(&label),
        observed_at,
    )
    .expect("bitcoin account should insert");
    let tracked = TrackedAddress::parse(address.canonical()).expect("tracked address should parse");
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &[SyncTransactionRecord {
            tx_hash: TxHash::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .expect("hash should parse"),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(100),
            block_hash: Some("block-100".to_string()),
            block_time: Some(observed_at),
            fee_amount: None,
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(tracked),
                script_pubkey_hex: "00".to_string(),
                value_amount: 10,
            }],
        }],
        observed_at,
    )
    .expect("transaction should reconcile");

    super::ledger_rebuild::rebuild_account_transaction_ledger(
        user_id,
        account.account_id,
        observed_at,
    )
    .expect("unavailable ledger should rebuild");

    let closing = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1",
            [account.account_id.to_string()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|err| crate::db::DbError::new(format!("closing query failed: {err}")))
    })
    .expect("closing should load");
    assert_eq!(closing, None);
}

#[test]
fn bitcoin_dated_balance_gate_keeps_rows_and_marks_incomplete_boundaries_unknown() {
    for provider_balance in [150_000_u128, 50_000_u128] {
        let _runtime = acquire_test_runtime().expect("test runtime should initialize");
        let user_id = UserId::new();
        initialize_user_db_for_test(user_id).expect("user db should initialize");
        let (account_id, wallet_id, _, _) =
            seed_btc_partial_backfill_fixture_with_balance(user_id, provider_balance);
        let from = dt("2026-01-05T00:00:00Z");
        let provisional = BalanceReliability::Provisional {
            reasons: vec![BalanceProvisionalReason::HistoricalBackfillInProgress],
        };

        let pages = page_query::load_account_transactions_pages(
            user_id,
            account_id,
            (1, 1),
            50,
            TransactionSortDirection::Ascending,
            &TransactionFilters {
                status: Vec::new(),
                from_date: Some(from),
                to_date: None,
            },
            TransactionCount::from_u32(u32::MAX),
        )
        .expect("incomplete Bitcoin page should load");
        assert_eq!(pages.opening_balance_state, NativeBalanceState::Unknown);
        assert_eq!(pages.closing_balance_state, NativeBalanceState::Unknown);
        assert_eq!(pages.opening_balance_reliability, provisional);
        assert_eq!(pages.confirmed.total, 1);
        assert_eq!(pages.confirmed.rows.len(), 1);
        assert_eq!(
            pages.confirmed.rows[0].balance_reliability,
            pages.opening_balance_reliability
        );

        let timezone =
            crate::models::UserTimezone("UTC".parse().expect("test timezone should parse"));
        let report = super::wallet_report::load_wallet_report(
            user_id,
            wallet_id,
            Some(chrono::NaiveDate::from_ymd_opt(2026, 1, 5).expect("valid from date")),
            Some(chrono::NaiveDate::from_ymd_opt(2026, 1, 20).expect("valid to date")),
            timezone,
            TransactionCount::from_u32(u32::MAX),
        )
        .expect("incomplete Bitcoin report should load");
        assert_eq!(
            report.accounts[0].opening_balance_state,
            super::wallet_report::WalletReportBalanceState::Unknown
        );
        assert_eq!(
            report.accounts[0].closing_balance_state,
            super::wallet_report::WalletReportBalanceState::Unknown
        );
        assert_eq!(
            report.accounts[0].opening_balance_reliability,
            pages.opening_balance_reliability
        );
    }
}

#[test]
fn bitcoin_history_cap_marks_limited_and_raising_it_restores_raw_coverage() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let (account_id, wallet_id, _, _) =
        seed_btc_partial_backfill_fixture_with_balance(user_id, 150_000);
    let filters = TransactionFilters {
        status: Vec::new(),
        from_date: Some(dt("2026-01-05T00:00:00Z")),
        to_date: None,
    };

    let limited = page_query::load_account_transactions_pages(
        user_id,
        account_id,
        (1, 1),
        50,
        TransactionSortDirection::Ascending,
        &filters,
        TransactionCount::from_u32(1),
    )
    .expect("limited page should load");
    assert_eq!(
        limited.bitcoin_history_coverage,
        Some(BitcoinAccountHistoryCoverage::Limited)
    );
    assert_eq!(
        limited.opening_balance_reliability,
        BalanceReliability::Provisional {
            reasons: vec![BalanceProvisionalReason::HistoricalCoverageLimited],
        }
    );

    let uncapped = page_query::load_account_transactions_pages(
        user_id,
        account_id,
        (1, 1),
        50,
        TransactionSortDirection::Ascending,
        &filters,
        TransactionCount::from_u32(2),
    )
    .expect("uncapped page should load");
    assert!(matches!(
        uncapped.bitcoin_history_coverage,
        Some(BitcoinAccountHistoryCoverage::Unscanned | BitcoinAccountHistoryCoverage::Syncing)
    ));

    let report = super::wallet_report::load_wallet_report(
        user_id,
        wallet_id,
        Some(chrono::NaiveDate::from_ymd_opt(2026, 1, 5).expect("valid from date")),
        Some(chrono::NaiveDate::from_ymd_opt(2026, 1, 20).expect("valid to date")),
        crate::models::UserTimezone("UTC".parse().expect("test timezone should parse")),
        TransactionCount::from_u32(1),
    )
    .expect("limited report should load");
    assert_eq!(
        report.accounts[0].bitcoin_history_coverage,
        Some(BitcoinAccountHistoryCoverage::Limited)
    );
    assert_eq!(
        report.accounts[0].opening_balance_reliability,
        limited.opening_balance_reliability
    );
}

#[test]
fn bitcoin_active_strict_repair_remains_syncing_over_cap_until_repair_ends() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let (account_id, _, _, _) = seed_btc_partial_backfill_fixture_with_balance(user_id, 150_000);
    let address_id = crate::db::get_sync_addresses_for_account(user_id, account_id)
        .expect("sync addresses should load")[0]
        .address_id;
    let repair_run = crate::db::raw_ingestion::start_sync_run(
        user_id,
        crate::db::raw_ingestion::StartSyncRunRequest {
            integration: crate::db::raw_ingestion::IntegrationKind::Mempool,
            scope_kind: crate::db::raw_ingestion::SyncRunScopeKind::Address,
            scope_address_id: address_id,
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            trigger_kind: crate::db::raw_ingestion::SyncRunTriggerKind::Backfill,
            started_at: dt("2026-01-20T11:00:00Z"),
            summary_json: None,
        },
    )
    .expect("repair run should start");
    crate::db::begin_mempool_history_scan(user_id, address_id, repair_run.sync_run_id)
        .expect("strict repair should begin");
    let filters = TransactionFilters {
        status: Vec::new(),
        from_date: Some(dt("2026-01-05T00:00:00Z")),
        to_date: None,
    };

    let active_repair = page_query::load_account_transactions_pages(
        user_id,
        account_id,
        (1, 1),
        50,
        TransactionSortDirection::Ascending,
        &filters,
        TransactionCount::from_u32(1),
    )
    .expect("active repair page should load");
    assert_eq!(
        active_repair.bitcoin_history_coverage,
        Some(BitcoinAccountHistoryCoverage::Syncing)
    );

    crate::db::with_user_db_mut(user_id, |conn| {
        conn.execute(
            "UPDATE transaction_sync_state
             SET mempool_history_scan_start_run_id = NULL
             WHERE address_id = ?1",
            [address_id.to_string()],
        )
        .map_err(|err| {
            crate::db::DbError::new(format!("repair completion update failed: {err}"))
        })?;
        Ok::<(), crate::db::DbError>(())
    })
    .expect("strict repair should end");

    let ended_repair = page_query::load_account_transactions_pages(
        user_id,
        account_id,
        (1, 1),
        50,
        TransactionSortDirection::Ascending,
        &filters,
        TransactionCount::from_u32(1),
    )
    .expect("post-repair page should load");
    assert_eq!(
        ended_repair.bitcoin_history_coverage,
        Some(BitcoinAccountHistoryCoverage::Limited)
    );
}

#[test]
fn bitcoin_dated_balance_gate_rejects_every_incomplete_coverage_state() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let (account_id, first_transaction_date, last_successful_sync_date) =
        seed_btc_partial_backfill_fixture(user_id);

    crate::db::with_user_db(user_id, |conn| {
        let meta = balance::load_account_meta(conn, account_id)?;
        for coverage in [
            BitcoinAccountHistoryCoverage::Unscanned,
            BitcoinAccountHistoryCoverage::Syncing,
            BitcoinAccountHistoryCoverage::Limited,
        ] {
            for (boundary_kind, requested_boundary_date) in [
                (
                    NativeBalanceBoundaryKind::Opening,
                    Some(first_transaction_date + chrono::Duration::days(1)),
                ),
                (
                    NativeBalanceBoundaryKind::Closing,
                    Some(last_successful_sync_date),
                ),
                (NativeBalanceBoundaryKind::Closing, None),
            ] {
                let resolution = balance::resolve_native_balance_at_boundary(
                    conn,
                    account_id,
                    &meta,
                    balance::NativeBalanceBoundaryRequest {
                        boundary_kind,
                        requested_boundary_date,
                        first_transaction_date: Some(first_transaction_date),
                        transaction_history_pending: false,
                    },
                    &AccountBalanceReliabilityContext {
                        last_successful_sync_date: Some(last_successful_sync_date),
                        balance_reliability: BalanceReliability::Provisional {
                            reasons: vec![BalanceProvisionalReason::HistoricalBackfillInProgress],
                        },
                        bitcoin_history_coverage: Some(coverage),
                    },
                )?;
                assert_eq!(resolution.state, NativeBalanceState::Unknown);
                assert_eq!(resolution.amount, None);
            }
        }
        Ok::<(), crate::db::DbError>(())
    })
    .expect("incomplete Bitcoin boundaries should resolve unavailable");
}

struct EmptyBitcoinCompletionFixture {
    account_id: DigitalAssetAccountId,
    address_id: crate::wallets::DigitalAssetAddressId,
    wallet_id: crate::wallets::WalletId,
    publication_at: DateTime<Utc>,
}

fn seed_empty_bitcoin_completion_without_account_success(
    user_id: UserId,
) -> EmptyBitcoinCompletionFixture {
    let created_at = dt("2026-07-24T12:00:00Z");
    let integration_started = dt("2026-07-24T12:01:00Z");
    let address_started = dt("2026-07-24T12:02:00Z");
    let address_completed = dt("2026-07-24T12:03:00Z");
    let publication_at = address_completed;
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let account = add_bitcoin_address(
        user_id,
        &address,
        Network::Mainnet,
        None,
        Some(&parse_wallet_label("Canonical empty")),
        created_at,
    )
    .expect("Bitcoin account should insert");
    mark_account_integration_sync_started(
        user_id,
        account.account_id,
        SyncIntegrationId::Mempool,
        integration_started,
    )
    .expect("account integration should start");
    let run_id = TransactionSyncRunId::new();
    mark_address_sync_started(user_id, account.address_id, run_id, address_started)
        .expect("address sync should start");
    mark_address_sync_completed_success(
        user_id,
        &AddressSyncSuccess {
            address_id: account.address_id,
            run_id,
            started_at: address_started,
            completed_at: address_completed,
            last_tip_height: ChainTipHeight::try_new(100).expect("tip should parse"),
            new_tx_count: TransactionCount::zero(),
            updated_tx_count: TransactionCount::zero(),
            api_confirmed_balance: Some(ApiConfirmedBalance::from_amount(UnsignedAmount::zero())),
        },
    )
    .expect("address sync should complete");
    assert!(
        publish_bitcoin_account_completion(
            user_id,
            BitcoinAccountCompletionPublication {
                account_id: account.account_id,
                final_address_proof: Some(BitcoinAddressProofPublication {
                    address_id: account.address_id,
                    proof: MempoolHistoryProof {
                        confirmed_tx_count: TransactionCount::zero(),
                        complete_height: ChainTipHeight::try_new(100).expect("height should parse"),
                    },
                    scan_start_run_id: None,
                }),
                completed_hd_discovery: None,
                observed_at: publication_at,
            },
        )
        .expect("canonical empty account should publish")
    );

    EmptyBitcoinCompletionFixture {
        account_id: account.account_id,
        address_id: account.address_id,
        wallet_id: account.wallet_id,
        publication_at,
    }
}

fn load_empty_bitcoin_page(
    user_id: UserId,
    fixture: &EmptyBitcoinCompletionFixture,
) -> AccountTransactionsPages {
    page_query::load_account_transactions_pages(
        user_id,
        fixture.account_id,
        (1, 1),
        50,
        TransactionSortDirection::Ascending,
        &TransactionFilters {
            status: Vec::new(),
            from_date: Some(fixture.publication_at),
            to_date: Some(fixture.publication_at),
        },
        TransactionCount::from_u32(u32::MAX),
    )
    .expect("canonical empty page should load")
}

#[test]
fn bitcoin_atomic_empty_completion_without_account_success_stays_provisional() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let fixture = seed_empty_bitcoin_completion_without_account_success(user_id);
    let provisional = BalanceReliability::Provisional {
        reasons: vec![BalanceProvisionalReason::HistoricalBackfillInProgress],
    };

    let (coverage, reliability) = crate::db::with_user_db(user_id, |conn| {
        Ok::<_, crate::db::DbError>((
            super::ledger_rebuild::load_bitcoin_account_history_coverage(conn, fixture.account_id)?,
            crate::db::balance_reliability::load_account_balance_reliability_context(
                conn,
                fixture.account_id,
            )?
            .balance_reliability,
        ))
    })
    .expect("consumer coverage should load");
    assert_eq!(coverage, Some(BitcoinAccountHistoryCoverage::Syncing));
    assert_eq!(reliability, provisional);

    let pages = load_empty_bitcoin_page(user_id, &fixture);
    assert_ne!(
        pages.opening_balance_state,
        NativeBalanceState::CanonicalZero
    );
    assert_ne!(
        pages.closing_balance_state,
        NativeBalanceState::CanonicalZero
    );
    assert_eq!(pages.opening_balance_reliability, provisional);
    assert_eq!(pages.closing_balance_reliability, provisional);

    let report = super::wallet_report::load_wallet_report(
        user_id,
        fixture.wallet_id,
        Some(chrono::NaiveDate::from_ymd_opt(2026, 7, 24).expect("valid report date")),
        Some(chrono::NaiveDate::from_ymd_opt(2026, 7, 24).expect("valid report date")),
        crate::models::UserTimezone("UTC".parse().expect("test timezone should parse")),
        TransactionCount::from_u32(u32::MAX),
    )
    .expect("canonical empty report should load");
    assert_ne!(
        report.accounts[0].opening_balance_state,
        super::wallet_report::WalletReportBalanceState::CanonicalZero
    );
    assert_eq!(report.accounts[0].opening_balance_reliability, provisional);
}

#[test]
fn bitcoin_account_success_exposes_complete_canonical_zero() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let fixture = seed_empty_bitcoin_completion_without_account_success(user_id);
    refresh_account_integration_sync_state(
        user_id,
        fixture.account_id,
        SyncIntegrationId::Mempool,
        fixture.publication_at + chrono::Duration::seconds(1),
    )
    .expect("account integration success should persist");

    let coverage = crate::db::with_user_db(user_id, |conn| {
        super::ledger_rebuild::load_bitcoin_account_history_coverage(conn, fixture.account_id)
    })
    .expect("consumer coverage should load");
    assert_eq!(
        coverage,
        Some(BitcoinAccountHistoryCoverage::Complete {
            coverage_height: ChainTipHeight::try_new(100).expect("height should parse"),
        })
    );

    let pages = load_empty_bitcoin_page(user_id, &fixture);
    assert_eq!(
        pages.opening_balance_state,
        NativeBalanceState::CanonicalZero
    );
    assert_eq!(
        pages.closing_balance_state,
        NativeBalanceState::CanonicalZero
    );
    assert_eq!(
        pages.opening_balance_reliability,
        BalanceReliability::finalized()
    );
    assert_eq!(
        pages.closing_balance_reliability,
        BalanceReliability::finalized()
    );

    let report = super::wallet_report::load_wallet_report(
        user_id,
        fixture.wallet_id,
        Some(chrono::NaiveDate::from_ymd_opt(2026, 7, 24).expect("valid report date")),
        Some(chrono::NaiveDate::from_ymd_opt(2026, 7, 24).expect("valid report date")),
        crate::models::UserTimezone("UTC".parse().expect("test timezone should parse")),
        TransactionCount::from_u32(u32::MAX),
    )
    .expect("canonical empty report should load");
    assert_eq!(
        report.accounts[0].opening_balance_state,
        super::wallet_report::WalletReportBalanceState::CanonicalZero
    );
    assert_eq!(
        report.accounts[0].opening_balance_reliability,
        BalanceReliability::finalized()
    );
}

#[test]
fn bitcoin_later_account_start_or_failure_revokes_consumer_complete() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let fixture = seed_empty_bitcoin_completion_without_account_success(user_id);
    refresh_account_integration_sync_state(
        user_id,
        fixture.account_id,
        SyncIntegrationId::Mempool,
        fixture.publication_at + chrono::Duration::seconds(1),
    )
    .expect("account integration success should persist");
    let later_started = fixture.publication_at + chrono::Duration::minutes(1);
    mark_account_integration_sync_started(
        user_id,
        fixture.account_id,
        SyncIntegrationId::Mempool,
        later_started,
    )
    .expect("later account integration should start");

    let coverage_after_start = crate::db::with_user_db(user_id, |conn| {
        super::ledger_rebuild::load_bitcoin_account_history_coverage(conn, fixture.account_id)
    })
    .expect("coverage after later start should load");
    assert_eq!(
        coverage_after_start,
        Some(BitcoinAccountHistoryCoverage::Syncing)
    );
    let provisional = BalanceReliability::Provisional {
        reasons: vec![BalanceProvisionalReason::HistoricalBackfillInProgress],
    };
    let pages_after_start = load_empty_bitcoin_page(user_id, &fixture);
    assert_eq!(
        pages_after_start.opening_balance_state,
        NativeBalanceState::Unknown
    );
    assert_eq!(pages_after_start.opening_balance_reliability, provisional);

    let failed_at = later_started + chrono::Duration::minutes(1);
    let run_id = TransactionSyncRunId::new();
    mark_address_sync_started(user_id, fixture.address_id, run_id, later_started)
        .expect("later address sync should start");
    mark_address_sync_completed_failure(
        user_id,
        fixture.address_id,
        run_id,
        later_started,
        failed_at,
        &SyncErrorMessage::sanitize("later integration failure"),
        true,
    )
    .expect("later address failure should persist");
    refresh_account_integration_sync_state(
        user_id,
        fixture.account_id,
        SyncIntegrationId::Mempool,
        failed_at,
    )
    .expect("account integration failure should persist");

    let (coverage_after_failure, reliability_after_failure) =
        crate::db::with_user_db(user_id, |conn| {
            Ok::<_, crate::db::DbError>((
                super::ledger_rebuild::load_bitcoin_account_history_coverage(
                    conn,
                    fixture.account_id,
                )?,
                crate::db::balance_reliability::load_account_balance_reliability_context(
                    conn,
                    fixture.account_id,
                )?
                .balance_reliability,
            ))
        })
        .expect("coverage after account failure should load");
    assert_eq!(
        coverage_after_failure,
        Some(BitcoinAccountHistoryCoverage::Syncing)
    );
    assert_eq!(
        reliability_after_failure,
        BalanceReliability::Provisional {
            reasons: vec![
                BalanceProvisionalReason::FirstSuccessfulSyncPending,
                BalanceProvisionalReason::HistoricalBackfillInProgress,
            ],
        }
    );
}

#[test]
fn bitcoin_invalidation_rebuild_before_scan_start_keeps_closings_unknown() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let (account_id, _, rebuilt_at) = seed_btc_partial_backfill_fixture(user_id);
    let address_id = crate::db::get_sync_addresses_for_account(user_id, account_id)
        .expect("address should load")[0]
        .address_id;

    crate::db::invalidate_mempool_history_coverage(
        user_id,
        &crate::db::CoverageInvalidationTargets {
            address_ids: std::collections::HashSet::from([address_id]),
            account_ids: std::collections::HashSet::from([account_id]),
        },
    )
    .expect("coverage should invalidate");
    super::ledger_rebuild::rebuild_account_transaction_ledger_with_unknown_bitcoin_basis(
        user_id, account_id, rebuilt_at,
    )
    .expect("invalidation boundary should rebuild");

    let (scan_start, closing) = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT tss.mempool_history_scan_start_run_id,
                    atl.closing_balance_lo
             FROM transaction_sync_state tss
             JOIN digital_asset_addresses da ON da.id = tss.address_id
             JOIN account_transaction_ledger atl ON atl.account_id = da.account_id
             WHERE da.account_id = ?1",
            [account_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                ))
            },
        )
        .map_err(|err| crate::db::DbError::new(format!("invalidation state failed: {err}")))
    })
    .expect("invalidation state should load");
    assert_eq!(scan_start, None);
    assert_eq!(closing, None);
}

#[test]
fn bitcoin_regular_rebuild_swap_failure_clears_old_closings() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let (account_id, _, rebuilt_at) = seed_btc_partial_backfill_fixture(user_id);
    crate::db::with_user_db_mut(user_id, |conn| {
        conn.execute_batch(
            "CREATE TRIGGER reject_regular_bitcoin_ledger
             BEFORE INSERT ON account_transaction_ledger
             BEGIN
               SELECT RAISE(ABORT, 'reject regular ledger');
             END;",
        )
        .map_err(|err| crate::db::DbError::new(format!("trigger should install: {err}")))
    })
    .expect("trigger should install");

    super::ledger_rebuild::rebuild_account_transaction_ledger(user_id, account_id, rebuilt_at)
        .expect_err("regular ledger replacement should fail");

    let closing = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1",
            [account_id.to_string()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|err| crate::db::DbError::new(format!("closing query failed: {err}")))
    })
    .expect("old ledger row should remain");
    assert_eq!(closing, None);
}

#[test]
fn invalid_bitcoin_completion_request_does_not_clear_ethereum_closing() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let observed_at = dt("2026-07-24T12:15:00Z");
    let address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
    let account =
        create_eth_wallet_account_fixture(user_id, &address, "ETH atomic guard", observed_at);
    seed_eth_ledger_row(
        user_id,
        account.account_id,
        &address.checksummed(),
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "2026-07-24T12:15:00Z",
        UnsignedAmount::from_u128(42),
        UnsignedAmount::from_u128(42),
    );

    publish_bitcoin_account_completion(
        user_id,
        BitcoinAccountCompletionPublication {
            account_id: account.account_id,
            final_address_proof: Some(BitcoinAddressProofPublication {
                address_id: account.address_id,
                proof: MempoolHistoryProof {
                    confirmed_tx_count: TransactionCount::zero(),
                    complete_height: ChainTipHeight::try_new(1).expect("height should parse"),
                },
                scan_start_run_id: None,
            }),
            completed_hd_discovery: None,
            observed_at,
        },
    )
    .expect_err("Ethereum account should reject Bitcoin completion");

    let closing = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1",
            [account.account_id.to_string()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|err| crate::db::DbError::new(format!("Ethereum closing query failed: {err}")))
    })
    .expect("Ethereum closing should load");
    assert_eq!(closing, Some(42));
}

#[test]
fn bitcoin_half_null_provider_balance_rebuilds_as_unknown() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let (account_id, _, rebuilt_at) = seed_btc_partial_backfill_fixture(user_id);
    crate::db::with_user_db_mut(user_id, |conn| {
        conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
            .map_err(|err| crate::db::DbError::new(format!("fixture pragma failed: {err}")))?;
        conn.execute(
            "UPDATE transaction_sync_state
             SET api_confirmed_balance_lo = NULL
             WHERE address_id IN (
                 SELECT id FROM digital_asset_addresses WHERE account_id = ?1
             )",
            [account_id.to_string()],
        )
        .map_err(|err| crate::db::DbError::new(format!("provider fixture failed: {err}")))?;
        conn.execute_batch("PRAGMA ignore_check_constraints = OFF;")
            .map_err(|err| crate::db::DbError::new(format!("fixture pragma reset failed: {err}")))
    })
    .expect("half-null provider fixture should persist");

    super::ledger_rebuild::rebuild_account_transaction_ledger(user_id, account_id, rebuilt_at)
        .expect("half-null provider balance should be unavailable");
    let closing = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1",
            [account_id.to_string()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|err| crate::db::DbError::new(format!("closing query failed: {err}")))
    })
    .expect("closing should load");
    assert_eq!(closing, None);
}

#[test]
fn bitcoin_invalid_full_provider_balance_still_errors() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let (account_id, _, rebuilt_at) = seed_btc_partial_backfill_fixture(user_id);
    crate::db::with_user_db_mut(user_id, |conn| {
        conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
            .map_err(|err| crate::db::DbError::new(format!("fixture pragma failed: {err}")))?;
        conn.execute(
            "UPDATE transaction_sync_state
             SET api_confirmed_balance_hi = -1,
                 api_confirmed_balance_lo = 0
             WHERE address_id IN (
                 SELECT id FROM digital_asset_addresses WHERE account_id = ?1
             )",
            [account_id.to_string()],
        )
        .map_err(|err| crate::db::DbError::new(format!("provider fixture failed: {err}")))?;
        conn.execute_batch("PRAGMA ignore_check_constraints = OFF;")
            .map_err(|err| crate::db::DbError::new(format!("fixture pragma reset failed: {err}")))
    })
    .expect("invalid provider fixture should persist");

    super::ledger_rebuild::rebuild_account_transaction_ledger(user_id, account_id, rebuilt_at)
        .expect_err("fully present invalid provider balance should fail");
}

#[test]
fn bitcoin_split_amount_conversion_failure_clears_existing_closings() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");
    let (account_id, _, rebuilt_at) = seed_btc_partial_backfill_fixture(user_id);

    crate::db::with_user_db_mut(user_id, |conn| {
        conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
            .map_err(|err| crate::db::DbError::new(format!("fixture pragma failed: {err}")))?;
        conn.execute(
            "UPDATE transaction_outputs
             SET value_amount_lo = -1
             WHERE tx_id IN (
                 SELECT id
                 FROM chain_transactions
                 WHERE asset_id = 'bitcoin'
             )",
            [],
        )
        .map_err(|err| crate::db::DbError::new(format!("fixture corruption failed: {err}")))?;
        conn.execute_batch("PRAGMA ignore_check_constraints = OFF;")
            .map_err(|err| {
                crate::db::DbError::new(format!("fixture pragma reset failed: {err}"))
            })?;
        Ok::<(), crate::db::DbError>(())
    })
    .expect("invalid split fixture should persist");

    super::ledger_rebuild::rebuild_account_transaction_ledger(user_id, account_id, rebuilt_at)
        .expect_err("invalid split amount must fail the rebuild");

    let closing_parts = crate::db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT closing_balance_hi, closing_balance_lo
             FROM account_transaction_ledger
             WHERE account_id = ?1",
            [account_id.to_string()],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .map_err(|err| crate::db::DbError::new(format!("closing balance query failed: {err}")))
    })
    .expect("closing balance should load");
    assert_eq!(closing_parts, (None, None));
}

#[test]
fn load_account_transactions_pages_uses_address_success_date_when_completed_account_retains_cursor()
{
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let transaction_time = dt("2026-03-30T19:15:21Z");
    let address_started = dt("2026-03-30T20:03:38Z");
    let address_completed = dt("2026-03-30T20:03:44Z");
    let integration_started = dt("2026-03-30T20:01:57Z");

    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("BTC Transactions");
    let add_result = add_bitcoin_address(
        user_id,
        &address,
        Network::Mainnet,
        None,
        Some(&label),
        transaction_time,
    )
    .expect("bitcoin fixture should insert");

    let owned_tracked =
        TrackedAddress::parse(address.canonical()).expect("owned tracked address should parse");
    let tx_hash = TxHash::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        .expect("tx hash should parse");
    let records = vec![SyncTransactionRecord {
        tx_hash,
        status: ChainTransactionStatus::Confirmed,
        block_height: Some(100),
        block_hash: Some("blockhash-100".to_string()),
        block_time: Some(transaction_time),
        fee_amount: Some(200),
        inputs: Vec::new(),
        outputs: vec![SyncTransactionOutputRecord {
            output_index: 0,
            raw_address: Some(owned_tracked),
            script_pubkey_hex: "0014deadbeef".to_string(),
            value_amount: 50_000,
        }],
    }];
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &records,
        transaction_time,
    )
    .expect("transactions should reconcile");
    super::ledger_rebuild::rebuild_account_transaction_ledger(
        user_id,
        add_result.account_id,
        transaction_time,
    )
    .expect("ledger rebuild should succeed");

    mark_account_integration_sync_started(
        user_id,
        add_result.account_id,
        SyncIntegrationId::Mempool,
        integration_started,
    )
    .expect("integration start should persist");
    let run_id = TransactionSyncRunId::new();
    mark_address_sync_started(user_id, add_result.address_id, run_id, address_started)
        .expect("address sync start should persist");
    mark_address_sync_completed_success(
        user_id,
        &AddressSyncSuccess {
            address_id: add_result.address_id,
            run_id,
            started_at: address_started,
            completed_at: address_completed,
            last_tip_height: ChainTipHeight::try_new(777).expect("tip should be valid"),
            new_tx_count: TransactionCount::from_u32(1),
            updated_tx_count: TransactionCount::zero(),
            api_confirmed_balance: None,
        },
    )
    .expect("address sync success should persist");
    let cursor = MempoolCursorTxid::parse(
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    )
    .expect("cursor should parse");
    update_address_mempool_backfill_cursor(user_id, add_result.address_id, Some(&cursor))
        .expect("mempool cursor update should succeed");
    refresh_account_integration_sync_state(
        user_id,
        add_result.account_id,
        SyncIntegrationId::Mempool,
        address_completed,
    )
    .expect("integration state refresh should succeed");

    let persisted_row = crate::db::user_db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT last_completed_at, last_result
                 FROM account_integration_sync_state
                 WHERE account_id = ?1
                   AND integration_id = 'mempool'",
            params![add_result.account_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .map_err(|err| {
            crate::db::error::DbError::new(format!(
                "Failed to load persisted account integration sync state: {err}"
            ))
        })
    })
    .expect("account integration state should load");
    assert_eq!(
        persisted_row,
        (
            Some(address_completed.to_rfc3339()),
            Some("success".to_string())
        )
    );

    let pages = page_query::load_account_transactions_pages(
        user_id,
        add_result.account_id,
        (1, 1),
        50,
        TransactionSortDirection::Ascending,
        &TransactionFilters {
            status: Vec::new(),
            from_date: None,
            to_date: None,
        },
        TransactionCount::from_u32(u32::MAX),
    )
    .expect("account transactions pages should load");

    assert_eq!(pages.opening_balance_date, Some(transaction_time));
    assert_eq!(pages.closing_balance_state, NativeBalanceState::Unknown);
    assert_eq!(
        pages.opening_balance_reliability,
        BalanceReliability::Provisional {
            reasons: vec![BalanceProvisionalReason::HistoricalBackfillInProgress],
        }
    );
    assert_eq!(
        pages.confirmed.rows[0].balance_reliability,
        BalanceReliability::Provisional {
            reasons: vec![BalanceProvisionalReason::HistoricalBackfillInProgress],
        }
    );
    assert_eq!(pages.closing_balance_date, Some(address_completed));
}

#[test]
fn page_query_opening_balance_is_unknown_when_history_never_ingested() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let created_at = dt("2026-01-10T12:00:00Z");
    let sync_started = dt("2026-01-20T09:00:00Z");
    let sync_completed = dt("2026-01-20T10:00:00Z");
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("BTC Pending History");
    let add_result = add_bitcoin_address(
        user_id,
        &address,
        Network::Mainnet,
        None,
        Some(&label),
        created_at,
    )
    .expect("bitcoin fixture should insert");

    let run_id = TransactionSyncRunId::new();
    mark_address_sync_started(user_id, add_result.address_id, run_id, sync_started)
        .expect("address sync start should persist");
    mark_address_sync_completed_success(
        user_id,
        &AddressSyncSuccess {
            address_id: add_result.address_id,
            run_id,
            started_at: sync_started,
            completed_at: sync_completed,
            last_tip_height: ChainTipHeight::try_new(777).expect("tip should be valid"),
            new_tx_count: TransactionCount::zero(),
            updated_tx_count: TransactionCount::zero(),
            api_confirmed_balance: Some(ApiConfirmedBalance::from_amount(
                UnsignedAmount::from_u128(150_000),
            )),
        },
    )
    .expect("address sync success should persist");
    update_address_mempool_expected_tx_count(
        user_id,
        add_result.address_id,
        Some(TransactionCount::from_u32(5)),
    )
    .expect("expected transaction count should persist");

    let pages = page_query::load_account_transactions_pages(
        user_id,
        add_result.account_id,
        (1, 1),
        50,
        TransactionSortDirection::Ascending,
        &TransactionFilters {
            status: Vec::new(),
            from_date: Some(dt("2026-01-05T00:00:00Z")),
            to_date: None,
        },
        TransactionCount::from_u32(u32::MAX),
    )
    .expect("account transactions pages should load");

    assert_eq!(pages.opening_balance_state, NativeBalanceState::Unknown);
}

#[test]
fn page_query_opening_balance_is_unknown_when_transaction_history_is_partial() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let (account_id, _, _) = seed_btc_partial_backfill_fixture(user_id);
    let address_id = crate::db::get_sync_addresses_for_account(user_id, account_id)
        .expect("sync addresses should load")[0]
        .address_id;
    update_address_mempool_expected_tx_count(
        user_id,
        address_id,
        Some(TransactionCount::from_u32(2)),
    )
    .expect("expected transaction count should persist");

    let pages = page_query::load_account_transactions_pages(
        user_id,
        account_id,
        (1, 1),
        50,
        TransactionSortDirection::Ascending,
        &TransactionFilters {
            status: Vec::new(),
            from_date: Some(dt("1990-01-01T00:00:00Z")),
            to_date: None,
        },
        TransactionCount::from_u32(u32::MAX),
    )
    .expect("account transactions pages should load");

    assert_eq!(pages.opening_balance_state, NativeBalanceState::Unknown);
    assert_eq!(
        pages.current_balance_state,
        NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(150_000))
    );
    assert_eq!(
        pages.current_balance_checked_at,
        Some(dt("2026-01-20T10:00:00Z"))
    );
}

#[test]
fn bitcoin_outgoing_with_fee_stores_external_value_excluding_fee() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let now = dt("2026-06-12T10:00:00Z");
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("BTC Fee Test");
    let add_result =
        add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
            .expect("bitcoin address should insert");

    let owned_tracked =
        TrackedAddress::parse(address.canonical()).expect("owned tracked address should parse");
    let receive_hash =
        TxHash::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .expect("receive hash should parse");
    let send_hash =
        TxHash::parse("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .expect("send hash should parse");

    let records = vec![
        SyncTransactionRecord {
            tx_hash: receive_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(1),
            block_hash: Some("blockhash-1".to_string()),
            block_time: Some(dt("2026-06-12T10:01:00Z")),
            fee_amount: None,
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(owned_tracked.clone()),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 10_000_000,
            }],
        },
        SyncTransactionRecord {
            tx_hash: send_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(2),
            block_hash: Some("blockhash-2".to_string()),
            block_time: Some(dt("2026-06-12T10:02:00Z")),
            fee_amount: Some(3_172),
            inputs: vec![SyncTransactionInputRecord {
                input_index: 0,
                prev_tx_hash: receive_hash,
                prev_output_index: 0,
                prev_address: Some(owned_tracked),
                value_amount: Some(10_000_000),
            }],
            outputs: Vec::new(),
        },
    ];
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &records,
        now,
    )
    .expect("transactions should reconcile");
    super::ledger_rebuild::rebuild_account_transaction_ledger(user_id, add_result.account_id, now)
        .expect("ledger rebuild should succeed");

    let send_row = crate::db::user_db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT tx_type, value_amount_lo, fee_amount_lo, closing_balance_lo
                 FROM account_transaction_ledger
                 WHERE account_id = ?1
                   AND tx_hash = ?2",
            params![add_result.account_id.to_string(), send_hash.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .map_err(|err| {
            crate::db::error::DbError::new(format!("Failed to load send ledger row: {err}"))
        })
    })
    .expect("send ledger row should load");

    assert_eq!(send_row, ("send".to_string(), 9_996_828, Some(3_172), None));
}

#[test]
fn bitcoin_mixed_input_fee_above_account_outflow_caps_external_value_at_zero() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let now = dt("2026-06-12T10:30:00Z");
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("BTC Mixed Input Fee Test");
    let add_result =
        add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
            .expect("bitcoin address should insert");

    let owned_tracked =
        TrackedAddress::parse(address.canonical()).expect("owned tracked address should parse");
    let receive_hash =
        TxHash::parse("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
            .expect("receive hash should parse");
    let send_hash =
        TxHash::parse("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
            .expect("send hash should parse");
    let external_prev_hash =
        TxHash::parse("9999999999999999999999999999999999999999999999999999999999999999")
            .expect("external previous hash should parse");

    let records = vec![
        SyncTransactionRecord {
            tx_hash: receive_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(1),
            block_hash: Some("blockhash-1".to_string()),
            block_time: Some(dt("2026-06-12T10:31:00Z")),
            fee_amount: None,
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(owned_tracked.clone()),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 2_000,
            }],
        },
        SyncTransactionRecord {
            tx_hash: send_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(2),
            block_hash: Some("blockhash-2".to_string()),
            block_time: Some(dt("2026-06-12T10:32:00Z")),
            fee_amount: Some(3_172),
            inputs: vec![
                SyncTransactionInputRecord {
                    input_index: 0,
                    prev_tx_hash: receive_hash,
                    prev_output_index: 0,
                    prev_address: Some(owned_tracked),
                    value_amount: Some(2_000),
                },
                SyncTransactionInputRecord {
                    input_index: 1,
                    prev_tx_hash: external_prev_hash,
                    prev_output_index: 0,
                    prev_address: None,
                    value_amount: Some(10_000),
                },
            ],
            outputs: Vec::new(),
        },
    ];
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &records,
        now,
    )
    .expect("transactions should reconcile");
    super::ledger_rebuild::rebuild_account_transaction_ledger(user_id, add_result.account_id, now)
        .expect("ledger rebuild should succeed");

    let send_row = crate::db::user_db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT tx_type, value_amount_lo, fee_amount_lo, closing_balance_lo
                 FROM account_transaction_ledger
                 WHERE account_id = ?1
                   AND tx_hash = ?2",
            params![add_result.account_id.to_string(), send_hash.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .map_err(|err| {
            crate::db::error::DbError::new(format!(
                "Failed to load mixed-input send ledger row: {err}"
            ))
        })
    })
    .expect("mixed-input send ledger row should load");

    assert_eq!(send_row, ("send".to_string(), 0, Some(3_172), None));
}

#[test]
fn rebuild_persists_signed_balance_delta_for_cospend_send() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let now = dt("2026-06-13T08:30:00Z");
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("BTC Delta Persistence Test");
    let add_result =
        add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
            .expect("bitcoin address should insert");

    let owned_tracked =
        TrackedAddress::parse(address.canonical()).expect("owned tracked address should parse");
    let receive_hash =
        TxHash::parse("1111111111111111111111111111111111111111111111111111111111111111")
            .expect("receive hash should parse");
    let cospend_tx_hash =
        TxHash::parse("2222222222222222222222222222222222222222222222222222222222222222")
            .expect("send hash should parse");
    let external_prev_hash =
        TxHash::parse("3333333333333333333333333333333333333333333333333333333333333333")
            .expect("external previous hash should parse");

    let records = vec![
        SyncTransactionRecord {
            tx_hash: receive_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(1),
            block_hash: Some("blockhash-1".to_string()),
            block_time: Some(dt("2026-06-13T08:31:00Z")),
            fee_amount: None,
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(owned_tracked.clone()),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 888,
            }],
        },
        SyncTransactionRecord {
            tx_hash: cospend_tx_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(2),
            block_hash: Some("blockhash-2".to_string()),
            block_time: Some(dt("2026-06-13T08:32:00Z")),
            fee_amount: Some(30_624),
            inputs: vec![
                SyncTransactionInputRecord {
                    input_index: 0,
                    prev_tx_hash: receive_hash,
                    prev_output_index: 0,
                    prev_address: Some(owned_tracked),
                    value_amount: Some(888),
                },
                SyncTransactionInputRecord {
                    input_index: 1,
                    prev_tx_hash: external_prev_hash,
                    prev_output_index: 0,
                    prev_address: None,
                    value_amount: Some(6_523_671_306_814),
                },
            ],
            outputs: Vec::new(),
        },
    ];
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &records,
        now,
    )
    .expect("transactions should reconcile");
    super::ledger_rebuild::rebuild_account_transaction_ledger(user_id, add_result.account_id, now)
        .expect("ledger rebuild should succeed");

    let (hi, lo, negative): (i64, i64, i64) = crate::db::user_db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT balance_delta_hi, balance_delta_lo, balance_delta_negative
             FROM account_transaction_ledger WHERE tx_hash = ?1",
            params![cospend_tx_hash.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|err| {
            crate::db::error::DbError::new(format!("Failed to load cospend delta row: {err}"))
        })
    })
    .expect("ledger row present");
    assert_eq!(negative, 1);
    let delta = signed_balance_delta_from_split(hi, lo, negative == 1).expect("decode");
    assert_eq!(delta, -888);
}

#[test]
fn bitcoin_self_transfer_with_fee_keeps_owned_output_value_and_fee_balance_delta() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let now = dt("2026-06-12T11:00:00Z");
    let address = parse_btc_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
    let label = parse_wallet_label("BTC Self Transfer Fee Test");
    let add_result =
        add_bitcoin_address(user_id, &address, Network::Mainnet, None, Some(&label), now)
            .expect("bitcoin address should insert");

    let owned_tracked =
        TrackedAddress::parse(address.canonical()).expect("owned tracked address should parse");
    let receive_hash =
        TxHash::parse("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")
            .expect("receive hash should parse");
    let self_transfer_hash =
        TxHash::parse("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd")
            .expect("self-transfer hash should parse");

    let records = vec![
        SyncTransactionRecord {
            tx_hash: receive_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(1),
            block_hash: Some("blockhash-1".to_string()),
            block_time: Some(dt("2026-06-12T11:01:00Z")),
            fee_amount: None,
            inputs: Vec::new(),
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(owned_tracked.clone()),
                script_pubkey_hex: "0014deadbeef".to_string(),
                value_amount: 10_000_000,
            }],
        },
        SyncTransactionRecord {
            tx_hash: self_transfer_hash.clone(),
            status: ChainTransactionStatus::Confirmed,
            block_height: Some(2),
            block_hash: Some("blockhash-2".to_string()),
            block_time: Some(dt("2026-06-12T11:02:00Z")),
            fee_amount: Some(3_172),
            inputs: vec![SyncTransactionInputRecord {
                input_index: 0,
                prev_tx_hash: receive_hash,
                prev_output_index: 0,
                prev_address: Some(owned_tracked.clone()),
                value_amount: Some(10_000_000),
            }],
            outputs: vec![SyncTransactionOutputRecord {
                output_index: 0,
                raw_address: Some(owned_tracked),
                script_pubkey_hex: "0014feedface".to_string(),
                value_amount: 9_996_828,
            }],
        },
    ];
    reconcile_address_transactions(
        user_id,
        SyncedAssetId::Bitcoin,
        Network::Mainnet,
        &records,
        now,
    )
    .expect("transactions should reconcile");
    super::ledger_rebuild::rebuild_account_transaction_ledger(user_id, add_result.account_id, now)
        .expect("ledger rebuild should succeed");

    let self_transfer_row = crate::db::user_db::with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT tx_type, value_amount_lo, fee_amount_lo, closing_balance_lo
                 FROM account_transaction_ledger
                 WHERE account_id = ?1
                   AND tx_hash = ?2",
            params![
                add_result.account_id.to_string(),
                self_transfer_hash.as_str()
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .map_err(|err| {
            crate::db::error::DbError::new(format!(
                "Failed to load self-transfer ledger row: {err}"
            ))
        })
    })
    .expect("self-transfer ledger row should load");

    assert_eq!(
        self_transfer_row,
        ("self_transfer".to_string(), 9_996_828, Some(3_172), None,)
    );
}

#[test]
fn resolve_native_balance_at_boundary_rejects_missing_bitcoin_coverage_at_first_transaction() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let (account_id, first_transaction_date, last_successful_sync_date) =
        seed_btc_partial_backfill_fixture(user_id);

    let resolution = crate::db::user_db::with_user_db(user_id, |conn| {
        let meta = balance::load_account_meta(conn, account_id)?;
        balance::resolve_native_balance_at_boundary(
            conn,
            account_id,
            &meta,
            balance::NativeBalanceBoundaryRequest {
                boundary_kind: NativeBalanceBoundaryKind::Opening,
                requested_boundary_date: Some(first_transaction_date),
                first_transaction_date: Some(first_transaction_date),
                transaction_history_pending: false,
            },
            &AccountBalanceReliabilityContext {
                last_successful_sync_date: Some(last_successful_sync_date),
                balance_reliability: BalanceReliability::finalized(),
                bitcoin_history_coverage: None,
            },
        )
    })
    .expect("native opening balance should resolve");

    assert_eq!(resolution.state, NativeBalanceState::Unknown);
    assert_eq!(resolution.amount, None);
    assert_eq!(resolution.balance_date, Some(first_transaction_date));
}

#[test]
fn resolve_native_balance_at_boundary_rejects_missing_bitcoin_coverage_inside_history() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let (account_id, first_transaction_date, last_successful_sync_date) =
        seed_btc_partial_backfill_fixture(user_id);
    let from_boundary = dt("2026-01-11T00:00:00Z");

    let resolution = crate::db::user_db::with_user_db(user_id, |conn| {
        let meta = balance::load_account_meta(conn, account_id)?;
        balance::resolve_native_balance_at_boundary(
            conn,
            account_id,
            &meta,
            balance::NativeBalanceBoundaryRequest {
                boundary_kind: NativeBalanceBoundaryKind::Opening,
                requested_boundary_date: Some(from_boundary),
                first_transaction_date: Some(first_transaction_date),
                transaction_history_pending: false,
            },
            &AccountBalanceReliabilityContext {
                last_successful_sync_date: Some(last_successful_sync_date),
                balance_reliability: BalanceReliability::finalized(),
                bitcoin_history_coverage: None,
            },
        )
    })
    .expect("native opening balance should resolve");

    assert_eq!(resolution.state, NativeBalanceState::Unknown);
    assert_eq!(resolution.amount, None);
    assert_eq!(resolution.balance_date, Some(from_boundary));
}

#[test]
fn resolve_native_balance_at_boundary_rejects_missing_bitcoin_coverage_before_history() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let (account_id, first_transaction_date, last_successful_sync_date) =
        seed_btc_partial_backfill_fixture(user_id);
    let from_boundary = dt("2026-01-05T00:00:00Z");

    let resolution = crate::db::user_db::with_user_db(user_id, |conn| {
        let meta = balance::load_account_meta(conn, account_id)?;
        balance::resolve_native_balance_at_boundary(
            conn,
            account_id,
            &meta,
            balance::NativeBalanceBoundaryRequest {
                boundary_kind: NativeBalanceBoundaryKind::Opening,
                requested_boundary_date: Some(from_boundary),
                first_transaction_date: Some(first_transaction_date),
                transaction_history_pending: false,
            },
            &AccountBalanceReliabilityContext {
                last_successful_sync_date: Some(last_successful_sync_date),
                balance_reliability: BalanceReliability::finalized(),
                bitcoin_history_coverage: None,
            },
        )
    })
    .expect("native opening balance should resolve");

    assert_eq!(resolution.state, NativeBalanceState::Unknown);
    assert_eq!(resolution.amount, None);
    assert_eq!(resolution.balance_date, Some(from_boundary));
}

#[test]
fn resolve_native_balance_at_boundary_returns_unknown_for_early_to() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let (account_id, first_transaction_date, last_successful_sync_date) =
        seed_btc_partial_backfill_fixture(user_id);
    let to_boundary = dt("2026-01-05T23:59:59Z");

    let resolution = crate::db::user_db::with_user_db(user_id, |conn| {
        let meta = balance::load_account_meta(conn, account_id)?;
        balance::resolve_native_balance_at_boundary(
            conn,
            account_id,
            &meta,
            balance::NativeBalanceBoundaryRequest {
                boundary_kind: NativeBalanceBoundaryKind::Closing,
                requested_boundary_date: Some(to_boundary),
                first_transaction_date: Some(first_transaction_date),
                transaction_history_pending: false,
            },
            &AccountBalanceReliabilityContext {
                last_successful_sync_date: Some(last_successful_sync_date),
                balance_reliability: BalanceReliability::finalized(),
                bitcoin_history_coverage: None,
            },
        )
    })
    .expect("native closing balance should resolve");

    assert_eq!(resolution.state, NativeBalanceState::Unknown);
    assert_eq!(resolution.amount, None);
    assert_eq!(resolution.balance_date, Some(to_boundary));
}

#[test]
fn resolve_native_balance_at_boundary_preserves_account_model_zero_behavior() {
    let _runtime = acquire_test_runtime().expect("test runtime should initialize");
    let user_id = UserId::new();
    initialize_user_db_for_test(user_id).expect("user db should initialize");

    let created_at = dt("2026-02-15T12:00:00Z");
    let eth_address = parse_eth_address("0x52908400098527886E0F7030069857D2E4169EE7");
    let account =
        create_eth_wallet_account_fixture(user_id, &eth_address, "ETH Boundaries", created_at);
    seed_eth_ledger_row(
        user_id,
        account.account_id,
        &eth_address.checksummed(),
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "2026-02-15T12:00:00Z",
        UnsignedAmount::from_u128(42),
        UnsignedAmount::from_u128(42),
    );

    let opening_boundary = dt("2026-02-01T00:00:00Z");
    let resolution = crate::db::user_db::with_user_db(user_id, |conn| {
        let meta = balance::load_account_meta(conn, account.account_id)?;
        balance::resolve_native_balance_at_boundary(
            conn,
            account.account_id,
            &meta,
            balance::NativeBalanceBoundaryRequest {
                boundary_kind: NativeBalanceBoundaryKind::Opening,
                requested_boundary_date: Some(opening_boundary),
                first_transaction_date: Some(created_at),
                transaction_history_pending: false,
            },
            &AccountBalanceReliabilityContext {
                last_successful_sync_date: None,
                balance_reliability: BalanceReliability::finalized(),
                bitcoin_history_coverage: None,
            },
        )
    })
    .expect("native account-model opening balance should resolve");

    assert_eq!(resolution.state, NativeBalanceState::CanonicalZero);
    assert_eq!(resolution.amount, None);
    assert_eq!(resolution.balance_date, Some(opening_boundary));
}
