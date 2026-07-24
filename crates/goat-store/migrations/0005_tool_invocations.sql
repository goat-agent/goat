CREATE TABLE IF NOT EXISTS tool_invocations (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    persona_id TEXT NOT NULL,
    call_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    args_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('ok', 'error')),
    output_preview TEXT,
    error TEXT,
    started_at TEXT NOT NULL,
    finished_at TEXT NOT NULL,
    FOREIGN KEY(conversation_id) REFERENCES conversations(id),
    FOREIGN KEY(persona_id) REFERENCES personas(id)
);

CREATE INDEX IF NOT EXISTS idx_tool_invocations_conversation
    ON tool_invocations(conversation_id, started_at);
CREATE INDEX IF NOT EXISTS idx_tool_invocations_persona
    ON tool_invocations(persona_id, started_at);
