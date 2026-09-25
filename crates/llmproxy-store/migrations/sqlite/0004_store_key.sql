CREATE TABLE store_keys (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    encrypted_verifier TEXT NOT NULL
);
