-- SCM repository bindings, the webhook inbox and coalesced change intents (spec §19.2–19.3,
-- §20.1–20.2). The inbox keeps the normalized event and the payload digest, not the raw body:
-- PR text is untrusted and is read again from the API when needed.

CREATE TABLE repositories (
    id INTEGER PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    provider_instance TEXT NOT NULL,
    external_id TEXT NOT NULL,
    full_name TEXT NOT NULL,
    installation_id INTEGER,
    -- ProviderCapabilities JSON, fixed when the binding is connected or updated.
    capabilities TEXT,
    capabilities_at INTEGER,
    UNIQUE (tenant_id, connection_id, provider_instance, external_id)
);

ALTER TABLE changes ADD COLUMN repository_id INTEGER REFERENCES repositories (id);
ALTER TABLE changes ADD COLUMN external_number INTEGER;

CREATE TABLE inbox_events (
    id INTEGER PRIMARY KEY,
    provider_instance TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    delivery_id TEXT NOT NULL,
    event TEXT NOT NULL,
    action TEXT,
    payload_sha256 TEXT NOT NULL,
    normalized TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    -- Why the event did or did not schedule work (significance decision).
    decision TEXT NOT NULL,
    UNIQUE (provider_instance, connection_id, delivery_id)
);

-- One open intent per change and kind; later events move `due_at` but never past `latest_at`.
CREATE TABLE change_intents (
    id INTEGER PRIMARY KEY,
    change_id INTEGER NOT NULL REFERENCES changes (id),
    kind TEXT NOT NULL,
    due_at INTEGER NOT NULL,
    latest_at INTEGER NOT NULL,
    events INTEGER NOT NULL,
    first_inbox_id INTEGER NOT NULL REFERENCES inbox_events (id),
    last_inbox_id INTEGER NOT NULL REFERENCES inbox_events (id),
    created_at INTEGER NOT NULL,
    consumed_at INTEGER,
    CHECK (due_at <= latest_at)
);

CREATE UNIQUE INDEX change_intents_open ON change_intents (change_id, kind)
    WHERE consumed_at IS NULL;
CREATE INDEX change_intents_due ON change_intents (due_at) WHERE consumed_at IS NULL;
