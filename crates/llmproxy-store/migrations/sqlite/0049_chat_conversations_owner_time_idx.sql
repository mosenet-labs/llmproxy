CREATE INDEX chat_conversations_owner_time_idx ON chat_conversations(owner, updated_at DESC, id);
