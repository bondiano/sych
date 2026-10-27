# Sych

A review tool with a deterministic Rust core and bounded model calls.
The source of requirements is [SYCH_SPEC.md](docs/SYCH_SPEC.md).
The implementation plan with dependencies, Acceptance Criteria and Definition of Done is [SYCH_ROADMAP.md](docs/SYCH_ROADMAP.md).

## Current status

The Git repository, Rust workspace, CLI with `--help`/`--version` and quality gates are in place.
Done:

- M0.1 — domain contracts in `sych-core` (IDs, Git OID with its algorithm, SHA-256 and canonical JSON, micro-USD,
  `schema_version`, snapshot/policy/evidence/obligation/finding/result DTOs) and JSON Schema from the same types;
- M0.2 — a safe Git snapshot in `sych-adapters`: separately pinned `target_tip`, `diff_base`, `head`,
  a full diff manifest (renames, deletions, Unicode/non-UTF-8, binary, generated, symlink, submodule, LFS,
  oversized blobs — with explicit limitations) and a CodeRef verified by blob and line hash;
- M0.3 — typed `sych.yaml` and `.sych/*` (unknown keys are an error with the exact path), effective config
  with hash and provenance, PolicySnapshot only from the target tip, `policy_diff` for head without applying it,
  default-deny secret paths and redaction, `sych config init --personal | validate` and `sych doctor`.
  Details — [docs/CONFIG.md](docs/CONFIG.md), examples — `examples/personal/`;
- M0.4 — a deterministic planner (review units by directory, risk hints, exclusions with reasons,
  an obligation for every invariant rule with three-valued applicability; the `max_source_files` limit lists
  the unchecked files), freezing of ExternalInputManifest/EvidenceManifest (late external evidence is rejected)
  and ResultReducer: SUPPORTED requires VALID complete evidence of an acceptable kind not only from the author,
  VIOLATED without a finding becomes RULE_VIOLATION, recommendation/coverage are computed per §5.7;
- M0.5 — `sych-storage`: SQLite (WAL, a single writer), content-addressed artifacts with hash verification,
  an atomic commit of the result pointer, `sych db migrate`, `sych show --run`, `sych cache prune [--dry-run]`
  with 7/30-day retention and protection of active runs. Details — [docs/STORAGE.md](docs/STORAGE.md);
- M0.6 — a complete local path `review → saved JSON/Markdown → show/explain`: `--format json|markdown`,
  `--output`, `--fake-responses` (evidence and assessments fixture, the harness computes CodeRef), `--exit-policy strict`
  (10 — a significant violation, 11 — incomplete scope), cancellation on Ctrl-C (130, run `CANCELLED`),
  `sych explain --run --obligation` and `sych eval run --suite eval/smoke.yaml --models fake`.
  Guide — [docs/LOCAL_REVIEW.md](docs/LOCAL_REVIEW.md), output examples — `docs/examples/local-review/`.

M1 in progress:

- M1.3 (deterministic part) — `sych_core::model`: typed `ModelProvider`/`Route`/`Capabilities`, prompt layers
  with isolated data blocks, strict structured output with a single repair, a monetary upper bound before dispatch and
  a SQLite ledger (atomic run/day/month reservation, idempotent settlement, `UNKNOWN_CHARGE`, a subscription fixed fee
  once a month, billing mismatch). The result's usage summary separates actual money, the price-list estimate,
  reasoning tokens, attempts on included quota and the month's fixed fee. Details — [docs/MODELS.md](docs/MODELS.md).
- M1.9 — an OpenRouter adapter with pinned model/upstreams, no hidden fallback, `require_parameters` and
  `data_collection: deny`, streaming, typed errors and exact cost; opt-in `sych doctor --probe-model
  PROFILE` prints the accounting record. Real probes of DeepSeek V4.1 Flash, Astra and Fable have passed.
- M1.10–M1.12 (subscriptions) — ChatGPT plan via Sign in with ChatGPT (`sych auth chatgpt`, text-only Responses),
  GLM Coding Plan via a pinned Pi CLI in an isolated process, OpenCode Go over HTTP; quota/allowance is separate from
  money, overage is confirmed by the owner. Fixtures and loopback smokes exist; real subscription smokes are waiting for
  the owner's accounts. M1.13 — `--models roles`: pass roles, bounded fallback with cooldown within one budget,
  premium via `risk_auditor`. Details — [docs/MODELS.md](docs/MODELS.md).
