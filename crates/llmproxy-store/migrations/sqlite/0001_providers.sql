CREATE TABLE providers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 80),
    protocol TEXT NOT NULL CHECK (protocol IN ('openai_chat', 'openai_responses', 'anthropic_messages')),
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
    updated_at INTEGER NOT NULL
);
