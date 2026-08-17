use crate::db::DbError;
use rusqlite::{Connection, params};

fn apply(conn: &Connection, sql: &str) -> Result<(), DbError> {
    conn.execute_batch(sql)
        .map_err(|err| DbError::new(format!("migration failed: {err}")))
}

fn apply_manual_asset_schema_through_v39(conn: &Connection) {
    apply(
        conn,
        include_str!("../../migrations/user/V13__custom_asset_accounts.sql"),
    )
    .unwrap();
    apply(
        conn,
        include_str!("../../migrations/user/V14__custom_asset_entered_balance_text.sql"),
    )
    .unwrap();
    apply(
        conn,
        include_str!("../../migrations/user/V36__user_price_overrides.sql"),
    )
    .unwrap();

    conn.execute_batch(
        "CREATE TABLE wallets (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            label_key TEXT NOT NULL,
            master_fingerprint TEXT,
            identity_source TEXT NOT NULL,
            verified_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );",
    )
    .unwrap();

    apply(
        conn,
        include_str!("../../migrations/user/V39__manual_assets_parallel_legacy_promotion.sql"),
    )
    .unwrap();
}

fn insert_wallet(conn: &Connection) {
    conn.execute(
        "INSERT INTO wallets
         (id, label, label_key, identity_source, created_at, updated_at)
         VALUES ('w1', 'Wallet', 'wallet', 'user_provided', ?1, ?1)",
        params!["2026-06-01T00:00:00Z"],
    )
    .unwrap();
}

struct ManualAssetSnapshotExpectation {
    id: &'static str,
    asset_id: &'static str,
    network_id: &'static str,
    namespace_type: &'static str,
    decimal_precision: i64,
    unit_code: &'static str,
    symbol: Option<&'static str>,
    asset_name: &'static str,
    network_name: &'static str,
    coingecko_id: &'static str,
}

