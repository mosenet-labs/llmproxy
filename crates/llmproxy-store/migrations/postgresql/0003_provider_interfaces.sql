ALTER TABLE providers
    ADD COLUMN openai_chat_path TEXT,
    ADD COLUMN openai_responses_path TEXT,
    ADD COLUMN anthropic_messages_path TEXT,
    ADD COLUMN models_path TEXT NOT NULL DEFAULT '/models',
    ADD COLUMN models_auth TEXT NOT NULL DEFAULT 'bearer';

UPDATE providers SET
    openai_chat_path = CASE WHEN protocol = 'openai_chat' THEN '/v1/chat/completions' END,
    openai_responses_path = CASE WHEN protocol = 'openai_responses' THEN '/v1/responses' END,
    anthropic_messages_path = CASE WHEN protocol = 'anthropic_messages' THEN '/v1/messages' END,
    models_auth = CASE WHEN protocol = 'anthropic_messages' THEN 'anthropic' ELSE 'bearer' END;

ALTER TABLE providers DROP COLUMN protocol;
