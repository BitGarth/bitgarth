CREATE TABLE legal_acceptances (
    legal_acceptance_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    document_kind TEXT NOT NULL CHECK (document_kind IN ('terms', 'privacy')),
    document_version TEXT NOT NULL,
    accepted_at TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('registration', 'payment')),
    created_at TEXT NOT NULL
);

CREATE INDEX idx_legal_acceptances_user_kind
ON legal_acceptances(user_id, document_kind, accepted_at);
