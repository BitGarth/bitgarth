DROP TABLE free_tier_entitlement_cache;

CREATE TABLE free_tier_entitlement_cache (
    id TEXT PRIMARY KEY CHECK (id = 'singleton'),
    capability_schema_version INTEGER NOT NULL CHECK (capability_schema_version = 4),
    capabilities_json TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
