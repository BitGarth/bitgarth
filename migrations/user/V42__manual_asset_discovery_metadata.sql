CREATE TABLE manual_asset_accounts_new (
    id TEXT PRIMARY KEY,
    wallet_id TEXT NOT NULL REFERENCES wallets(id) ON DELETE CASCADE,
    label TEXT NOT NULL CHECK(length(label) <= 255),
    label_key TEXT NOT NULL,
    asset_id TEXT NOT NULL,
    network_id TEXT NOT NULL,
    decimal_precision INTEGER NOT NULL CHECK(decimal_precision >= 0 AND decimal_precision <= 18),
    unit_code TEXT NOT NULL,
    symbol TEXT,
    asset_name TEXT NOT NULL,
    network_name TEXT NOT NULL,
    coingecko_id TEXT NOT NULL,
    asset_source TEXT NOT NULL CHECK(asset_source IN ('bitgarth_catalog', 'coingecko_discovery')),
    precision_source TEXT NOT NULL CHECK(precision_source IN ('bitgarth_catalog', 'coingecko_platform', 'user_override', 'user_default')),
    coingecko_platform_id TEXT,
    provider_platform_asset_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO manual_asset_accounts_new
    (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
     unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
     precision_source, coingecko_platform_id, provider_platform_asset_ref,
     created_at, updated_at)
SELECT
    id,
    wallet_id,
    label,
    label_key,
    asset_id,
    network_id,
    decimal_precision,
    unit_code,
    symbol,
    asset_name,
    network_name,
    coingecko_id,
    'bitgarth_catalog',
    'bitgarth_catalog',
    NULL,
    NULL,
    created_at,
    updated_at
FROM manual_asset_accounts;

CREATE TABLE manual_asset_balance_assertions_new (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES manual_asset_accounts_new(id) ON DELETE CASCADE,
    asserted_on TEXT NOT NULL,
    balance_amount_hi INTEGER NOT NULL CHECK(balance_amount_hi >= 0),
    balance_amount_lo INTEGER NOT NULL CHECK(balance_amount_lo >= 0),
    entered_balance_text TEXT,
    note TEXT CHECK(length(note) <= 500),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(account_id, asserted_on)
);

INSERT INTO manual_asset_balance_assertions_new
    (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo,
     entered_balance_text, note, created_at, updated_at)
SELECT
    id,
    account_id,
    asserted_on,
    balance_amount_hi,
    balance_amount_lo,
    entered_balance_text,
    note,
    created_at,
    updated_at
FROM manual_asset_balance_assertions;

DROP TABLE manual_asset_balance_assertions;
DROP TABLE manual_asset_accounts;
ALTER TABLE manual_asset_accounts_new RENAME TO manual_asset_accounts;
ALTER TABLE manual_asset_balance_assertions_new RENAME TO manual_asset_balance_assertions;

CREATE INDEX idx_maa_wallet ON manual_asset_accounts(wallet_id);
CREATE INDEX idx_maa_asset_instance ON manual_asset_accounts(asset_id, network_id);
CREATE UNIQUE INDEX idx_maa_label_key ON manual_asset_accounts(wallet_id, label_key);
CREATE INDEX idx_maba_account_asserted_on
ON manual_asset_balance_assertions(account_id, asserted_on DESC);
