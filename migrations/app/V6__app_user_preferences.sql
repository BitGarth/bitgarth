-- Non-secret per-user app preferences. Stores only a boolean consent flag plus
-- timestamps. Never store asset ids, currency, prices, balances, addresses,
-- xpubs, transactions, labels, or keys here.
CREATE TABLE app_user_preferences (
    user_id TEXT PRIMARY KEY,
    price_fetching_enabled INTEGER NOT NULL CHECK (price_fetching_enabled IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(user_id) ON DELETE CASCADE
);
