-- API idempotency (spec §25.2) and run outcome columns for metrics (spec §26.4).

CREATE TABLE idempotency_keys (
    id INTEGER PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    key TEXT NOT NULL,
    body_sha256 TEXT NOT NULL,
    -- NULL while the first request is being handled.
    response_status INTEGER,
    response_body TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    UNIQUE (tenant_id, actor, endpoint, key)
);

ALTER TABLE runs ADD COLUMN coverage TEXT;
ALTER TABLE runs ADD COLUMN unknown_obligations INTEGER;
