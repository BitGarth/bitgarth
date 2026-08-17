CREATE TABLE payment_token_history (
    token_id TEXT PRIMARY KEY CHECK(length(token_id) = 26),
    subscription_subject_id TEXT NOT NULL CHECK(length(subscription_subject_id) = 26),
    active_token TEXT NOT NULL,
    entitlement_tier TEXT NOT NULL,
    subscription_valid_until TEXT NOT NULL,
    token_expires_at TEXT NOT NULL,
    token_issued_at TEXT NOT NULL,
    capability_set_id TEXT NULL,
    capabilities_json TEXT NULL,
    status TEXT NOT NULL CHECK(status IN ('active', 'inactive', 'revoked', 'superseded', 'expired', 'invalidated')),
    status_reason TEXT NULL,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    deactivated_at TEXT NULL
);

CREATE UNIQUE INDEX idx_payment_token_history_one_active ON payment_token_history(status)
WHERE status = 'active';

ALTER TABLE payment_subject ADD COLUMN active_token_history_id TEXT NULL;

INSERT INTO payment_token_history (
    token_id,
    subscription_subject_id,
    active_token,
    entitlement_tier,
    subscription_valid_until,
    token_expires_at,
    token_issued_at,
    capability_set_id,
    capabilities_json,
    status,
    status_reason,
    first_seen_at,
    last_seen_at,
    deactivated_at
)
SELECT
    token_id,
    subscription_subject_id,
    active_token,
    entitlement_tier,
    subscription_valid_until,
    token_expires_at,
    token_issued_at,
    capability_set_id,
    capabilities_json,
    'active',
    NULL,
    COALESCE(token_issued_at, updated_at),
    COALESCE(token_issued_at, updated_at),
    NULL
FROM payment_subject
WHERE id = 'premium'
    AND active_token IS NOT NULL
    AND length(trim(active_token)) > 0
    AND token_id IS NOT NULL
    AND length(token_id) = 26
    AND subscription_subject_id IS NOT NULL
    AND length(subscription_subject_id) = 26
    AND entitlement_tier IS NOT NULL
    AND subscription_valid_until IS NOT NULL
    AND token_expires_at IS NOT NULL
    AND token_issued_at IS NOT NULL;

UPDATE payment_subject SET active_token_history_id = (
    SELECT token_id FROM payment_token_history WHERE status = 'active' LIMIT 1
)
WHERE id = 'premium'
    AND active_token_history_id IS NULL
    AND EXISTS (SELECT 1 FROM payment_token_history WHERE status = 'active');
