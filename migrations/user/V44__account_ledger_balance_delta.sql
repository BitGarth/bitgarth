-- Persist the per-transaction signed balance delta so the hledger export can
-- post the authoritative asset-leg magnitude instead of reconstructing it from
-- value + fee. Magnitude uses the same hi/lo split as the sibling amount
-- columns (base units, 10^18 limb divisor). balance_delta_negative is the sign
-- flag (1 when balance_delta < 0). Existing rows default to a zero delta and are
-- backfilled by the user-data repair added alongside this migration.
ALTER TABLE account_transaction_ledger
    ADD COLUMN balance_delta_hi INTEGER NOT NULL DEFAULT 0
        CHECK (balance_delta_hi >= 0);

ALTER TABLE account_transaction_ledger
    ADD COLUMN balance_delta_lo INTEGER NOT NULL DEFAULT 0
        CHECK (balance_delta_lo >= 0 AND balance_delta_lo < 1000000000000000000);

ALTER TABLE account_transaction_ledger
    ADD COLUMN balance_delta_negative INTEGER NOT NULL DEFAULT 0
        CHECK (balance_delta_negative IN (0, 1));
