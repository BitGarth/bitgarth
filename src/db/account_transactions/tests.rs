use super::balance;
use super::ledger_rebuild;
use super::page_query;
use super::types::*;
use super::wallet_report;
use crate::amounts::UnsignedAmount;
use crate::balance_reliability::{BalanceProvisionalReason, BalanceReliability};
#[cfg(feature = "server")]
use crate::db::balance_reliability::AccountBalanceReliabilityContext;
#[cfg(feature = "server")]
use crate::db::{
    AddressSyncSuccess, SyncTransactionOutputRecord, SyncTransactionRecord, acquire_test_runtime,
    add_bitcoin_address, create_eth_wallet_account_fixture, initialize_user_db_for_test,
    mark_account_integration_sync_started, mark_address_sync_completed_success,
    mark_address_sync_started, reconcile_address_transactions,
    refresh_account_integration_sync_state, update_address_mempool_backfill_cursor,
};
#[cfg(feature = "server")]
use crate::ethereum::{EthAddress, RawEthAddress};
use crate::models::{UserId, UserTimezone, parse_datetime};
use crate::report_dates::LocalReportDateRange;
use crate::transactions::{
    AccountTransactionDirection, ApiConfirmedBalance, ChainTransactionStatus,
};
#[cfg(feature = "server")]
use crate::transactions::{
    ChainTipHeight, MempoolCursorTxid, SyncIntegrationId, TrackedAddress, TransactionCount,
    TransactionSyncRunId, TxHash,
};
use crate::wallets::{SyncedAssetId, DigitalAssetAccountId, Network, TransactionFilters};
#[cfg(feature = "server")]
use crate::wallets::{
    BtcAddress, Label, RawBtcAddress, TransactionSortDirection, WALLET_LABEL_MAX_LENGTH,
};
use chrono::{DateTime, NaiveDate, Utc};
#[cfg(feature = "server")]
use rusqlite::params;
use ulid::Ulid;

fn dt(s: &str) -> DateTime<Utc> {
    parse_datetime(s).expect("valid test datetime")
}

#[allow(clippy::too_many_arguments)]
fn test_entry(
    tx_hash: &str,
    status: ChainTransactionStatus,
    occurred_at: &str,
    first_seen_at: &str,
    block_height: Option<i64>,
    nonce: Option<i64>,
    min_transfer_index: Option<i64>,
    delta: i128,
) -> ledger_rebuild::LedgerBuildEntry {
    ledger_rebuild::LedgerBuildEntry {
        chain_transaction_id: Ulid::new().to_string(),
        tx_hash: tx_hash.to_string(),
        status,
        occurred_at: parse_datetime(occurred_at).expect("valid datetime"),
        first_seen_at: parse_datetime(first_seen_at).expect("valid datetime"),
        block_height,
        nonce,
        min_transfer_index,
        direction: AccountTransactionDirection::Incoming,
        value: UnsignedAmount::from_u128(1),
        fee: None,
        balance_delta: delta,
        closing_balance: None,
        same_block_parent_hashes: Vec::new(),
        from_addresses: Vec::new(),
        to_addresses: Vec::new(),
    }
}

fn api_balance_row(
    address_id: crate::wallets::DigitalAssetAddressId,
    api_confirmed_balance: Option<ApiConfirmedBalance>,
) -> crate::db::transaction_sync::AddressApiConfirmedBalanceRow {
    crate::db::transaction_sync::AddressApiConfirmedBalanceRow {
        address_id,
        last_completed_at: None,
        api_confirmed_balance,
    }
}

#[cfg(feature = "server")]
fn parse_eth_address(value: &str) -> EthAddress {
    let raw = RawEthAddress::new(value.to_string());
    EthAddress::parse(&raw).expect("test eth address should parse")
}

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
fn seed_btc_partial_backfill_fixture(
    user_id: UserId,
) -> (DigitalAssetAccountId, DateTime<Utc>, DateTime<Utc>) {
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
                UnsignedAmount::from_u128(150_000),
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

    (add_result.account_id, transaction_time, sync_completed)
}

