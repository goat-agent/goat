-- Lossless record of what an integration watcher saw at the moment it fired.
-- The analogue of `messages.raw` for external services: the full payload is
-- kept verbatim so nothing the watcher observed is ever lost, while memory
-- keeps only what the agent distills (facts reference rows here via
-- `observation:<id>`). Rows are integration-owned and append-only.

CREATE TABLE integration_observations (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    persona_id  TEXT NOT NULL REFERENCES personas(id),
    integration TEXT NOT NULL,
    account     TEXT NOT NULL,
    external_ref TEXT NOT NULL,
    kind        TEXT NOT NULL,
    payload     TEXT NOT NULL,
    observed_at TEXT NOT NULL
);

CREATE INDEX idx_observations_ref
    ON integration_observations(persona_id, integration, external_ref);
