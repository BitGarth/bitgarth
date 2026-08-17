use rusqlite::{Connection, OptionalExtension, params};

const V47: &str = include_str!("../../migrations/user/V47__account_transfer_provider_key.sql");

fn legacy_connection() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory DB should open");
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE chain_transactions (id TEXT PRIMARY KEY);
         CREATE TABLE digital_asset_addresses (id TEXT PRIMARY KEY);
         CREATE TABLE account_transfers (
             id TEXT PRIMARY KEY,
             chain_transaction_id TEXT NOT NULL REFERENCES chain_transactions(id) ON DELETE CASCADE,
             asset_id TEXT NOT NULL,
             network TEXT NOT NULL,
             tx_hash TEXT NOT NULL,
             transfer_index INTEGER NOT NULL CHECK (transfer_index >= 0),
             transfer_kind TEXT NOT NULL,
             from_address TEXT,
             from_address_id TEXT REFERENCES digital_asset_addresses(id) ON DELETE SET NULL,
             to_address TEXT,
             to_address_id TEXT REFERENCES digital_asset_addresses(id) ON DELETE SET NULL,
             value_amount_hi INTEGER NOT NULL,
             value_amount_lo INTEGER NOT NULL,
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             UNIQUE(asset_id, network, tx_hash, transfer_index)
         );
         CREATE INDEX idx_account_transfers_from ON account_transfers(from_address_id);
         CREATE INDEX idx_account_transfers_to ON account_transfers(to_address_id);
         CREATE INDEX idx_account_transfers_chain_tx ON account_transfers(chain_transaction_id);
         INSERT INTO chain_transactions (id) VALUES ('chain-1');
         INSERT INTO account_transfers
             (id, chain_transaction_id, asset_id, network, tx_hash, transfer_index,
              transfer_kind, value_amount_hi, value_amount_lo, created_at, updated_at)
         VALUES
             ('transfer-0', 'chain-1', 'ethereum', 'mainnet', 'hash', 0,
              'normal', 0, 1, '2026-07-19T00:00:00Z', '2026-07-19T00:00:00Z'),
             ('transfer-2', 'chain-1', 'ethereum', 'mainnet', 'hash', 2,
              'internal', 0, 2, '2026-07-19T00:00:00Z', '2026-07-19T00:00:00Z');",
    )
    .expect("legacy schema should seed");
    conn
}

#[test]
fn v47_backfills_provider_identity_and_preserves_schema_contract() {
    let conn = legacy_connection();
    conn.execute_batch(V47).expect("V47 should apply");

    let keys = conn
        .prepare("SELECT provider_transfer_key FROM account_transfers ORDER BY transfer_index")
        .expect("key query should prepare")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("keys should query")
        .collect::<Result<Vec<_>, _>>()
        .expect("keys should read");
    assert_eq!(keys, vec!["legacy:0", "legacy:2"]);

    let provider_not_null: i64 = conn
        .query_row(
            "SELECT \"notnull\" FROM pragma_table_info('account_transfers') WHERE name = 'provider_transfer_key'",
            [],
            |row| row.get(0),
        )
        .expect("provider column should exist");
    assert_eq!(provider_not_null, 1);

    let indexes = conn
        .prepare("SELECT name FROM pragma_index_list('account_transfers') ORDER BY name")
        .expect("index query should prepare")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("indexes should query")
        .collect::<Result<Vec<_>, _>>()
        .expect("indexes should read");
    assert!(
        indexes
            .iter()
            .any(|name| name == "idx_account_transfers_from")
    );
    assert!(
        indexes
            .iter()
            .any(|name| name == "idx_account_transfers_to")
    );
    assert!(
        indexes
            .iter()
            .any(|name| name == "idx_account_transfers_chain_tx")
    );

    let foreign_key_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_foreign_key_list('account_transfers')",
            [],
            |row| row.get(0),
        )
        .expect("foreign keys should query");
    assert_eq!(foreign_key_count, 3);

    conn.execute(
        "INSERT INTO account_transfers
             (id, chain_transaction_id, asset_id, network, tx_hash, provider_transfer_key,
              transfer_index, transfer_kind, value_amount_hi, value_amount_lo, created_at, updated_at)
         VALUES (?1, 'chain-1', 'ethereum', 'mainnet', 'new-hash', ?2,
                 1, 'internal', 0, 3, '2026-07-19T00:00:00Z', '2026-07-19T00:00:00Z')",
        params!["new-a", "internal:0_1"],
    )
    .expect("first provider key should insert");
    conn.execute(
        "INSERT INTO account_transfers
             (id, chain_transaction_id, asset_id, network, tx_hash, provider_transfer_key,
              transfer_index, transfer_kind, value_amount_hi, value_amount_lo, created_at, updated_at)
         VALUES (?1, 'chain-1', 'ethereum', 'mainnet', 'new-hash', ?2,
                 1, 'internal', 0, 4, '2026-07-19T00:00:00Z', '2026-07-19T00:00:00Z')",
        params!["new-b", "internal:1"],
    )
    .expect("duplicate display index with distinct provider key should insert");
    assert!(
        conn.execute(
            "INSERT INTO account_transfers
                 (id, chain_transaction_id, asset_id, network, tx_hash, provider_transfer_key,
                  transfer_index, transfer_kind, value_amount_hi, value_amount_lo, created_at, updated_at)
             VALUES ('new-c', 'chain-1', 'ethereum', 'mainnet', 'new-hash', 'internal:1',
                     2, 'internal', 0, 5, '2026-07-19T00:00:00Z', '2026-07-19T00:00:00Z')",
            [],
        )
        .is_err(),
        "duplicate provider identity must remain unique"
    );
    assert_eq!(
        conn.query_row("PRAGMA foreign_key_check", [], |_| Ok(1_i64))
            .optional()
            .unwrap(),
        None
    );
}