#[test]
fn assign_closing_balances_orders_confirmed_oldest_first() {
    let mut entries = vec![
        test_entry(
            "b",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:01:00Z",
            "2026-02-12T10:01:00Z",
            None,
            Some(2),
            Some(1),
            5,
        ),
        test_entry(
            "a",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            None,
            Some(1),
            Some(0),
            7,
        ),
    ];

    ledger_rebuild::assign_closing_balances(&mut entries, None).expect("assignment should succeed");
    let first = entries
        .iter()
        .find(|entry| entry.tx_hash == "a")
        .expect("first tx should exist");
    let second = entries
        .iter()
        .find(|entry| entry.tx_hash == "b")
        .expect("second tx should exist");
    assert_eq!(first.closing_balance.expect("balance").value(), 7_u128);
    assert_eq!(second.closing_balance.expect("balance").value(), 12_u128);
}

#[test]
fn assign_closing_balances_uses_confirmed_tiebreakers_for_stable_order() {
    let mut entries = vec![
        test_entry(
            "hash-d",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            Some(101),
            Some(0),
            Some(0),
            5,
        ),
        test_entry(
            "hash-c",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            Some(100),
            Some(2),
            Some(0),
            4,
        ),
        test_entry(
            "hash-b",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            Some(100),
            Some(1),
            Some(1),
            3,
        ),
        test_entry(
            "hash-aa",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            Some(100),
            Some(1),
            Some(0),
            1,
        ),
        test_entry(
            "hash-a",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            Some(100),
            Some(1),
            Some(0),
            2,
        ),
    ];

    ledger_rebuild::assign_closing_balances(&mut entries, None).expect("assignment should succeed");

    let by_hash = |tx_hash: &str| {
        entries
            .iter()
            .find(|entry| entry.tx_hash == tx_hash)
            .and_then(|entry| entry.closing_balance)
            .expect("entry balance should exist")
            .value()
    };

    assert_eq!(by_hash("hash-a"), 2_u128);
    assert_eq!(by_hash("hash-aa"), 3_u128);
    assert_eq!(by_hash("hash-b"), 6_u128);
    assert_eq!(by_hash("hash-c"), 10_u128);
    assert_eq!(by_hash("hash-d"), 15_u128);
}

#[test]
fn assign_closing_balances_orders_pending_using_first_seen_then_nonce_then_hash() {
    let mut entries = vec![
        test_entry(
            "confirmed",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            Some(100),
            Some(0),
            Some(0),
            10,
        ),
        test_entry(
            "pending-c",
            ChainTransactionStatus::Pending,
            "2026-02-12T10:02:00Z",
            "2026-02-12T10:02:00Z",
            None,
            Some(2),
            None,
            -3,
        ),
        test_entry(
            "pending-a",
            ChainTransactionStatus::Pending,
            "2026-02-12T10:01:00Z",
            "2026-02-12T10:01:00Z",
            None,
            Some(9),
            None,
            4,
        ),
        test_entry(
            "pending-b",
            ChainTransactionStatus::Pending,
            "2026-02-12T10:02:00Z",
            "2026-02-12T10:02:00Z",
            None,
            Some(1),
            None,
            1,
        ),
        test_entry(
            "pending-aa",
            ChainTransactionStatus::Pending,
            "2026-02-12T10:02:00Z",
            "2026-02-12T10:02:00Z",
            None,
            Some(1),
            None,
            2,
        ),
        test_entry(
            "dropped",
            ChainTransactionStatus::Dropped,
            "2026-02-12T10:03:00Z",
            "2026-02-12T10:03:00Z",
            None,
            None,
            None,
            99,
        ),
    ];

    ledger_rebuild::assign_closing_balances(&mut entries, None).expect("assignment should succeed");

    let by_hash = |tx_hash: &str| {
        entries
            .iter()
            .find(|entry| entry.tx_hash == tx_hash)
            .expect("entry should exist")
    };

    assert_eq!(
        by_hash("pending-a")
            .closing_balance
            .expect("pending-a balance")
            .value(),
        14_u128
    );
    assert_eq!(
        by_hash("pending-aa")
            .closing_balance
            .expect("pending-aa balance")
            .value(),
        16_u128
    );
    assert_eq!(
        by_hash("pending-b")
            .closing_balance
            .expect("pending-b balance")
            .value(),
        17_u128
    );
    assert_eq!(
        by_hash("pending-c")
            .closing_balance
            .expect("pending-c balance")
            .value(),
        14_u128
    );
    assert!(by_hash("dropped").closing_balance.is_none());
}

