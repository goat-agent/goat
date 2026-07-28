-- Integration anchoring and watcher state.
--
-- `goals.external_ref` ties a goal to an external work item
-- (e.g. `linear/default:issue:GOA-123`) so integration watchers can upsert
-- idempotently: one anchor per (persona, external item), re-polls update it
-- in place instead of duplicating. Partial unique index keeps ordinary goals
-- (NULL ref) unconstrained.
--
-- `integration_state` holds one opaque JSON blob per
-- (persona, integration, account, stream) — cursors, watermarks and seen-sets
-- owned entirely by the vendor crate. The store never interprets `state`.

ALTER TABLE goals ADD COLUMN external_ref TEXT;

CREATE UNIQUE INDEX idx_goals_external_ref
    ON goals(persona_id, external_ref) WHERE external_ref IS NOT NULL;

CREATE TABLE integration_state (
    persona_id  TEXT NOT NULL REFERENCES personas(id),
    integration TEXT NOT NULL,
    account     TEXT NOT NULL,
    stream      TEXT NOT NULL,
    state       TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (persona_id, integration, account, stream)
);
