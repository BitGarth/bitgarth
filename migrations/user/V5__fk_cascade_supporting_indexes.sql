-- Foreign-key cascade pruning on large wallets can issue many parent-row deletes.
-- Add the missing child-key index so SQLite does indexed FK checks instead of table scans.
-- Note: transaction_inputs(tx_id) and transaction_outputs(tx_id) are already covered by
-- existing unique indexes on (tx_id, input_index) and (tx_id, output_index).
CREATE INDEX IF NOT EXISTS idx_account_tx_ledger_chain_tx_id
    ON account_transaction_ledger(chain_transaction_id);
