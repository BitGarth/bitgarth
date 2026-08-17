CREATE TABLE account_integration_sync_state (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES digital_asset_accounts(id) ON DELETE CASCADE,
    integration_id TEXT NOT NULL CHECK (integration_id IN ('mempool', 'etherscan')),
    last_started_at TEXT,
    last_completed_at TEXT,
    last_result TEXT CHECK (last_result IN ('success', 'partial', 'failure')),
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(account_id, integration_id)
);

CREATE INDEX idx_account_integration_sync_state_updated_at
ON account_integration_sync_state(updated_at);
