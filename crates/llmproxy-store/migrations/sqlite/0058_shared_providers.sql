CREATE TABLE providers_shared (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    tls INTEGER NOT NULL CHECK (tls IN (0, 1)),
    encrypted_key TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    anthropic_version TEXT,
    connect_timeout_ms INTEGER NOT NULL CHECK (connect_timeout_ms > 0),
    read_timeout_ms INTEGER NOT NULL CHECK (read_timeout_ms > 0),
    write_timeout_ms INTEGER NOT NULL CHECK (write_timeout_ms > 0),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_at INTEGER NOT NULL,
    openai_chat_path TEXT,
    openai_responses_path TEXT,
    anthropic_messages_path TEXT,
    models_path TEXT NOT NULL DEFAULT '/models',
    models_protocol TEXT NOT NULL DEFAULT 'openai_chat',
    models_probe_status TEXT NOT NULL DEFAULT 'unprobed',
    messages_auth TEXT NOT NULL DEFAULT 'x-api-key',
    gemini_path TEXT
);
-- #[toasty::breakpoint]
INSERT INTO providers_shared (id, name, host, port, tls, encrypted_key, enabled, anthropic_version, connect_timeout_ms, read_timeout_ms, write_timeout_ms, version, updated_at, openai_chat_path, openai_responses_path, anthropic_messages_path, models_path, models_protocol, models_probe_status, messages_auth, gemini_path) SELECT id, name, host, port, tls, encrypted_key, enabled, anthropic_version, connect_timeout_ms, read_timeout_ms, write_timeout_ms, version, updated_at, openai_chat_path, openai_responses_path, anthropic_messages_path, models_path, models_protocol, models_probe_status, messages_auth, gemini_path FROM providers;
-- #[toasty::breakpoint]
DROP TABLE providers;
-- #[toasty::breakpoint]
ALTER TABLE providers_shared RENAME TO providers;
-- #[toasty::breakpoint]
CREATE INDEX providers_name_idx ON providers(name);
