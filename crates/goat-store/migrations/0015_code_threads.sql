CREATE TABLE code_threads (
    id INTEGER PRIMARY KEY,
    cwd TEXT NOT NULL,
    title TEXT,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    account TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    effort TEXT
);
CREATE INDEX idx_code_threads_cwd ON code_threads(cwd);

CREATE TABLE code_turns (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL,
    task_id INTEGER NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    account TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    effort TEXT
);
CREATE INDEX idx_code_turns_thread ON code_turns(thread_id);

CREATE TABLE code_messages (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL,
    turn_id INTEGER,
    role TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_code_messages_thread ON code_messages(thread_id);

CREATE TABLE code_tool_calls (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL,
    turn_id INTEGER NOT NULL,
    call_id TEXT NOT NULL,
    name TEXT NOT NULL,
    input TEXT NOT NULL,
    status TEXT NOT NULL,
    summary TEXT,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX idx_code_tool_calls_thread ON code_tool_calls(thread_id);

CREATE TABLE code_compactions (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL,
    summary TEXT NOT NULL,
    after_message_id INTEGER NOT NULL,
    tail_from_message_id INTEGER,
    preserved_message_ids TEXT NOT NULL,
    tokens_before INTEGER NOT NULL,
    tokens_after INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_code_compactions_thread ON code_compactions(thread_id);

CREATE TABLE code_open_prompts (
    thread_id INTEGER NOT NULL,
    call_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    task_id INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (thread_id, call_id)
);

CREATE TABLE code_processes (
    id INTEGER PRIMARY KEY,
    pgid INTEGER NOT NULL,
    command TEXT NOT NULL,
    cwd TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX idx_code_processes_status ON code_processes(status);
