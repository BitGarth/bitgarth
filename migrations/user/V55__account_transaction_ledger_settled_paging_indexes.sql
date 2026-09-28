DROP INDEX idx_account_tx_ledger_pending_page;
DROP INDEX idx_account_tx_ledger_confirmed_page;

CREATE INDEX idx_account_tx_ledger_pending_page
ON account_transaction_ledger(
    account_id, first_seen_at,
    COALESCE(nonce, 9223372036854775807), tx_hash
) WHERE status IN ('pending', 'dropped');

CREATE INDEX idx_account_tx_ledger_confirmed_page
ON account_transaction_ledger(
    account_id, occurred_at,
    COALESCE(block_height, 9223372036854775807),
    COALESCE(nonce, 9223372036854775807),
    COALESCE(min_transfer_index, 9223372036854775807), tx_hash
) WHERE status IN ('confirmed', 'failed');
