-- Align account transaction ledger indexes with paging query patterns.
DROP INDEX IF EXISTS idx_account_tx_ledger_pending_order;
DROP INDEX IF EXISTS idx_account_tx_ledger_confirmed_order;

CREATE INDEX IF NOT EXISTS idx_account_tx_ledger_account_status
    ON account_transaction_ledger(account_id, status);
CREATE INDEX IF NOT EXISTS idx_account_tx_ledger_pending_page
    ON account_transaction_ledger(
        account_id,
        first_seen_at,
        COALESCE(nonce, 9223372036854775807),
        tx_hash
    )
    WHERE status IN ('pending', 'dropped', 'failed');
CREATE INDEX IF NOT EXISTS idx_account_tx_ledger_confirmed_page
    ON account_transaction_ledger(
        account_id,
        occurred_at,
        COALESCE(block_height, 9223372036854775807),
        COALESCE(nonce, 9223372036854775807),
        COALESCE(min_transfer_index, 9223372036854775807),
        tx_hash
    )
    WHERE status = 'confirmed';
