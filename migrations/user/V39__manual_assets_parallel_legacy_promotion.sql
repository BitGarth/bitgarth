CREATE TABLE manual_asset_accounts (
    id TEXT PRIMARY KEY,
    wallet_id TEXT NOT NULL REFERENCES wallets(id) ON DELETE CASCADE,
    label TEXT NOT NULL CHECK(length(label) <= 255),
    label_key TEXT NOT NULL,
    asset_id TEXT NOT NULL,
    network_id TEXT NOT NULL,
    namespace_type TEXT NOT NULL CHECK(namespace_type IN ('native', 'erc20')),
    namespace_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_maa_wallet ON manual_asset_accounts(wallet_id);
CREATE INDEX idx_maa_asset_instance
ON manual_asset_accounts(asset_id, network_id, namespace_type, namespace_ref);
CREATE UNIQUE INDEX idx_maa_label_key ON manual_asset_accounts(wallet_id, label_key);

CREATE TABLE manual_asset_balance_assertions (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES manual_asset_accounts(id) ON DELETE CASCADE,
    asserted_on TEXT NOT NULL,
    balance_amount_hi INTEGER NOT NULL CHECK(balance_amount_hi >= 0),
    balance_amount_lo INTEGER NOT NULL CHECK(balance_amount_lo >= 0),
    entered_balance_text TEXT,
    note TEXT CHECK(length(note) <= 500),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(account_id, asserted_on)
);

CREATE INDEX idx_maba_account_asserted_on
ON manual_asset_balance_assertions(account_id, asserted_on DESC);

ALTER TABLE user_price_overrides RENAME TO custom_user_price_overrides;

DROP INDEX idx_user_price_overrides_lookup;

CREATE INDEX idx_custom_user_price_overrides_lookup
ON custom_user_price_overrides(subject_type, subject_id, quote_currency, price_time_utc);

CREATE TABLE user_price_overrides (
    id TEXT PRIMARY KEY,
    asset_id TEXT NOT NULL,
    quote_currency TEXT NOT NULL,
    price_time_utc TEXT NOT NULL,
    price TEXT NOT NULL,
    source_note TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(asset_id, quote_currency, price_time_utc)
);

CREATE INDEX idx_user_price_overrides_lookup
ON user_price_overrides(asset_id, quote_currency, price_time_utc);

INSERT INTO user_price_overrides
    (id, asset_id, quote_currency, price_time_utc, price, source_note, created_at, updated_at)
SELECT id, subject_id, quote_currency, price_time_utc, price, source_note, created_at, updated_at
FROM custom_user_price_overrides
WHERE subject_type = 'native_asset'
  AND subject_id IN ('bitcoin', 'ethereum');

DELETE FROM custom_user_price_overrides
WHERE subject_type = 'native_asset'
  AND subject_id IN ('bitcoin', 'ethereum');

CREATE TEMP TABLE manual_asset_promotion_map (
    unit_code TEXT PRIMARY KEY,
    asset_id TEXT NOT NULL,
    network_id TEXT NOT NULL,
    namespace_type TEXT NOT NULL,
    namespace_ref TEXT,
    catalog_display_scale INTEGER NOT NULL
);

INSERT INTO manual_asset_promotion_map
    (unit_code, asset_id, network_id, namespace_type, namespace_ref, catalog_display_scale)
VALUES
    ('ADA', 'cardano', 'cardano-mainnet', 'native', NULL, 6),
    ('USDC', 'usd-coin', 'ethereum-mainnet', 'erc20', '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48', 6);

CREATE TEMP TABLE promotable_custom_asset_accounts AS
SELECT
    a.id,
    a.wallet_id,
    a.label,
    a.label_key,
    UPPER(TRIM(a.unit_code)) AS unit_code,
    a.display_scale,
    a.created_at,
    a.updated_at,
    m.asset_id,
    m.network_id,
    m.namespace_type,
    m.namespace_ref,
    m.catalog_display_scale,
    (m.catalog_display_scale - a.display_scale) AS delta
FROM custom_asset_accounts a
JOIN manual_asset_promotion_map m ON m.unit_code = UPPER(TRIM(a.unit_code))
WHERE a.display_scale <= m.catalog_display_scale
  AND (m.catalog_display_scale - a.display_scale) BETWEEN 0 AND 18
  AND NOT EXISTS (
      SELECT 1
      FROM custom_asset_balance_assertions b
      WHERE b.account_id = a.id
        AND b.balance_amount_hi > (
            9223372036854775807 -
            CASE (m.catalog_display_scale - a.display_scale)
                WHEN 0 THEN 0
                WHEN 1 THEN b.balance_amount_lo / 100000000000000000
                WHEN 2 THEN b.balance_amount_lo / 10000000000000000
                WHEN 3 THEN b.balance_amount_lo / 1000000000000000
                WHEN 4 THEN b.balance_amount_lo / 100000000000000
                WHEN 5 THEN b.balance_amount_lo / 10000000000000
                WHEN 6 THEN b.balance_amount_lo / 1000000000000
                WHEN 7 THEN b.balance_amount_lo / 100000000000
                WHEN 8 THEN b.balance_amount_lo / 10000000000
                WHEN 9 THEN b.balance_amount_lo / 1000000000
                WHEN 10 THEN b.balance_amount_lo / 100000000
                WHEN 11 THEN b.balance_amount_lo / 10000000
                WHEN 12 THEN b.balance_amount_lo / 1000000
                WHEN 13 THEN b.balance_amount_lo / 100000
                WHEN 14 THEN b.balance_amount_lo / 10000
                WHEN 15 THEN b.balance_amount_lo / 1000
                WHEN 16 THEN b.balance_amount_lo / 100
                WHEN 17 THEN b.balance_amount_lo / 10
                WHEN 18 THEN b.balance_amount_lo
            END
        ) / CASE (m.catalog_display_scale - a.display_scale)
                WHEN 0 THEN 1
                WHEN 1 THEN 10
                WHEN 2 THEN 100
                WHEN 3 THEN 1000
                WHEN 4 THEN 10000
                WHEN 5 THEN 100000
                WHEN 6 THEN 1000000
                WHEN 7 THEN 10000000
                WHEN 8 THEN 100000000
                WHEN 9 THEN 1000000000
                WHEN 10 THEN 10000000000
                WHEN 11 THEN 100000000000
                WHEN 12 THEN 1000000000000
                WHEN 13 THEN 10000000000000
                WHEN 14 THEN 100000000000000
                WHEN 15 THEN 1000000000000000
                WHEN 16 THEN 10000000000000000
                WHEN 17 THEN 100000000000000000
                WHEN 18 THEN 1000000000000000000
            END
  );

INSERT INTO manual_asset_accounts
    (id, wallet_id, label, label_key, asset_id, network_id, namespace_type, namespace_ref, created_at, updated_at)
SELECT
    id, wallet_id, label, label_key, asset_id, network_id, namespace_type, namespace_ref, created_at, updated_at
FROM promotable_custom_asset_accounts;

INSERT INTO manual_asset_balance_assertions
    (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo, entered_balance_text, note, created_at, updated_at)
SELECT
    b.id,
    b.account_id,
    b.asserted_on,
    b.balance_amount_hi * CASE p.delta
        WHEN 0 THEN 1 WHEN 1 THEN 10 WHEN 2 THEN 100 WHEN 3 THEN 1000
        WHEN 4 THEN 10000 WHEN 5 THEN 100000 WHEN 6 THEN 1000000
        WHEN 7 THEN 10000000 WHEN 8 THEN 100000000 WHEN 9 THEN 1000000000
        WHEN 10 THEN 10000000000 WHEN 11 THEN 100000000000
        WHEN 12 THEN 1000000000000 WHEN 13 THEN 10000000000000
        WHEN 14 THEN 100000000000000 WHEN 15 THEN 1000000000000000
        WHEN 16 THEN 10000000000000000 WHEN 17 THEN 100000000000000000
        WHEN 18 THEN 1000000000000000000
    END +
    CASE p.delta
        WHEN 0 THEN 0 WHEN 1 THEN b.balance_amount_lo / 100000000000000000
        WHEN 2 THEN b.balance_amount_lo / 10000000000000000
        WHEN 3 THEN b.balance_amount_lo / 1000000000000000
        WHEN 4 THEN b.balance_amount_lo / 100000000000000
        WHEN 5 THEN b.balance_amount_lo / 10000000000000
        WHEN 6 THEN b.balance_amount_lo / 1000000000000
        WHEN 7 THEN b.balance_amount_lo / 100000000000
        WHEN 8 THEN b.balance_amount_lo / 10000000000
        WHEN 9 THEN b.balance_amount_lo / 1000000000
        WHEN 10 THEN b.balance_amount_lo / 100000000
        WHEN 11 THEN b.balance_amount_lo / 10000000
        WHEN 12 THEN b.balance_amount_lo / 1000000
        WHEN 13 THEN b.balance_amount_lo / 100000
        WHEN 14 THEN b.balance_amount_lo / 10000
        WHEN 15 THEN b.balance_amount_lo / 1000
        WHEN 16 THEN b.balance_amount_lo / 100
        WHEN 17 THEN b.balance_amount_lo / 10
        WHEN 18 THEN b.balance_amount_lo
    END,
    (b.balance_amount_lo % CASE p.delta
        WHEN 0 THEN 1000000000000000000 WHEN 1 THEN 100000000000000000
        WHEN 2 THEN 10000000000000000 WHEN 3 THEN 1000000000000000
        WHEN 4 THEN 100000000000000 WHEN 5 THEN 10000000000000
        WHEN 6 THEN 1000000000000 WHEN 7 THEN 100000000000
        WHEN 8 THEN 10000000000 WHEN 9 THEN 1000000000
        WHEN 10 THEN 100000000 WHEN 11 THEN 10000000
        WHEN 12 THEN 1000000 WHEN 13 THEN 100000
        WHEN 14 THEN 10000 WHEN 15 THEN 1000
        WHEN 16 THEN 100 WHEN 17 THEN 10 WHEN 18 THEN 1
    END) * CASE p.delta
        WHEN 0 THEN 1 WHEN 1 THEN 10 WHEN 2 THEN 100 WHEN 3 THEN 1000
        WHEN 4 THEN 10000 WHEN 5 THEN 100000 WHEN 6 THEN 1000000
        WHEN 7 THEN 10000000 WHEN 8 THEN 100000000 WHEN 9 THEN 1000000000
        WHEN 10 THEN 10000000000 WHEN 11 THEN 100000000000
        WHEN 12 THEN 1000000000000 WHEN 13 THEN 10000000000000
        WHEN 14 THEN 100000000000000 WHEN 15 THEN 1000000000000000
        WHEN 16 THEN 10000000000000000 WHEN 17 THEN 100000000000000000
        WHEN 18 THEN 1000000000000000000
    END,
    b.entered_balance_text,
    b.note,
    b.created_at,
    b.updated_at
FROM custom_asset_balance_assertions b
JOIN promotable_custom_asset_accounts p ON p.id = b.account_id;

INSERT INTO user_price_overrides
    (id, asset_id, quote_currency, price_time_utc, price, source_note, created_at, updated_at)
SELECT
    o.id,
    p.asset_id,
    o.quote_currency,
    o.price_time_utc,
    o.price,
    o.source_note,
    o.created_at,
    o.updated_at
FROM custom_user_price_overrides o
JOIN manual_asset_promotion_map p ON p.unit_code = UPPER(TRIM(o.subject_id))
WHERE o.subject_type = 'custom_unit_code'
  AND UPPER(TRIM(o.subject_id)) IN (SELECT unit_code FROM promotable_custom_asset_accounts)
ON CONFLICT(asset_id, quote_currency, price_time_utc)
DO UPDATE SET
    price = excluded.price,
    source_note = excluded.source_note,
    updated_at = excluded.updated_at;

DELETE FROM custom_user_price_overrides
WHERE subject_type = 'custom_unit_code'
  AND UPPER(TRIM(subject_id)) IN (SELECT unit_code FROM promotable_custom_asset_accounts)
  AND NOT EXISTS (
      SELECT 1
      FROM custom_asset_accounts a
      WHERE UPPER(TRIM(a.unit_code)) = UPPER(TRIM(custom_user_price_overrides.subject_id))
        AND a.id NOT IN (SELECT id FROM promotable_custom_asset_accounts)
  );

DELETE FROM custom_asset_balance_assertions
WHERE account_id IN (SELECT id FROM promotable_custom_asset_accounts);

DELETE FROM custom_asset_accounts
WHERE id IN (SELECT id FROM promotable_custom_asset_accounts);

DROP TABLE promotable_custom_asset_accounts;
DROP TABLE manual_asset_promotion_map;
