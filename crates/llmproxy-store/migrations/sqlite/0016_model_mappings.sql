CREATE TABLE model_mappings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    alias TEXT NOT NULL UNIQUE CHECK (length(alias) BETWEEN 1 AND 200),
    provider_id INTEGER NOT NULL REFERENCES providers(id) ON DELETE RESTRICT,
    upstream_model_id TEXT NOT NULL CHECK (length(upstream_model_id) BETWEEN 1 AND 200),
    openai_chat INTEGER NOT NULL CHECK (openai_chat IN (0, 1)),
    openai_responses INTEGER NOT NULL CHECK (openai_responses IN (0, 1)),
    anthropic_messages INTEGER NOT NULL CHECK (anthropic_messages IN (0, 1)),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_at INTEGER NOT NULL,
    CHECK (openai_chat OR openai_responses OR anthropic_messages)
);
