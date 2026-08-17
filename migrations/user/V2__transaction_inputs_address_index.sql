-- Add a direct address_id index to avoid full scans when deleting addresses
-- (foreign-key cascade) and when reconciling input ownership lookups.
CREATE INDEX IF NOT EXISTS idx_tx_inputs_address
    ON transaction_inputs(address_id);
