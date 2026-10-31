# sych-storage

Durable storage: PostgreSQL for the server or SQLite/WAL for the autonomous CLI, using the same
`Storage` methods and one-connection SQLx Any pool. Immutable run documents remain in the
content-addressed artifact directory, not in the database.

## What's inside

| Module | Purpose |
|---|---|
| `lib` | `Storage`: `migrate` (the only operation that changes the schema), `open` (refuses an unmigrated or newer DB), the run lifecycle (`begin_run` → `complete_run`/`fail_run`/`cancel_run`), `put_artifact`/`load_artifact` |
| `artifacts` | `ArtifactStore`: files keyed by SHA-256 with atomic writes |
| `ledger` | ModelGateway cost ledger: atomic reservations, idempotent settlement, `UNKNOWN_CHARGE` |
| `gc`, `retention` | `prune`: orphaned and expired artifacts; row retention; active runs and unresolved charges/publications are protected |
| `connection`, `sql` | backend connections, explicit migrations, the shared single-writer transaction; shared SQL fragments |
| `import` | transactional import of a stopped SQLite snapshot into an empty PostgreSQL store |
| `admission` | webhook receipt, repository binding and scheduling intent in one transaction |
| `scheduler`, `jobs` | change state, generations and the event log; durable jobs with leases, reaper and fence |
| `checkpoints` | accepted model answers pinned under the run's fence for successor reuse |
| `outbox`, `publish`, `recreate` | publication intents committed with the result; the outbox state machine; audited return of `UNCERTAIN` to `PENDING` |
| `lineage` | finding lineage, append-only history, feedback and waivers |
| `api` | idempotency keys, run/publication read models and `/metrics` numbers |
| `tour_sessions` | stored tours and each reader's own session |

## Invariants

- A failed or cancelled run never becomes `COMPLETED`.
- An artifact is immutable: identical content has the same address; a document is read via `decode` with a
  `schema_version` check.
- Access is partitioned by tenant and access scope (`Partition`); the same hash does not grant access from another partition.
- Retention: source data (snapshot, diff) — 7 days, results and evidence — 30 days.

Errors are `StorageError` (`thiserror`); `tracing` records migrations and the run lifecycle (started, completed,
failed).

Migrations live in `migrations/sqlite/` and `migrations/postgres/` at the repository root.
Only `Storage::migrate` creates files/schema. PostgreSQL writes take a transaction-scoped advisory
lock before business reads; SQLite uses `BEGIN IMMEDIATE`. Both preserve one-writer semantics,
including across independent pools; this is not a multi-worker scaling change.

## Checks

```sh
cargo test --locked -p sych-storage
# Disposable local PostgreSQL admin database; each fixture creates its own database.
SYCH_TEST_POSTGRES_URL=postgres://postgres:sych-test@127.0.0.1:55432/postgres just test-postgres
```
