-- Unified user database schema baseline.
--
-- This file merges prior user migrations V0 through V14 into a single
-- initialize-from-scratch schema. Full user DB reset is expected.

-- Table: settings (per-user UI and sync preferences).
-- Each user has exactly one settings row (enforced by CHECK constraint).
--
-- Added from prior incremental migrations:
-- - session_duration (V1)
-- - receive_addresses_per_batch (V5)
-- - mempool_base_url (V8)
-- - etherscan_api_key (V12)
CREATE TABLE settings (
    settings_id TEXT PRIMARY KEY CHECK (settings_id = 'settings'),
    theme TEXT NULL,
    language TEXT NULL,
    date_time_format TEXT NULL,
    number_format TEXT NULL,
    currency TEXT NULL,
    timezone TEXT NULL,
    updated_at TEXT NULL,
    session_duration TEXT NULL,
    receive_addresses_per_batch TEXT NULL,
    mempool_base_url TEXT NULL,
    etherscan_api_key TEXT,
    etherscan_base_url TEXT NULL
);

-- Legacy hardware wallet tables from the old V2 schema were removed in V4.
-- Data preservation from that schema was intentionally not required.
-- Those tables are intentionally omitted from this baseline.

-- Table: wallets (top-level wallet containers and identity metadata).
CREATE TABLE wallets (
    id TEXT PRIMARY KEY,
    label TEXT NOT NULL CHECK(length(label) <= 255),
    label_key TEXT NOT NULL,
    master_fingerprint TEXT UNIQUE,
    identity_source TEXT NOT NULL CHECK (
        identity_source IN ('device_verified', 'user_provided', 'inferred')
    ),
    verified_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Index: enforce unique wallet labels per user DB (uniqueness checked via canonical key).
CREATE UNIQUE INDEX idx_wallets_label_key ON wallets(label_key);

-- Table: wallet_accessors (linked hardware/software accessors per wallet).
CREATE TABLE wallet_accessors (
    id TEXT PRIMARY KEY,
    wallet_id TEXT REFERENCES wallets(id) ON DELETE CASCADE,
    accessor_kind TEXT NOT NULL CHECK (
        accessor_kind IN ('trezor', 'ledger', 'software', 'unknown')
    ),
    accessor_label TEXT CHECK(length(accessor_label) <= 255),
    device_id_hash TEXT,
    device_model TEXT,
    accessor_version TEXT,
    firmware_version TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Index: enforce unique unlinked accessor device identity.
CREATE UNIQUE INDEX idx_wallet_accessors_device_unlinked
ON wallet_accessors(accessor_kind, device_id_hash)
WHERE wallet_id IS NULL AND device_id_hash IS NOT NULL;

-- Index: enforce unique linked accessor device identity per wallet.
CREATE UNIQUE INDEX idx_wallet_accessors_device_linked
ON wallet_accessors(wallet_id, accessor_kind, device_id_hash)
WHERE wallet_id IS NOT NULL AND device_id_hash IS NOT NULL;

-- Table: digital_asset_accounts (asset/network accounts under wallets).
CREATE TABLE digital_asset_accounts (
    id TEXT PRIMARY KEY,
    wallet_id TEXT NOT NULL REFERENCES wallets(id) ON DELETE CASCADE,
    label TEXT NOT NULL CHECK(length(label) <= 255),
    label_key TEXT NOT NULL,
    asset_id TEXT NOT NULL CHECK (asset_id IN ('bitcoin', 'ethereum')),
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    account_kind TEXT NOT NULL CHECK (account_kind IN ('hd_pubkey', 'single_address')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Index: speed account lookups by parent wallet.
CREATE INDEX idx_daa_wallet ON digital_asset_accounts(wallet_id);
-- Index: speed account filtering by asset/network.
CREATE INDEX idx_daa_asset ON digital_asset_accounts(asset_id, network);
-- Index: enforce unique account labels within a wallet (uniqueness checked via canonical key).
CREATE UNIQUE INDEX idx_daa_label_key ON digital_asset_accounts(wallet_id, label_key);

-- Table: digital_asset_account_hd_keys (xpub-derived key metadata for HD accounts).
CREATE TABLE digital_asset_account_hd_keys (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES digital_asset_accounts(id) ON DELETE CASCADE,
    key_role TEXT NOT NULL CHECK (key_role IN ('primary', 'cosigner', 'backup')),
    extended_pubkey TEXT NOT NULL,
    normalized_extended_pubkey TEXT NOT NULL,
    derivation_purpose INTEGER NOT NULL,
    derivation_coin_type INTEGER NOT NULL,
    derivation_account INTEGER NOT NULL,
    address_scheme TEXT NOT NULL CHECK (
        address_scheme IN (
            'legacy',
            'nested_segwit',
            'native_segwit',
            'taproot',
            'standard'
        )
    ),
    key_source TEXT NOT NULL CHECK (key_source IN ('device_verified', 'user_provided', 'inferred')),
    verified_by_accessor_id TEXT REFERENCES wallet_accessors(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Index: prevent duplicate extended public keys per address scheme.
CREATE UNIQUE INDEX idx_daa_hd_normalized_scheme
ON digital_asset_account_hd_keys(normalized_extended_pubkey, address_scheme);

-- Index: speed HD key lookups by account.
CREATE INDEX idx_daa_hd_account
ON digital_asset_account_hd_keys(account_id);

-- Table: digital_asset_addresses (tracked addresses for each asset account).
CREATE TABLE digital_asset_addresses (
    id TEXT PRIMARY KEY,
    account_id TEXT REFERENCES digital_asset_accounts(id) ON DELETE CASCADE,
    asset_id TEXT NOT NULL CHECK (asset_id IN ('bitcoin', 'ethereum')),
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    address TEXT NOT NULL,
    address_normalized TEXT NOT NULL,
    address_scheme TEXT NOT NULL CHECK (
        address_scheme IN (
            'legacy',
            'nested_segwit',
            'native_segwit',
            'taproot',
            'standard'
        )
    ),
    derivation_change INTEGER,
    derivation_index INTEGER,
    source_type TEXT NOT NULL CHECK (source_type IN ('derived', 'user_provided', 'imported', 'observed')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Index: enforce unique normalized address per asset/network.
CREATE UNIQUE INDEX idx_addresses_unique
ON digital_asset_addresses(asset_id, network, address_normalized);

-- Index: speed address lookups by account.
CREATE INDEX idx_addresses_account
ON digital_asset_addresses(account_id);

-- Table: chain_transactions (shared transaction envelope per asset/network tx hash).
-- Includes generalized fee amount columns and nonce/failed support.
CREATE TABLE chain_transactions (
    id TEXT PRIMARY KEY,
    asset_id TEXT NOT NULL CHECK (asset_id IN ('bitcoin', 'ethereum')),
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    tx_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'confirmed', 'dropped', 'failed')),
    block_height INTEGER CHECK (block_height >= 0),
    block_hash TEXT,
    block_time TEXT,
    fee_amount_lo INTEGER CHECK (fee_amount_lo >= 0),
    fee_amount_hi INTEGER CHECK (fee_amount_hi >= 0),
    nonce INTEGER CHECK (nonce >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(asset_id, network, tx_hash)
);

-- Index: speed polling/filtering transactions by status.
CREATE INDEX idx_chain_txs_status ON chain_transactions(asset_id, network, status);
-- Index: speed block-height based transaction queries.
CREATE INDEX idx_chain_txs_height ON chain_transactions(asset_id, network, block_height);

-- Table: transaction_inputs (owned-address transaction input edges).
CREATE TABLE transaction_inputs (
    id TEXT PRIMARY KEY,
    tx_id TEXT NOT NULL REFERENCES chain_transactions(id) ON DELETE CASCADE,
    input_index INTEGER NOT NULL CHECK (input_index >= 0),
    prev_tx_hash TEXT NOT NULL,
    prev_output_index INTEGER NOT NULL CHECK (prev_output_index >= 0),
    address_id TEXT NOT NULL REFERENCES digital_asset_addresses(id) ON DELETE CASCADE,
    value_amount INTEGER CHECK (value_amount >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(tx_id, input_index)
);

-- Index: speed prevout lookup for input reconciliation.
CREATE INDEX idx_tx_inputs_prev ON transaction_inputs(prev_tx_hash, prev_output_index);

-- Table: transaction_outputs (owned-address transaction output edges).
CREATE TABLE transaction_outputs (
    id TEXT PRIMARY KEY,
    tx_id TEXT NOT NULL REFERENCES chain_transactions(id) ON DELETE CASCADE,
    output_index INTEGER NOT NULL CHECK (output_index >= 0),
    address_id TEXT NOT NULL REFERENCES digital_asset_addresses(id) ON DELETE CASCADE,
    raw_address TEXT,
    script_pubkey_hex TEXT NOT NULL,
    value_amount INTEGER NOT NULL CHECK (value_amount >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(tx_id, output_index)
);

-- Index: speed output lookup by owned address.
CREATE INDEX idx_tx_outputs_address ON transaction_outputs(address_id);
-- Index: speed output lookup by raw (external) address text.
CREATE INDEX idx_tx_outputs_raw_address ON transaction_outputs(raw_address);

-- Table: utxos (current UTXO state for owned addresses).
CREATE TABLE utxos (
    id TEXT PRIMARY KEY,
    asset_id TEXT NOT NULL CHECK (asset_id IN ('bitcoin', 'ethereum')),
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    tx_hash TEXT NOT NULL,
    output_index INTEGER NOT NULL CHECK (output_index >= 0),
    address_id TEXT NOT NULL REFERENCES digital_asset_addresses(id) ON DELETE CASCADE,
    value_amount INTEGER NOT NULL CHECK (value_amount >= 0),
    status TEXT NOT NULL CHECK (status IN ('pending', 'confirmed', 'dropped')),
    replaced_by_tx_hash TEXT,
    spent_by_tx_hash TEXT,
    spent_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(asset_id, network, tx_hash, output_index)
);

-- Index: speed live unspent balance scans by address.
CREATE INDEX idx_utxos_address_unspent_live
    ON utxos(address_id)
    WHERE spent_by_tx_hash IS NULL
      AND status IN ('pending', 'confirmed');

-- Index: speed spent-output lookups by spending tx hash.
CREATE INDEX idx_utxos_spent ON utxos(spent_by_tx_hash) WHERE spent_by_tx_hash IS NOT NULL;

-- Table: account_sync_state (per-account derivation/sync cursors).
CREATE TABLE account_sync_state (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES digital_asset_accounts(id) ON DELETE CASCADE,
    last_scanned_height INTEGER CHECK (last_scanned_height >= 0),
    last_scanned_time TEXT,
    gap_limit INTEGER NOT NULL CHECK (gap_limit >= 0),
    last_derived_external_index INTEGER CHECK (last_derived_external_index >= 0),
    last_derived_internal_index INTEGER CHECK (last_derived_internal_index >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(account_id)
);

-- Table: transaction_sync_state (per-address sync run telemetry).
CREATE TABLE transaction_sync_state (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL CHECK (scope IN ('address')),
    address_id TEXT NOT NULL REFERENCES digital_asset_addresses(id) ON DELETE CASCADE,
    last_run_id TEXT NOT NULL,
    last_started_at TEXT NOT NULL,
    last_completed_at TEXT,
    last_result TEXT NOT NULL CHECK (last_result IN ('success', 'failure')),
    last_error TEXT,
    last_tip_height INTEGER,
    new_tx_count INTEGER NOT NULL,
    updated_tx_count INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(scope, address_id)
);

-- Table: account_transfers (account-model transfer rows linked to chain transactions).
-- Linked to chain_transactions for shared tx envelope (status, block info, fees).
CREATE TABLE account_transfers (
    id TEXT PRIMARY KEY,
    chain_transaction_id TEXT NOT NULL REFERENCES chain_transactions(id) ON DELETE CASCADE,
    asset_id TEXT NOT NULL CHECK (asset_id IN ('bitcoin', 'ethereum')),
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    tx_hash TEXT NOT NULL,
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
    UNIQUE(asset_id, network, tx_hash, transfer_index)
);

-- Index: speed transfer queries by source owned address.
CREATE INDEX idx_account_transfers_from ON account_transfers(from_address_id);
-- Index: speed transfer queries by destination owned address.
CREATE INDEX idx_account_transfers_to ON account_transfers(to_address_id);
-- Index: speed transfer lookups by parent chain transaction.
CREATE INDEX idx_account_transfers_chain_tx ON account_transfers(chain_transaction_id);

-- Table: account_transaction_ledger (per-account tx projection for paged history views).
-- Stores one row per (account, tx_hash) with deterministic ordering fields and
-- precomputed resulting balances.
CREATE TABLE account_transaction_ledger (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES digital_asset_accounts(id) ON DELETE CASCADE,
    chain_transaction_id TEXT NOT NULL REFERENCES chain_transactions(id) ON DELETE CASCADE,
    asset_id TEXT NOT NULL CHECK (asset_id IN ('bitcoin', 'ethereum')),
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    tx_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'confirmed', 'dropped', 'failed')),
    occurred_at TEXT NOT NULL,
    first_seen_at TEXT NOT NULL,
    block_height INTEGER CHECK (block_height >= 0),
    nonce INTEGER CHECK (nonce >= 0),
    min_transfer_index INTEGER CHECK (min_transfer_index >= 0),
    tx_type TEXT NOT NULL CHECK (tx_type IN ('receive', 'send', 'self_transfer')),
    from_addresses_json TEXT NOT NULL,
    to_addresses_json TEXT NOT NULL,
    value_amount_hi INTEGER NOT NULL CHECK (value_amount_hi >= 0),
    value_amount_lo INTEGER NOT NULL CHECK (value_amount_lo >= 0 AND value_amount_lo < 1000000000000000000),
    fee_amount_hi INTEGER CHECK (fee_amount_hi >= 0),
    fee_amount_lo INTEGER CHECK (fee_amount_lo >= 0 AND fee_amount_lo < 1000000000000000000),
    resulting_balance_hi INTEGER CHECK (resulting_balance_hi >= 0),
    resulting_balance_lo INTEGER CHECK (resulting_balance_lo >= 0 AND resulting_balance_lo < 1000000000000000000),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(account_id, asset_id, network, tx_hash)
);

-- Indexes: support account-scoped filtering and deterministic paging order.
CREATE INDEX idx_account_tx_ledger_account_status
    ON account_transaction_ledger(account_id, status);
CREATE INDEX idx_account_tx_ledger_pending_page
    ON account_transaction_ledger(
        account_id,
        first_seen_at,
        COALESCE(nonce, 9223372036854775807),
        tx_hash
    )
    WHERE status IN ('pending', 'dropped', 'failed');
CREATE INDEX idx_account_tx_ledger_confirmed_page
    ON account_transaction_ledger(
        account_id,
        occurred_at,
        COALESCE(block_height, 9223372036854775807),
        COALESCE(nonce, 9223372036854775807),
        COALESCE(min_transfer_index, 9223372036854775807),
        tx_hash
    )
    WHERE status = 'confirmed';

-- Trigger-based consistency/pruning rules (V14).

-- Trigger: remove transfer rows once both owned-side references are NULL.
CREATE TRIGGER trg_account_transfers_delete_when_unowned
AFTER UPDATE OF from_address_id, to_address_id ON account_transfers
FOR EACH ROW
WHEN NEW.from_address_id IS NULL AND NEW.to_address_id IS NULL
BEGIN
    DELETE FROM account_transfers WHERE id = NEW.id;
END;

-- Trigger: discard inserted transfer rows with no owned-side references.
CREATE TRIGGER trg_account_transfers_skip_unowned_insert
AFTER INSERT ON account_transfers
FOR EACH ROW
WHEN NEW.from_address_id IS NULL AND NEW.to_address_id IS NULL
BEGIN
    DELETE FROM account_transfers WHERE id = NEW.id;
END;

-- Trigger: prune chain transaction after input deletion if no children remain.
CREATE TRIGGER trg_chain_tx_prune_after_input_delete
AFTER DELETE ON transaction_inputs
FOR EACH ROW
BEGIN
    DELETE FROM chain_transactions
     WHERE id = OLD.tx_id
       AND NOT EXISTS (
           SELECT 1 FROM transaction_inputs ti WHERE ti.tx_id = OLD.tx_id
       )
       AND NOT EXISTS (
           SELECT 1 FROM transaction_outputs to2 WHERE to2.tx_id = OLD.tx_id
       )
       AND NOT EXISTS (
           SELECT 1 FROM account_transfers at2 WHERE at2.chain_transaction_id = OLD.tx_id
       );
END;

-- Trigger: prune chain transaction after output deletion if no children remain.
CREATE TRIGGER trg_chain_tx_prune_after_output_delete
AFTER DELETE ON transaction_outputs
FOR EACH ROW
BEGIN
    DELETE FROM chain_transactions
     WHERE id = OLD.tx_id
       AND NOT EXISTS (
           SELECT 1 FROM transaction_inputs ti WHERE ti.tx_id = OLD.tx_id
       )
       AND NOT EXISTS (
           SELECT 1 FROM transaction_outputs to2 WHERE to2.tx_id = OLD.tx_id
       )
       AND NOT EXISTS (
           SELECT 1 FROM account_transfers at2 WHERE at2.chain_transaction_id = OLD.tx_id
       );
END;

-- Trigger: prune chain transaction after transfer deletion if no children remain.
CREATE TRIGGER trg_chain_tx_prune_after_transfer_delete
AFTER DELETE ON account_transfers
FOR EACH ROW
BEGIN
    DELETE FROM chain_transactions
     WHERE id = OLD.chain_transaction_id
       AND NOT EXISTS (
           SELECT 1 FROM transaction_inputs ti WHERE ti.tx_id = OLD.chain_transaction_id
       )
       AND NOT EXISTS (
           SELECT 1 FROM transaction_outputs to2 WHERE to2.tx_id = OLD.chain_transaction_id
       )
       AND NOT EXISTS (
           SELECT 1 FROM account_transfers at2 WHERE at2.chain_transaction_id = OLD.chain_transaction_id
       );
END;