#[test]
fn assign_bitcoin_closing_balances_rejects_negative_intermediate_balance() {
    let mut entries = vec![
        test_entry(
            "outgoing-1",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            Some(100),
            Some(0),
            Some(0),
            -5,
        ),
        test_entry(
            "incoming-small",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:01:00Z",
            "2026-02-12T10:01:00Z",
            Some(101),
            Some(1),
            Some(0),
            2,
        ),
        test_entry(
            "incoming-large",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:02:00Z",
            "2026-02-12T10:02:00Z",
            Some(102),
            Some(2),
            Some(0),
            10,
        ),
    ];

    let error = ledger_rebuild::assign_bitcoin_closing_balances(
        &mut entries,
        NativeBalanceState::CanonicalZero,
    )
    .expect_err("negative intermediate balance must fail");

    assert!(error.to_string().contains("negative"));
    assert!(entries.iter().all(|entry| entry.closing_balance.is_none()));
}

#[test]
fn assign_closing_balances_sets_dropped_and_failed_to_none() {
    let mut entries = vec![
        test_entry(
            "confirmed",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            None,
            None,
            None,
            10,
        ),
        test_entry(
            "pending",
            ChainTransactionStatus::Pending,
            "2026-02-12T10:01:00Z",
            "2026-02-12T10:01:00Z",
            None,
            None,
            None,
            5,
        ),
        test_entry(
            "dropped",
            ChainTransactionStatus::Dropped,
            "2026-02-12T10:02:00Z",
            "2026-02-12T10:02:00Z",
            None,
            None,
            None,
            -2,
        ),
        test_entry(
            "failed",
            ChainTransactionStatus::Failed,
            "2026-02-12T10:03:00Z",
            "2026-02-12T10:03:00Z",
            None,
            None,
            None,
            -2,
        ),
    ];

    ledger_rebuild::assign_closing_balances(&mut entries, None).expect("assignment should succeed");

    let pending = entries
        .iter()
        .find(|entry| entry.tx_hash == "pending")
        .expect("pending entry");
    assert_eq!(
        pending.closing_balance.expect("pending balance").value(),
        15_u128
    );
    let dropped = entries
        .iter()
        .find(|entry| entry.tx_hash == "dropped")
        .expect("dropped entry");
    let failed = entries
        .iter()
        .find(|entry| entry.tx_hash == "failed")
        .expect("failed entry");
    assert!(dropped.closing_balance.is_none());
    assert!(failed.closing_balance.is_none());
}

#[test]
fn assign_closing_balances_with_opening_balance_shifts_confirmed_running_totals() {
    let opening = super::types::OpeningBalance(UnsignedAmount::from_u128(10));
    let mut entries = vec![
        test_entry(
            "tx-a",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            Some(100),
            Some(0),
            Some(0),
            5,
        ),
        test_entry(
            "tx-b",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:01:00Z",
            "2026-02-12T10:01:00Z",
            Some(101),
            Some(1),
            Some(0),
            -2,
        ),
    ];

    ledger_rebuild::assign_closing_balances(&mut entries, Some(opening))
        .expect("assignment should succeed");

    let by_hash = |tx_hash: &str| {
        entries
            .iter()
            .find(|entry| entry.tx_hash == tx_hash)
            .and_then(|entry| entry.closing_balance)
            .expect("entry balance should exist")
            .value()
    };

    assert_eq!(by_hash("tx-a"), 15_u128);
    assert_eq!(by_hash("tx-b"), 13_u128);
}

#[test]
fn assign_closing_balances_zero_opening_balance_produces_same_as_none() {
    let mut entries_none = vec![test_entry(
        "tx",
        ChainTransactionStatus::Confirmed,
        "2026-02-12T10:00:00Z",
        "2026-02-12T10:00:00Z",
        Some(100),
        Some(0),
        Some(0),
        7,
    )];
    let mut entries_zero = entries_none.clone();

    ledger_rebuild::assign_closing_balances(&mut entries_none, None)
        .expect("assignment should succeed");
    ledger_rebuild::assign_closing_balances(
        &mut entries_zero,
        Some(super::types::OpeningBalance(UnsignedAmount::zero())),
    )
    .expect("assignment should succeed");

    assert_eq!(
        entries_none[0].closing_balance,
        entries_zero[0].closing_balance
    );
}

