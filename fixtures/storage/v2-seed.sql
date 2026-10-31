-- A database at schema v2 (runs, artifacts and the model ledger) as an early `sych review` left it:
-- one completed run with its result pointer, a pinned artifact and an UNKNOWN model charge.
INSERT INTO changes (id, tenant_id, access_scope, change_key, desired_generation)
VALUES (1, 'local', '/repo', 'main...HEAD', 1);
INSERT INTO runs (id, tenant_id, access_scope, change_id, generation, source_snapshot_id,
    policy_snapshot_id, state, stage, result_id, result_sha256, model_attempts, input_tokens,
    output_tokens, cost_known_micro_usd, unknown_cost_attempts, created_at, finished_at)
VALUES ('01a17470-0000-7000-8000-000000000001', 'local', '/repo', 1, 1,
    'sha256:1111111111111111111111111111111111111111111111111111111111111111',
    'sha256:2222222222222222222222222222222222222222222222222222222222222222',
    'COMPLETED', 'REDUCE', '01a17470-0000-7000-8000-000000000002',
    'sha256:3333333333333333333333333333333333333333333333333333333333333333',
    2, 1000, 200, 1500, 1, 1790000000, 1790000060);
INSERT INTO artifacts (tenant_id, access_scope, sha256, kind, schema_version, store_key, size,
    created_at, retain_until)
VALUES ('local', '/repo', 'sha256:3333333333333333333333333333333333333333333333333333333333333333',
    'review_result', 1, 'local/x/33', 10, 1790000000, 1792592000);
INSERT INTO run_artifacts (run_id, role, tenant_id, access_scope, sha256)
VALUES ('01a17470-0000-7000-8000-000000000001', 'review_result', 'local', '/repo',
    'sha256:3333333333333333333333333333333333333333333333333333333333333333');
INSERT INTO model_attempts (id, tenant_id, run_id, purpose, kind, template, profile, provider,
    requested_model, billing_kind, enforcement, day, month, reserved_micro_usd,
    input_tokens_bound, output_tokens_bound, state, created_at)
VALUES ('01a17470-0000-7000-8000-000000000003', 'local', '01a17470-0000-7000-8000-000000000001',
    'ANALYSIS', 'INITIAL', 'review@3', 'cheap_api', 'openrouter', 'deepseek/deepseek-v4.1-flash',
    'api_usage', 'api_hard', '2026-09-21', '2026-09', 1500, 1000, 2000, 'UNKNOWN', 1790000000);
