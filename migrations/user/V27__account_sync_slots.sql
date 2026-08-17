CREATE TABLE account_sync_slots (
    account_id TEXT PRIMARY KEY NOT NULL,
    selected_at TEXT NOT NULL,
    selected_under_tier TEXT NOT NULL,
    FOREIGN KEY (account_id) REFERENCES digital_asset_accounts(id) ON DELETE CASCADE
);

CREATE INDEX idx_account_sync_slots_selected_at
    ON account_sync_slots(selected_at, account_id);
