CREATE TABLE subscription_nodes (
    node_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    backend TEXT NOT NULL,
    models_json TEXT NOT NULL,
    concurrency BIGINT NOT NULL,
    encrypted_node_key TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    provider_id BIGINT REFERENCES providers(id) ON DELETE SET NULL,
    version BIGINT NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL
);
