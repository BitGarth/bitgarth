CREATE TEMP TABLE manual_asset_snapshot_map (
    asset_id TEXT NOT NULL,
    network_id TEXT NOT NULL,
    decimal_precision INTEGER NOT NULL,
    unit_code TEXT NOT NULL,
    symbol TEXT,
    asset_name TEXT NOT NULL,
    network_name TEXT NOT NULL,
    coingecko_id TEXT NOT NULL,
    PRIMARY KEY(asset_id, network_id)
);

INSERT INTO manual_asset_snapshot_map
    (asset_id, network_id, decimal_precision, unit_code, symbol, asset_name, network_name, coingecko_id)
VALUES
    ('ripple', 'ripple-xrp-mainnet', 6, 'XRP', NULL, 'Ripple', 'Ripple', 'ripple'),
    ('binancecoin', 'bnb-smart-chain-mainnet', 18, 'BNB', NULL, 'Binance Coin', 'BNB Smart Chain', 'binancecoin'),
    ('solana', 'solana-mainnet', 9, 'SOL', NULL, 'Solana', 'Solana', 'solana'),
    ('usd-coin', 'ethereum-mainnet', 6, 'USDC', NULL, 'USDC on Ethereum', 'Ethereum', 'usd-coin'),
    ('usd-coin', 'polygon-mainnet', 6, 'USDC', NULL, 'USDC on Polygon', 'Polygon', 'usd-coin'),
    ('usd-coin', 'algorand-mainnet', 6, 'USDC', NULL, 'USDC on Algorand', 'Algorand', 'usd-coin'),
    ('cardano', 'cardano-mainnet', 6, 'ADA', '₳', 'Cardano', 'Cardano', 'cardano'),
    ('dogecoin', 'dogecoin-mainnet', 8, 'DOGE', NULL, 'Dogecoin', 'Dogecoin', 'dogecoin'),
    ('tron', 'tron-mainnet', 6, 'TRX', NULL, 'TRON', 'Tron', 'tron'),
    ('zcash', 'zcash-mainnet', 8, 'ZEC', 'ZEC', 'Zcash', 'Zcash', 'zcash'),
    ('monero', 'monero-mainnet', 12, 'XMR', NULL, 'Monero', 'Monero', 'monero'),
    ('uniswap', 'arbitrum-one', 18, 'UNI', NULL, 'Uniswap on Arbitrum One', 'Arbitrum One', 'uniswap'),
    ('uniswap', 'avalanche-c-chain', 18, 'UNI', NULL, 'Uniswap on Avalanche C-Chain', 'Avalanche C-Chain', 'uniswap'),
    ('uniswap', 'bnb-smart-chain-mainnet', 18, 'UNI', NULL, 'Uniswap on BNB Smart Chain', 'BNB Smart Chain', 'uniswap'),
    ('uniswap', 'ethereum-mainnet', 18, 'UNI', NULL, 'Uniswap on Ethereum', 'Ethereum', 'uniswap'),
    ('uniswap', 'optimism-mainnet', 18, 'UNI', NULL, 'Uniswap on Optimism', 'Optimism', 'uniswap'),
    ('uniswap', 'polygon-mainnet', 18, 'UNI', NULL, 'Uniswap on Polygon', 'Polygon', 'uniswap'),
    ('tezos', 'tezos-mainnet', 6, 'XTZ', NULL, 'Tezos', 'Tezos', 'tezos'),
    ('algorand', 'algorand-mainnet', 6, 'ALGO', NULL, 'Algorand', 'Algorand', 'algorand');

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
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO manual_asset_accounts_new
    (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision, unit_code, symbol, asset_name, network_name, coingecko_id, created_at, updated_at)
SELECT
    a.id,
    a.wallet_id,
    a.label,
    a.label_key,
    a.asset_id,
    a.network_id,
    m.decimal_precision,
    m.unit_code,
    m.symbol,
    m.asset_name,
    m.network_name,
    m.coingecko_id,
    a.created_at,
    a.updated_at
FROM manual_asset_accounts a
LEFT JOIN manual_asset_snapshot_map m
  ON m.asset_id = a.asset_id
 AND m.network_id = a.network_id;

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
    (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo, entered_balance_text, note, created_at, updated_at)
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

DROP TABLE manual_asset_snapshot_map;
