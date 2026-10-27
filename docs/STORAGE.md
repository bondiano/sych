# Local storage

Implements roadmap M0.5: §8.2, §20 and §24.4 of the specification.

## Components

- **SQLite** (`sych-storage`): WAL, `synchronous=FULL`, one connection — one writer and one worker.
  Tables `changes`, `runs`, `artifacts`, `run_artifacts`; ledger — `model_attempts`, `subscription_fees`;
  server (M2) — `repositories`, `inbox_events`, `change_intents`, `jobs`, `change_events`, `publication_objects`,
  `publication_operations`, `audit_events` ([SERVER.md](SERVER.md)). Nested domain
  objects are not decomposed into SQL.
- **Artifacts**: canonical JSON in a local directory, keyed by the SHA-256 of the bytes, with the path partitioned by tenant/access scope.
  The file is written to a temporary file, `fsync`ed, then `rename`d; the hash is verified on every read.
- Paths: `storage.database_url_env` (value `sqlite://PATH`, `sqlite:PATH` or a path) and
  `storage.artifact_store.path` are relative to the `sych.yaml` directory (absolute paths stay absolute), not the CWD; without them — `./.sych-data/sych.db` and `./.sych-data/artifacts`.

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

## Server run finalization (M2.4)

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
| `change_events`, `audit_events`, `model_attempts` (`SETTLED`/`RELEASED`), publication operations (final) | 90 days |
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
migration the data and foreign keys are intact. Backward compatibility is not promised before R1, but data in existing databases
is carried over.

## Backup

Locally it is enough to stop `sych` and copy the whole `.sych-data/` directory (the database, `-wal` and artifacts
together). This is the documented procedure for personal mode, not a verified corporate restore.
