-- Preserve AUTOINCREMENT high-water marks, including IDs of deleted resources.
CREATE TABLE resource_group_migration_sequences (name TEXT PRIMARY KEY, seq BIGINT NOT NULL);
-- #[toasty::breakpoint]
INSERT INTO resource_group_migration_sequences SELECT name, seq FROM sqlite_sequence WHERE name IN ('model_mappings', 'model_routes');
-- #[toasty::breakpoint]
CREATE TABLE model_group_memberships (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id BIGINT NOT NULL REFERENCES groups(id) ON DELETE RESTRICT,
    model_id BIGINT NOT NULL REFERENCES model_mappings(id) ON DELETE CASCADE,
    UNIQUE (group_id, model_id)
);
-- #[toasty::breakpoint]
CREATE TABLE model_mappings_shared (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
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
    CHECK (openai_chat OR openai_responses OR anthropic_messages OR gemini)
);
-- #[toasty::breakpoint]
INSERT INTO model_mappings_shared (id, alias, provider_id, upstream_model_id, openai_chat, openai_responses, anthropic_messages, gemini, version, updated_at, input_price_per_million, output_price_per_million, thinking_json) SELECT id, alias, provider_id, upstream_model_id, openai_chat, openai_responses, anthropic_messages, gemini, version, updated_at, input_price_per_million, output_price_per_million, thinking_json FROM model_mappings;
-- #[toasty::breakpoint]
DROP TABLE model_mappings;
-- #[toasty::breakpoint]
ALTER TABLE model_mappings_shared RENAME TO model_mappings;
-- #[toasty::breakpoint]
CREATE INDEX model_group_memberships_model_id_idx ON model_group_memberships(model_id);
-- #[toasty::breakpoint]
CREATE TABLE route_group_memberships (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id BIGINT NOT NULL REFERENCES groups(id) ON DELETE RESTRICT,
    route_id BIGINT NOT NULL REFERENCES model_routes(id) ON DELETE CASCADE,
    UNIQUE (group_id, route_id)
);
-- #[toasty::breakpoint]
CREATE TABLE model_routes_shared (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    protocol TEXT NOT NULL CHECK (protocol IN ('openai_chat', 'openai_responses', 'anthropic_messages', 'gemini')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_at INTEGER NOT NULL,
    provider_protocol TEXT NOT NULL DEFAULT ''
);
-- #[toasty::breakpoint]
INSERT INTO model_routes_shared (id, name, protocol, enabled, version, updated_at, provider_protocol) SELECT id, name, protocol, enabled, version, updated_at, provider_protocol FROM model_routes;
-- #[toasty::breakpoint]
DROP TABLE model_routes;
-- #[toasty::breakpoint]
ALTER TABLE model_routes_shared RENAME TO model_routes;
-- #[toasty::breakpoint]
CREATE INDEX route_group_memberships_route_id_idx ON route_group_memberships(route_id);
-- #[toasty::breakpoint]
CREATE INDEX model_mappings_provider_id_idx ON model_mappings(provider_id);
-- #[toasty::breakpoint]
INSERT INTO sqlite_sequence (name, seq) SELECT name, seq FROM resource_group_migration_sequences AS saved WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = saved.name);
-- #[toasty::breakpoint]
UPDATE sqlite_sequence SET seq = MAX(seq, COALESCE((SELECT seq FROM resource_group_migration_sequences WHERE name = sqlite_sequence.name), 0)) WHERE name IN ('model_mappings', 'model_routes');
-- #[toasty::breakpoint]
DROP TABLE resource_group_migration_sequences;
