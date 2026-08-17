CREATE TABLE app_update_state (
    id TEXT PRIMARY KEY CHECK (id = 'singleton'),
    update_check_enabled INTEGER NOT NULL CHECK (update_check_enabled IN (0, 1)),
    last_checked_at TEXT NULL,
    latest_seen TEXT NULL,
    release_url TEXT NULL,
    published_at TEXT NULL,
    updated_at TEXT NOT NULL
);
