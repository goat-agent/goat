CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id                 INTEGER PRIMARY KEY,
    persona_id         TEXT NOT NULL REFERENCES personas(id),
    task               TEXT NOT NULL,
    tools              TEXT NOT NULL,
    origin_conv        TEXT NOT NULL REFERENCES conversations(id),
    schedule_kind      TEXT NOT NULL CHECK(schedule_kind IN ('once', 'cron')),
    once_at            TEXT,
    cron               TEXT,
    status             TEXT NOT NULL CHECK(status IN ('active', 'cancelled', 'done')),
    created_at         TEXT NOT NULL,
    created_by_msg_id  TEXT
);

CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_persona_status
    ON scheduled_tasks(persona_id, status);

CREATE TABLE IF NOT EXISTS task_runs (
    id                  INTEGER PRIMARY KEY,
    task_id             INTEGER NOT NULL REFERENCES scheduled_tasks(id),
    task_snapshot       TEXT NOT NULL,
    run_at              TEXT NOT NULL,
    started_at          TEXT,
    finished_at         TEXT,
    status              TEXT NOT NULL CHECK(status IN ('pending', 'running', 'done', 'failed', 'skipped')),
    running_since       TEXT,
    attempts            INTEGER NOT NULL DEFAULT 0,
    result_summary      TEXT
);

CREATE INDEX IF NOT EXISTS idx_task_runs_pending
    ON task_runs(status, run_at);
CREATE INDEX IF NOT EXISTS idx_task_runs_by_task
    ON task_runs(task_id, run_at DESC);
