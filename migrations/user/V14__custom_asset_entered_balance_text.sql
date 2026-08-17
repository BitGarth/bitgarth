ALTER TABLE custom_asset_balance_assertions
ADD COLUMN entered_balance_text TEXT
CHECK(entered_balance_text IS NULL OR length(entered_balance_text) <= 64);
