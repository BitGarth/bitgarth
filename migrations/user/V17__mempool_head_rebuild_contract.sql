PRAGMA foreign_keys=OFF;

ALTER TABLE raw_mempool_transaction_observations
RENAME TO raw_mempool_transaction_observations_old;

ALTER TABLE raw_mempool_transaction_versions
RENAME TO raw_mempool_transaction_versions_old;

DROP INDEX IF EXISTS idx_raw_mempool_tx_obs_address_observed;
DROP INDEX IF EXISTS idx_raw_mempool_tx_obs_version_observed;
DROP INDEX IF EXISTS idx_raw_mempool_tx_obs_run_observed;
DROP INDEX IF EXISTS idx_raw_mempool_tx_versions_txid_created;
DROP INDEX IF EXISTS idx_raw_mempool_tx_versions_txid_hash;

CREATE TABLE raw_mempool_transaction_versions (
    id TEXT PRIMARY KEY,
    source_connection_id TEXT NOT NULL REFERENCES source_connections(id) ON DELETE RESTRICT,
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    txid TEXT NOT NULL CHECK (length(txid) = 64 AND txid NOT GLOB '*[^0-9a-f]*'),
    payload_hash_sha256_hex TEXT NOT NULL CHECK (length(payload_hash_sha256_hex) = 64 AND payload_hash_sha256_hex NOT GLOB '*[^0-9a-f]*'),
    payload_bytes BLOB NOT NULL,
    first_observed_at TEXT NOT NULL,
    supersedes_raw_version_id TEXT NULL REFERENCES raw_mempool_transaction_versions(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    CHECK (length(payload_bytes) > 0),
    CHECK (supersedes_raw_version_id IS NULL OR supersedes_raw_version_id != id)
);

CREATE INDEX idx_raw_mempool_tx_versions_txid_created
ON raw_mempool_transaction_versions(source_connection_id, txid, created_at DESC);

CREATE INDEX idx_raw_mempool_tx_versions_txid_hash
ON raw_mempool_transaction_versions(source_connection_id, txid, payload_hash_sha256_hex);

CREATE INDEX idx_raw_mempool_tx_versions_supersedes
ON raw_mempool_transaction_versions(supersedes_raw_version_id);

CREATE TABLE raw_mempool_transaction_observations (
    id TEXT PRIMARY KEY,
    sync_run_id TEXT NOT NULL REFERENCES sync_runs(id) ON DELETE CASCADE,
    source_connection_id TEXT NOT NULL REFERENCES source_connections(id) ON DELETE RESTRICT,
    raw_observation_set_id TEXT NOT NULL REFERENCES raw_observation_sets(id) ON DELETE CASCADE,
    raw_mempool_transaction_version_id TEXT NOT NULL REFERENCES raw_mempool_transaction_versions(id) ON DELETE CASCADE,
    page_item_index INTEGER NOT NULL CHECK (page_item_index >= 0),
    observed_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(sync_run_id, source_connection_id, raw_mempool_transaction_version_id, raw_observation_set_id),
    UNIQUE(raw_observation_set_id, page_item_index)
);

CREATE INDEX idx_raw_mempool_tx_obs_address_observed
ON raw_mempool_transaction_observations(source_connection_id, observed_at);

CREATE INDEX idx_raw_mempool_tx_obs_version_observed
ON raw_mempool_transaction_observations(raw_mempool_transaction_version_id, observed_at);

CREATE INDEX idx_raw_mempool_tx_obs_run_observed
ON raw_mempool_transaction_observations(sync_run_id, observed_at);

INSERT INTO raw_mempool_transaction_versions (
    id,
    source_connection_id,
    network,
    txid,
    payload_hash_sha256_hex,
    payload_bytes,
    first_observed_at,
    supersedes_raw_version_id,
    created_at
)
SELECT
    id,
    source_connection_id,
    network,
    txid,
    payload_hash_sha256_hex,
    payload_bytes,
    first_observed_at,
    supersedes_raw_version_id,
    created_at
FROM raw_mempool_transaction_versions_old;

INSERT INTO raw_mempool_transaction_observations (
    id,
    sync_run_id,
    source_connection_id,
    raw_observation_set_id,
    raw_mempool_transaction_version_id,
    page_item_index,
    observed_at,
    created_at
)
SELECT
    id,
    sync_run_id,
    source_connection_id,
    raw_observation_set_id,
    raw_mempool_transaction_version_id,
    page_item_index,
    observed_at,
    created_at
FROM raw_mempool_transaction_observations_old;

DROP TABLE raw_mempool_transaction_observations_old;
DROP TABLE raw_mempool_transaction_versions_old;

PRAGMA foreign_keys=ON;
