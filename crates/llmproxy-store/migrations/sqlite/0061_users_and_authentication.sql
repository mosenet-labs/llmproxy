CREATE TABLE users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    email TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'user')),
    enabled INTEGER NOT NULL,
    email_verified INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_login_at INTEGER,
    version INTEGER NOT NULL DEFAULT 0
);
-- #[toasty::breakpoint]
CREATE TABLE user_sessions (
    digest TEXT PRIMARY KEY NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users(id),
    csrf TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
-- #[toasty::breakpoint]
CREATE INDEX user_sessions_user_id_idx ON user_sessions(user_id);
-- #[toasty::breakpoint]
CREATE TABLE email_challenges (
    scope TEXT PRIMARY KEY NOT NULL,
    encrypted_code TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL,
    used INTEGER NOT NULL
);
