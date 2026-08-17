CREATE TABLE historical_price_attempts (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(id)) > 0),
    provider TEXT NOT NULL CHECK (provider IN ('coingecko')),
    asset_id TEXT NOT NULL CHECK (length(trim(asset_id)) > 0),
    from_date TEXT NOT NULL CHECK (length(trim(from_date)) > 0),
    to_date TEXT NOT NULL CHECK (length(trim(to_date)) > 0),
    status TEXT NOT NULL CHECK (status IN (
        'success_with_prices',
        'success_empty',
        'transient_failure',
        'rate_limited',
        'permanent_failure'
    )),
    attempted_at TEXT NOT NULL,
    rows_returned INTEGER NOT NULL CHECK (rows_returned >= 0),
    next_retry_after TEXT,
    error_code TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (from_date <= to_date),
    UNIQUE(provider, asset_id, from_date, to_date)
);

CREATE INDEX idx_historical_price_attempts_scheduler
    ON historical_price_attempts (
        provider,
        asset_id,
        from_date,
        to_date,
        status
    );

CREATE INDEX idx_historical_price_attempts_latest
    ON historical_price_attempts (
        provider,
        asset_id,
        attempted_at DESC,
        from_date,
        to_date
    );

CREATE INDEX idx_historical_price_attempts_retry_cooldown
    ON historical_price_attempts (
        provider,
        next_retry_after,
        status
    )
    WHERE next_retry_after IS NOT NULL;
