CREATE TABLE chain_state (
    id TEXT PRIMARY KEY,
    asset_id TEXT NOT NULL,
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    chain_height INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(asset_id, network)
);