#[test]
fn assign_closing_balances_opening_balance_with_no_confirmed_entries() {
    let opening = super::types::OpeningBalance(UnsignedAmount::from_u128(50));
    let mut entries = vec![test_entry(
        "pending",
        ChainTransactionStatus::Pending,
        "2026-02-12T10:00:00Z",
        "2026-02-12T10:00:00Z",
        None,
        None,
        None,
        5,
    )];

    ledger_rebuild::assign_closing_balances(&mut entries, Some(opening))
        .expect("assignment should succeed");

    assert_eq!(
        entries[0].closing_balance.expect("pending balance").value(),
        55_u128
    );
}

#[test]
fn assign_closing_balances_opening_balance_carries_into_pending_running_total() {
    let opening = super::types::OpeningBalance(UnsignedAmount::from_u128(100));
    let mut entries = vec![
        test_entry(
            "confirmed",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            Some(100),
            Some(0),
            Some(0),
            -30,
        ),
        test_entry(
            "pending",
            ChainTransactionStatus::Pending,
            "2026-02-12T10:01:00Z",
            "2026-02-12T10:01:00Z",
            None,
            None,
            None,
            10,
        ),
    ];

    ledger_rebuild::assign_closing_balances(&mut entries, Some(opening))
        .expect("assignment should succeed");

    let confirmed = entries
        .iter()
        .find(|e| e.tx_hash == "confirmed")
        .and_then(|e| e.closing_balance)
        .expect("confirmed balance")
        .value();
    let pending = entries
        .iter()
        .find(|e| e.tx_hash == "pending")
        .and_then(|e| e.closing_balance)
        .expect("pending balance")
        .value();

    assert_eq!(confirmed, 70_u128); // 100 (opening) + (-30)
    assert_eq!(pending, 80_u128); // 70 (confirmed total) + 10
}

#[test]
fn compute_incomplete_bitcoin_basis_returns_expected_amount_when_complete() {
    let address_a = crate::wallets::DigitalAssetAddressId::new();
    let address_b = crate::wallets::DigitalAssetAddressId::new();
    let api_balances = vec![
        api_balance_row(
            address_a,
            Some(ApiConfirmedBalance::from_amount(UnsignedAmount::from_u128(
                120,
            ))),
        ),
        api_balance_row(
            address_b,
            Some(ApiConfirmedBalance::from_amount(UnsignedAmount::from_u128(
                30,
            ))),
        ),
    ];
    let entries = vec![
        test_entry(
            "confirmed-a",
            ChainTransactionStatus::Confirmed,
            "2026-01-10T12:00:00Z",
            "2026-01-10T12:00:00Z",
            Some(100),
            None,
            None,
            20,
        ),
        test_entry(
            "confirmed-b",
            ChainTransactionStatus::Confirmed,
            "2026-01-11T12:00:00Z",
            "2026-01-11T12:00:00Z",
            Some(101),
            None,
            None,
            10,
        ),
    ];

    let basis =
        ledger_rebuild::compute_incomplete_bitcoin_basis_from_inputs(&api_balances, &entries)
            .expect("helper should succeed");

    assert_eq!(
        basis,
        NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(120))
    );
}

#[test]
fn compute_incomplete_bitcoin_basis_returns_unknown_when_balance_missing() {
    let api_balances = vec![
        api_balance_row(
            crate::wallets::DigitalAssetAddressId::new(),
            Some(ApiConfirmedBalance::from_amount(UnsignedAmount::from_u128(
                120,
            ))),
        ),
        api_balance_row(crate::wallets::DigitalAssetAddressId::new(), None),
    ];

    let basis = ledger_rebuild::compute_incomplete_bitcoin_basis_from_inputs(&api_balances, &[])
        .expect("helper should succeed");

    assert_eq!(basis, NativeBalanceState::Unknown);
}

#[test]
fn compute_incomplete_bitcoin_basis_returns_unknown_when_negative() {
    let api_balances = vec![api_balance_row(
        crate::wallets::DigitalAssetAddressId::new(),
        Some(ApiConfirmedBalance::from_amount(UnsignedAmount::from_u128(
            20,
        ))),
    )];
    let entries = vec![test_entry(
        "confirmed-a",
        ChainTransactionStatus::Confirmed,
        "2026-01-10T12:00:00Z",
        "2026-01-10T12:00:00Z",
        Some(100),
        None,
        None,
        30,
    )];

    let basis =
        ledger_rebuild::compute_incomplete_bitcoin_basis_from_inputs(&api_balances, &entries)
            .expect("helper should succeed");

    assert_eq!(basis, NativeBalanceState::Unknown);
}

