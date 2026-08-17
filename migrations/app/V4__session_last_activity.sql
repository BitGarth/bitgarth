-- Track last activity timestamp for idle timeout enforcement.
-- NULL for pre-existing rows; treated as created_at by validation code.
ALTER TABLE sessions ADD COLUMN last_activity_at TEXT NULL;
