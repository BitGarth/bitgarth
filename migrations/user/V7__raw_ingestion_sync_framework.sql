CREATE TABLE source_connections (
    id TEXT PRIMARY KEY,
    integration TEXT NOT NULL CHECK (integration IN ('mempool', 'etherscan')),
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    source_kind TEXT NOT NULL CHECK (source_kind = 'wallet_address_api_watch'),
    normalized_source_key TEXT NOT NULL CHECK (length(trim(normalized_source_key)) > 0),
    status TEXT NOT NULL CHECK (status IN ('active', 'inactive')),
    current_digital_asset_address_id TEXT NULL REFERENCES digital_asset_addresses(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    activated_at TEXT NOT NULL,
    deactivated_at TEXT NULL,
    CHECK (
        (status = 'active' AND current_digital_asset_address_id IS NOT NULL AND deactivated_at IS NULL)
        OR
        (status = 'inactive' AND current_digital_asset_address_id IS NULL AND deactivated_at IS NOT NULL)
    ),
    UNIQUE(integration, network, normalized_source_key)
);

CREATE UNIQUE INDEX idx_source_connections_current_address
ON source_connections(current_digital_asset_address_id)
WHERE current_digital_asset_address_id IS NOT NULL;

CREATE INDEX idx_source_connections_status_integration
ON source_connections(status, integration, network, updated_at);

CREATE TABLE sync_runs (
    id TEXT PRIMARY KEY,
    integration TEXT NOT NULL CHECK (integration IN ('mempool', 'etherscan')),
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('address')),
    scope_address_id TEXT NOT NULL,
    source_connection_id TEXT NOT NULL REFERENCES source_connections(id) ON DELETE RESTRICT,
    asset_id TEXT NOT NULL CHECK (asset_id IN ('bitcoin', 'ethereum')),
    network TEXT NOT NULL CHECK (network IN ('mainnet', 'testnet', 'signet', 'regtest')),
    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('scheduled', 'manual', 'backfill')),
    status TEXT NOT NULL CHECK (status IN ('started', 'completed_success', 'completed_failure')),
    started_at TEXT NOT NULL,
    completed_at TEXT NULL,
    summary_json TEXT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK ((status = 'started' AND completed_at IS NULL) OR (status IN ('completed_success', 'completed_failure') AND completed_at IS NOT NULL))
);

CREATE INDEX idx_sync_runs_scope_started
ON sync_runs(integration, source_connection_id, started_at);

CREATE INDEX idx_sync_runs_status_started
ON sync_runs(status, started_at);

CREATE TABLE request_attempts (
    id TEXT PRIMARY KEY,
    sync_run_id TEXT NOT NULL REFERENCES sync_runs(id) ON DELETE CASCADE,
    integration TEXT NOT NULL CHECK (integration IN ('mempool', 'etherscan')),
    request_kind TEXT NOT NULL CHECK (
        request_kind IN (
            'mempool_address_transactions_first_page',
            'mempool_address_transactions_after_confirmed',
            'etherscan_normal_transactions_page',
            'etherscan_internal_transactions_page'
        )
    ),
    request_url TEXT NOT NULL,
    request_method TEXT NOT NULL CHECK (request_method IN ('GET')),
    scope_address_id TEXT NOT NULL REFERENCES digital_asset_addresses(id) ON DELETE CASCADE,
    request_query_json TEXT NULL,
    page_cursor TEXT NULL,
    page_kind TEXT NULL,
    attempted_at TEXT NOT NULL,
    outcome_kind TEXT NOT NULL CHECK (outcome_kind IN ('http_response', 'transport_error', 'deserialize_error')),
    http_status_code INTEGER NULL,
    response_headers_json TEXT NULL,
    response_body_truncated BLOB NULL,
    response_body_was_truncated INTEGER NOT NULL CHECK (response_body_was_truncated IN (0, 1)),
    transport_error_message TEXT NULL,
    created_at TEXT NOT NULL,
    CHECK (length(trim(request_url)) > 0),
    CHECK (
        (
            request_kind = 'mempool_address_transactions_first_page'
            AND page_kind = 'first_page'
            AND page_cursor IS NULL
            AND request_query_json IS NULL
        )
        OR
        (
            request_kind = 'mempool_address_transactions_after_confirmed'
            AND page_kind = 'paginated_after_confirmed'
            AND page_cursor IS NOT NULL
            AND length(trim(page_cursor)) > 0
            AND request_query_json IS NULL
        )
        OR
        (
            request_kind IN ('etherscan_normal_transactions_page', 'etherscan_internal_transactions_page')
            AND page_kind IS NULL
            AND page_cursor IS NULL
            AND request_query_json IS NOT NULL
            AND length(trim(request_query_json)) > 0
            AND json_valid(request_query_json)
            AND json_type(request_query_json) = 'object'
        )
    ),
    CHECK (
        ((outcome_kind = 'http_response' OR outcome_kind = 'deserialize_error') AND http_status_code BETWEEN 100 AND 599)
        OR
        (outcome_kind = 'transport_error' AND http_status_code IS NULL)
    ),
    CHECK (
        (outcome_kind = 'transport_error' AND transport_error_message IS NOT NULL AND length(trim(transport_error_message)) > 0)
        OR
        (outcome_kind IN ('http_response', 'deserialize_error') AND transport_error_message IS NULL)
    )
);

