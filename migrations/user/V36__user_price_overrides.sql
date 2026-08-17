CREATE TABLE user_price_overrides (
    id TEXT PRIMARY KEY,
    subject_type TEXT NOT NULL CHECK (subject_type IN ('native_asset', 'custom_unit_code')),
    subject_id TEXT NOT NULL,
    quote_currency TEXT NOT NULL,
    price_time_utc TEXT NOT NULL,
    price TEXT NOT NULL,
    source_note TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(subject_type, subject_id, quote_currency, price_time_utc)
);

CREATE INDEX idx_user_price_overrides_lookup
    ON user_price_overrides(subject_type, subject_id, quote_currency, price_time_utc);
