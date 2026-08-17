CREATE TABLE api_keys (
    provider TEXT PRIMARY KEY CHECK (provider IN ('etherscan', 'coingecko')),
    api_key TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO api_keys (provider, api_key, created_at, updated_at)
SELECT
    'etherscan',
    TRIM(etherscan_api_key),
    COALESCE(updated_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM settings
WHERE etherscan_api_key IS NOT NULL
  AND TRIM(etherscan_api_key) <> ''
ON CONFLICT(provider) DO UPDATE SET
    api_key = excluded.api_key,
    updated_at = excluded.updated_at;
