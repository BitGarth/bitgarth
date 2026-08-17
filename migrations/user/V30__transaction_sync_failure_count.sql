ALTER TABLE transaction_sync_state
    ADD COLUMN consecutive_failure_count INTEGER NOT NULL DEFAULT 0;
