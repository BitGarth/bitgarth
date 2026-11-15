CREATE TABLE payment_plan_descriptions (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    fetched_at_micros INTEGER NOT NULL,
    descriptions_json TEXT NOT NULL
);
