-- Track last successful login for inactivity detection.
-- NULL for pre-existing rows; inactivity logic treats NULL as created_at.
ALTER TABLE users ADD COLUMN last_login_at TEXT NULL;
