DROP TABLE IF EXISTS asset_price_sources;

CREATE TABLE price_points_v1 (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(id)) > 0),
    asset_id TEXT NOT NULL CHECK (length(trim(asset_id)) > 0),
    quote_currency TEXT NOT NULL CHECK (length(trim(quote_currency)) > 0),
    price_time_utc TEXT NOT NULL,
    date_utc TEXT,
    price TEXT NOT NULL CHECK (length(trim(price)) > 0),
    provider TEXT NOT NULL CHECK (provider IN ('coingecko')),
    provider_asset_id TEXT,
    provider_quote_id TEXT,
    granularity TEXT NOT NULL CHECK (granularity IN ('daily', 'hourly', 'point')),
    price_kind TEXT NOT NULL CHECK (price_kind IN ('daily_point', 'current_snapshot', 'transaction_time')),
    license_scope TEXT NOT NULL CHECK (license_scope IN ('public_keyless', 'coingecko_pro_key')),
    retrieved_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(asset_id, quote_currency, provider, price_time_utc, granularity, price_kind)
);

INSERT INTO price_points_v1 (
    id,
    asset_id,
    quote_currency,
    price_time_utc,
    date_utc,
    price,
    provider,
    provider_asset_id,
    provider_quote_id,
    granularity,
    price_kind,
    license_scope,
    retrieved_at,
    created_at,
    updated_at
)
SELECT
    id,
    subject_id,
    quote_currency,
    price_time_utc,
    date_utc,
    price,
    provider,
    provider_asset_id,
    provider_quote_id,
    granularity,
    price_kind,
    license_scope,
    retrieved_at,
    created_at,
    updated_at
FROM price_points;

DROP TABLE price_points;
ALTER TABLE price_points_v1 RENAME TO price_points;

CREATE INDEX idx_price_points_lookup
    ON price_points(asset_id, quote_currency, price_time_utc);

CREATE INDEX idx_price_points_daily_lookup
    ON price_points(asset_id, quote_currency, date_utc)
    WHERE date_utc IS NOT NULL;

CREATE TABLE current_price_cache_v1 (
    asset_id TEXT NOT NULL CHECK (length(trim(asset_id)) > 0),
    quote_currency TEXT NOT NULL CHECK (length(trim(quote_currency)) > 0),
    provider TEXT NOT NULL CHECK (provider IN ('coingecko')),
    provider_asset_id TEXT,
    provider_quote_id TEXT,
    price TEXT NOT NULL CHECK (length(trim(price)) > 0),
    observed_at TEXT,
    retrieved_at TEXT NOT NULL,
    license_scope TEXT NOT NULL CHECK (license_scope IN ('public_keyless', 'coingecko_pro_key')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (asset_id, quote_currency, provider)
);

INSERT INTO current_price_cache_v1 (
    asset_id,
    quote_currency,
    provider,
    provider_asset_id,
    provider_quote_id,
    price,
    observed_at,
    retrieved_at,
    license_scope,
    created_at,
    updated_at
)
SELECT
    subject_id,
    quote_currency,
    provider,
    provider_asset_id,
    provider_quote_id,
    price,
    observed_at,
    retrieved_at,
    license_scope,
    created_at,
    updated_at
FROM current_price_cache;

DROP TABLE current_price_cache;
ALTER TABLE current_price_cache_v1 RENAME TO current_price_cache;
