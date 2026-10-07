CREATE TABLE chat_conversations (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    title TEXT NOT NULL,
    selection_json TEXT NOT NULL,
    thinking TEXT NOT NULL,
    active_turn TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    version BIGINT NOT NULL DEFAULT 0
);
