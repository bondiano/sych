# sych-storage

Durable local storage: SQLite (WAL, a single connection) for runs, result pointers and the
model ledger, and a content-addressed artifact store for immutable run documents.

## What's inside

| Module | Purpose |
|---|---|
| `lib` | `Storage`: `migrate` (the only operation that changes the schema), `open` (refuses an unmigrated or newer DB), the run lifecycle (`begin_run` → `complete_run`/`fail_run`/`cancel_run`), `put_artifact`/`load_artifact` |
| `artifacts` | `ArtifactStore`: files keyed by SHA-256 with atomic writes |
| `ledger` | ModelGateway cost ledger: atomic reservations, idempotent settlement, `UNKNOWN_CHARGE` |
| `gc` | `prune`: orphaned and expired artifacts; active runs and retention are protected |

## Invariants

- A failed or cancelled run never becomes `COMPLETED`.
- An artifact is immutable: identical content has the same address; a document is read via `decode` with a
  `schema_version` check.
- Access is partitioned by tenant and access scope (`Partition`); the same hash does not grant access from another partition.
- Retention: source data (snapshot, diff) — 7 days, results and evidence — 30 days.

Errors are `StorageError` (`thiserror`); `tracing` records migrations and the run lifecycle (started, completed,
failed).

Migrations live in `migrations/sqlite/` at the repository root.

## Checks

```sh
cargo test -p sych-storage
```
