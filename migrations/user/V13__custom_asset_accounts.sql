CREATE TABLE IF NOT EXISTS custom_asset_accounts (
    id TEXT PRIMARY KEY,
    wallet_id TEXT NOT NULL REFERENCES wallets(id) ON DELETE CASCADE,
    label TEXT NOT NULL CHECK(length(label) <= 255),
    label_key TEXT NOT NULL,
    unit_code TEXT NOT NULL CHECK(length(unit_code) >= 1 AND length(unit_code) <= 20),
    display_scale INTEGER NOT NULL CHECK(display_scale >= 0 AND display_scale <= 255),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_caa_wallet ON custom_asset_accounts(wallet_id);
CREATE INDEX IF NOT EXISTS idx_caa_unit_code ON custom_asset_accounts(unit_code);
CREATE UNIQUE INDEX IF NOT EXISTS idx_caa_label_key ON custom_asset_accounts(wallet_id, label_key);

CREATE TABLE IF NOT EXISTS custom_asset_balance_assertions (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES custom_asset_accounts(id) ON DELETE CASCADE,
    asserted_on TEXT NOT NULL,
    balance_amount_hi INTEGER NOT NULL CHECK(balance_amount_hi >= 0),
    balance_amount_lo INTEGER NOT NULL CHECK(balance_amount_lo >= 0),
    note TEXT CHECK(length(note) <= 500),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(account_id, asserted_on)
);

CREATE INDEX IF NOT EXISTS idx_caba_account_asserted_on
ON custom_asset_balance_assertions(account_id, asserted_on DESC);
