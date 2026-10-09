CREATE TABLE groups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 80),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    version BIGINT NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL
);
-- #[toasty::breakpoint]
INSERT INTO groups (id, name, enabled, updated_at) VALUES (1, 'default', TRUE, 0);
-- #[toasty::breakpoint]
CREATE TABLE providers_grouped (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id BIGINT NOT NULL DEFAULT 1 REFERENCES groups(id) ON DELETE RESTRICT,
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
    gemini_path TEXT,
    UNIQUE (group_id, name)
);
-- #[toasty::breakpoint]
INSERT INTO providers_grouped (id, name, host, port, tls, encrypted_key, enabled, anthropic_version, connect_timeout_ms, read_timeout_ms, write_timeout_ms, version, updated_at, openai_chat_path, openai_responses_path, anthropic_messages_path, models_path, models_protocol, models_probe_status, messages_auth, gemini_path) SELECT id, name, host, port, tls, encrypted_key, enabled, anthropic_version, connect_timeout_ms, read_timeout_ms, write_timeout_ms, version, updated_at, openai_chat_path, openai_responses_path, anthropic_messages_path, models_path, models_protocol, models_probe_status, messages_auth, gemini_path FROM providers;
-- #[toasty::breakpoint]
DROP TABLE providers;
-- #[toasty::breakpoint]
ALTER TABLE providers_grouped RENAME TO providers;
-- #[toasty::breakpoint]
CREATE TABLE model_mappings_grouped (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id BIGINT NOT NULL DEFAULT 1 REFERENCES groups(id) ON DELETE RESTRICT,
    alias TEXT NOT NULL CHECK (length(alias) BETWEEN 1 AND 200),
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
    thinking_json TEXT,
    CHECK (openai_chat OR openai_responses OR anthropic_messages OR gemini),
    UNIQUE (group_id, alias)
);
-- #[toasty::breakpoint]
INSERT INTO model_mappings_grouped (id, alias, provider_id, upstream_model_id, openai_chat, openai_responses, anthropic_messages, gemini, version, updated_at, input_price_per_million, output_price_per_million, thinking_json) SELECT id, alias, provider_id, upstream_model_id, openai_chat, openai_responses, anthropic_messages, gemini, version, updated_at, input_price_per_million, output_price_per_million, thinking_json FROM model_mappings;
-- #[toasty::breakpoint]
DROP TABLE model_mappings;
-- #[toasty::breakpoint]
ALTER TABLE model_mappings_grouped RENAME TO model_mappings;
-- #[toasty::breakpoint]
CREATE INDEX model_mappings_provider_id_idx ON model_mappings(provider_id);
-- #[toasty::breakpoint]
CREATE TABLE model_routes_grouped (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id BIGINT NOT NULL DEFAULT 1 REFERENCES groups(id) ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    protocol TEXT NOT NULL CHECK (protocol IN ('openai_chat', 'openai_responses', 'anthropic_messages', 'gemini')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_at INTEGER NOT NULL,
    provider_protocol TEXT NOT NULL DEFAULT '',
    UNIQUE (group_id, name, protocol)
);
-- #[toasty::breakpoint]
INSERT INTO model_routes_grouped (id, name, protocol, enabled, version, updated_at, provider_protocol) SELECT id, name, protocol, enabled, version, updated_at, provider_protocol FROM model_routes;
-- #[toasty::breakpoint]
DROP TABLE model_routes;
-- #[toasty::breakpoint]
ALTER TABLE model_routes_grouped RENAME TO model_routes;
-- #[toasty::breakpoint]
CREATE TABLE virtual_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id BIGINT NOT NULL REFERENCES groups(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    digest TEXT NOT NULL UNIQUE,
    prefix TEXT NOT NULL,
    all_routes BOOLEAN NOT NULL,
    route_ids TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    expires_at BIGINT,
    created_at BIGINT NOT NULL,
    version BIGINT NOT NULL DEFAULT 0
);
-- #[toasty::breakpoint]
CREATE INDEX virtual_keys_group_idx ON virtual_keys(group_id);
