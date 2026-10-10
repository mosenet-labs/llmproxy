CREATE TABLE mail_settings (
    id BIGINT PRIMARY KEY CHECK (id = 1),
    enabled BOOLEAN NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    tls TEXT NOT NULL CHECK (tls IN ('starttls', 'tls', 'none')),
    from_email TEXT NOT NULL,
    username TEXT NOT NULL,
    encrypted_password TEXT NOT NULL,
    version BIGINT NOT NULL DEFAULT 0
);
