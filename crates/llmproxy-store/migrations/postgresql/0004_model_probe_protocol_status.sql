ALTER TABLE providers
    ADD COLUMN models_protocol TEXT NOT NULL DEFAULT 'openai_chat',
    ADD COLUMN models_probe_status TEXT NOT NULL DEFAULT 'unprobed';

UPDATE providers SET models_protocol = CASE
    WHEN models_auth = 'anthropic' AND anthropic_messages_path IS NOT NULL THEN 'anthropic_messages'
    WHEN models_auth = 'bearer' AND openai_chat_path IS NOT NULL THEN 'openai_chat'
    WHEN models_auth = 'bearer' AND openai_responses_path IS NOT NULL THEN 'openai_responses'
    WHEN openai_chat_path IS NOT NULL THEN 'openai_chat'
    WHEN openai_responses_path IS NOT NULL THEN 'openai_responses'
    ELSE 'anthropic_messages'
END;

ALTER TABLE providers DROP COLUMN models_auth;
