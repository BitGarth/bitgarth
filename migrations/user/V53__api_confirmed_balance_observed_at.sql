ALTER TABLE transaction_sync_state
    ADD COLUMN api_confirmed_balance_observed_at TEXT;

UPDATE transaction_sync_state
SET api_confirmed_balance_observed_at = last_completed_at
WHERE api_confirmed_balance_hi IS NOT NULL
  AND api_confirmed_balance_lo IS NOT NULL;
