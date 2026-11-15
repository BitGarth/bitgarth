CREATE TABLE coingecko_pair_interest (
    asset_id TEXT NOT NULL,
    quote_currency TEXT NOT NULL,
    last_used_day TEXT NOT NULL,
    PRIMARY KEY(asset_id, quote_currency)
);

-- Earlier quotes cannot distinguish user activity from unattended collection.
-- Give each previously priced pair one grace period starting on upgrade.
INSERT INTO coingecko_pair_interest(asset_id, quote_currency, last_used_day)
SELECT asset_id, quote_currency, CURRENT_DATE FROM price_points WHERE provider='coingecko'
UNION
SELECT asset_id, quote_currency, CURRENT_DATE FROM current_price_cache WHERE provider='coingecko';
