ALTER TABLE manual_asset_accounts ADD COLUMN admitted_at TEXT;
CREATE INDEX idx_manual_asset_accounts_admitted_at
    ON manual_asset_accounts(admitted_at, id);
