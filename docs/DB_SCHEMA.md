# Database and application schema linter

Sych includes a deterministic database and application schema linter for **PostgreSQL**, **MySQL** and **SQLite**.
It checks structural database hygiene (keys, foreign key indexes, duplicate and prefix-redundant indexes, nullable
booleans, narrow integer keys, tenant/required columns) and cross-validates the database schema against application
ORM models (unmapped objects, required fields on nullable columns, unique fields without unique indexes, relations
without foreign keys, unenforced `max_length` and enum values).

It runs in three ways:

1. **Standalone CLI** (`sych schema …`) — locally or in CI, with text, JSON, Markdown and SARIF 2.1.0 outputs and a
   comment-preserving baseline workflow.
2. **Opt-in review integration** (`sych review` and GitHub App server) — off by default (`schema_lint.enabled: false`),
   emitting deterministic `DB-*` findings, `ContextBroker` observations, `block_on` gates and waiver support even with
   `--models fake`.
3. **Stateless HTTP API** (`GET /v1/schema/rules`, `POST /v1/schema/check`) — evaluates a schema snapshot and optional
   model manifest without touching live databases or storage.

## Standalone CLI

```sh
sych schema check --dsn postgres://readonly@localhost:5432/app
sych schema check --sql "migrations/postgres/*.sql" --dialect postgres --format sarif
sych schema check --snapshot db-snapshot.json --model-manifest models.json --format json
sych schema baseline --dsn sqlite://./app.db --config .sych/schema-lint.yaml
sych schema snapshot --dsn postgres://readonly@localhost:5432/app --output db-snapshot.json
sych schema extract --repo . --output models.json
sych schema rules [--dialect postgres|mysql|sqlite] [--format text|json]
sych schema explain fk-without-index
sych schema manifest-schema
```

| Subcommand | Does |
|---|---|
| `sych schema check` | Evaluate a live database (`--dsn`), saved snapshot (`--snapshot`) or static SQL files (`--sql` / auto-discovered) against rules and application models. |
| `sych schema baseline` | Append current findings as exact exclusions into `.sych/schema-lint.yaml` (`--config`), preserving existing comments and formatting; idempotent on repeat runs (`--dry-run` prints to stdout). |
| `sych schema snapshot` | Export a normalized `DbSchemaSnapshot` JSON document from `--dsn` or static SQL for CI artifacts or `/v1/schema/check`. |
| `sych schema extract` | Export a normalized `DbModelManifest` JSON document using built-in static ORM extractors or a local `--adapter`. |
| `sych schema rules` | List all 18 built-in rules, optionally filtered by `--dialect`. |
| `sych schema explain <RULE>` | Explain a rule's rationale, fix, valid exceptions, options and dialects (accepts `fk-without-index` or `DB-FK-WITHOUT-INDEX`). |
| `sych schema manifest-schema` | Print the JSON Schema for `DbModelManifest`. |

### Exit codes (`sych schema`)

| Code | When |
|---:|---|
| 0 | Check passed with zero findings and zero stale exclusions (or non-check command succeeded) |
| 1 | `sych schema check` reported one or more findings or stale exclusions |
| 2 | Invalid CLI flags, configuration, DSN, SQL parse options or unknown rule |
| 3 | Live database inspection failure, external adapter failure or I/O error |

## Schema and model sources

### Database schema sources

- **Live read-only inspection (`--dsn`)**: connects read-only to PostgreSQL (`default_transaction_read_only=on`,
  `statement_timeout`), MySQL (`SET SESSION TRANSACTION READ ONLY`, `max_execution_time`) and SQLite
  (`read_only(true)`, `create_if_missing(false)`). Credentials are always redacted from error messages and logs.
- **Static SQL DDL reconstruction (`--sql` / auto-discovery)**: parses SQL schema and migration files offline,
  tracking file and line positions for every table, column, index and foreign key. Unparsed or procedural statements
  (such as `DO $$ … $$` blocks) are never ignored silently — they are recorded in `limitations`.
- **Saved / CI snapshots (`--snapshot`, `db_schema_snapshot`)**: portable `DbSchemaSnapshot` JSON produced by
  `sych schema snapshot` in CI and imported via `--ci-facts` + `--ci-manifest` or `ci.expected_reports`.

### Application model sources

- **Built-in static ORM extractors** (zero code execution; run identically in local CLI and server reviews over pinned
  Git blobs):
  - Go: `gorm`, `bun`, `ent`
  - Prisma: `prisma` (`**/*.prisma`)
  - TypeScript: `drizzle`
  - Python: `sqlalchemy`, `django`
  - Rust: `diesel`, `seaorm`
