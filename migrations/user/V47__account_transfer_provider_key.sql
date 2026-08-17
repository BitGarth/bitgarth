CREATE TABLE account_transfers_v47 (
    id TEXT PRIMARY KEY,
    chain_transaction_id TEXT NOT NULL REFERENCES chain_transactions(id) ON DELETE CASCADE,
    asset_id TEXT NOT NULL CHECK (asset_id IN ('bitcoin', 'ethereum')),
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    tx_hash TEXT NOT NULL,
    provider_transfer_key TEXT NOT NULL,
    transfer_index INTEGER NOT NULL CHECK (transfer_index >= 0),
    transfer_kind TEXT NOT NULL CHECK (transfer_kind IN ('normal', 'internal', 'self_destruct')),
    from_address TEXT,
    from_address_id TEXT REFERENCES digital_asset_addresses(id) ON DELETE SET NULL,
    to_address TEXT,
    to_address_id TEXT REFERENCES digital_asset_addresses(id) ON DELETE SET NULL,
    value_amount_hi INTEGER NOT NULL CHECK (value_amount_hi >= 0),
    value_amount_lo INTEGER NOT NULL CHECK (value_amount_lo >= 0 AND value_amount_lo < 1000000000000000000),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(asset_id, network, tx_hash, provider_transfer_key)
);

INSERT INTO account_transfers_v47 (
    id, chain_transaction_id, asset_id, network, tx_hash, provider_transfer_key,
    transfer_index, transfer_kind, from_address, from_address_id, to_address,
    to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at
)
SELECT
    id, chain_transaction_id, asset_id, network, tx_hash,
    'legacy:' || transfer_index,
    transfer_index, transfer_kind, from_address, from_address_id, to_address,
    to_address_id, value_amount_hi, value_amount_lo, created_at, updated_at
FROM account_transfers;

DROP TABLE account_transfers;
ALTER TABLE account_transfers_v47 RENAME TO account_transfers;

CREATE INDEX idx_account_transfers_from ON account_transfers(from_address_id);
CREATE INDEX idx_account_transfers_to ON account_transfers(to_address_id);
CREATE INDEX idx_account_transfers_chain_tx ON account_transfers(chain_transaction_id);
