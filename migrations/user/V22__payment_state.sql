CREATE TABLE payment_subject (
    id TEXT PRIMARY KEY CHECK (id = 'premium'),
    entitlement_holder_id TEXT NOT NULL CHECK(length(entitlement_holder_id) = 26),
    management_secret TEXT NULL,
    active_token TEXT NULL,
    token_id TEXT NULL,
    subscription_subject_id TEXT NULL CHECK(subscription_subject_id IS NULL OR length(subscription_subject_id) = 26),
    subscription_valid_until TEXT NULL,
    token_expires_at TEXT NULL,
    token_issued_at TEXT NULL,
    last_refresh_at TEXT NULL,
    last_refresh_status TEXT NULL CHECK(last_refresh_status IS NULL OR last_refresh_status IN ('active', 'revoked', 'unavailable', 'error')),
    updated_at TEXT NOT NULL
);

CREATE TABLE payment_orders (
    order_id TEXT PRIMARY KEY CHECK(length(order_id) = 26),
    order_secret TEXT NOT NULL,
    product_tier TEXT NOT NULL CHECK(product_tier IN ('premium')),
    order_amount_minor_units INTEGER NOT NULL CHECK(order_amount_minor_units > 0),
    order_currency TEXT NOT NULL,
    order_display_scale INTEGER NOT NULL CHECK(order_display_scale >= 0),
    status TEXT NOT NULL CHECK(status IN ('pending', 'paid', 'expired', 'failed', 'canceled')),
    paid_at TEXT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_payment_orders_status_updated
ON payment_orders(status, updated_at DESC);
