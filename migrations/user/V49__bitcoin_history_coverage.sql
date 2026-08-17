ALTER TABLE transaction_sync_state
ADD COLUMN mempool_history_complete_tx_count INTEGER
CHECK (
    mempool_history_complete_tx_count IS NULL
    OR mempool_history_complete_tx_count >= 0
);

ALTER TABLE transaction_sync_state
ADD COLUMN mempool_history_complete_height INTEGER
CHECK (
    mempool_history_complete_height IS NULL
    OR mempool_history_complete_height >= 0
)
CHECK (
    (
        mempool_history_complete_tx_count IS NULL
        AND mempool_history_complete_height IS NULL
    )
    OR
    (
        mempool_history_complete_tx_count IS NOT NULL
        AND mempool_history_complete_height IS NOT NULL
    )
);

ALTER TABLE transaction_sync_state
ADD COLUMN mempool_history_scan_start_run_id TEXT
REFERENCES sync_runs(id) ON DELETE SET NULL;

ALTER TABLE account_sync_state
ADD COLUMN mempool_history_next_address_id TEXT
REFERENCES digital_asset_addresses(id) ON DELETE SET NULL;

UPDATE transaction_sync_state
SET mempool_backfill_cursor_txid = NULL,
    mempool_expected_tx_count = NULL
WHERE address_id IN (
    SELECT id
    FROM digital_asset_addresses
    WHERE asset_id = 'bitcoin'
);

UPDATE account_sync_state
SET last_scanned_height = NULL,
    last_scanned_time = NULL,
    mempool_history_next_address_id = NULL
WHERE account_id IN (
    SELECT id
    FROM digital_asset_accounts
    WHERE asset_id = 'bitcoin'
      AND account_kind = 'hd_pubkey'
);

DELETE FROM hd_account_chain_sync_state
WHERE account_id IN (
    SELECT id
    FROM digital_asset_accounts
    WHERE asset_id = 'bitcoin'
      AND account_kind = 'hd_pubkey'
);

INSERT INTO hd_account_chain_sync_state (
    id,
    account_id,
    derivation_change,
    frontier_phase,
    next_index_to_scan,
    consecutive_unused,
    active_rescan_from_index,
    created_at,
    updated_at
)
SELECT
    lower(hex(randomblob(16))),
    accounts.id,
    branches.derivation_change,
    'existing_addresses',
    0,
    0,
    NULL,
    timestamp.value,
    timestamp.value
FROM digital_asset_accounts AS accounts
CROSS JOIN (
    SELECT 0 AS derivation_change
    UNION ALL
    SELECT 1
) AS branches
CROSS JOIN (
    SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now') AS value
) AS timestamp
WHERE accounts.asset_id = 'bitcoin'
  AND accounts.account_kind = 'hd_pubkey';

UPDATE account_transaction_ledger
SET closing_balance_hi = NULL,
    closing_balance_lo = NULL
WHERE account_id IN (
    SELECT id
    FROM digital_asset_accounts
    WHERE asset_id = 'bitcoin'
);

INSERT INTO user_data_repairs (
    repair_key,
    status,
    last_attempted_at,
    completed_at,
    last_error,
    created_at,
    updated_at
)
SELECT
    'bitcoin_history_full_resync_v1',
    'pending',
    NULL,
    NULL,
    NULL,
    timestamp.value,
    timestamp.value
FROM (
    SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now') AS value
) AS timestamp;
