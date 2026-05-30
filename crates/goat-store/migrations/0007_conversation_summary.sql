-- Rolling conversation summary for context compaction. Holds a running
-- summary of the oldest messages in a conversation plus a watermark
-- (`summarized_count`) counting how many text messages, in ts-ascending
-- order, are already folded into that summary. Messages are append-only,
-- so the count is a stable offset.

CREATE TABLE conversation_summary (
    conversation_id  TEXT PRIMARY KEY REFERENCES conversations(id),
    persona_id       TEXT NOT NULL REFERENCES personas(id),
    summary          TEXT NOT NULL,
    summarized_count INTEGER NOT NULL,
    updated_at       TEXT NOT NULL
);
