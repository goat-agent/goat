-- Rich memory: core (always-in-context blocks), episodic (event log + embeddings),
-- semantic (markdown source-of-truth + embeddings). All persona-scoped.

CREATE TABLE core_memory (
    persona_id  TEXT NOT NULL REFERENCES personas(id),
    slug        TEXT NOT NULL,
    text        TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (persona_id, slug)
);

CREATE TABLE episodic_memory (
    id              TEXT PRIMARY KEY,
    persona_id      TEXT NOT NULL REFERENCES personas(id),
    conversation_id TEXT NOT NULL REFERENCES conversations(id),
    kind            TEXT NOT NULL CHECK (kind IN ('user','assistant','observation')),
    text            TEXT NOT NULL,
    embedding       BLOB,  -- little-endian f32 array; NULL until embedder available
    ts              TEXT NOT NULL
);
CREATE INDEX idx_episodic_persona_ts ON episodic_memory(persona_id, ts);
CREATE INDEX idx_episodic_conv_ts    ON episodic_memory(conversation_id, ts);

CREATE TABLE semantic_memory (
    id          TEXT PRIMARY KEY,
    persona_id  TEXT NOT NULL REFERENCES personas(id),
    topic       TEXT NOT NULL,
    text        TEXT NOT NULL,
    source_path TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    embedding   BLOB NOT NULL,
    UNIQUE (persona_id, topic)
);
CREATE INDEX idx_semantic_persona ON semantic_memory(persona_id);
