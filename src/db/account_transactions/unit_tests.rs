use super::balance;
use super::ledger_rebuild;
use super::types::*;
use super::wallet_report;
use crate::amounts::UnsignedAmount;
use crate::models::{UserTimezone, parse_datetime};
use crate::report_dates::LocalReportDateRange;
use crate::transactions::{
    AccountTransactionDirection, ApiConfirmedBalance, ChainTransactionStatus,
};
use crate::wallets::DigitalAssetAddressId;
use chrono::{DateTime, NaiveDate, Utc};
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
    address_id: DigitalAssetAddressId,
    api_confirmed_balance: Option<ApiConfirmedBalance>,
) -> crate::db::transaction_sync::AddressApiConfirmedBalanceRow {
    crate::db::transaction_sync::AddressApiConfirmedBalanceRow {
        address_id,
        last_completed_at: None,
        api_confirmed_balance,
    }
}

#[test]
fn classify_utxo_owned_change_with_external_outflow_as_outgoing() {
    let classified = ledger_rebuild::classify_utxo_ledger_flow(
        UnsignedAmount::from_u128(8_731_871_339_142),
        UnsignedAmount::from_u128(8_701_271_334_606),
        Some(UnsignedAmount::from_u128(4_536)),
    );

    assert_eq!(classified.direction, AccountTransactionDirection::Outgoing);
    assert_eq!(classified.value, UnsignedAmount::from_u128(30_600_000_000));
}

#[test]
fn classify_utxo_owned_input_without_owned_output_as_outgoing() {
    let classified = ledger_rebuild::classify_utxo_ledger_flow(
        UnsignedAmount::from_u128(100_000),
        UnsignedAmount::zero(),
        Some(UnsignedAmount::from_u128(1_000)),
    );

    assert_eq!(classified.direction, AccountTransactionDirection::Outgoing);
    assert_eq!(classified.value, UnsignedAmount::from_u128(99_000));
}

