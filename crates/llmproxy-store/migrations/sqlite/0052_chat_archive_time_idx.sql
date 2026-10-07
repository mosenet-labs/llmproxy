CREATE INDEX chat_conversations_archive_time_idx ON chat_conversations(owner, archived, updated_at DESC, id);
