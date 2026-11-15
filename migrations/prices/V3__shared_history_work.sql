-- This worker exclusively uses public keyless CoinGecko history.
CREATE TABLE shared_history_pairs (
    asset_id TEXT NOT NULL,
    quote_currency TEXT NOT NULL,
    provider_asset_id TEXT NOT NULL,
    recent_due INTEGER NOT NULL,
    history_due INTEGER NOT NULL,
    last_turn INTEGER NOT NULL,
    history_end TEXT,
    PRIMARY KEY(asset_id, quote_currency)
);

CREATE TABLE shared_history_deferrals (
    asset_id TEXT NOT NULL,
    quote_currency TEXT NOT NULL,
    provider_asset_id TEXT NOT NULL,
    from_date TEXT NOT NULL,
    to_date TEXT NOT NULL,
    retry_at INTEGER NOT NULL,
    -- A history limit proves that the requested start and all earlier dates
    -- are inaccessible. Remember the upper end to bisect a straddling request.
    probe_end TEXT,
    PRIMARY KEY(asset_id, quote_currency, provider_asset_id, from_date, to_date)
);

CREATE TABLE public_history_backoff (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    retry_at INTEGER NOT NULL
);
CREATE INDEX idx_price_history_pair_sources
    ON price_points(provider, asset_id, quote_currency, provider_asset_id);