#[test]
fn classify_utxo_exact_owned_input_to_output_plus_fee_as_self_transfer() {
    let classified = ledger_rebuild::classify_utxo_ledger_flow(
        UnsignedAmount::from_u128(100_000),
        UnsignedAmount::from_u128(99_000),
        Some(UnsignedAmount::from_u128(1_000)),
    );

    assert_eq!(
        classified.direction,
        AccountTransactionDirection::SelfTransfer
    );
    assert_eq!(classified.value, UnsignedAmount::from_u128(99_000));
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
fn assign_bitcoin_closing_balances_orders_same_block_parent_before_child() {
    let mut child = test_entry(
        "a-child",
        ChainTransactionStatus::Confirmed,
        "2026-02-12T10:00:00Z",
        "2026-02-12T10:00:00Z",
        Some(100),
        None,
        None,
        -4,
    );
    child.same_block_parent_hashes = vec!["z-parent".to_string()];
    let parent = test_entry(
        "z-parent",
        ChainTransactionStatus::Confirmed,
        "2026-02-12T10:01:00Z",
        "2026-02-12T10:01:00Z",
        Some(100),
        None,
        None,
        10,
    );
    let mut entries = vec![child, parent];

    ledger_rebuild::assign_bitcoin_closing_balances(
        &mut entries,
        NativeBalanceState::CanonicalZero,
    )
    .expect("dependency order should succeed");

    let by_hash = |tx_hash: &str| {
        entries
            .iter()
            .find(|entry| entry.tx_hash == tx_hash)
            .and_then(|entry| entry.closing_balance)
            .expect("entry balance should exist")
            .value()
    };
    assert_eq!(by_hash("z-parent"), 10);
    assert_eq!(by_hash("a-child"), 6);
}

#[test]
fn assign_bitcoin_closing_balances_orders_independent_same_block_by_hash() {
    let mut entries = vec![
        test_entry(
            "b",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            Some(100),
            None,
            None,
            5,
        ),
        test_entry(
            "a",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:01:00Z",
            "2026-02-12T10:01:00Z",
            Some(100),
            None,
            None,
            3,
        ),
    ];

    ledger_rebuild::assign_bitcoin_closing_balances(
        &mut entries,
        NativeBalanceState::CanonicalZero,
    )
    .expect("hash tie-break should succeed");

    let by_hash = |tx_hash: &str| {
        entries
            .iter()
            .find(|entry| entry.tx_hash == tx_hash)
            .and_then(|entry| entry.closing_balance)
            .expect("entry balance should exist")
            .value()
    };
    assert_eq!(by_hash("a"), 3);
    assert_eq!(by_hash("b"), 8);
}

#[test]
fn assign_bitcoin_closing_balances_rejects_confirmed_heightless_row() {
    let mut entry = test_entry(
        "heightless",
        ChainTransactionStatus::Confirmed,
        "2026-02-12T10:00:00Z",
        "2026-02-12T10:00:00Z",
        None,
        None,
        None,
        1,
    );
    entry.closing_balance = Some(UnsignedAmount::from_u128(99));
    let mut entries = vec![entry];

    ledger_rebuild::assign_bitcoin_closing_balances(
        &mut entries,
        NativeBalanceState::CanonicalZero,
    )
    .expect_err("heightless confirmed row must fail");

    assert!(entries[0].closing_balance.is_none());
}

#[test]
fn assign_bitcoin_closing_balances_rejects_cycle_and_unresolved_dependency() {
    let mut first = test_entry(
        "a",
        ChainTransactionStatus::Confirmed,
        "2026-02-12T10:00:00Z",
        "2026-02-12T10:00:00Z",
        Some(100),
        None,
        None,
        1,
    );
    first.same_block_parent_hashes = vec!["b".to_string()];
    let mut second = test_entry(
        "b",
        ChainTransactionStatus::Confirmed,
        "2026-02-12T10:00:00Z",
        "2026-02-12T10:00:00Z",
        Some(100),
        None,
        None,
        1,
    );
    second.same_block_parent_hashes = vec!["a".to_string()];
    let mut cycle = vec![first, second];

    ledger_rebuild::assign_bitcoin_closing_balances(&mut cycle, NativeBalanceState::CanonicalZero)
        .expect_err("cycle must fail");

    let mut unresolved = vec![test_entry(
        "child",
        ChainTransactionStatus::Confirmed,
        "2026-02-12T10:00:00Z",
        "2026-02-12T10:00:00Z",
        Some(100),
        None,
        None,
        1,
    )];
    unresolved[0].same_block_parent_hashes = vec!["missing-parent".to_string()];

    ledger_rebuild::assign_bitcoin_closing_balances(
        &mut unresolved,
        NativeBalanceState::CanonicalZero,
    )
    .expect_err("unresolved same-block dependency must fail");
}

#[test]
fn assign_bitcoin_closing_balances_clears_unknown_and_unsafe_conversion() {
    let mut unknown = vec![test_entry(
        "confirmed",
        ChainTransactionStatus::Confirmed,
        "2026-02-12T10:00:00Z",
        "2026-02-12T10:00:00Z",
        Some(100),
        None,
        None,
        1,
    )];
    unknown[0].closing_balance = Some(UnsignedAmount::from_u128(99));

    ledger_rebuild::assign_bitcoin_closing_balances(&mut unknown, NativeBalanceState::Unknown)
        .expect("unknown basis should be unavailable without failing");
    assert!(unknown[0].closing_balance.is_none());

    let mut overflow = unknown;
    ledger_rebuild::assign_bitcoin_closing_balances(
        &mut overflow,
        NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(i128::MAX as u128 + 1)),
    )
    .expect_err("basis outside signed range must fail");
    assert!(overflow[0].closing_balance.is_none());
}