- **Explicit JSON manifest (`--model-manifest` / `db_model_manifest`)**: versioned `DbModelManifest`
  (`schema_version: 1`; `version: 1` accepted as input alias). Inspect the exact contract with
  `sych schema manifest-schema` or [schemas/db-model-manifest.schema.json](../schemas/db-model-manifest.schema.json).
- **External adapter CLI (`--adapter <name>`, standalone local CLI only)**: executes `sych-schema-adapter-<name>` from
  `PATH` (with repeatable `--adapter-arg`) and reads a `DbModelManifest` JSON document from stdout. External adapters
  are **never** executed during `sych review` or in server mode.

## Built-in rules

| Rule ID | Review ID | Category | Dialects | Default | Severity | Checks |
|---|---|---|---|---|---|---|
| `table-without-primary-key` | `DB-TABLE-WITHOUT-PRIMARY-KEY` | database | pg, mysql, sqlite | on | high | Table has no primary key |
| `fk-without-index` | `DB-FK-WITHOUT-INDEX` | database | pg, mysql, sqlite | on | high | Foreign key columns lack a covering non-partial, non-prefix index |
| `duplicate-index` | `DB-DUPLICATE-INDEX` | database | pg, mysql, sqlite | on | medium | Two indexes have identical definitions |
| `prefix-redundant-index` | `DB-PREFIX-REDUNDANT-INDEX` | database | pg, mysql, sqlite | on | medium | Non-unique index is a strict left-prefix of another index with matching order/method/predicate |
| `nullable-boolean` | `DB-NULLABLE-BOOLEAN` | database | pg, mysql, sqlite | on | low | Boolean column allows `NULL` |
| `narrow-key` | `DB-NARROW-KEY` | database | pg, mysql, sqlite | on | medium | Primary key or FK-referenced integer key is narrower than 64-bit (`smallint`/`integer`) |
| `plain-tables` | `DB-PLAIN-TABLES` | model | pg, mysql, sqlite | on | medium | Database table, column or FK is missing from the application model (`options.ignore_tables` supported) |
| `required-column` | `DB-REQUIRED-COLUMN` | database | pg, mysql, sqlite | off | high | Enforces a required column (e.g. `tenant_id`) via the five child rules below (`options: {name, reference_table, reference_column, types, exclude_tables}`) |
| `required-column-missing` | `DB-REQUIRED-COLUMN-MISSING` | database | pg, mysql, sqlite | child | high | Table is missing the configured required column |
| `required-column-nullable` | `DB-REQUIRED-COLUMN-NULLABLE` | database | pg, mysql, sqlite | child | high | Configured required column allows `NULL` |
| `required-column-no-foreign-key` | `DB-REQUIRED-COLUMN-NO-FOREIGN-KEY` | database | pg, mysql, sqlite | child | high | Configured required column has no FK to `reference_table(reference_column)` |
| `required-column-no-leading-index` | `DB-REQUIRED-COLUMN-NO-LEADING-INDEX` | database | pg, mysql, sqlite | child | high | Table has no index leading with the required column |
| `required-column-unique-index` | `DB-REQUIRED-COLUMN-UNIQUE-INDEX` | database | pg, mysql, sqlite | child | high | Unique index does not include or lead with the required column |
| `required-field-nullable-column` | `DB-REQUIRED-FIELD-NULLABLE-COLUMN` | model | pg, mysql, sqlite | on | high | Non-nullable application model field maps to a nullable database column |
| `unique-field-without-unique-index` | `DB-UNIQUE-FIELD-WITHOUT-UNIQUE-INDEX` | model | pg, mysql, sqlite | on | high | Unique application model field lacks a matching unique index or primary key |
| `relation-without-foreign-key` | `DB-RELATION-WITHOUT-FOREIGN-KEY` | model | pg, mysql, sqlite | on | high | Application model relation lacks a matching database foreign key |
| `max-length-not-enforced` | `DB-MAX-LENGTH-NOT-ENFORCED` | model | pg, mysql, sqlite | on | medium | Model `max_length` is not enforced by column type length or `CHECK` constraint |
| `enum-not-enforced` | `DB-ENUM-NOT-ENFORCED` | model | pg, mysql, sqlite | on | medium | Model enum values are not enforced by a DB enum type or `CHECK` constraint |

