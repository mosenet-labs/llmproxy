CREATE TABLE model_mappings_gemini (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    alias TEXT NOT NULL UNIQUE CHECK (length(alias) BETWEEN 1 AND 200),
    provider_id INTEGER NOT NULL REFERENCES providers(id) ON DELETE RESTRICT,
    upstream_model_id TEXT NOT NULL CHECK (length(upstream_model_id) BETWEEN 1 AND 200),
    openai_chat INTEGER NOT NULL CHECK (openai_chat IN (0, 1)),
    openai_responses INTEGER NOT NULL CHECK (openai_responses IN (0, 1)),
    anthropic_messages INTEGER NOT NULL CHECK (anthropic_messages IN (0, 1)),
    gemini INTEGER NOT NULL CHECK (gemini IN (0, 1)),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_at INTEGER NOT NULL,
    input_price_per_million TEXT,
    output_price_per_million TEXT,
    CHECK (openai_chat OR openai_responses OR anthropic_messages OR gemini)
);
-- #[toasty::breakpoint]
INSERT INTO model_mappings_gemini (id, alias, provider_id, upstream_model_id, openai_chat, openai_responses, anthropic_messages, gemini, version, updated_at, input_price_per_million, output_price_per_million)
SELECT id, alias, provider_id, upstream_model_id, openai_chat, openai_responses, anthropic_messages, 0, version, updated_at, input_price_per_million, output_price_per_million FROM model_mappings;
-- #[toasty::breakpoint]
DROP TABLE model_mappings;
-- #[toasty::breakpoint]
ALTER TABLE model_mappings_gemini RENAME TO model_mappings;
-- #[toasty::breakpoint]
CREATE INDEX model_mappings_provider_id_idx ON model_mappings(provider_id);
