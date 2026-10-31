# Storage

## Components

- **Database** (`sych-storage`): PostgreSQL for the server; SQLite/WAL with `synchronous=FULL` for the local CLI.
  Both use the same business queries and a one-connection pool. Writes serialize before business reads:
  SQLite `BEGIN IMMEDIATE`, PostgreSQL READ COMMITTED plus a transaction-scoped advisory lock.
  This retains one-writer semantics, not multi-worker scaling.
  Tables `changes`, `runs`, `artifacts`, `run_artifacts`; ledger — `model_attempts`, `subscription_fees`;
  server — `repositories`, `inbox_events`, `change_intents`, `jobs`, `change_events`, `publication_objects`,
  `publication_operations`, `audit_events`, `sessions` ([SERVER.md](SERVER.md#web-ui-sign-in-and-audit)); `tour_sessions` — one reader's progress through a
  stored tour, no shared state ([LOCAL_REVIEW.md](LOCAL_REVIEW.md#reading-a-tour)). Nested domain
  objects are not decomposed into SQL.
- **Artifacts**: canonical JSON in a local directory, keyed by the SHA-256 of the bytes, with the path partitioned by tenant/access scope.
  The file is written to a temporary file, `fsync`ed, then `rename`d; the hash is verified on every read.
- `storage.database_url_env` accepts a `postgres://…` or `postgresql://…` URL for PostgreSQL; invalid or unsupported
  URLs fail without falling back to SQLite. PostgreSQL connection strings are redacted in logs and doctor output.
  SQLite values (`sqlite://PATH`, `sqlite:PATH` or a raw path) and `storage.artifact_store.path` resolve relative
  to `sych.yaml` (absolute paths stay absolute), not the CWD. Without configuration the CLI uses
  `./.sych-data/sych.db` and `./.sych-data/artifacts`.

## Invariants

- Only `sych db migrate` changes the schema. All other commands open the database only at the current schema, otherwise exit 3
  with a hint; they do not touch a newer schema either.
- A run is created `RUNNING` with the next generation of its change. Each artifact first gets a row and a pin
  to the run, then its bytes are written.
- The result pointer, usage and `COMPLETED` are committed in one transaction, and only if the result bytes are already
  readable and pass the hash check. A failure yields `FAILED`, an accepted cancellation (SIGINT/Ctrl-C) — `CANCELLED`, never `COMPLETED`.
- Artifact roles of a single review: `source_snapshot`, `diff_manifest`, `policy_snapshot`, `policy_review`
  (provenance and head policy diff), `external_input_manifest`, `evidence/<id>`, `evidence_manifest`, `review_result`, and also
  `checkpoint/<key>` — an accepted model response with a dependency record (written under the fence right after the response).
- `sych show --run ID` and `sych explain` read these artifacts and do not run analysis. A deleted or corrupted artifact
  is explicitly unavailable (`ArtifactUnavailable`/`Corrupt`); nothing is restored by regeneration. The exception is
  the source snapshot and diff manifest, which have a shorter retention: after they are deleted, `show`/`explain` still work,
  and `snapshot`/`diff_manifest` in JSON are `null` (in Markdown — "unavailable").

## Server run finalization

The authoritative commit of a run is a single transaction with a predicate:

- the run is `RUNNING` with frozen inputs (`stage = ANALYZE`), and tenant and access scope match;
- the job is `CLAIMED` by the same owner with the same `lease_epoch`, and the lease is alive (`lease_until ≥ now`);
- `run.generation = change.desired_generation` — a push between the read and the commit does not make the old run current;
- the result bytes are already stored under this run and are readable with a matching hash.

In the same transaction: the result pointer, `COMPLETED`, usage, job `DONE` and the outbox intent (if the connection publishes).
Every artifact pin of a server run also carries the fence: a late worker pins nothing. External git/LSP/model/SCM
calls do not hold the transaction. Cancel/supersede revokes the lease (`lease_epoch + 1`); a committed `COMPLETED` is not
rewritten.

| Failure | Outcome |
|---|---|
| lease expired before commit | commit rejected, the reaper hands the job to a new owner with a new epoch |
| push during analysis | the old run is `SUPERSEDED`, its commit and pins are rejected |
| result bytes lost before the pointer | `complete_run` rejected, the run is not `COMPLETED`, no intent |
| inputs not frozen | commit rejected |
| cancel before commit | commit and intent rejected |
| process death after commit | job `DONE`: the reaper does not return it, no re-analysis and no model attempts; the `PENDING` intent waits for the publisher |
| DB failure mid-transaction (outbox insert) | rollback: no pointer, no `DONE`, no intent; the run stays `RUNNING` |

## Retention and GC

| What | Period |
|---|---|
| source snapshot, diff manifest | 7 days |
| policy snapshot/review, evidence, manifests, review result | 30 days |
| artifacts of an active run (`QUEUED…FINALIZING`) or of a run with a `PENDING/PUBLISHING/UNCERTAIN` publication | not deleted |
| artifacts without a completed/active run (failure, unfinished commit) | after a 1-hour grace period |
| mirrors (`<data>/mirrors/…`, source cache) | 7 days since the last fetch; not while the repository is in an active run |
| `change_intents` (consumed), `inbox_events` without a live intent, `jobs` (terminal) | 30 days |
| `change_events`, `model_attempts` (`SETTLED`/`RELEASED`), publication operations (final) | 90 days |
| `sessions` | once expired (8 hours after sign-in) |
| `audit_events` | not deleted |
| `model_attempts` `RESERVED`/`UNKNOWN`, operations `PENDING/PUBLISHING/UNCERTAIN` | not deleted by age |
| `runs` rows (usage/audit) | not deleted; 90 days is the lower bound |

`sych cache prune --dry-run` prints the plan (artifacts, row counts per table with a rule, mirrors) and deletes
nothing; without the flag, artifacts are deleted one by one under the write lock with a re-check of pin and retention, and rows
in one transaction. A pin or a retention extension accepted by another process after the initial listing keeps the
artifact. Raw CI artifacts are not stored at all: the zip is read in memory, and only the normalized report is kept.

## Schema compatibility

The schema is changed only by `sych db migrate`. 0004 rebuilds `runs` (snapshot/policy ids became nullable) using the
SQLite procedure with foreign keys disabled inside a single transaction. `cargo test -p sych-storage --test upgrade`
brings up a v2 database from `fixtures/storage/v2-seed.sql` (completed run, pin, `UNKNOWN` charge) and verifies that after
migration the data and foreign keys are intact. Backward compatibility is not promised yet, but data in existing databases
is carried over.

PostgreSQL has its own migration history, starting with `migrations/postgres/0001_storage.sql`.
It stores the same 22 application tables; domain IDs and JSON remain text, integer data remains signed 64-bit.
SQLite 0011 and PostgreSQL 0002 keep a finding's severity and title for the API lists; findings recorded
before them stay `NULL` until a later revision touches them. SQLite 0012 and PostgreSQL 0003 add browser
`sessions` and the repository of an audited action. Opening either backend refuses missing or newer migrations. Switching the configured URL does not copy history
or artifact files.

## Moving server history to PostgreSQL

Stop every writer and GC. Take a consistent SQLite backup and retain the matching artifact files.
If the snapshot is older, run `db migrate` on the **copy**, never on the original. Unknown or newer
schemas must be handled by the matching reviewed build, not forced through import.

Point `pg.yaml` at the empty PostgreSQL target and the copied artifact directory, then run:

```sh
sych db migrate --config pg.yaml
sych db import-sqlite --source /protected/snapshot/sych.db --config pg.yaml
```

Import never migrates either database. It requires the current SQLite schema and all PostgreSQL
application tables empty; a populated or repeated target is refused. One write transaction copies
all 22 tables, restores predecessor links, resets identities, and compares ordered row counts/digests.
Every extant artifact row must have its expected safe key, size and matching file hash. Already-pruned
metadata references remain unavailable; missing files are not regenerated. Any error rolls back the
imported rows; per-table counts/digests are logged only after commit. Source data and files stay untouched.

Keep writers stopped until the verified cutover. Once the PostgreSQL server can write, recovery must
preserve PostgreSQL and its files and fix forward; the original SQLite is stale, not a safe rollback.

## Backup

For SQLite, stop all writers and GC, then use SQLite's backup API for a consistent database snapshot and copy
the artifact directory from the same stopped interval. Preserve the original database and WAL.
For PostgreSQL, retain a database backup **and** the artifact files; neither the database backup nor a retained
PVC alone is a complete restore. This is not a verified corporate restore.

## Backend contracts

`just check` uses SQLite by default, including its upgrade and PRAGMA checks.
With a disposable local PostgreSQL 18.6 cluster, run:

```sh
SYCH_TEST_POSTGRES_URL=postgres://postgres:sych-test@127.0.0.1:55432/postgres just test-postgres
```

The recipe runs the application/storage contracts on isolated databases and then explicitly runs
the ignored PostgreSQL import/race cases. Do not supply a production URL. CI requires both suites
and pins the PostgreSQL service image by digest.
