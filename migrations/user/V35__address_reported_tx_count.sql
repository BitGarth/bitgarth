-- Integration-reported total transaction count for an address (e.g. mempool
-- chain_stats.tx_count + mempool_stats.tx_count). NULL when no integration has
-- reported a count for this address yet. Distinct from mempool_expected_tx_count,
-- which is backfill-scoped and cleared when backfill completes.
ALTER TABLE transaction_sync_state
    ADD COLUMN reported_tx_count INTEGER;
