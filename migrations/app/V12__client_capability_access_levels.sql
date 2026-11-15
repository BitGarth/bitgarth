-- Paired clients may now be granted `read` (all portfolio views) or `full`
-- (read plus changes) in addition to the legacy `balances_read` level.
-- SQLite cannot alter a CHECK constraint, so rebuild the table in place.
CREATE TABLE client_capabilities_v12 (
    capability_id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    key_verifier BLOB NOT NULL UNIQUE CHECK (length(key_verifier) = 32),
    wrapped_dek BLOB,
    wrap_nonce BLOB,
    permission TEXT NOT NULL CHECK (permission IN ('balances_read', 'read', 'full')),
    created_at TEXT NOT NULL,
    expires_at TEXT,
    last_used_at TEXT,
    revoked_at TEXT,
    CHECK (
        (wrapped_dek IS NULL AND wrap_nonce IS NULL)
        OR (wrapped_dek IS NOT NULL AND wrap_nonce IS NOT NULL)
    )
);

INSERT INTO client_capabilities_v12 (
    capability_id, user_id, key_verifier, wrapped_dek, wrap_nonce,
    permission, created_at, expires_at, last_used_at, revoked_at
)
SELECT
    capability_id, user_id, key_verifier, wrapped_dek, wrap_nonce,
    permission, created_at, expires_at, last_used_at, revoked_at
FROM client_capabilities;

DROP TABLE client_capabilities;
ALTER TABLE client_capabilities_v12 RENAME TO client_capabilities;

CREATE INDEX idx_client_capabilities_user_id
    ON client_capabilities(user_id);

CREATE INDEX idx_client_capabilities_active_expiry
    ON client_capabilities(expires_at)
    WHERE revoked_at IS NULL AND wrapped_dek IS NOT NULL;
