-- Model attempt ledger and subscription fees (spec §17.2, §20.2). Money is integer micro-USD.
-- An attempt holds `reserved_micro_usd` until settled with a known charge; RELEASED means the
-- request never left Sych. Token columns are NULL when unknown, never zero.

CREATE TABLE model_attempts (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs (id),
    purpose TEXT NOT NULL,
    kind TEXT NOT NULL,
    template TEXT NOT NULL,
    profile TEXT NOT NULL,
    provider TEXT NOT NULL,
    requested_model TEXT NOT NULL,
    resolved_model TEXT,
    generation_id TEXT,
    billing_kind TEXT NOT NULL,
    enforcement TEXT NOT NULL,
    day TEXT NOT NULL,
    month TEXT NOT NULL,
    reserved_micro_usd INTEGER NOT NULL,
    input_tokens_bound INTEGER NOT NULL,
    -- NULL: the route has no enforced output cap (subscription_observable).
    output_tokens_bound INTEGER,
    state TEXT NOT NULL CHECK (state IN ('RESERVED', 'SETTLED', 'UNKNOWN', 'RELEASED')),
    -- NULL on SETTLED: the subscription reported no money for the request.
    charge_micro_usd INTEGER,
    reference_micro_usd INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    reasoning_tokens INTEGER,
    billing_mismatch INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    settled_at INTEGER
);

CREATE INDEX model_attempts_by_run ON model_attempts (run_id);
CREATE INDEX model_attempts_by_period ON model_attempts (tenant_id, month, day);

-- One row per subscription and calendar month (UTC): the fee is counted once, not per request.
CREATE TABLE subscription_fees (
    tenant_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    month TEXT NOT NULL,
    amount_micro_usd INTEGER NOT NULL,
    allocated_micro_usd INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, provider, month)
);