const ALL_PRE_V40_MANUAL_ASSET_SNAPSHOTS: &[ManualAssetSnapshotExpectation] = &[
    ManualAssetSnapshotExpectation {
        id: "ripple_xrp_mainnet",
        asset_id: "ripple",
        network_id: "ripple-xrp-mainnet",
        namespace_type: "native",
        decimal_precision: 6,
        unit_code: "XRP",
        symbol: None,
        asset_name: "Ripple",
        network_name: "Ripple",
        coingecko_id: "ripple",
    },
    ManualAssetSnapshotExpectation {
        id: "binancecoin_bnb_smart_chain_mainnet",
        asset_id: "binancecoin",
        network_id: "bnb-smart-chain-mainnet",
        namespace_type: "native",
        decimal_precision: 18,
        unit_code: "BNB",
        symbol: None,
        asset_name: "Binance Coin",
        network_name: "BNB Smart Chain",
        coingecko_id: "binancecoin",
    },
    ManualAssetSnapshotExpectation {
        id: "solana_solana_mainnet",
        asset_id: "solana",
        network_id: "solana-mainnet",
        namespace_type: "native",
        decimal_precision: 9,
        unit_code: "SOL",
        symbol: None,
        asset_name: "Solana",
        network_name: "Solana",
        coingecko_id: "solana",
    },
    ManualAssetSnapshotExpectation {
        id: "usd_coin_ethereum_mainnet",
        asset_id: "usd-coin",
        network_id: "ethereum-mainnet",
        namespace_type: "erc20",
        decimal_precision: 6,
        unit_code: "USDC",
        symbol: None,
        asset_name: "USDC on Ethereum",
        network_name: "Ethereum",
        coingecko_id: "usd-coin",
    },
    ManualAssetSnapshotExpectation {
        id: "usd_coin_polygon_mainnet",
        asset_id: "usd-coin",
        network_id: "polygon-mainnet",
        namespace_type: "erc20",
        decimal_precision: 6,
        unit_code: "USDC",
        symbol: None,
        asset_name: "USDC on Polygon",
        network_name: "Polygon",
        coingecko_id: "usd-coin",
    },
    ManualAssetSnapshotExpectation {
        id: "cardano_cardano_mainnet",
        asset_id: "cardano",
        network_id: "cardano-mainnet",
        namespace_type: "native",
        decimal_precision: 6,
        unit_code: "ADA",
        symbol: Some("₳"),
        asset_name: "Cardano",
        network_name: "Cardano",
        coingecko_id: "cardano",
    },
    ManualAssetSnapshotExpectation {
        id: "dogecoin_dogecoin_mainnet",
        asset_id: "dogecoin",
        network_id: "dogecoin-mainnet",
        namespace_type: "native",
        decimal_precision: 8,
        unit_code: "DOGE",
        symbol: None,
        asset_name: "Dogecoin",
        network_name: "Dogecoin",
        coingecko_id: "dogecoin",
    },
    ManualAssetSnapshotExpectation {
        id: "tron_tron_mainnet",
        asset_id: "tron",
        network_id: "tron-mainnet",
        namespace_type: "native",
        decimal_precision: 6,
        unit_code: "TRX",
        symbol: None,
        asset_name: "TRON",
        network_name: "Tron",
        coingecko_id: "tron",
    },
    ManualAssetSnapshotExpectation {
        id: "zcash_zcash_mainnet",
        asset_id: "zcash",
        network_id: "zcash-mainnet",
        namespace_type: "native",
        decimal_precision: 8,
        unit_code: "ZEC",
        symbol: Some("ZEC"),
        asset_name: "Zcash",
        network_name: "Zcash",
        coingecko_id: "zcash",
    },
    ManualAssetSnapshotExpectation {
        id: "monero_monero_mainnet",
        asset_id: "monero",
        network_id: "monero-mainnet",
        namespace_type: "native",
        decimal_precision: 12,
        unit_code: "XMR",
        symbol: None,
        asset_name: "Monero",
        network_name: "Monero",
        coingecko_id: "monero",
    },
    ManualAssetSnapshotExpectation {
        id: "uniswap_arbitrum_one",
        asset_id: "uniswap",
        network_id: "arbitrum-one",
        namespace_type: "erc20",
        decimal_precision: 18,
        unit_code: "UNI",
        symbol: None,
        asset_name: "Uniswap on Arbitrum One",
        network_name: "Arbitrum One",
        coingecko_id: "uniswap",
    },
    ManualAssetSnapshotExpectation {
        id: "uniswap_avalanche_c_chain",
        asset_id: "uniswap",
        network_id: "avalanche-c-chain",
        namespace_type: "erc20",
        decimal_precision: 18,
        unit_code: "UNI",
        symbol: None,
        asset_name: "Uniswap on Avalanche C-Chain",
        network_name: "Avalanche C-Chain",
        coingecko_id: "uniswap",
    },
    ManualAssetSnapshotExpectation {
        id: "uniswap_bnb_smart_chain_mainnet",
        asset_id: "uniswap",
        network_id: "bnb-smart-chain-mainnet",
        namespace_type: "erc20",
        decimal_precision: 18,
        unit_code: "UNI",
        symbol: None,
        asset_name: "Uniswap on BNB Smart Chain",
        network_name: "BNB Smart Chain",
        coingecko_id: "uniswap",
    },
    ManualAssetSnapshotExpectation {
        id: "uniswap_ethereum_mainnet",
        asset_id: "uniswap",
        network_id: "ethereum-mainnet",
        namespace_type: "erc20",
        decimal_precision: 18,
        unit_code: "UNI",
        symbol: None,
        asset_name: "Uniswap on Ethereum",
        network_name: "Ethereum",
        coingecko_id: "uniswap",
    },
    ManualAssetSnapshotExpectation {
        id: "uniswap_optimism_mainnet",
        asset_id: "uniswap",
        network_id: "optimism-mainnet",
        namespace_type: "erc20",
        decimal_precision: 18,
        unit_code: "UNI",
        symbol: None,
        asset_name: "Uniswap on Optimism",
        network_name: "Optimism",
        coingecko_id: "uniswap",
    },
    ManualAssetSnapshotExpectation {
        id: "uniswap_polygon_mainnet",
        asset_id: "uniswap",
        network_id: "polygon-mainnet",
        namespace_type: "erc20",
        decimal_precision: 18,
        unit_code: "UNI",
        symbol: None,
        asset_name: "Uniswap on Polygon",
        network_name: "Polygon",
        coingecko_id: "uniswap",
    },
    ManualAssetSnapshotExpectation {
        id: "tezos_tezos_mainnet",
        asset_id: "tezos",
        network_id: "tezos-mainnet",
        namespace_type: "native",
        decimal_precision: 6,
        unit_code: "XTZ",
        symbol: None,
        asset_name: "Tezos",
        network_name: "Tezos",
        coingecko_id: "tezos",
    },
    ManualAssetSnapshotExpectation {
        id: "usd_coin_algorand_mainnet",
        asset_id: "usd-coin",
        network_id: "algorand-mainnet",
        namespace_type: "native",
        decimal_precision: 6,
        unit_code: "USDC",
        symbol: None,
        asset_name: "USDC on Algorand",
        network_name: "Algorand",
        coingecko_id: "usd-coin",
    },
    ManualAssetSnapshotExpectation {
        id: "algorand_algorand_mainnet",
        asset_id: "algorand",
        network_id: "algorand-mainnet",
        namespace_type: "native",
        decimal_precision: 6,
        unit_code: "ALGO",
        symbol: None,
        asset_name: "Algorand",
        network_name: "Algorand",
        coingecko_id: "algorand",
    },
];

