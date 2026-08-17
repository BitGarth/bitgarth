-- Chain transaction/unowned-transfer cleanup is now handled explicitly in
-- write/delete transactions.
-- Drop legacy row-level triggers to avoid repeated per-row work on large deletes.
DROP TRIGGER IF EXISTS trg_chain_tx_prune_after_input_delete;
DROP TRIGGER IF EXISTS trg_chain_tx_prune_after_output_delete;
DROP TRIGGER IF EXISTS trg_chain_tx_prune_after_transfer_delete;
DROP TRIGGER IF EXISTS trg_account_transfers_skip_unowned_insert;
DROP TRIGGER IF EXISTS trg_account_transfers_delete_when_unowned;
