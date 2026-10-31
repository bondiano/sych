# Deployment model

`.sych/deploy.yaml` (path: `policy.deployment_file`) describes one environment's rollout, as declared by its
owner. Sych never derives it from Git: `diff_base` and `target_tip` are not the deployed version. Every claim is
`{value, source}`; a claim with an empty `source` counts as not declared. Unknown fields are rejected. Schemas:
[deployment-model](../schemas/deployment-model.schema.json), [transition-matrix](../schemas/transition-matrix.schema.json).

```yaml
schema_version: 1
environment: staging
owner: payments
initial_versions:
  value:
    app_image: ghcr.io/acme/orders@sha256:…      # what actually runs
    source_commit: "<40-hex commit it was built from>"
    schema_phase: expanded
    supported_clients: ["web >= 2026.10.01", "billing-consumer v3"]
  source: argocd app orders-staging, 2026-10-20
order: {value: [schema, app, frontend, jobs], source: deploy/README.md}
mixed_versions: {value: true, source: "rolling update, maxSurge 1"}
downtime: {value: false, source: SLO doc}
compatibility_window: {value: 14d, source: frontend release policy}
backfill: {value: "batched, checkpointed by id", source: jobs/backfill.go}
contract: {value: "drop after backfill gate + 14d", source: RFC-31}
recovery: {value: roll_forward_repair, source: runbook RB-12}   # compatible_rollback | roll_forward_repair | restore_replay | other
excluded_states:
  - {value: old_frontend_new_backend, source: "server-rendered, no long-lived tabs"}
observed: {value: "backfill done up to id 120000", source: "staging job log, 2026-10-20"}   # optional
```

## Transition matrix

Built from the model ([`deploy::matrix`](../crates/sych-core/src/deploy.rs)); every state below is
reachable unless a sourced constraint excludes it. A missing description never makes a state `NOT_APPLICABLE`.

| State | What must hold |
|---|---|
| `old_app_expanded_schema` | old readers and writers keep working |
| `mixed_apps` | data, API and events stay compatible between versions (excluded by `mixed_versions: false` with a source) |
| `old_frontend_new_backend` | the declared contract of the old client is supported |
| `partial_backfill_concurrent_writes` | fresher data is not lost and both forms are handled |
| `backfill_retry` | the backfill is idempotent or resumes correctly |
| `contract_phase` | the old path is no longer needed within the compatibility window |
| `recovery_after_new_writes` | the restored version handles the actual data state |

## What the environment owner provides

What is not declared is reported in `missing_inputs`; obligations that depend on it stay `UNKNOWN`, unrelated
static review is not blocked.

| Gap | Owner input |
|---|---|
| `ENVIRONMENT_OWNER` | `environment`, `owner` |
| `INITIAL_VERSION_UNKNOWN` | `initial_versions` with a source |
| `BUILD_SOURCE_UNKNOWN` | `app_image` together with its `source_commit` |
| `ORDER_UNKNOWN` | `order` of schema/app/frontend/jobs |
| `COMPATIBILITY_WINDOW_UNKNOWN` | `compatibility_window` |
| `RECOVERY_UNKNOWN` | `recovery` path |
| `EXTERNAL_CONSUMERS_UNKNOWN` | `supported_clients`: searching this repository does not prove a field removal safe |

DB engine, migration framework and controller semantics (locks, transactions, ordering) are prerequisites the
owner chooses; Sych does not pick them and keeps them outside the confirmed scope until a DB-specific profile exists.

## Migration rules

The generic `MIG-*` rules activate on `**/migrations/**`, `**/migrate/**` and `**/*.sql` and are bound to the
matrix. Every one carries `evaluation_mode: PLAN_REVIEW`: the proposed order and the mechanisms that enforce its
preconditions (for example a gate on backfill completion) are judged; `SUPPORTED` means the plan is conditionally
sound, not that a backfill or drain has run. `OBSERVED_TRANSITION` needs evidence of the environment's actual
state: only a sourced `observed` entry of `deploy.yaml` (what the environment reports, e.g. how far a backfill got)
switches the obligations to it, and the criterion asks to judge the change against that reported state. A plan never
stands in for an observation.

**Context.** For each changed migration Sych scans the DDL that affects existing code (`DROP`/`RENAME`/`ALTER`/
`ADD COLUMN`, `DROP TABLE`, `RENAME TO`) and searches both sides of the repository for the table or column name (at most
20 hits per side, outside migration paths): the base side holds the old app's readers and writers, the head the new
app's. Those locations become the obligation's code references, so the reviewer sees them, and a limitation states the
count and that consumers outside the repository are not searched. Earlier migrations count too (they define the schema being changed); `ADD [CONSTRAINT …] CHECK (col …)` is read as a
change of `col`. Each hit is shown with up to 12 lines around it, clamped to the file, so the assessor sees the
function, not the matching line alone. What the owner declared (order, mixed versions, compatibility window, backfill,
contract, recovery, with their sources) is an acceptance criterion of every migration obligation: the assessor judges
the change against it rather than asking for it. The matrix itself is derived evidence
(`sych-transition-matrix`, kind `contract`) that every migration obligation cites.

| Rule | Rollout states it covers | Owner inputs it cannot do without |
|---|---|---|
| `MIG-COMPAT` | `old_app_expanded_schema`, `mixed_apps`, `old_frontend_new_backend` | — |
| `MIG-RESTART` | `partial_backfill_concurrent_writes`, `backfill_retry` | — |
| `MIG-CONTRACT` | `contract_phase` | compatibility window, supported clients/consumers |
| `MIG-RECOVERY` | `recovery_after_new_writes` | recovery path |

The reachable states become acceptance criteria of the obligation. When all of its states are excluded by sourced
constraints, the obligation is `NOT_APPLICABLE` with those sources as the basis. A missing input it cannot do
without makes it `UNKNOWN` with `DEPLOYMENT_INPUT_MISSING` and the named gap; other missing inputs are listed as
limitations. A reverse migration or an old image is not treated as proof of recovery: the recovery criterion asks
whether the chosen path handles the data written after the change.
