CREATE TABLE hd_account_chain_sync_state (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES digital_asset_accounts(id) ON DELETE CASCADE,
    derivation_change INTEGER NOT NULL CHECK (derivation_change >= 0),
    frontier_phase TEXT NOT NULL CHECK (
        frontier_phase IN ('existing_addresses', 'derived_addresses', 'active_rescan')
    ),
    next_index_to_scan INTEGER NOT NULL CHECK (next_index_to_scan >= 0),
    consecutive_unused INTEGER NOT NULL CHECK (consecutive_unused >= 0),
    active_rescan_from_index INTEGER CHECK (active_rescan_from_index >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(account_id, derivation_change)
);

CREATE UNIQUE INDEX idx_hd_account_chain_sync_state_account_change
ON hd_account_chain_sync_state(account_id, derivation_change);

CREATE INDEX idx_hd_account_chain_sync_state_updated_at
ON hd_account_chain_sync_state(updated_at);
