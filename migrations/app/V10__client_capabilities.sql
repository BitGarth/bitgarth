CREATE TABLE client_capabilities (
    capability_id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    key_verifier BLOB NOT NULL UNIQUE CHECK (length(key_verifier) = 32),
    wrapped_dek BLOB,
    wrap_nonce BLOB,
    permission TEXT NOT NULL CHECK (permission = 'balances_read'),
    created_at TEXT NOT NULL,
    expires_at TEXT,
    last_used_at TEXT,
    revoked_at TEXT,
    CHECK (
        (wrapped_dek IS NULL AND wrap_nonce IS NULL)
        OR (wrapped_dek IS NOT NULL AND wrap_nonce IS NOT NULL)
    )
);

CREATE INDEX idx_client_capabilities_user_id
    ON client_capabilities(user_id);

CREATE INDEX idx_client_capabilities_active_expiry
    ON client_capabilities(expires_at)
    WHERE revoked_at IS NULL AND wrapped_dek IS NOT NULL;
