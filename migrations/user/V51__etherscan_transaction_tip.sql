ALTER TABLE transaction_sync_state
    ADD COLUMN etherscan_transaction_tip_height INTEGER
    CHECK (etherscan_transaction_tip_height IS NULL
           OR etherscan_transaction_tip_height >= 0);
