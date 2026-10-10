CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'user')),
    enabled BOOLEAN NOT NULL,
    email_verified BOOLEAN NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    last_login_at BIGINT,
    version BIGINT NOT NULL DEFAULT 0
);
-- #[toasty::breakpoint]
CREATE TABLE user_sessions (
    digest TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    csrf TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    last_seen_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL
);
-- #[toasty::breakpoint]
CREATE INDEX user_sessions_user_id_idx ON user_sessions(user_id);
-- #[toasty::breakpoint]
CREATE TABLE email_challenges (
    scope TEXT PRIMARY KEY,
    encrypted_code TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    attempts BIGINT NOT NULL,
    used BOOLEAN NOT NULL
);
