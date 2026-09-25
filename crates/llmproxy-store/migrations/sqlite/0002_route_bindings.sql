CREATE TABLE route_bindings (
    protocol TEXT PRIMARY KEY CHECK (protocol IN ('openai_chat', 'openai_responses', 'anthropic_messages')),
    provider_id INTEGER REFERENCES providers(id) ON DELETE RESTRICT,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0)
);