A stale exclusion that no longer matches any active finding fails standalone checks and emits `DB-STALE-EXCLUSION` in
review mode so resolved tech debt is cleaned up rather than silently masking regressions.

## Configuration and exclusions

Standalone `sych schema check` reads `.sych/schema-lint.yaml` by default (override with `--config PATH`):

```yaml
rules:
  nullable-boolean:
    enabled: false
  fk-without-index:
    severity: critical
  plain-tables:
    options:
      ignore_tables: ["schema_migrations", "_prisma_migrations"]
  required-column:
    enabled: true
    options:
      name: tenant_id
      reference_table: tenants
      reference_column: id
      exclude_tables: ["tenants", "schema_migrations"]

models:
  extractors: [auto]          # or explicit list: [gorm, prisma, drizzle, ...], or [none]

exclusions:
  - rule: fk-without-index
    object: public.events.fk_events_user_id
    reason: "Legacy backfill table; index scheduled in #412"
```

Every exclusion names an exact `(rule, object)` pair and a non-empty `reason`. Use `sych schema baseline` to record
existing findings when adopting the linter on an established database.

## Review pipeline integration

Schema linting in `sych review` is **opt-in** (`schema_lint.enabled: false` by default) and follows Sych's trusted
policy model: `.sych/config.yaml` and optional `.sych/schema-lint.yaml` (`policy.schema_lint_file`) are read strictly
from the **target tip**, so a PR cannot weaken rules or hide violations with exclusions.

```yaml
# .sych/config.yaml (on the target branch)
schema_version: 1
policy:
  schema_lint_file: .sych/schema-lint.yaml   # optional; merged with inline schema_lint rules/exclusions
review:
  block_on:
    - DB-FK-WITHOUT-INDEX
    - DB-TABLE-WITHOUT-PRIMARY-KEY
schema_lint:
  enabled: true
  dialect: postgres                          # optional hint for static SQL
  sql_paths: ["migrations/postgres/*.sql"]   # optional globs; auto-discovered when omitted
  # ci_report: db-schema                     # optional ci.expected_reports[].id (format: db_schema_snapshot)
  # model_ci_report: db-models               # optional ci.expected_reports[].id (format: db_model_manifest)
  # connection: staging                      # optional read-only connection from server sych.yaml
```

Database source resolution order during a review:

1. Local CLI `--db-dsn` (personal local mode only; automatically enables schema linting for that run unless
   `--schema-lint off` is passed) or trusted server `schema_lint.connections.<name>` referenced by
   `schema_lint.connection`.
2. Bound CI `db_schema_snapshot` artifact (`--ci-facts` + `--ci-manifest` locally, or `ci.expected_reports` with
   `format: db_schema_snapshot` on the server).
3. Static SQL DDL reconstruction from pinned Git blobs on **both** `diff_base` and `head` — only `NEW` findings
   introduced by the change become `RuleViolation` findings; existing debt (`SAME`) and resolved items (`fixed`) stay
   visible in `db_schema_facts` and the Markdown `## Database schema` section.

### Blocking and waivers

Every finding carries `rule_id: DB-<UPPER_KEBAB>` (or `DB-STALE-EXCLUSION`) so `review.block_on` works natively
(including globs such as `DB-*`). Active exceptions on that rule id in `.sych/rules.yaml` or server waivers recorded
via `POST /v1/waivers` lower the finding to `IMPROVEMENT` at `LOW` and clear blocking while keeping the finding and its
waiver reason visible.

### Server-side read-only connections

An operator may optionally configure named read-only database connections in trusted `sych.yaml`:

```yaml
schema_lint:
  connections:
    staging:
      dsn_env: SYCH_STAGING_SCHEMA_DSN
      repositories: ["your-org/your-repo"]   # required on a server; empty denies all repositories
      timeout_seconds: 30
```

A repository outside `repositories`, an unknown connection name or a missing environment variable records a failed
`DbSchemaFacts` target and a `DB_SCHEMA_UNAVAILABLE` task gap (coverage `PARTIAL`) without leaking DSN credentials.

## Stateless HTTP API

| Endpoint | Role | Description |
|---|---|---|
| `GET /v1/schema/rules` | `reader` | Lists all 18 built-in rules (`SchemaRulesResponse`). |
| `POST /v1/schema/check` | `reader` | Evaluates `{schema_version, snapshot, manifest?, config?}` (`SchemaCheckRequest`) and returns `DbSchemaReport`. Stateless; rejects invalid configs/manifests with `400 INVALID_REQUEST` and never connects to a live database. |
