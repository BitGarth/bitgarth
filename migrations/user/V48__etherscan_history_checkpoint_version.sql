ALTER TABLE transaction_sync_state
ADD COLUMN etherscan_history_checkpoint_version INTEGER
CHECK (
    etherscan_history_checkpoint_version IS NULL
    OR etherscan_history_checkpoint_version = 1
);
