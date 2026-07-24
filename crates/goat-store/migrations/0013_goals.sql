-- Goals / intentions — the spine of autonomy. A goal is a persistent objective
-- the assistant works toward across turns, reviewed on a schedule
-- (`next_review_at`). Goals may come from the owner or be self-formed. They are
-- addressable per persona (so a review can reach the owner on a channel) and
-- decompose into sub-goals via `parent`.

CREATE TABLE goals (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    persona_id     TEXT NOT NULL REFERENCES personas(id),
    title          TEXT NOT NULL,
    detail         TEXT,                       -- markdown body / acceptance criteria
    parent         INTEGER REFERENCES goals(id),
    status         TEXT NOT NULL DEFAULT 'active'
                     CHECK(status IN ('active','blocked','waiting','done','dropped')),
    priority       INTEGER NOT NULL DEFAULT 3, -- 1 (highest) .. 5 (lowest)
    origin         TEXT NOT NULL DEFAULT 'owner'
                     CHECK(origin IN ('owner','self')),
    origin_conv    TEXT REFERENCES conversations(id),  -- where to report progress
    next_review_at TEXT,                       -- when to re-examine (drives reviews)
    last_reviewed_at TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE INDEX idx_goals_persona_status ON goals(persona_id, status);
CREATE INDEX idx_goals_review
    ON goals(next_review_at) WHERE status = 'active' AND next_review_at IS NOT NULL;
