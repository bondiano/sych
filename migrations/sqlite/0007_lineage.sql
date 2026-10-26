-- Finding lineage and lifecycle, structured feedback and scoped waivers (spec §5.6, §11.6,
-- §18.4). History is append-only; a waiver never rewrites an obligation status.

CREATE TABLE findings (
    id INTEGER PRIMARY KEY,
    change_id INTEGER NOT NULL REFERENCES changes (id),
    fingerprint TEXT NOT NULL,
    finding_id TEXT NOT NULL,
    category TEXT NOT NULL,
    rule_id TEXT,
    path TEXT,
    state TEXT NOT NULL CHECK (state IN ('OPEN', 'RESOLVED', 'DISMISSED', 'OBSOLETE')),
    first_run_id TEXT NOT NULL,
    last_run_id TEXT NOT NULL,
    -- The finding of an earlier revision this one continues (rename, move).
    predecessor_id INTEGER REFERENCES findings (id),
    updated_at INTEGER NOT NULL,
    UNIQUE (change_id, fingerprint)
);

CREATE TABLE finding_events (
    id INTEGER PRIMARY KEY,
    finding_id INTEGER NOT NULL REFERENCES findings (id),
    run_id TEXT,
    from_state TEXT,
    to_state TEXT NOT NULL,
    reason TEXT NOT NULL,
    actor TEXT,
    -- Evidence ids or obligation ids the transition rests on, as JSON.
    evidence TEXT NOT NULL DEFAULT '[]',
    at INTEGER NOT NULL
);

CREATE INDEX finding_events_by_finding ON finding_events (finding_id, id);

ALTER TABLE runs ADD COLUMN lineage_applied INTEGER NOT NULL DEFAULT 0;

CREATE TABLE feedback (
    id INTEGER PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    change_id INTEGER NOT NULL REFERENCES changes (id),
    finding_id INTEGER NOT NULL REFERENCES findings (id),
    actor TEXT NOT NULL,
    judgement TEXT NOT NULL CHECK (judgement IN ('USEFUL', 'NOT_USEFUL', 'FALSE_POSITIVE', 'WONT_FIX')),
    scope TEXT NOT NULL,
    rationale TEXT NOT NULL,
    policy_snapshot_id TEXT,
    at INTEGER NOT NULL
);

CREATE TABLE waivers (
    id INTEGER PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    repository_id INTEGER NOT NULL REFERENCES repositories (id),
    rule_id TEXT NOT NULL,
    paths TEXT NOT NULL,
    reason TEXT NOT NULL,
    owner TEXT NOT NULL,
    author TEXT NOT NULL,
    expires_at INTEGER,
    until TEXT,
    policy_snapshot_id TEXT,
    created_at INTEGER NOT NULL,
    revoked_at INTEGER,
    CHECK (expires_at IS NOT NULL OR until IS NOT NULL)
);
