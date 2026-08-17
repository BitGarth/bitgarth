CREATE TABLE payment_order_history (
    order_id TEXT PRIMARY KEY CHECK(length(order_id) = 26),
    product_tier TEXT NOT NULL CHECK(product_tier IN ('premium')),
    order_amount_minor_units INTEGER NOT NULL CHECK(order_amount_minor_units > 0),
    order_currency TEXT NOT NULL,
    order_display_scale INTEGER NOT NULL CHECK(order_display_scale >= 0),
    status TEXT NOT NULL CHECK(status IN ('pending', 'paid', 'expired', 'failed', 'canceled')),
    paid_at TEXT NULL,
    imported_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_payment_order_history_status_updated
ON payment_order_history(status, updated_at DESC);