#[test]
fn v39_promotes_ada_and_keeps_unknown_custom_assets() {
    let conn = Connection::open_in_memory().expect("db");
    apply(
        &conn,
        include_str!("../../migrations/user/V13__custom_asset_accounts.sql"),
    )
    .unwrap();
    apply(
        &conn,
        include_str!("../../migrations/user/V14__custom_asset_entered_balance_text.sql"),
    )
    .unwrap();
    apply(
        &conn,
        include_str!("../../migrations/user/V36__user_price_overrides.sql"),
    )
    .unwrap();

    conn.execute_batch(
        "CREATE TABLE wallets (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            label_key TEXT NOT NULL,
            master_fingerprint TEXT,
            identity_source TEXT NOT NULL,
            verified_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );",
    )
    .unwrap();

    let now = "2026-05-28T00:00:00Z";
    conn.execute(
        "INSERT INTO wallets
         (id, label, label_key, identity_source, created_at, updated_at)
         VALUES ('w1', 'Wallet', 'wallet', 'user_provided', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO custom_asset_accounts
         (id, wallet_id, label, label_key, unit_code, display_scale, created_at, updated_at)
         VALUES ('ada1', 'w1', 'ADA Account 1', 'ada account 1', 'ADA', 6, ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO custom_asset_balance_assertions
         (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo, entered_balance_text, note, created_at, updated_at)
         VALUES ('b1', 'ada1', '2026-05-28', 0, 1500000, '1.5', 'opening', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO custom_asset_accounts
         (id, wallet_id, label, label_key, unit_code, display_scale, created_at, updated_at)
         VALUES ('x1', 'w1', 'My Token', 'my token', 'MYTOKEN', 8, ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO custom_asset_accounts
         (id, wallet_id, label, label_key, unit_code, display_scale, created_at, updated_at)
         VALUES ('ada_unsafe', 'w1', 'ADA High Precision', 'ada high precision', 'ADA', 7, ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO user_price_overrides
         (id, subject_type, subject_id, quote_currency, price_time_utc, price, source_note, created_at, updated_at)
         VALUES ('pbtc', 'native_asset', 'bitcoin', 'USD', ?1, '50000', NULL, ?1, ?1),
                ('pada', 'custom_unit_code', 'ADA', 'USD', ?1, '0.45', NULL, ?1, ?1),
                ('px', 'custom_unit_code', 'MYTOKEN', 'USD', ?1, '1.23', NULL, ?1, ?1)",
        params![now],
    )
    .unwrap();

    apply(
        &conn,
        include_str!("../../migrations/user/V39__manual_assets_parallel_legacy_promotion.sql"),
    )
    .unwrap();

    let manual_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM manual_asset_accounts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(manual_count, 1);

    let promoted_ada_legacy_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM custom_asset_accounts WHERE id = 'ada1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(promoted_ada_legacy_count, 0);

    let retained_ada_legacy_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM custom_asset_accounts WHERE id = 'ada_unsafe'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained_ada_legacy_count, 1);

    let unknown_legacy_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM custom_asset_accounts WHERE unit_code = 'MYTOKEN'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unknown_legacy_count, 1);

    let new_price_subjects: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT asset_id FROM user_price_overrides ORDER BY asset_id")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(
        new_price_subjects,
        vec!["bitcoin".to_string(), "cardano".to_string()]
    );

    let legacy_price_subjects: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT subject_id FROM custom_user_price_overrides ORDER BY subject_id")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(
        legacy_price_subjects,
        vec!["ADA".to_string(), "MYTOKEN".to_string()]
    );
}

