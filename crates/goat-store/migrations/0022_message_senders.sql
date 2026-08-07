ALTER TABLE messages ADD COLUMN sender_kind TEXT CHECK(sender_kind IN ('user', 'agent'));
ALTER TABLE messages ADD COLUMN sender_key TEXT CHECK((sender_kind IS NULL) = (sender_key IS NULL));
ALTER TABLE messages ADD COLUMN sender_display TEXT CHECK(sender_display IS NULL OR sender_kind = 'user');
ALTER TABLE messages ADD COLUMN attachments TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(attachments) AND json_type(attachments) = 'array');
UPDATE messages SET sender_kind = 'agent', sender_key = agent_id WHERE direction = 'out';
CREATE INDEX idx_messages_sender ON messages(sender_kind, sender_key, ts);
