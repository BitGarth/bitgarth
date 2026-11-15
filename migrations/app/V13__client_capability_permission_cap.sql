-- Lets a user lower a Paired Client's access from the web UI. The granted
-- `permission` stays bound into the wrapped key; requests are authorized with
-- the lower of `permission` and `permission_cap`, so a cap can never raise
-- access.
ALTER TABLE client_capabilities
    ADD COLUMN permission_cap TEXT
    CHECK (permission_cap IS NULL OR permission_cap IN ('balances_read', 'read', 'full'));