- M1.1/M1.14 (semantic backends, first part) — `sych semantic` collects definitions, references, callers and
  implementations of changed constructs for base and head separately via pinned rust-analyzer and TypeScript 7
  `tsc --lsp`; Sych itself downloads, verifies SHA-256 and caches the servers according to its own manifest. Details and
  limitations — [docs/SEMANTIC.md](docs/SEMANTIC.md). Path classes, pull diagnostics and Cargo
  editions/features have been added.
- M1.2 (ContextBroker) — `sych review` turns semantic facts into derived evidence and builds context packets
  (§9.1–9.5): exact `CodeRef`, reason/priority, completeness, links, what was excluded and gaps, the §9.3 limits and
  `UNKNOWN/CONTEXT_LIMIT` for required context beyond the budget; callers, callees, imports and tests; typed
  operations §9.4 with time-bounded search, and the semantic cache §9.6.
- M1.4 (rules) — the generic rule pack §11.4 by profile, three-valued applicability with carry-over and semantic links,
  waivers without rewriting the status, `--policy-ref` for an explicit run with a new policy. Details —
  [docs/CONFIG.md](docs/CONFIG.md).
- M1.5–M1.7 (model review) — `sych review --models PROFILE`: assessment of obligations (for a small PR together with
  discovery in a single call), high-risk audit, discovery with typed context requests and a verifier; the final statuses
  are given by the engine. Details — [docs/ANALYSIS.md](docs/ANALYSIS.md).
- Syntax facts (M1.1/M1.14) — syn and oxc parse changed Rust and TS/React files: guards across `.await`,
  detached tasks, discarded results, cfg/serde/public API; effects and cleanup, listeners/timers, stale async,
  mutation/invalidation, controlled inputs, form submit, project hooks with a link to the definition.
- CI facts (M1.14) — `sych review --ci-facts FILE --ci-manifest FILE` imports cargo/clippy JSON and JUnit as
  external input with a binding to the tested commit; a local import remains an author claim.
- Isolated tooling profile (M1.1/M1.14) — `tooling.dependency_install` installs dependencies with pinned pnpm and
  `cargo fetch` in bwrap or docker (fallback and `fail|degrade` are configurable), without scripts or host credentials;
  `.cargo/config*`, `rust-analyzer.toml` and a toolchain `path` from the repository are not executed.
