CREATE TABLE app_entitlement_snapshots (
    snapshot_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    source TEXT NOT NULL CHECK(source IN (
        'payment_poll',
        'payment_reconcile',
        'login_refresh',
        'payments_refresh',
        'refresh'
    )),
    entitlement_holder_id TEXT NOT NULL CHECK(length(entitlement_holder_id) = 26),
    subscription_subject_id TEXT NULL CHECK(subscription_subject_id IS NULL OR length(subscription_subject_id) = 26),
    token_id TEXT NULL,
    entitlement_tier TEXT NOT NULL,
    subscription_valid_until TEXT NULL,
    token_expires_at TEXT NULL,
    token_issued_at TEXT NULL,
    capability_set_id TEXT NULL,
    capabilities_json TEXT NULL
);

CREATE INDEX idx_app_entitlement_snapshots_user_recorded
ON app_entitlement_snapshots(user_id, recorded_at DESC);

CREATE INDEX idx_app_entitlement_snapshots_tier_recorded
ON app_entitlement_snapshots(entitlement_tier, recorded_at DESC);

CREATE UNIQUE INDEX idx_app_entitlement_snapshots_token_id
ON app_entitlement_snapshots(token_id)
WHERE token_id IS NOT NULL;
