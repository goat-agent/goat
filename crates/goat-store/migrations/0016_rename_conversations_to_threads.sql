ALTER TABLE conversations RENAME TO threads;
ALTER TABLE conversation_summary RENAME TO thread_summary;
ALTER TABLE messages RENAME COLUMN conversation_id TO thread_id;
ALTER TABLE tool_invocations RENAME COLUMN conversation_id TO thread_id;
ALTER TABLE thread_summary RENAME COLUMN conversation_id TO thread_id;
