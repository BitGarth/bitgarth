ALTER TABLE transaction_sync_state
    ADD COLUMN etherscan_backfill_end_block INTEGER;

UPDATE transaction_sync_state
   SET etherscan_backfill_start_block = NULL;
