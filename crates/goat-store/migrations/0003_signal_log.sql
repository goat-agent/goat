-- Self-tick idempotency: every signal fire writes a row keyed by
-- blake3(persona || kind || canonical_reason). Re-fires are gated by
-- ttl_seconds — NULL means "permanent" (PendingIntent: fire once).

CREATE TABLE signal_log (
    idempotency_key BLOB PRIMARY KEY,
    persona_id      TEXT NOT NULL REFERENCES personas(id),
    kind            TEXT NOT NULL,
    reason          TEXT NOT NULL,
    ts              TEXT NOT NULL,
    ttl_seconds     INTEGER
);
CREATE INDEX idx_signal_persona_ts ON signal_log(persona_id, ts);
