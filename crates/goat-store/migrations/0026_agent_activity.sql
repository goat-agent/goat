CREATE TABLE agent_activity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    run_id INTEGER NOT NULL,
    detail TEXT,
    ok INTEGER,
    at INTEGER NOT NULL
);

CREATE INDEX idx_agent_activity_agent ON agent_activity(agent_id, id);
