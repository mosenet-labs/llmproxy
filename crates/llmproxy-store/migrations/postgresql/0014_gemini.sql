ALTER TABLE providers ADD COLUMN gemini_path TEXT;

ALTER TABLE model_mappings ADD COLUMN gemini BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE model_mappings DROP CONSTRAINT model_mappings_check;
ALTER TABLE model_mappings ADD CONSTRAINT model_mappings_protocol_check CHECK (openai_chat OR openai_responses OR anthropic_messages OR gemini);

ALTER TABLE model_routes DROP CONSTRAINT model_routes_v2_protocol_check;
ALTER TABLE model_routes ADD CONSTRAINT model_routes_protocol_check CHECK (protocol IN ('openai_chat', 'openai_responses', 'anthropic_messages', 'gemini'));
