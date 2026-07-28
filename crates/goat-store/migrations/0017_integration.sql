-- Watcher state for external-service integrations. One opaque JSON blob per
-- (persona, integration, account, stream) — cursors, watermarks and seen-sets
-- owned entirely by the vendor crate. The store never interprets `state`.

CREATE TABLE integration_state (
    persona_id  TEXT NOT NULL REFERENCES personas(id),
    integration TEXT NOT NULL,
    account     TEXT NOT NULL,
    stream      TEXT NOT NULL,
    state       TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (persona_id, integration, account, stream)
);
