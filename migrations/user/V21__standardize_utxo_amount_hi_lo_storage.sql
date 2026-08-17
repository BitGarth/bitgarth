CREATE TABLE transaction_inputs_new (
    id TEXT PRIMARY KEY,
    tx_id TEXT NOT NULL REFERENCES chain_transactions(id) ON DELETE CASCADE,
    input_index INTEGER NOT NULL CHECK (input_index >= 0),
    prev_tx_hash TEXT NOT NULL,
    prev_output_index INTEGER NOT NULL CHECK (prev_output_index >= 0),
    address_id TEXT NOT NULL REFERENCES digital_asset_addresses(id) ON DELETE CASCADE,
    value_amount_hi INTEGER CHECK (value_amount_hi >= 0),
    value_amount_lo INTEGER CHECK (value_amount_lo >= 0 AND value_amount_lo < 1000000000000000000),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK ((value_amount_hi IS NULL) = (value_amount_lo IS NULL)),
    UNIQUE(tx_id, input_index)
);

INSERT INTO transaction_inputs_new (
    id,
    tx_id,
    input_index,
    prev_tx_hash,
    prev_output_index,
    address_id,
    value_amount_hi,
    value_amount_lo,
    created_at,
    updated_at
)
SELECT
    id,
    tx_id,
    input_index,
    prev_tx_hash,
    prev_output_index,
    address_id,
    CASE WHEN value_amount IS NULL THEN NULL ELSE 0 END,
    value_amount,
    created_at,
    updated_at
FROM transaction_inputs;

DROP TABLE transaction_inputs;
ALTER TABLE transaction_inputs_new RENAME TO transaction_inputs;
CREATE INDEX idx_tx_inputs_prev ON transaction_inputs(prev_tx_hash, prev_output_index);
CREATE INDEX idx_tx_inputs_address ON transaction_inputs(address_id);

CREATE TABLE transaction_outputs_new (
    id TEXT PRIMARY KEY,
    tx_id TEXT NOT NULL REFERENCES chain_transactions(id) ON DELETE CASCADE,
    output_index INTEGER NOT NULL CHECK (output_index >= 0),
    address_id TEXT NOT NULL REFERENCES digital_asset_addresses(id) ON DELETE CASCADE,
    raw_address TEXT,
    script_pubkey_hex TEXT NOT NULL,
    value_amount_hi INTEGER NOT NULL CHECK (value_amount_hi >= 0),
    value_amount_lo INTEGER NOT NULL CHECK (value_amount_lo >= 0 AND value_amount_lo < 1000000000000000000),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(tx_id, output_index)
);

INSERT INTO transaction_outputs_new (
    id,
    tx_id,
    output_index,
    address_id,
    raw_address,
    script_pubkey_hex,
    value_amount_hi,
    value_amount_lo,
    created_at,
    updated_at
)
SELECT
    id,
    tx_id,
    output_index,
    address_id,
    raw_address,
    script_pubkey_hex,
    0,
    value_amount,
    created_at,
    updated_at
FROM transaction_outputs;

DROP TABLE transaction_outputs;
ALTER TABLE transaction_outputs_new RENAME TO transaction_outputs;
CREATE INDEX idx_tx_outputs_address ON transaction_outputs(address_id);
CREATE INDEX idx_tx_outputs_raw_address ON transaction_outputs(raw_address);

CREATE TABLE utxos_new (
    id TEXT PRIMARY KEY,
    asset_id TEXT NOT NULL CHECK (asset_id IN ('bitcoin', 'ethereum')),
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    tx_hash TEXT NOT NULL,
    output_index INTEGER NOT NULL CHECK (output_index >= 0),
    address_id TEXT NOT NULL REFERENCES digital_asset_addresses(id) ON DELETE CASCADE,
    value_amount_hi INTEGER NOT NULL CHECK (value_amount_hi >= 0),
    value_amount_lo INTEGER NOT NULL CHECK (value_amount_lo >= 0 AND value_amount_lo < 1000000000000000000),
    status TEXT NOT NULL CHECK (status IN ('pending', 'confirmed', 'dropped')),
    replaced_by_tx_hash TEXT,
    spent_by_tx_hash TEXT,
    spent_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(asset_id, network, tx_hash, output_index)
);

INSERT INTO utxos_new (
    id,
    asset_id,
    network,
    tx_hash,
    output_index,
    address_id,
    value_amount_hi,
    value_amount_lo,
    status,
    replaced_by_tx_hash,
    spent_by_tx_hash,
    spent_at,
    created_at,
    updated_at
)
SELECT
    id,
    asset_id,
    network,
    tx_hash,
    output_index,
    address_id,
    0,
    value_amount,
    status,
    replaced_by_tx_hash,
    spent_by_tx_hash,
    spent_at,
    created_at,
    updated_at
FROM utxos;

DROP TABLE utxos;
ALTER TABLE utxos_new RENAME TO utxos;
CREATE INDEX idx_utxos_address_unspent_live
    ON utxos(address_id)
    WHERE spent_by_tx_hash IS NULL
      AND status IN ('pending', 'confirmed');
CREATE INDEX idx_utxos_spent ON utxos(spent_by_tx_hash) WHERE spent_by_tx_hash IS NOT NULL;