#[test]
fn assign_bitcoin_closing_balances_leaves_non_confirmed_rows_null() {
    let mut entries = vec![
        test_entry(
            "confirmed",
            ChainTransactionStatus::Confirmed,
            "2026-02-12T10:00:00Z",
            "2026-02-12T10:00:00Z",
            Some(100),
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

    ledger_rebuild::assign_bitcoin_closing_balances(
        &mut entries,
        NativeBalanceState::CanonicalZero,
    )
    .expect("assignment should succeed");

    assert_eq!(
        entries[0].closing_balance,
        Some(UnsignedAmount::from_u128(10))
    );
    assert!(
        entries[1..]
            .iter()
            .all(|entry| entry.closing_balance.is_none())
    );
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
    let opening = OpeningBalance(UnsignedAmount::from_u128(10));
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
        Some(OpeningBalance(UnsignedAmount::zero())),
    )
    .expect("assignment should succeed");

    assert_eq!(
        entries_none[0].closing_balance,
        entries_zero[0].closing_balance
    );
}

#[test]
fn assign_closing_balances_opening_balance_with_no_confirmed_entries() {
    let opening = OpeningBalance(UnsignedAmount::from_u128(50));
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
    let opening = OpeningBalance(UnsignedAmount::from_u128(100));
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

    assert_eq!(confirmed, 70_u128);
    assert_eq!(pending, 80_u128);
}

#[test]
fn compute_incomplete_bitcoin_basis_returns_known_synthetic_amount() {
    let address_a = DigitalAssetAddressId::new();
    let address_b = DigitalAssetAddressId::new();
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

    assert_eq!(
        ledger_rebuild::compute_incomplete_bitcoin_basis_from_inputs(&api_balances, &entries)
            .expect("helper should succeed"),
        NativeBalanceState::KnownAmount(UnsignedAmount::from_u128(120))
    );
}

#[test]
fn compute_incomplete_bitcoin_basis_returns_unknown_when_balance_missing() {
    let api_balances = vec![
        api_balance_row(
            DigitalAssetAddressId::new(),
            Some(ApiConfirmedBalance::from_amount(UnsignedAmount::from_u128(
                120,
            ))),
        ),
        api_balance_row(DigitalAssetAddressId::new(), None),
    ];

    assert_eq!(
        ledger_rebuild::compute_incomplete_bitcoin_basis_from_inputs(&api_balances, &[])
            .expect("helper should succeed"),
        NativeBalanceState::Unknown
    );
}

#[test]
fn compute_incomplete_bitcoin_basis_returns_unknown_when_negative() {
    let api_balances = vec![api_balance_row(
        DigitalAssetAddressId::new(),
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

    assert_eq!(
        ledger_rebuild::compute_incomplete_bitcoin_basis_from_inputs(&api_balances, &entries)
            .expect("helper should succeed"),
        NativeBalanceState::Unknown
    );
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
fn resolve_wallet_report_range_plan_exposes_requested_and_default_ranges() {
    let sync_dates = vec![Some(dt("2026-03-30T22:30:00Z"))];
    let timezone = UserTimezone("Europe/Amsterdam".parse().expect("valid timezone"));

    let plan = wallet_report::resolve_wallet_report_range_plan(
        NaiveDate::from_ymd_opt(2026, 7, 3).expect("valid date"),
        &sync_dates,
        timezone,
        Some(NaiveDate::from_ymd_opt(2025, 1, 1).expect("valid date")),
        Some(NaiveDate::from_ymd_opt(2026, 7, 3).expect("valid date")),
    )
    .expect("range plan should resolve");

    assert_eq!(
        plan.requested_range.from(),
        NaiveDate::from_ymd_opt(2025, 1, 1).expect("valid date")
    );
    assert_eq!(
        plan.requested_range.to(),
        NaiveDate::from_ymd_opt(2026, 7, 3).expect("valid date")
    );
    assert_eq!(
        plan.default_range.from(),
        NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date")
    );
    assert_eq!(
        plan.default_range.to(),
        NaiveDate::from_ymd_opt(2026, 3, 31).expect("valid date")
    );
}

#[test]
fn resolve_holdings_report_default_range_uses_current_year_to_date_when_no_native_accounts() {
    let timezone = UserTimezone("Europe/Amsterdam".parse().expect("valid timezone"));
    let today = NaiveDate::from_ymd_opt(2026, 7, 4).expect("valid date");

    let result = wallet_report::resolve_holdings_report_default_range(today, &[], timezone)
        .expect("default range should validate");

    assert_eq!(
        result,
        LocalReportDateRange::new(
            NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"),
            today,
        )
        .expect("expected range should validate")
    );
}

#[test]
fn resolve_holdings_report_default_range_uses_earliest_local_sync_date_when_all_accounts_synced() {
    let timezone = UserTimezone("Europe/Amsterdam".parse().expect("valid timezone"));
    let sync_dates = vec![
        Some(dt("2026-04-15T08:00:00Z")),
        Some(dt("2026-03-29T22:30:00Z")),
        Some(dt("2026-04-01T12:00:00Z")),
    ];

    let result = wallet_report::resolve_holdings_report_default_range(
        NaiveDate::from_ymd_opt(2026, 7, 4).expect("valid date"),
        &sync_dates,
        timezone,
    )
    .expect("default range should validate");

    assert_eq!(
        result,
        LocalReportDateRange::new(
            NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"),
            NaiveDate::from_ymd_opt(2026, 3, 30).expect("valid date"),
        )
        .expect("expected range should validate")
    );
}

#[test]
fn resolve_holdings_report_default_range_uses_today_when_any_account_never_synced() {
    let timezone = UserTimezone("Europe/Amsterdam".parse().expect("valid timezone"));
    let today = NaiveDate::from_ymd_opt(2026, 7, 4).expect("valid date");
    let sync_dates = vec![Some(dt("2026-04-15T08:00:00Z")), None];

    let result = wallet_report::resolve_holdings_report_default_range(today, &sync_dates, timezone)
        .expect("default range should validate");

    assert_eq!(
        result,
        LocalReportDateRange::new(
            NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date"),
            today,
        )
        .expect("expected range should validate")
    );
}

#[test]
fn resolve_holdings_report_default_range_clamps_stale_sync_date_to_year_start() {
    let timezone = UserTimezone("Europe/Amsterdam".parse().expect("valid timezone"));
    let sync_dates = vec![Some(dt("2025-12-31T12:00:00Z"))];

    let result = wallet_report::resolve_holdings_report_default_range(
        NaiveDate::from_ymd_opt(2026, 7, 4).expect("valid date"),
        &sync_dates,
        timezone,
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