#[cfg(feature = "server")]
fn parse_btc_address(value: &str) -> BtcAddress {
    let raw = RawBtcAddress::new(value.to_string());
    BtcAddress::parse(&raw, Network::Mainnet).expect("test btc address should parse")
}

#[cfg(feature = "server")]
fn parse_wallet_label(value: &str) -> Label {
    Label::parse_with_limit(value, WALLET_LABEL_MAX_LENGTH).expect("test label should parse")
}

#[cfg(feature = "server")]
#[test]
fn load_account_transactions_pages_uses_address_success_date_when_completed_account_retains_cursor() {
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
        (Some(address_completed.to_rfc3339()), Some("success".to_string()))
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
        crate::transactions::TransactionCount::from_u32(u32::MAX),
    )
    .expect("account transactions pages should load");

    assert_eq!(pages.opening_balance_date, Some(transaction_time));
    assert_eq!(
        pages.closing_balance_state,
        NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(50_000))
    );
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

#[cfg(feature = "server")]
#[test]
fn resolve_native_balance_at_boundary_returns_known_amount_for_partial_backfill_opening() {
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
            NativeBalanceBoundaryKind::Opening,
            Some(first_transaction_date),
            Some(first_transaction_date),
            &AccountBalanceReliabilityContext {
                last_successful_sync_date: Some(last_successful_sync_date),
                balance_reliability: BalanceReliability::finalized(),
                bitcoin_history_coverage: None,
            },
        )
    })
    .expect("native opening balance should resolve");

    assert_eq!(
        resolution.state,
        NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(100_000))
    );
    assert_eq!(resolution.amount, Some(UnsignedAmount::from_u128(100_000)));
    assert_eq!(resolution.balance_date, Some(first_transaction_date));
}

#[cfg(feature = "server")]
#[test]
fn resolve_native_balance_at_boundary_returns_known_amount_for_from_inside_history() {
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
            NativeBalanceBoundaryKind::Opening,
            Some(from_boundary),
            Some(first_transaction_date),
            &AccountBalanceReliabilityContext {
                last_successful_sync_date: Some(last_successful_sync_date),
                balance_reliability: BalanceReliability::finalized(),
                bitcoin_history_coverage: None,
            },
        )
    })
    .expect("native opening balance should resolve");

    assert_eq!(
        resolution.state,
        NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(150_000))
    );
    assert_eq!(resolution.amount, Some(UnsignedAmount::from_u128(150_000)));
    assert_eq!(resolution.balance_date, Some(from_boundary));
}

#[cfg(feature = "server")]
#[test]
fn resolve_native_balance_at_boundary_returns_synthetic_opening_for_early_from() {
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
            NativeBalanceBoundaryKind::Opening,
            Some(from_boundary),
            Some(first_transaction_date),
            &AccountBalanceReliabilityContext {
                last_successful_sync_date: Some(last_successful_sync_date),
                balance_reliability: BalanceReliability::finalized(),
                bitcoin_history_coverage: None,
            },
        )
    })
    .expect("native opening balance should resolve");

    assert_eq!(
        resolution.state,
        NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(100_000))
    );
    assert_eq!(resolution.amount, Some(UnsignedAmount::from_u128(100_000)));
    assert_eq!(resolution.balance_date, Some(from_boundary));
}

#[cfg(feature = "server")]
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
            NativeBalanceBoundaryKind::Closing,
            Some(to_boundary),
            Some(first_transaction_date),
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

#[cfg(feature = "server")]
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
            NativeBalanceBoundaryKind::Opening,
            Some(opening_boundary),
            Some(created_at),
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

#[test]
fn resolve_balance_dates_no_filters_with_transactions_and_sync() {
    let result = balance::resolve_balance_dates(balance::BalanceResolutionInputs {
        from_date: None,
        to_date: None,
        first_transaction_date: Some(dt("2026-01-15T10:00:00Z")),
        last_successful_sync_date: Some(dt("2026-03-30T08:00:00Z")),
    });
    assert_eq!(
        result.opening_balance_date,
        Some(dt("2026-01-15T10:00:00Z"))
    );
    assert_eq!(
        result.closing_balance_date,
        Some(dt("2026-03-30T08:00:00Z"))
    );
}

