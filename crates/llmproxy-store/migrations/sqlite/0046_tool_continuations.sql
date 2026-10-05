CREATE TABLE tool_continuations (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    encrypted_payload TEXT NOT NULL,
    payload_bytes INTEGER NOT NULL CHECK (payload_bytes >= 0),
    expires_at INTEGER NOT NULL
);
-- #[toasty::breakpoint]
CREATE INDEX tool_continuations_expiry_idx ON tool_continuations (expires_at);