#[test]
fn v40_rebuilds_manual_assets_with_snapshots_and_drops_namespace_columns() {
    let conn = Connection::open_in_memory().expect("db");
    apply_manual_asset_schema_through_v39(&conn);
    insert_wallet(&conn);

    let now = "2026-06-01T00:00:00Z";
    for (id, label, label_key, asset_id, network_id, namespace_type, namespace_ref) in [
        (
            "usdc_eth",
            "USDC Ethereum",
            "usdc ethereum",
            "usd-coin",
            "ethereum-mainnet",
            "erc20",
            Some("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"),
        ),
        (
            "ada",
            "ADA",
            "ada",
            "cardano",
            "cardano-mainnet",
            "native",
            None,
        ),
        (
            "xrp",
            "XRP",
            "xrp",
            "ripple",
            "ripple-xrp-mainnet",
            "native",
            None,
        ),
        (
            "bnb",
            "BNB",
            "bnb",
            "binancecoin",
            "bnb-smart-chain-mainnet",
            "native",
            None,
        ),
        (
            "usdc_algo",
            "USDC Algorand",
            "usdc algorand",
            "usd-coin",
            "algorand-mainnet",
            "native",
            None,
        ),
        (
            "algo",
            "ALGO",
            "algo",
            "algorand",
            "algorand-mainnet",
            "native",
            None,
        ),
    ] {
        conn.execute(
            "INSERT INTO manual_asset_accounts
             (id, wallet_id, label, label_key, asset_id, network_id, namespace_type, namespace_ref, created_at, updated_at)
             VALUES (?1, 'w1', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                id,
                label,
                label_key,
                asset_id,
                network_id,
                namespace_type,
                namespace_ref,
                now
            ],
        )
        .unwrap();
    }

    apply(
        &conn,
        include_str!("../../migrations/user/V40__manual_asset_account_snapshots.sql"),
    )
    .unwrap();

    let columns = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(manual_asset_accounts)")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert!(!columns.contains(&"namespace_type".to_string()));
    assert!(!columns.contains(&"namespace_ref".to_string()));
    for required in [
        "decimal_precision",
        "unit_code",
        "symbol",
        "asset_name",
        "network_name",
        "coingecko_id",
    ] {
        assert!(
            columns.contains(&required.to_string()),
            "{required} missing"
        );
    }

    let rows = {
        let mut stmt = conn
            .prepare(
                "SELECT id, asset_id, network_id, decimal_precision, unit_code, symbol,
                        asset_name, network_name, coingecko_id
                 FROM manual_asset_accounts
                 ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    assert_eq!(
        rows,
        vec![
            (
                "ada".to_string(),
                "cardano".to_string(),
                "cardano-mainnet".to_string(),
                6,
                "ADA".to_string(),
                Some("₳".to_string()),
                "Cardano".to_string(),
                "Cardano".to_string(),
                "cardano".to_string(),
            ),
            (
                "algo".to_string(),
                "algorand".to_string(),
                "algorand-mainnet".to_string(),
                6,
                "ALGO".to_string(),
                None,
                "Algorand".to_string(),
                "Algorand".to_string(),
                "algorand".to_string(),
            ),
            (
                "bnb".to_string(),
                "binancecoin".to_string(),
                "bnb-smart-chain-mainnet".to_string(),
                18,
                "BNB".to_string(),
                None,
                "Binance Coin".to_string(),
                "BNB Smart Chain".to_string(),
                "binancecoin".to_string(),
            ),
            (
                "usdc_algo".to_string(),
                "usd-coin".to_string(),
                "algorand-mainnet".to_string(),
                6,
                "USDC".to_string(),
                None,
                "USDC on Algorand".to_string(),
                "Algorand".to_string(),
                "usd-coin".to_string(),
            ),
            (
                "usdc_eth".to_string(),
                "usd-coin".to_string(),
                "ethereum-mainnet".to_string(),
                6,
                "USDC".to_string(),
                None,
                "USDC on Ethereum".to_string(),
                "Ethereum".to_string(),
                "usd-coin".to_string(),
            ),
            (
                "xrp".to_string(),
                "ripple".to_string(),
                "ripple-xrp-mainnet".to_string(),
                6,
                "XRP".to_string(),
                None,
                "Ripple".to_string(),
                "Ripple".to_string(),
                "ripple".to_string(),
            ),
        ]
    );
}

#[test]
fn v40_snapshot_map_covers_all_pre_v40_manual_asset_instances() {
    let conn = Connection::open_in_memory().expect("db");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    apply_manual_asset_schema_through_v39(&conn);
    insert_wallet(&conn);

    let now = "2026-06-01T00:00:00Z";
    for expected in ALL_PRE_V40_MANUAL_ASSET_SNAPSHOTS {
        conn.execute(
            "INSERT INTO manual_asset_accounts
             (id, wallet_id, label, label_key, asset_id, network_id, namespace_type, namespace_ref, created_at, updated_at)
             VALUES (?1, 'w1', ?2, ?1, ?3, ?4, ?5, NULL, ?6, ?6)",
            params![
                expected.id,
                expected.asset_name,
                expected.asset_id,
                expected.network_id,
                expected.namespace_type,
                now
            ],
        )
        .unwrap();
    }

    apply(
        &conn,
        include_str!("../../migrations/user/V40__manual_asset_account_snapshots.sql"),
    )
    .unwrap();

    let migrated_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM manual_asset_accounts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        migrated_count,
        i64::try_from(ALL_PRE_V40_MANUAL_ASSET_SNAPSHOTS.len()).unwrap()
    );

    let null_snapshot_count: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM manual_asset_accounts
             WHERE decimal_precision IS NULL
                OR unit_code IS NULL
                OR asset_name IS NULL
                OR network_name IS NULL
                OR coingecko_id IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(null_snapshot_count, 0);

    for expected in ALL_PRE_V40_MANUAL_ASSET_SNAPSHOTS {
        let row = conn
            .query_row(
                "SELECT decimal_precision, unit_code, symbol, asset_name, network_name, coingecko_id
                 FROM manual_asset_accounts
                 WHERE asset_id = ?1 AND network_id = ?2",
                params![expected.asset_id, expected.network_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            row,
            (
                expected.decimal_precision,
                expected.unit_code.to_string(),
                expected.symbol.map(str::to_string),
                expected.asset_name.to_string(),
                expected.network_name.to_string(),
                expected.coingecko_id.to_string(),
            ),
            "{} on {}",
            expected.asset_id,
            expected.network_id,
        );
    }

    let fk_violation_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(fk_violation_count, 0);
}

#[test]
fn v41_normalizes_multi_character_manual_asset_symbols() {
    let conn = Connection::open_in_memory().expect("db");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    apply_manual_asset_schema_through_v39(&conn);
    insert_wallet(&conn);

    let now = "2026-06-01T00:00:00Z";
    for expected in ALL_PRE_V40_MANUAL_ASSET_SNAPSHOTS {
        conn.execute(
            "INSERT INTO manual_asset_accounts
             (id, wallet_id, label, label_key, asset_id, network_id, namespace_type, namespace_ref, created_at, updated_at)
             VALUES (?1, 'w1', ?2, ?1, ?3, ?4, ?5, NULL, ?6, ?6)",
            params![
                expected.id,
                expected.asset_name,
                expected.asset_id,
                expected.network_id,
                expected.namespace_type,
                now
            ],
        )
        .unwrap();
    }

    apply(
        &conn,
        include_str!("../../migrations/user/V40__manual_asset_account_snapshots.sql"),
    )
    .unwrap();
    apply(
        &conn,
        include_str!("../../migrations/user/V41__normalize_manual_asset_symbols.sql"),
    )
    .unwrap();

    let zcash_symbol: Option<String> = conn
        .query_row(
            "SELECT symbol
             FROM manual_asset_accounts
             WHERE asset_id = 'zcash' AND network_id = 'zcash-mainnet'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(zcash_symbol, None);

    let invalid_symbol_count: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM manual_asset_accounts
             WHERE symbol IS NOT NULL AND length(symbol) != 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(invalid_symbol_count, 0);
}

#[test]
fn v40_preserves_manual_asset_assertions_with_foreign_keys_enabled() {
    let conn = Connection::open_in_memory().expect("db");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    apply_manual_asset_schema_through_v39(&conn);
    insert_wallet(&conn);

    let now = "2026-06-01T00:00:00Z";
    conn.execute(
        "INSERT INTO manual_asset_accounts
         (id, wallet_id, label, label_key, asset_id, network_id, namespace_type, namespace_ref, created_at, updated_at)
         VALUES ('acct1', 'w1', 'USDC Ethereum', 'usdc ethereum', 'usd-coin', 'ethereum-mainnet',
                 'erc20', '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO manual_asset_balance_assertions
         (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo, entered_balance_text, note, created_at, updated_at)
         VALUES ('assertion1', 'acct1', '2026-06-01', 0, 123456, '0.123456', 'opening', ?1, ?1)",
        params![now],
    )
    .unwrap();

    apply(
        &conn,
        include_str!("../../migrations/user/V40__manual_asset_account_snapshots.sql"),
    )
    .unwrap();

    let assertion_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM manual_asset_balance_assertions WHERE id = 'assertion1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(assertion_count, 1);

    let fk_violation_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(fk_violation_count, 0);

    let columns = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(manual_asset_accounts)")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert!(!columns.contains(&"namespace_type".to_string()));
    assert!(!columns.contains(&"namespace_ref".to_string()));
    assert!(columns.contains(&"decimal_precision".to_string()));
    assert!(columns.contains(&"unit_code".to_string()));
    assert!(columns.contains(&"symbol".to_string()));
    assert!(columns.contains(&"asset_name".to_string()));
    assert!(columns.contains(&"network_name".to_string()));
    assert!(columns.contains(&"coingecko_id".to_string()));
}

#[test]
fn v42_adds_manual_asset_discovery_metadata_and_preserves_assertions() {
    let conn = Connection::open_in_memory().expect("db");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    apply_manual_asset_schema_through_v39(&conn);
    insert_wallet(&conn);

    let now = "2026-06-01T00:00:00Z";
    conn.execute(
        "INSERT INTO manual_asset_accounts
         (id, wallet_id, label, label_key, asset_id, network_id, namespace_type, namespace_ref, created_at, updated_at)
         VALUES ('acct1', 'w1', 'ADA', 'ada', 'cardano', 'cardano-mainnet',
                 'native', NULL, ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO manual_asset_balance_assertions
         (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo, entered_balance_text, note, created_at, updated_at)
         VALUES ('assertion1', 'acct1', '2026-06-01', 0, 123456, '0.123456', 'opening', ?1, ?1)",
        params![now],
    )
    .unwrap();

    apply(
        &conn,
        include_str!("../../migrations/user/V40__manual_asset_account_snapshots.sql"),
    )
    .unwrap();
    apply(
        &conn,
        include_str!("../../migrations/user/V41__normalize_manual_asset_symbols.sql"),
    )
    .unwrap();
    apply(
        &conn,
        include_str!("../../migrations/user/V42__manual_asset_discovery_metadata.sql"),
    )
    .unwrap();

    let row: (String, String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT asset_source, precision_source, coingecko_platform_id, provider_platform_asset_ref
             FROM manual_asset_accounts WHERE id = 'acct1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "bitgarth_catalog".to_string(),
            "bitgarth_catalog".to_string(),
            None,
            None,
        )
    );

    let assertion_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM manual_asset_balance_assertions WHERE id = 'assertion1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(assertion_count, 1);

    let invalid_source = conn.execute(
        "INSERT INTO manual_asset_accounts
         (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
          unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
          precision_source, coingecko_platform_id, provider_platform_asset_ref,
          created_at, updated_at)
         VALUES ('bad_source', 'w1', 'Bad', 'bad', 'cardano', 'cardano-mainnet', 6,
                 'ADA', NULL, 'Cardano', 'Cardano', 'cardano', 'bad',
                 'bitgarth_catalog', NULL, NULL, ?1, ?1)",
        params![now],
    );
    assert!(invalid_source.is_err());

    let invalid_precision_source = conn.execute(
        "INSERT INTO manual_asset_accounts
         (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
          unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
          precision_source, coingecko_platform_id, provider_platform_asset_ref,
          created_at, updated_at)
         VALUES ('bad_precision_source', 'w1', 'Bad 2', 'bad 2', 'cardano',
                 'cardano-mainnet', 6, 'ADA', NULL, 'Cardano', 'Cardano',
                 'cardano', 'bitgarth_catalog', 'bad', NULL, NULL, ?1, ?1)",
        params![now],
    );
    assert!(invalid_precision_source.is_err());

    let fk_violation_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(fk_violation_count, 0);
}

#[test]
fn v40_rejects_unmapped_manual_asset_account_before_dropping_old_table() {
    let conn = Connection::open_in_memory().expect("db");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    apply_manual_asset_schema_through_v39(&conn);
    insert_wallet(&conn);

    conn.execute(
        "INSERT INTO manual_asset_accounts
         (id, wallet_id, label, label_key, asset_id, network_id, namespace_type, namespace_ref, created_at, updated_at)
         VALUES ('mystery', 'w1', 'Mystery', 'mystery', 'mystery-coin', 'mystery-mainnet', 'native', NULL, ?1, ?1)",
        params!["2026-06-01T00:00:00Z"],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO manual_asset_balance_assertions
         (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo, entered_balance_text, note, created_at, updated_at)
         VALUES ('mystery_assertion', 'mystery', '2026-06-01', 0, 1, '0.000001', NULL, ?1, ?1)",
        params!["2026-06-01T00:00:00Z"],
    )
    .unwrap();

    let err = conn
        .execute_batch(include_str!(
            "../../migrations/user/V40__manual_asset_account_snapshots.sql"
        ))
        .expect_err("unmapped manual account should abort V40");
    assert!(err.to_string().contains("NOT NULL"), "{err}");

    let account_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM manual_asset_accounts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(account_count, 1);

    let assertion_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM manual_asset_balance_assertions WHERE id = 'mystery_assertion'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(assertion_count, 1);

    let old_table_has_namespace: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM pragma_table_info('manual_asset_accounts')
             WHERE name IN ('namespace_type', 'namespace_ref')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_table_has_namespace, 2);
}

fn object_exists(conn: &Connection, object_type: &str, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2)",
        params![object_type, name],
        |row| row.get(0),
    )
    .expect("schema object query should work")
}

fn row_count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .expect("row count should load")
}