#[test]
fn resolve_balance_dates_no_filters_no_transactions_with_sync() {
    let result = balance::resolve_balance_dates(balance::BalanceResolutionInputs {
        from_date: None,
        to_date: None,
        first_transaction_date: None,
        last_successful_sync_date: Some(dt("2026-03-30T08:00:00Z")),
    });
    assert_eq!(
        result.opening_balance_date,
        Some(dt("2026-03-30T08:00:00Z"))
    );
    assert_eq!(
        result.closing_balance_date,
        Some(dt("2026-03-30T08:00:00Z"))
    );
}

#[test]
fn resolve_balance_dates_no_filters_no_transactions_no_sync() {
    let result = balance::resolve_balance_dates(balance::BalanceResolutionInputs {
        from_date: None,
        to_date: None,
        first_transaction_date: None,
        last_successful_sync_date: None,
    });
    assert_eq!(result.opening_balance_date, None);
    assert_eq!(result.closing_balance_date, None);
}

#[test]
fn resolve_balance_dates_from_date_set_no_to() {
    let result = balance::resolve_balance_dates(balance::BalanceResolutionInputs {
        from_date: Some(dt("2026-02-01T00:00:00Z")),
        to_date: None,
        first_transaction_date: Some(dt("2026-01-15T10:00:00Z")),
        last_successful_sync_date: Some(dt("2026-03-30T08:00:00Z")),
    });
    assert_eq!(
        result.opening_balance_date,
        Some(dt("2026-02-01T00:00:00Z"))
    );
    assert_eq!(
        result.closing_balance_date,
        Some(dt("2026-03-30T08:00:00Z"))
    );
}

#[test]
fn resolve_balance_dates_to_date_set_no_from() {
    let result = balance::resolve_balance_dates(balance::BalanceResolutionInputs {
        from_date: None,
        to_date: Some(dt("2026-02-28T23:59:59Z")),
        first_transaction_date: Some(dt("2026-01-15T10:00:00Z")),
        last_successful_sync_date: Some(dt("2026-03-30T08:00:00Z")),
    });
    assert_eq!(
        result.opening_balance_date,
        Some(dt("2026-01-15T10:00:00Z"))
    );
    assert_eq!(
        result.closing_balance_date,
        Some(dt("2026-02-28T23:59:59Z"))
    );
}

#[test]
fn resolve_balance_dates_both_dates_set() {
    let result = balance::resolve_balance_dates(balance::BalanceResolutionInputs {
        from_date: Some(dt("2026-02-01T00:00:00Z")),
        to_date: Some(dt("2026-02-28T23:59:59Z")),
        first_transaction_date: Some(dt("2026-01-15T10:00:00Z")),
        last_successful_sync_date: Some(dt("2026-03-30T08:00:00Z")),
    });
    assert_eq!(
        result.opening_balance_date,
        Some(dt("2026-02-01T00:00:00Z"))
    );
    assert_eq!(
        result.closing_balance_date,
        Some(dt("2026-02-28T23:59:59Z"))
    );
}

#[test]
fn resolve_balance_dates_from_date_set_no_sync() {
    let result = balance::resolve_balance_dates(balance::BalanceResolutionInputs {
        from_date: Some(dt("2026-02-01T00:00:00Z")),
        to_date: None,
        first_transaction_date: None,
        last_successful_sync_date: None,
    });
    assert_eq!(
        result.opening_balance_date,
        Some(dt("2026-02-01T00:00:00Z"))
    );
    assert_eq!(result.closing_balance_date, None);
}

#[test]
fn resolve_balance_dates_from_equals_to() {
    let result = balance::resolve_balance_dates(balance::BalanceResolutionInputs {
        from_date: Some(dt("2026-02-15T00:00:00Z")),
        to_date: Some(dt("2026-02-15T23:59:59Z")),
        first_transaction_date: Some(dt("2026-01-01T10:00:00Z")),
        last_successful_sync_date: Some(dt("2026-03-30T08:00:00Z")),
    });
    assert_eq!(
        result.opening_balance_date,
        Some(dt("2026-02-15T00:00:00Z"))
    );
    assert_eq!(
        result.closing_balance_date,
        Some(dt("2026-02-15T23:59:59Z"))
    );
}

