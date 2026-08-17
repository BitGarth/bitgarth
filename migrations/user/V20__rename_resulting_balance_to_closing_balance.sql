-- Rename resulting_balance_hi/lo columns to closing_balance_hi/lo in account_transaction_ledger
-- to match the domain language used in the rest of the codebase.

ALTER TABLE account_transaction_ledger
  RENAME COLUMN resulting_balance_hi TO closing_balance_hi;

ALTER TABLE account_transaction_ledger
  RENAME COLUMN resulting_balance_lo TO closing_balance_lo;
