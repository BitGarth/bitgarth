CREATE TABLE etherscan_pending_ranges (
    address_id TEXT PRIMARY KEY NOT NULL
        REFERENCES digital_asset_addresses(id) ON DELETE CASCADE,
    start_block INTEGER NOT NULL CHECK (start_block >= 0),
    end_block INTEGER NOT NULL CHECK (end_block >= start_block),
    -- NULL means retained older history; otherwise this is the captured recent tip.
    transaction_tip INTEGER CHECK (transaction_tip IS NULL OR transaction_tip >= end_block)
);