#[test]
fn resolve_balance_dates_empty_account_with_filters_and_sync_preserves_explicit_dates() {
    let result = balance::resolve_balance_dates(balance::BalanceResolutionInputs {
        from_date: Some(dt("2026-02-01T00:00:00Z")),
        to_date: Some(dt("2026-02-28T23:59:59Z")),
        first_transaction_date: None,
        last_successful_sync_date: Some(dt("2026-03-30T08:00:00Z")),
    });
    assert_eq!(
        result.opening_balance_date,
        Some(dt("2026-02-01T00:00:00Z"))
    );
    assert_eq!(
        result.closing_balance_date,
        Some(dt("2026-02-28T23:59:59Z"))
    );
}

#[test]
fn wallet_report_balance_state_from_native_maps_known_zero_to_canonical_zero() {
    let result =
        wallet_report::wallet_report_balance_state_from_native(NativeBalanceState::CanonicalZero);

    assert_eq!(
        result,
        wallet_report::WalletReportBalanceState::CanonicalZero
    );
}

#[test]
fn wallet_report_balance_state_from_native_maps_known_amount() {
    let amount = UnsignedAmount::from_u128(42);
    let result = wallet_report::wallet_report_balance_state_from_native(
        NativeBalanceState::KnownAmount(amount),
    );

    assert_eq!(
        result,
        wallet_report::WalletReportBalanceState::KnownAmount(amount)
    );
}

#[test]
fn wallet_report_balance_state_from_native_maps_unknown() {
    let result =
        wallet_report::wallet_report_balance_state_from_native(NativeBalanceState::Unknown);

    assert_eq!(result, wallet_report::WalletReportBalanceState::Unknown);
}

#[test]
fn wallet_consistent_sync_date_uses_earliest_local_date_when_all_accounts_synced() {
    let timezone = UserTimezone("Europe/Amsterdam".parse().expect("valid timezone"));
    let sync_dates = vec![
        Some(dt("2026-03-30T22:30:00Z")),
        Some(dt("2026-03-29T12:00:00Z")),
        Some(dt("2026-03-31T08:15:00Z")),
    ];

    let result = wallet_report::wallet_consistent_sync_date(&sync_dates, timezone);

    assert_eq!(
        result,
        Some(NaiveDate::from_ymd_opt(2026, 3, 29).expect("valid date"))
    );
}

#[test]
fn wallet_consistent_sync_date_returns_none_when_any_account_never_synced() {
    let timezone = UserTimezone("Europe/Amsterdam".parse().expect("valid timezone"));
    let sync_dates = vec![Some(dt("2026-03-30T22:30:00Z")), None];

    let result = wallet_report::wallet_consistent_sync_date(&sync_dates, timezone);

    assert_eq!(result, None);
}

#[test]
fn resolve_wallet_report_range_uses_defaults_when_query_params_absent() {
    let defaults = LocalReportDateRange::new(
        NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"),
        NaiveDate::from_ymd_opt(2026, 3, 29).expect("valid date"),
    )
    .expect("default range should validate");

    let result = wallet_report::resolve_wallet_report_range(defaults, None, None)
        .expect("resolved range should validate");

    assert_eq!(result, defaults);
}

#[test]
fn resolve_wallet_report_range_rejects_inverted_query_dates() {
    let defaults = LocalReportDateRange::new(
        NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"),
        NaiveDate::from_ymd_opt(2026, 3, 29).expect("valid date"),
    )
    .expect("default range should validate");

    let result = wallet_report::resolve_wallet_report_range(
        defaults,
        Some(NaiveDate::from_ymd_opt(2026, 4, 1).expect("valid date")),
        Some(NaiveDate::from_ymd_opt(2026, 3, 1).expect("valid date")),
    );

    assert!(matches!(
        result,
        Err(wallet_report::WalletReportLoadError::InvalidDateRange(
            crate::report_dates::LocalReportDateRangeError::InvertedRange
        ))
    ));
}

#[test]
fn resolve_wallet_report_default_range_clamps_stale_sync_date_to_year_start() {
    let today = NaiveDate::from_ymd_opt(2026, 3, 30).expect("valid date");

    let result = wallet_report::resolve_wallet_report_default_range(
        today,
        Some(NaiveDate::from_ymd_opt(2025, 12, 31).expect("valid date")),
    )
    .expect("default range should validate");

    assert_eq!(
        result,
        LocalReportDateRange::new(
            NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"),
            NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"),
        )
        .expect("expected range should validate")
    );
}
