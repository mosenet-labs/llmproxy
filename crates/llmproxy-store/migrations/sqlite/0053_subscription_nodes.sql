CREATE TABLE subscription_nodes (
    node_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    backend TEXT NOT NULL,
    models_json TEXT NOT NULL,
    concurrency INTEGER NOT NULL,
    encrypted_node_key TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    provider_id INTEGER REFERENCES providers(id) ON DELETE SET NULL,
    version INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);
