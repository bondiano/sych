-- Publication objects and the durable outbox, and the audit log (spec §18.3–18.6, §20.1–20.2).
-- An object is one bot-owned slot on a change (the summary, an inline comment, a check run); an
-- operation is one desired revision of it. The rendered body and its digest are fixed when the
-- operation becomes PUBLISHING, before any network write.

CREATE TABLE publication_objects (
    id INTEGER PRIMARY KEY,
    change_id INTEGER NOT NULL REFERENCES changes (id),
    slot TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('SUMMARY', 'INLINE', 'CHECK')),
    -- Embedded in the body; reconciliation finds the object by it, never alone by text.
    marker TEXT NOT NULL,
    remote_id TEXT,
    desired_generation INTEGER NOT NULL,
    desired_result_id TEXT NOT NULL,
    observed_generation INTEGER,
    observed_body_sha256 TEXT,
    -- Delivery and relevance are separate: a delivered old comment is a fact that turns STALE.
    relevance TEXT NOT NULL DEFAULT 'CURRENT' CHECK (relevance IN ('CURRENT', 'STALE')),
    updated_at INTEGER NOT NULL,
    UNIQUE (change_id, slot)
);

CREATE TABLE publication_operations (
    id INTEGER PRIMARY KEY,
    object_id INTEGER NOT NULL REFERENCES publication_objects (id),
    run_id TEXT NOT NULL REFERENCES runs (id),
    result_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('PENDING', 'PUBLISHING', 'DELIVERED', 'UNCERTAIN',
        'FAILED', 'CANCELLED', 'SUPERSEDED')),
    attempt_id TEXT,
    request_digest TEXT,
    body TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    owner TEXT,
    lease_epoch INTEGER NOT NULL DEFAULT 0,
    lease_until INTEGER,
    remote_id TEXT,
    last_response TEXT,
    last_error TEXT,
    reconcile_deadline INTEGER,
    next_action_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    finished_at INTEGER,
    UNIQUE (object_id, generation)
);

CREATE INDEX publication_operations_ready ON publication_operations (state, next_action_at);

CREATE TABLE audit_events (
    id INTEGER PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    target TEXT NOT NULL,
    correlation_id TEXT NOT NULL,
    -- Sanitized: no tokens, prompts or source text.
    metadata TEXT NOT NULL,
    at INTEGER NOT NULL
);

CREATE INDEX audit_events_by_tenant ON audit_events (tenant_id, at);
