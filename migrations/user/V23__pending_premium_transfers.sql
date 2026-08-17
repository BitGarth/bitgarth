CREATE TABLE pending_premium_transfers (
    id TEXT PRIMARY KEY CHECK(length(id) = 26),
    source_file_name TEXT NOT NULL,
    imported_at TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN (
        'pending_confirmation',
        'retryable_failure',
        'non_retryable_failure',
        'completed'
    )),
    imported_management_secret TEXT NOT NULL,
    imported_active_token TEXT NULL,
    imported_token_id TEXT NULL CHECK(imported_token_id IS NULL OR length(imported_token_id) = 26),
    imported_subscription_subject_id TEXT NULL CHECK(imported_subscription_subject_id IS NULL OR length(imported_subscription_subject_id) = 26),
    imported_subscription_valid_until TEXT NULL,
    imported_token_expires_at TEXT NULL,
    imported_token_issued_at TEXT NULL,
    last_attempt_at TEXT NULL,
    last_error_code TEXT NULL,
    last_error_message TEXT NULL,
    completed_at TEXT NULL
);
