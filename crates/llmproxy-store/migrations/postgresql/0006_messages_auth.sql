ALTER TABLE providers
    ADD COLUMN messages_auth TEXT NOT NULL DEFAULT 'x-api-key';
