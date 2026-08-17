CREATE TABLE asset_price_sources (
    subject_type TEXT NOT NULL CHECK (subject_type IN ('native_asset', 'custom_unit_code')),
    subject_id TEXT NOT NULL CHECK (length(trim(subject_id)) > 0),
    provider TEXT NOT NULL CHECK (provider IN ('coingecko')),
    provider_asset_id TEXT NOT NULL CHECK (length(trim(provider_asset_id)) > 0),
    provider_platform TEXT,
    chain_id TEXT,
    contract_address TEXT,
    mapping_status TEXT NOT NULL CHECK (mapping_status IN ('confirmed', 'guessed', 'ambiguous', 'rejected')),
    mapping_confidence TEXT,
    last_checked_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (subject_type, subject_id, provider, provider_asset_id)
);

CREATE INDEX idx_asset_price_sources_provider_asset
    ON asset_price_sources(provider, provider_asset_id);

CREATE INDEX idx_asset_price_sources_status
    ON asset_price_sources(provider, mapping_status);

CREATE TABLE coingecko_asset_catalog (
    provider_asset_id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(provider_asset_id)) > 0),
    symbol TEXT NOT NULL,
    normalized_symbol TEXT NOT NULL,
    name TEXT NOT NULL,
    platforms_json TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'inactive')),
    retrieved_at TEXT NOT NULL
);

CREATE INDEX idx_coingecko_asset_catalog_symbol
    ON coingecko_asset_catalog(normalized_symbol);

CREATE TABLE price_points (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(id)) > 0),
    subject_type TEXT NOT NULL CHECK (subject_type IN ('native_asset', 'custom_unit_code')),
    subject_id TEXT NOT NULL CHECK (length(trim(subject_id)) > 0),
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
    UNIQUE(subject_type, subject_id, quote_currency, provider, price_time_utc, granularity, price_kind)
);

CREATE INDEX idx_price_points_lookup
    ON price_points(subject_type, subject_id, quote_currency, price_time_utc);

CREATE INDEX idx_price_points_daily_lookup
    ON price_points(subject_type, subject_id, quote_currency, date_utc)
    WHERE date_utc IS NOT NULL;

CREATE TABLE current_price_cache (
    subject_type TEXT NOT NULL CHECK (subject_type IN ('native_asset', 'custom_unit_code')),
    subject_id TEXT NOT NULL CHECK (length(trim(subject_id)) > 0),
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
    PRIMARY KEY (subject_type, subject_id, quote_currency, provider)
);

INSERT INTO asset_price_sources (
    subject_type,
    subject_id,
    provider,
    provider_asset_id,
    provider_platform,
    chain_id,
    contract_address,
    mapping_status,
    mapping_confidence,
    last_checked_at,
    created_at,
    updated_at
) VALUES
    (
        'native_asset',
        'bitcoin',
        'coingecko',
        'bitcoin',
        NULL,
        NULL,
        NULL,
        'confirmed',
        'seeded_native_asset',
        '2026-06-06T00:00:00Z',
        '2026-06-06T00:00:00Z',
        '2026-06-06T00:00:00Z'
    ),
    (
        'native_asset',
        'ethereum',
        'coingecko',
        'ethereum',
        NULL,
        NULL,
        NULL,
        'confirmed',
        'seeded_native_asset',
        '2026-06-06T00:00:00Z',
        '2026-06-06T00:00:00Z',
        '2026-06-06T00:00:00Z'
    );