fn seed_v45_legacy_and_supported_data(conn: &Connection) {
    let now = "2026-07-14T12:00:00Z";
    conn.execute_batch(&format!(
        "INSERT INTO wallets
             (id, label, label_key, identity_source, created_at, updated_at)
         VALUES ('legacy-wallet', 'Legacy Wallet', 'legacy wallet', 'user_provided', '{now}', '{now}');

         INSERT INTO custom_asset_accounts
             (id, wallet_id, label, label_key, unit_code, display_scale, created_at, updated_at)
         VALUES ('legacy-account', 'legacy-wallet', 'Old Token', 'old token', 'OLD', 8, '{now}', '{now}');

         INSERT INTO custom_asset_balance_assertions
             (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo,
              entered_balance_text, note, created_at, updated_at)
         VALUES ('legacy-assertion', 'legacy-account', '2026-01-01', 0, 1,
                 '0.00000001', NULL, '{now}', '{now}');

         INSERT INTO custom_user_price_overrides
             (id, subject_type, subject_id, quote_currency, price_time_utc, price,
              source_note, created_at, updated_at)
         VALUES ('legacy-price', 'custom_unit_code', 'OLD', 'USD', '{now}', '1.25',
                 NULL, '{now}', '{now}');

         INSERT INTO manual_asset_accounts
             (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
              unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
              precision_source, coingecko_platform_id, provider_platform_asset_ref,
              created_at, updated_at)
         VALUES ('manual-account', 'legacy-wallet', 'Cardano', 'cardano', 'cardano',
                 'cardano-mainnet', 6, 'ADA', NULL, 'Cardano', 'Cardano', 'cardano',
                 'bitgarth_catalog', 'bitgarth_catalog', NULL, NULL, '{now}', '{now}');

         INSERT INTO manual_asset_balance_assertions
             (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo,
              entered_balance_text, note, created_at, updated_at)
         VALUES ('manual-assertion', 'manual-account', '2026-01-01', 0, 1,
                 '0.000001', NULL, '{now}', '{now}');

         INSERT INTO user_price_overrides
             (id, asset_id, quote_currency, price_time_utc, price, source_note,
              created_at, updated_at)
         VALUES ('catalog-price', 'cardano', 'USD', '{now}', '1.00', NULL, '{now}', '{now}');

         INSERT INTO settings (settings_id, theme, updated_at)
         VALUES ('settings', 'dark', '{now}');"
    ))
    .expect("V45 fixture should seed");
}

#[test]
fn v46_drops_legacy_tables_and_preserves_supported_data() {
    let mut conn = Connection::open_in_memory().expect("db");
    crate::db::user_db::migrations_runner()
        .expect("migration runner")
        .set_target(refinery::Target::Version(45))
        .run(&mut conn)
        .expect("migrations through V45 should apply");
    seed_v45_legacy_and_supported_data(&conn);
    conn.execute_batch(include_str!(
        "../../migrations/user/V46__remove_legacy_custom_asset_accounts.sql"
    ))
    .expect("V46 migration should apply");

    for table in [
        "custom_asset_accounts",
        "custom_asset_balance_assertions",
        "custom_user_price_overrides",
    ] {
        assert!(!object_exists(&conn, "table", table));
    }
    assert_eq!(row_count(&conn, "wallets"), 1);
    assert_eq!(row_count(&conn, "manual_asset_accounts"), 1);
    assert_eq!(row_count(&conn, "manual_asset_balance_assertions"), 1);
    assert_eq!(row_count(&conn, "user_price_overrides"), 1);
    assert_eq!(row_count(&conn, "settings"), 1);
}
