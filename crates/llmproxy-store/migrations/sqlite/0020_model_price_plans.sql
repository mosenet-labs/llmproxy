CREATE TABLE model_price_plans (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_id INTEGER NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    upstream_model_id TEXT NOT NULL CHECK (length(upstream_model_id) BETWEEN 1 AND 200),
    currency TEXT NOT NULL CHECK (length(currency) = 3),
    source_kind TEXT NOT NULL,
    source_url TEXT,
    recorded_at INTEGER NOT NULL,
    effective_at INTEGER,
    is_current INTEGER NOT NULL CHECK (is_current IN (0, 1)),
    schedule_json TEXT
);
