-- Runtime flags: a tiny key/value table for daemon-wide switches the CLI sets
-- and the background loops read. Currently holds the autonomy kill switch
-- (`paused`), the only governance control besides transparency.
CREATE TABLE runtime_flags (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
