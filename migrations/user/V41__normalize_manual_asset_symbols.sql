UPDATE manual_asset_accounts
SET symbol = NULL
WHERE symbol IS NOT NULL
  AND length(symbol) != 1;
