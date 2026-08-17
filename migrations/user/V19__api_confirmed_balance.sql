ALTER TABLE transaction_sync_state
  ADD COLUMN api_confirmed_balance_hi INTEGER CHECK (api_confirmed_balance_hi >= 0);

ALTER TABLE transaction_sync_state
  ADD COLUMN api_confirmed_balance_lo INTEGER CHECK (
    api_confirmed_balance_lo >= 0
    AND api_confirmed_balance_lo < 1000000000000000000
  );
