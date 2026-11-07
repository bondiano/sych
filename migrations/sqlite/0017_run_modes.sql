-- no-transaction
-- A run is a REVIEW of the change's generation or a PLAYGROUND try a maintainer asked for: the
-- latter shares the generation it was asked at, supersedes nothing, publishes nothing and stays
-- out of lineage and analytics. So a generation is unique among reviews only; SQLite keeps an
-- inline UNIQUE as an automatic index, and `runs` is rebuilt as 0004 did.

PRAGMA foreign_keys = OFF;
BEGIN;

CREATE TABLE runs_new (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    access_scope TEXT NOT NULL,
    change_id INTEGER NOT NULL REFERENCES changes (id),
    generation INTEGER NOT NULL,
    parent_run_id TEXT,
    inputs TEXT,
    inputs_digest TEXT,
    source_snapshot_id TEXT,
    policy_snapshot_id TEXT,
    state TEXT NOT NULL,
    stage TEXT NOT NULL,
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
    lineage_applied INTEGER NOT NULL DEFAULT 0,
    coverage TEXT,
    unknown_obligations INTEGER,
    mode TEXT NOT NULL DEFAULT 'REVIEW' CHECK (mode IN ('REVIEW', 'PLAYGROUND')),
    CHECK (state <> 'COMPLETED' OR (result_sha256 IS NOT NULL AND result_id IS NOT NULL))
);

INSERT INTO runs_new (id, tenant_id, access_scope, change_id, generation, parent_run_id, inputs,
    inputs_digest, source_snapshot_id, policy_snapshot_id, state, stage, evidence_deadline,
    expected_sources, result_id, result_sha256, model_attempts, input_tokens, output_tokens,
    cost_known_micro_usd, unknown_cost_attempts, failure, created_at, finished_at,
    lineage_applied, coverage, unknown_obligations)
SELECT id, tenant_id, access_scope, change_id, generation, parent_run_id, inputs,
    inputs_digest, source_snapshot_id, policy_snapshot_id, state, stage, evidence_deadline,
    expected_sources, result_id, result_sha256, model_attempts, input_tokens, output_tokens,
    cost_known_micro_usd, unknown_cost_attempts, failure, created_at, finished_at,
    lineage_applied, coverage, unknown_obligations
FROM runs;

DROP TABLE runs;
ALTER TABLE runs_new RENAME TO runs;
CREATE INDEX runs_by_change ON runs (change_id, generation);
CREATE UNIQUE INDEX runs_review_generation ON runs (change_id, generation) WHERE mode = 'REVIEW';

COMMIT;
PRAGMA foreign_keys = ON;
