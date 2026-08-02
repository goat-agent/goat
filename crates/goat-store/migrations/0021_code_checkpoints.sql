ALTER TABLE code_messages ADD COLUMN parent_message_id INTEGER;
ALTER TABLE code_threads ADD COLUMN head_message_id INTEGER;

UPDATE code_messages
SET parent_message_id = (
    SELECT previous.id
    FROM code_messages AS previous
    WHERE previous.thread_id = code_messages.thread_id
      AND previous.id < code_messages.id
    ORDER BY previous.id DESC
    LIMIT 1
);

UPDATE code_threads
SET head_message_id = (
    SELECT MAX(code_messages.id)
    FROM code_messages
    WHERE code_messages.thread_id = code_threads.id
);

CREATE TABLE code_checkpoints (
    id INTEGER PRIMARY KEY,
    thread_id INTEGER NOT NULL,
    prompt_message_id INTEGER NOT NULL,
    parent_message_id INTEGER,
    draft TEXT NOT NULL,
    attachments TEXT NOT NULL,
    files_available INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_code_checkpoints_thread ON code_checkpoints(thread_id);
CREATE UNIQUE INDEX idx_code_checkpoints_prompt ON code_checkpoints(prompt_message_id);

CREATE TABLE code_checkpoint_blobs (
    hash TEXT PRIMARY KEY,
    content BLOB NOT NULL
);

CREATE TABLE code_checkpoint_files (
    checkpoint_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    present INTEGER NOT NULL,
    blob_hash TEXT,
    mode INTEGER,
    supported INTEGER NOT NULL DEFAULT 1,
    touched INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (checkpoint_id, path),
    FOREIGN KEY (checkpoint_id) REFERENCES code_checkpoints(id) ON DELETE CASCADE
);
CREATE INDEX idx_code_checkpoint_files_path ON code_checkpoint_files(path);
