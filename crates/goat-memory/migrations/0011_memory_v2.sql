-- Memory v2: markdown files own prose, `facts` owns discrete claims, and the
-- SQLite tables below are a derived, rebuildable search index over both.
-- Scope is assistant-global, keyed by a scope string ('owner' | 'self' |
-- 'domain:<name>'), NOT by persona. The dimension-dependent vec0 table
-- (`mem_vec`) is created at runtime once the embedding dimension is known.

-- Source of truth for discrete claims. Bi-temporal: a contradicted fact is
-- invalidated (invalid_at set, superseded_by pointed at the replacement),
-- never rewritten in place. Current beliefs = invalid_at IS NULL.
CREATE TABLE facts (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    scope         TEXT NOT NULL,
    subject       TEXT,
    text          TEXT NOT NULL,
    origin        TEXT NOT NULL,          -- 'owner_stated' | 'inferred' | 'consolidated'
    source_kind   TEXT NOT NULL,          -- 'message' | 'note' | 'manual'
    source_ref    TEXT NOT NULL,          -- message id or note path (provenance)
    stated_at     TEXT NOT NULL,
    valid_from    TEXT,
    invalid_at    TEXT,                   -- NULL = currently believed true
    superseded_by INTEGER REFERENCES facts(id),
    importance    REAL NOT NULL DEFAULT 0.5,
    strength      REAL NOT NULL DEFAULT 1.0
);
CREATE INDEX idx_facts_scope_current ON facts(scope) WHERE invalid_at IS NULL;
CREATE INDEX idx_facts_subject       ON facts(scope, subject);

-- Derived search index. Rebuildable from memory files + the facts table.
-- One row per chunk; `chunk_key` is a stable identity (heading-based for
-- notes/core, 'fact:<id>') so recall stats survive reindex/rechunk.
CREATE TABLE mem_index (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    scope      TEXT NOT NULL,
    kind       TEXT NOT NULL,             -- 'core' | 'note' | 'journal' | 'fact'
    source_ref TEXT NOT NULL,             -- file-relative path, or 'fact:<id>'
    chunk_key  TEXT NOT NULL,
    chunk_no   INTEGER NOT NULL DEFAULT 0,
    text       TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_mem_index_scope  ON mem_index(scope);
CREATE INDEX idx_mem_index_source ON mem_index(scope, source_ref);

-- Plain (non external-content) FTS5 over the same chunk text. Duplicating the
-- text avoids the external-content sync-protocol footgun; the corpus is small.
CREATE VIRTUAL TABLE mem_fts USING fts5(text, scope UNINDEXED, index_id UNINDEXED);

-- Durable recall-reinforcement stats, keyed by stable chunk identity.
CREATE TABLE recall_stats (
    scope            TEXT NOT NULL,
    chunk_key        TEXT NOT NULL,
    recall_count     INTEGER NOT NULL DEFAULT 0,
    last_recalled_at TEXT NOT NULL,
    PRIMARY KEY (scope, chunk_key)
);

-- Single-row metadata for the vector index: embedding model + dimension it was
-- built with. A mismatch at boot triggers a full reindex.
CREATE TABLE mem_index_meta (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    embed_model   TEXT NOT NULL,
    embed_dim     INTEGER NOT NULL,
    built_at      TEXT NOT NULL
);
