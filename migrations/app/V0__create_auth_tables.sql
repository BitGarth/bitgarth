-- Users table
CREATE TABLE IF NOT EXISTS users (
    user_id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Index for username lookups
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_username ON users(username COLLATE NOCASE);

-- Auth credentials (supports multiple auth methods)
CREATE TABLE IF NOT EXISTS auth_credentials (
    credential_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    auth_method TEXT NOT NULL CHECK(auth_method IN ('password', 'hardware_wallet', 'passkey')),
    is_primary BOOLEAN NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(user_id) ON DELETE CASCADE
);

-- Index for looking up credentials by user
CREATE INDEX IF NOT EXISTS idx_auth_credentials_user_id ON auth_credentials(user_id);

-- Password credentials (Argon2id hashed)
CREATE TABLE IF NOT EXISTS password_credentials (
    credential_id TEXT PRIMARY KEY,
    password_hash TEXT NOT NULL,
    salt TEXT NOT NULL,
    FOREIGN KEY (credential_id) REFERENCES auth_credentials(credential_id) ON DELETE CASCADE
);

-- Sessions table
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    token TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(user_id) ON DELETE CASCADE
);

-- Index for session token lookups
CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_token ON sessions(token);

-- Index for cleaning up expired sessions
CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);
