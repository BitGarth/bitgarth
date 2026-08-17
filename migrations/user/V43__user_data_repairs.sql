CREATE TABLE IF NOT EXISTS user_data_repairs (
    repair_key TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('pending', 'completed')),
    last_attempted_at TEXT,
    completed_at TEXT,
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
