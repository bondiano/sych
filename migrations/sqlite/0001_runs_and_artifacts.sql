-- Runs, result pointers and immutable artifacts (spec §20.1). Nested domain objects live in
-- versioned JSON artifacts; tables hold identity, partition, state and pointers only.
-- Times are Unix seconds (UTC) so comparisons are numeric.

CREATE TABLE changes (
    id INTEGER PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    access_scope TEXT NOT NULL,
    change_key TEXT NOT NULL,
    desired_generation INTEGER NOT NULL,
    UNIQUE (tenant_id, access_scope, change_key)
);

CREATE TABLE runs (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    access_scope TEXT NOT NULL,
    change_id INTEGER NOT NULL REFERENCES changes (id),
    generation INTEGER NOT NULL,
    source_snapshot_id TEXT NOT NULL,
    policy_snapshot_id TEXT NOT NULL,
    state TEXT NOT NULL,
    stage TEXT NOT NULL,
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
    -- A completed run always points at a stored result.
    CHECK (state <> 'COMPLETED' OR (result_sha256 IS NOT NULL AND result_id IS NOT NULL))
);

CREATE TABLE artifacts (
    tenant_id TEXT NOT NULL,
    access_scope TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    kind TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    store_key TEXT NOT NULL,
    size INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    retain_until INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, access_scope, sha256)
);

CREATE TABLE run_artifacts (
    run_id TEXT NOT NULL REFERENCES runs (id),
    role TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    access_scope TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY (run_id, role)
);

CREATE INDEX run_artifacts_by_artifact ON run_artifacts (tenant_id, access_scope, sha256);
CREATE INDEX artifacts_by_retention ON artifacts (retain_until);
