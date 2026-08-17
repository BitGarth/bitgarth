ALTER TABLE payment_subject
ADD COLUMN entitlement_tier TEXT NULL;

ALTER TABLE payment_subject
ADD COLUMN capability_set_id TEXT NULL;

ALTER TABLE payment_subject
ADD COLUMN capabilities_json TEXT NULL;

ALTER TABLE payment_subject
ADD COLUMN last_capability_refresh_at TEXT NULL;

ALTER TABLE payment_subject
ADD COLUMN last_successful_capability_refresh_at TEXT NULL;
