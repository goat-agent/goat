CREATE TABLE proxy_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    source TEXT NOT NULL,
    provider TEXT NOT NULL,
    account TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    error_kind TEXT
);

CREATE INDEX idx_proxy_requests_ts ON proxy_requests (ts);
CREATE INDEX idx_proxy_requests_provider_ts ON proxy_requests (provider, ts);

CREATE TABLE proxy_rate_limits (
    provider TEXT NOT NULL,
    account TEXT NOT NULL,
    snapshot TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (provider, account)
);
