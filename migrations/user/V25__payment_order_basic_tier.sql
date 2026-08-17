CREATE TABLE payment_orders_new (
    order_id TEXT PRIMARY KEY CHECK(length(order_id) = 26),
    order_secret TEXT NOT NULL,
    product_tier TEXT NOT NULL CHECK(product_tier IN ('basic', 'premium')),
    order_amount_minor_units INTEGER NOT NULL CHECK(order_amount_minor_units > 0),
    order_currency TEXT NOT NULL,
    order_display_scale INTEGER NOT NULL CHECK(order_display_scale >= 0),
    status TEXT NOT NULL CHECK(status IN ('pending', 'paid', 'expired', 'failed', 'canceled')),
    paid_at TEXT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO payment_orders_new (
    order_id,
    order_secret,
    product_tier,
    order_amount_minor_units,
    order_currency,
    order_display_scale,
    status,
    paid_at,
    created_at,
    updated_at
)
SELECT
    order_id,
    order_secret,
    product_tier,
    order_amount_minor_units,
    order_currency,
    order_display_scale,
    status,
    paid_at,
    created_at,
    updated_at
FROM payment_orders;

DROP TABLE payment_orders;
ALTER TABLE payment_orders_new RENAME TO payment_orders;

CREATE INDEX idx_payment_orders_status_updated
ON payment_orders(status, updated_at DESC);

CREATE TABLE payment_order_history_new (
    order_id TEXT PRIMARY KEY CHECK(length(order_id) = 26),
    product_tier TEXT NOT NULL CHECK(product_tier IN ('basic', 'premium')),
    order_amount_minor_units INTEGER NOT NULL CHECK(order_amount_minor_units > 0),
    order_currency TEXT NOT NULL,
    order_display_scale INTEGER NOT NULL CHECK(order_display_scale >= 0),
    status TEXT NOT NULL CHECK(status IN ('pending', 'paid', 'expired', 'failed', 'canceled')),
    paid_at TEXT NULL,
    imported_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO payment_order_history_new (
    order_id,
    product_tier,
    order_amount_minor_units,
    order_currency,
    order_display_scale,
    status,
    paid_at,
    imported_at,
    updated_at
)
SELECT
    order_id,
    product_tier,
    order_amount_minor_units,
    order_currency,
    order_display_scale,
    status,
    paid_at,
    imported_at,
    updated_at
FROM payment_order_history;

DROP TABLE payment_order_history;
ALTER TABLE payment_order_history_new RENAME TO payment_order_history;

CREATE INDEX idx_payment_order_history_status_updated
ON payment_order_history(status, updated_at DESC);
