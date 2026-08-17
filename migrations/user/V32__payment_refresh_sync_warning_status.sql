CREATE TABLE payment_subject_new (
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
    last_refresh_status TEXT NULL CHECK(last_refresh_status IS NULL OR last_refresh_status IN ('active', 'revoked', 'unavailable', 'error', 'sync_warning')),
    updated_at TEXT NOT NULL,
    entitlement_tier TEXT NULL,
    capability_set_id TEXT NULL,
    capabilities_json TEXT NULL,
    last_capability_refresh_at TEXT NULL,
    last_successful_capability_refresh_at TEXT NULL
);

INSERT INTO payment_subject_new (
    id,
    entitlement_holder_id,
    management_secret,
    active_token,
    token_id,
    subscription_subject_id,
    subscription_valid_until,
    token_expires_at,
    token_issued_at,
    last_refresh_at,
    last_refresh_status,
    updated_at,
    entitlement_tier,
    capability_set_id,
    capabilities_json,
    last_capability_refresh_at,
    last_successful_capability_refresh_at
)
SELECT
    id,
    entitlement_holder_id,
    management_secret,
    active_token,
    token_id,
    subscription_subject_id,
    subscription_valid_until,
    token_expires_at,
    token_issued_at,
    last_refresh_at,
    last_refresh_status,
    updated_at,
    entitlement_tier,
    capability_set_id,
    capabilities_json,
    last_capability_refresh_at,
    last_successful_capability_refresh_at
FROM payment_subject;

DROP TABLE payment_subject;

ALTER TABLE payment_subject_new RENAME TO payment_subject;