- M2.1 (GitHub App, start) — `sych serve`: `POST /hooks/github/:connection` with signature verification over the raw body,
  a 2 MiB limit, content/event types and an atomic write of receipt + binding + intent; a repeated delivery is recognized,
  bot comments do not schedule work; App JWT → a token for a single repository, capabilities from permissions, v1 error
  mapping. M2.2 — scheduler (decisions by API state: same inputs, supersede, draft, commands with permissions, close,
  CI wake/successor), durable jobs with lease/fence, a reaper, reconcile of missed webhooks, fetch of pinned commits into
  the mirror. M2.3 — CI evidence by trusted mapping (exact run/attempt/job/artifact), a safe in-memory zip,
  SARIF/Code Quality/LCOV/Cobertura/manifest, binding `SOURCE_HEAD`/`MERGE_RESULT`/`UNVERIFIED` and specific gap codes.
  M2.4 — fenced finalization (frozen inputs, lease, generation, bytes before the pointer) together with the job and outbox intent,
  a fault matrix, retention of records and mirrors without deleting unknown charges/unfinished publications, a schema
  upgrade test ([docs/STORAGE.md](docs/STORAGE.md)). M2.5 — durable outbox: `PUBLISHING` before the network, `UNCERTAIN` instead of
  a blind retry, reconciliation by id/marker of the bot's comments only, gates (generation, open PR), repair of a late
  UPDATE, `sych reconcile` with auditable resolution of a new CREATE. M2.6 — a summary from the immutable result (coverage,
  problems, UNKNOWN, collapsed nits, cost), up to 8 inline comments per revision by explicit commit/path/line/side, 422 →
  summary, an optional neutral check run. M2.7 — findings lineage (RESOLVED only after a comparable re-check,
  a rename does not close a finding), feedback and waivers by repository permissions. M2.8 — `/v1` (identity, repository scope,
  idempotency, error envelope), `serve`/`api`/`worker` roles, health/metrics/alerts, `review --change`,
  `publish`/`cancel`/`rerun`, OpenAPI from DTOs. M2.9 — self-review with a pin of a trusted
  build, production (`bondiano/sych`) only this way ([SERVER.md](docs/SERVER.md#self-review-bondianosych)), server routes without local-only subscriptions, an example CI
  mirror without `pull_request_target`. The real connection is waiting for the owner's App —
  [docs/SERVER.md](docs/SERVER.md).
- Quality suite (M1.8/M1.14, labelling) — 103 development cases in `eval/quality/dev` (Rust, TS/React and Go
  safe/unsafe/UNKNOWN pairs, routing/billing/privacy triples, clean, nitpick, real commits) and `sych eval validate`;
  the holdout is labelled by the owner. `sych eval measure` on DeepSeek V4.1 Flash, baselines A/B/C: the full pipeline C —
  recall 34/39, precision 39/61, false remarks on 22 of 58 safe cases, $0.17 per suite; a single reviewer A — 39/39, but
  59/113 and 34/58 (a single run and repeats, not a quality assessment — [docs/ANALYSIS.md](docs/ANALYSIS.md)).

M0 is closed on the fake adapter: there are no external API calls, and review quality has not been measured.
`sych review --base REF [--head REF] [--diff-base REF] [--config sych.yaml] --models fake` requires a prior
`sych db migrate`, otherwise exit code 3. Without `--fake-responses` applicable obligations remain `UNKNOWN/NOT_ASSESSED`.
An invalid policy on the target tip gives exit code 2.
The significance threshold is fixed for now (`MEDIUM`); `change_kinds` without a semantic provider give `UNKNOWN`.
Invalid flags, refs or repository path give exit code 2; a missing merge-base gives exit code 3 with a reason
(`NO_MERGE_BASE`, `SHALLOW_HISTORY`, `AMBIGUOUS_MERGE_BASE`), without comparing against an arbitrary commit.
Tests cover the `sych-core` contracts, Git snapshot, storage and a real CLI smoke on temporary repositories (`git` must be in `PATH`); a green test run does not confirm the specification's acceptance cases.

The agreed scope includes high-quality Rust review, OpenRouter and GPT/GLM/OpenCode subscription access, the cheap DeepSeek V4.1 Flash and bounded fallback with escalation to Astra/Fable. These are requirements for the future implementation, not ready CLI features. GitHub self-review of Sych is described in roadmap M2.9 and [SERVER.md](docs/SERVER.md#self-review-bondianosych): CI runs the quality checks, `image.yml` builds the reviewer image from `main`, and the bot itself runs in petinfra.

## Local development

You need [rustup](https://rustup.rs/) and [just](https://just.systems/) (just 1.59.0 in CI).
Rust 1.94.1, rustfmt and Clippy are pinned in `rust-toolchain.toml`; dependencies are pinned in `Cargo.lock`.
Finding unused dependencies requires [cargo-machete](https://github.com/bnjbvr/cargo-machete) 0.9.2; CI pins the same version.

```sh
rustup show active-toolchain
cargo install cargo-machete --version 0.9.2 --locked
just check
just run
```

`rustup` uses the toolchain from the file and installs it when needed.
`just run` shows help by default; arguments are passed through directly, for example `just run --version`.
The list of recipes is printed by the `just` command.

## Quality gates

- `just fmt` — formats Rust code.
- `just lint` — rustfmt, Clippy for the whole workspace with all targets and features (warnings are errors), `sych-style`
  and `cargo machete` for finding unused dependencies.
- `just test` — workspace tests with all features.
- `just check` — lint and test; the same recipe runs in GitHub Actions.
- `just build` — builds the application.
- `just schemas` — regenerates `schemas/*.schema.json` from the Rust DTOs; `just test` fails if the committed schemas are stale.

Cargo build and test commands, as well as Clippy, use `--locked`: dependency changes require an explicit lockfile update.
Shared lints are inherited by every package via `[lints] workspace = true`.
As in flick-lang, unsafe code and ignoring must-use results are forbidden;
Clippy all/pedantic and checks for `unwrap` and absolute paths are enabled.
`.editorconfig` and `.gitattributes` set uniform line endings and keep DOCX as binary files.

[CI](.github/workflows/ci.yml) checks pull requests and pushes to `main`; manual runs are also available.
The workflow has only `contents: read` and does not persist Git credentials in the checkout.
After the repository is published, the `check` job must be separately marked as required in branch protection/ruleset.

## Development and release rules

- We follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/): `feat`, `fix`, `docs` and other suitable types; breaking changes are marked with `!` or a `BREAKING CHANGE:` footer.
- Code style: an `if` block followed by another statement in the same block is followed by a blank line. `sych-style` checks this in `just lint`, and `just style-fix` fixes it.
- Until the full feature scope is done and the final release gate from the roadmap is passed, **nothing is rolled out to production**. Intermediate servers and integration checks are test/staging/sandbox only; a live bot connection, including shadow mode, also counts as production. A local read-only review is not a rollout by itself.

## Structure

```text
crates/sych-app/       CLI: lib (args, commands/, services) and a thin bin/sych.rs
crates/sych-core/      domain types, canonical hashing, schema_version, SnapshotReader port
crates/sych-adapters/  Git snapshot, OpenRouter, pinned language servers, semantic/syntax/CI facts, sandbox; SCM later
crates/sych-storage/   SQLite runs/pointers, artifact store, GC
crates/sych-style/     style check that rustfmt/clippy lack (blank line after `if`)
migrations/sqlite/    SQLite migrations (only via `sych db migrate`)
schemas/              JSON Schema generated from sych-core (do not edit by hand)
fixtures/core/        frozen v1 documents, an incompatible version, the reference canonical hash
fixtures/git/         reference diff manifest of the scenario repository (regenerate: SYCH_BLESS=1 just test)
fixtures/config/      invalid config/policy: unknown key, secrets, rule conflict, wrong path, weakening
fixtures/review/      a saved VIOLATED run: scope, policy, external inputs, evidence manifest and result
examples/personal/    a working personal sych.yaml and .sych/ (also written by `sych config init --personal`)
eval/                 fake eval suite `smoke.yaml` and fake responses fixtures (`sych eval run`)
.github/workflows/    CI
docs/                 specification, guides and output examples (`docs/examples/`)
```

Each crate has its own README with its purpose, module map and invariants.

External JSON documents carry `schema_version`; `sych_core::decode` rejects an unknown major version
before parsing fields. New fields are added with a default so that `fixtures/core/v1` keeps being readable.
Canonical hash: object keys are sorted, output is compact, array order is significant — sets are sorted before hashing.
Git is invoked with a cleared environment, without system/global config, replace refs or lazy fetch,
only via `--git-dir` and with `--no-ext-diff --no-textconv`: hooks, filters, textconv, fsmonitor and external diff
from the repository are not executed, the work tree and index are not read, and symlinks are not resolved on the host.
`ExecutionOutcome` (`PLAN_EXECUTED | LIMIT_REACHED | RETRIES_EXHAUSTED | INPUT_UNAVAILABLE`) holds values
not explicitly listed in §5.7 of the specification; the decision is [ADR 0001](docs/adr/0001-execution-outcome-values.md).

The remaining crates and libraries from §4.2 of the specification are added together with the code that uses them,
without empty packages or unused dependencies. Clap is included, and in `sych-core` — Serde, serde_json, serde_yaml_ng, serde_path_to_error, Schemars, jiff, sha2, uuid and thiserror; in `sych-storage` — SQLx (SQLite, bundled) and Tokio (a single-threaded runtime in the CLI, signal/time for cancellation and fake delay); tempfile — in tests, for temporary repositories of `sych eval run` and the semantic backends workspace; in `sych-adapters` — flate2 and tar for unpacking pinned language servers, toml for Cargo manifests, syn/quote/proc-macro2 and oxc for syntax facts, quick-xml for JUnit.
The future `sych-core` must not depend on HTTP, SQL or a specific LLM SDK.
Publishing Cargo packages is disabled; the project license has not been chosen yet.
