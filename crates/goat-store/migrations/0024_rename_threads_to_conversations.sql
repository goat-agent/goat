ALTER TABLE threads RENAME TO conversations;
ALTER TABLE thread_summary RENAME TO conversation_summary;
ALTER TABLE messages RENAME COLUMN thread_id TO conversation_id;
ALTER TABLE tool_invocations RENAME COLUMN thread_id TO conversation_id;
ALTER TABLE conversation_summary RENAME COLUMN thread_id TO conversation_id;
DROP INDEX idx_threads_agent;
CREATE INDEX idx_conversations_agent ON conversations(agent_id);
