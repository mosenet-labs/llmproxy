CREATE TABLE model_routes_gemini (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    protocol TEXT NOT NULL CHECK (protocol IN ('openai_chat', 'openai_responses', 'anthropic_messages', 'gemini')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_at INTEGER NOT NULL,
    UNIQUE (name, protocol)
);
-- #[toasty::breakpoint]
INSERT INTO model_routes_gemini (id, name, protocol, enabled, version, updated_at)
SELECT id, name, protocol, enabled, version, updated_at FROM model_routes;
-- #[toasty::breakpoint]
DROP TABLE model_routes;
-- #[toasty::breakpoint]
ALTER TABLE model_routes_gemini RENAME TO model_routes;
