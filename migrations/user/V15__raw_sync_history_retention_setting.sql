ALTER TABLE settings
ADD COLUMN raw_sync_history_retention_days INTEGER
CHECK(raw_sync_history_retention_days IS NULL OR (raw_sync_history_retention_days >= 1 AND raw_sync_history_retention_days <= 365));
