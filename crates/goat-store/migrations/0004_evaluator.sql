-- Evaluator-driven model routing stats. One row per (persona × provider × model).
-- The brain accumulates `score_sum` (sum of evaluator scores in [0,1]) and
-- `latency_ms_sum`; `route_for("auto")` divides them by `n_calls` to pick.

CREATE TABLE model_scores (
    persona_id      TEXT NOT NULL REFERENCES personas(id),
    provider        TEXT NOT NULL,
    model_id        TEXT NOT NULL,
    n_calls         INTEGER NOT NULL DEFAULT 0,
    score_sum       REAL    NOT NULL DEFAULT 0.0,
    latency_ms_sum  INTEGER NOT NULL DEFAULT 0,
    last_seen       TEXT,
    PRIMARY KEY (persona_id, provider, model_id)
);