CREATE INDEX idx_request_attempts_run_attempted
ON request_attempts(sync_run_id, attempted_at);

CREATE INDEX idx_request_attempts_scope_attempted
ON request_attempts(integration, scope_address_id, attempted_at);

CREATE INDEX idx_request_attempts_status_attempted
ON request_attempts(http_status_code, attempted_at);

CREATE TABLE raw_observation_sets (
    id TEXT PRIMARY KEY,
    sync_run_id TEXT NOT NULL REFERENCES sync_runs(id) ON DELETE CASCADE,
    source_connection_id TEXT NOT NULL REFERENCES source_connections(id) ON DELETE RESTRICT,
    grouping_kind TEXT NOT NULL CHECK (
        grouping_kind IN (
            'mempool_address_transactions_page',
            'etherscan_normal_transactions_page',
            'etherscan_internal_transactions_page'
        )
    ),
    grouping_metadata_json TEXT NOT NULL CHECK (
        length(trim(grouping_metadata_json)) > 0
        AND json_valid(grouping_metadata_json)
        AND json_type(grouping_metadata_json) = 'object'
    ),
    observed_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_raw_observation_sets_run_observed
ON raw_observation_sets(sync_run_id, observed_at DESC);

CREATE INDEX idx_raw_observation_sets_source_observed
ON raw_observation_sets(source_connection_id, observed_at DESC);

CREATE TABLE raw_parse_attempts (
    id TEXT PRIMARY KEY,
    sync_run_id TEXT NOT NULL REFERENCES sync_runs(id) ON DELETE CASCADE,
    integration TEXT NOT NULL CHECK (integration IN ('mempool', 'etherscan')),
    raw_object_kind TEXT NOT NULL CHECK (
        raw_object_kind IN (
            'mempool_transaction',
            'etherscan_normal_transaction',
            'etherscan_internal_transaction'
        )
    ),
    raw_object_key_json TEXT NOT NULL,
    raw_version_id TEXT NOT NULL,
    parser_kind TEXT NOT NULL CHECK (
        parser_kind IN (
            'mempool_transaction_to_sync_record',
            'etherscan_normal_transaction_to_sync_record',
            'etherscan_internal_transaction_to_sync_record'
        )
    ),
    parser_version TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('success', 'failure')),
    error_message TEXT NULL,
    attempted_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CHECK (
        length(trim(raw_object_key_json)) > 0
        AND json_valid(raw_object_key_json)
        AND json_type(raw_object_key_json) = 'object'
    ),
    CHECK (length(trim(parser_version)) > 0),
    CHECK (
        (status = 'failure' AND error_message IS NOT NULL AND length(trim(error_message)) > 0)
        OR
        (status = 'success' AND error_message IS NULL)
    )
);

CREATE INDEX idx_raw_parse_attempts_object_attempted
ON raw_parse_attempts(integration, raw_object_kind, raw_object_key_json, attempted_at DESC);

CREATE INDEX idx_raw_parse_attempts_version_attempted
ON raw_parse_attempts(raw_version_id, attempted_at DESC);

CREATE INDEX idx_raw_parse_attempts_run_attempted
ON raw_parse_attempts(sync_run_id, attempted_at);
