-- no-transaction
-- Scheduler, durable jobs and the change event log (spec §8.1–8.5, §19.3, §20.1–20.2).
-- A server run is created QUEUED before its snapshot exists, so the snapshot and policy ids
-- become nullable; `runs` is rebuilt for that (SQLite cannot drop NOT NULL in place), with
-- foreign keys off around one explicit transaction, as SQLite's table-rebuild procedure says.

PRAGMA foreign_keys = OFF;
BEGIN;

CREATE TABLE runs_new (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    access_scope TEXT NOT NULL,
    change_id INTEGER NOT NULL REFERENCES changes (id),
    generation INTEGER NOT NULL,
    -- The run this one succeeds after a significant post-freeze input; never reopened.
    parent_run_id TEXT,
    -- Canonical JSON of what the run reviews (head, target, profile, trigger) and its digest.
    inputs TEXT,
    inputs_digest TEXT,
    source_snapshot_id TEXT,
    policy_snapshot_id TEXT,
    state TEXT NOT NULL,
    stage TEXT NOT NULL,
    -- WAITING_EVIDENCE ends here at the latest; the run then continues with gaps.
    evidence_deadline INTEGER,
    expected_sources TEXT,
    result_id TEXT,
    result_sha256 TEXT,
    model_attempts INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_known_micro_usd INTEGER NOT NULL DEFAULT 0,
    unknown_cost_attempts INTEGER NOT NULL DEFAULT 0,
    failure TEXT,
    created_at INTEGER NOT NULL,
    finished_at INTEGER,
    UNIQUE (change_id, generation),
    CHECK (state <> 'COMPLETED' OR (result_sha256 IS NOT NULL AND result_id IS NOT NULL))
);

INSERT INTO runs_new (id, tenant_id, access_scope, change_id, generation, source_snapshot_id,
    policy_snapshot_id, state, stage, result_id, result_sha256, model_attempts, input_tokens,
    output_tokens, cost_known_micro_usd, unknown_cost_attempts, failure, created_at, finished_at)
SELECT id, tenant_id, access_scope, change_id, generation, source_snapshot_id,
    policy_snapshot_id, state, stage, result_id, result_sha256, model_attempts, input_tokens,
    output_tokens, cost_known_micro_usd, unknown_cost_attempts, failure, created_at, finished_at
FROM runs;

DROP TABLE runs;
ALTER TABLE runs_new RENAME TO runs;
CREATE INDEX runs_by_change ON runs (change_id, generation);

-- The SCM state last read from the API; webhooks only signal that it may have changed.
ALTER TABLE changes ADD COLUMN scm_state TEXT;
ALTER TABLE changes ADD COLUMN head_sha TEXT;
ALTER TABLE changes ADD COLUMN base_sha TEXT;
ALTER TABLE changes ADD COLUMN base_ref TEXT;
ALTER TABLE changes ADD COLUMN fork INTEGER NOT NULL DEFAULT 0;
ALTER TABLE changes ADD COLUMN synced_at INTEGER;

-- One job per run task. `lease_epoch` grows at every claim; a write must present it (fence).
CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs (id),
    kind TEXT NOT NULL,
    task_key TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('QUEUED', 'CLAIMED', 'DONE', 'FAILED', 'CANCELLED')),
    owner TEXT,
    lease_epoch INTEGER NOT NULL DEFAULT 0,
    lease_until INTEGER,
    deadline INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL,
    next_attempt_at INTEGER NOT NULL,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    finished_at INTEGER,
    UNIQUE (run_id, kind, task_key)
);

CREATE INDEX jobs_ready ON jobs (state, next_attempt_at);

-- Why the scheduler did what it did: significance decisions, supersede, cancel, retries.
CREATE TABLE change_events (
    id INTEGER PRIMARY KEY,
    change_id INTEGER NOT NULL REFERENCES changes (id),
    run_id TEXT,
    kind TEXT NOT NULL,
    detail TEXT NOT NULL,
    at INTEGER NOT NULL
);

CREATE INDEX change_events_by_change ON change_events (change_id, id);

COMMIT;
PRAGMA foreign_keys = ON;
