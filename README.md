<p align="center">
  <img src="docs/brand/readme-banner-2400x800.png" alt="Sych — deterministic code review" width="100%" />
</p>

<p align="center">
  <a href="https://github.com/bondiano/sych/actions/workflows/ci.yml"><img src="https://github.com/bondiano/sych/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <img src="https://img.shields.io/badge/rust-1.94.1-F97416?logo=rust&logoColor=white" alt="Rust 1.94.1" />
  <img src="https://img.shields.io/badge/status-pre--release-FFC83D" alt="Pre-release" />
</p>

**Sych** (Russian *сыч*, the little owl) is a code reviewer with a deterministic Rust core and bounded model calls.
The engine decides what must be checked, collects evidence and computes the verdict; models only propose
hypotheses and assessments inside fixed budgets. Every finding cites exact code, every gap is reported as
`UNKNOWN` instead of silence, and the same inputs give the same result.

## What it does

- **Pinned snapshot.** Reads Git objects only (no work tree, hooks, filters or textconv) and pins `target_tip`,
  `diff_base` and `head`; the policy always comes from the target tip, never from the reviewed head.
- **Obligations, not vibes.** A deterministic planner turns rules into obligations with three-valued applicability;
  the reducer accepts `SUPPORTED` only with complete evidence of an acceptable kind.
- **Facts before models.** Semantic facts from pinned language servers (rust-analyzer, TypeScript `tsc --lsp`, Go),
  syntax facts from syn and oxc, pinned linters, CI facts (cargo/clippy JSON, JUnit, SARIF, LCOV, Cobertura, Terraform plans) bound to the tested commit.
- **Bounded models.** OpenRouter with pinned upstreams, ChatGPT / GLM Coding Plan / OpenCode Go subscriptions,
  role routing with bounded fallback; a monetary upper bound before every call and a SQLite ledger after it.
- **Deployment-aware rules.** Migration (`MIG-*`) and Terraform rules are checked against an owner-declared
  deployment model and its reachable transition matrix.
- **GitHub App.** Verified webhooks, durable jobs with lease and fencing, a reconciling outbox, a summary plus up to
  eight inline comments per revision, findings lineage, feedback and waivers, and a `/v1` HTTP API.

Sych reviews its own pull requests through a live GitHub App; see [self-review](docs/SERVER.md#self-review-bondianosych)
and [the real connection](docs/SERVER.md#real-connection-2026-10-27).

## Status

Pre-release. The local pipeline (M0), model review (M1), the GitHub App server (M2) and the first
deployment/migration/Terraform checks (M3) are implemented. Nothing is rolled out to production except the pinned
self-review of this repository. Quality is measured on a labelled suite in `eval/quality`; the numbers and their
limits are in [ANALYSIS.md](docs/ANALYSIS.md). No license has been chosen yet.

## Quick start

```sh
just run db migrate          # create the local store
just run review --repo . --base origin/main --models fake --format markdown
```

`--models fake` makes no external calls; obligations without evidence stay `UNKNOWN/NOT_ASSESSED`. A real profile
(`--models PROFILE` or `--models roles`) makes paid or quota-counted calls within the configured budgets. With
`--exit-policy strict` the exit code is 10 for a significant violation and 11 for incomplete scope.
The walkthrough is in [LOCAL_REVIEW.md](docs/LOCAL_REVIEW.md); `just run --help` lists every command.

## Documentation

| Guide | Covers |
|---|---|
| [Local review](docs/LOCAL_REVIEW.md) | `review`, `show`, `explain`, output formats and exit codes |
| [Configuration](docs/CONFIG.md) | `sych.yaml`, `.sych/*`, rule packs, waivers, secret paths and redaction |
| [Models](docs/MODELS.md) | providers, routing, budgets and the cost ledger |
| [Model review](docs/ANALYSIS.md) | assessment, discovery, audit, verifier and quality measurements |
| [Semantic backends](docs/SEMANTIC.md) | pinned language servers and context facts |
| [Deployment model](docs/DEPLOYMENT.md) | `.sych/deploy.yaml`, transition matrix, `MIG-*` binding |
| [Server](docs/SERVER.md) | GitHub App, scheduler, CI evidence, publication, API and operations |
| [Storage](docs/STORAGE.md) | SQLite, artifacts, retention |
| [Decisions](docs/adr/) | architecture decision records |
| [Brand](docs/brand/README.md) | logo, icons, colors |

The full index is [docs/README.md](docs/README.md). Each crate has its own README with its module map and invariants.

## Development

You need [rustup](https://rustup.rs/), [just](https://just.systems/) and
[cargo-machete](https://github.com/bnjbvr/cargo-machete) 0.9.2. The toolchain (Rust 1.94.1, rustfmt, Clippy) is
pinned in `rust-toolchain.toml`, dependencies in `Cargo.lock`, and every Cargo command runs with `--locked`.

```sh
cargo install cargo-machete --version 0.9.2 --locked
just check      # what CI runs: lint + test
```

| Recipe | Does |
|---|---|
| `just fmt` | format Rust code |
| `just lint` | rustfmt check, Clippy (warnings are errors), `sych-style`, `cargo machete` |
| `just test` | workspace tests; fails on stale `schemas/` |
| `just schemas` | regenerate JSON Schemas from the Rust DTOs |
| `just style-fix` | apply the house style (blank line after an `if` block) |
| `just run …` | run the CLI |

Rules:

- [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/); breaking changes use `!` or a
  `BREAKING CHANGE:` footer.
- `main` is protected: work on a branch, open a PR, wait for green CI and the Sych review of the current head.
  Agents follow [AGENTS.md](AGENTS.md).
- `unsafe` and ignored must-use results are forbidden; Clippy all/pedantic, `unwrap` and absolute-path checks are on.
- External JSON documents carry `schema_version`; an unknown major version is rejected before parsing.
  New fields get a default so `fixtures/core/v1` stays readable.

## Layout

```text
crates/sych-app/       CLI and server: args, commands, services; thin bin/sych.rs
crates/sych-core/      domain types, planner, reducer, broker, model routing; no HTTP/SQL/LLM SDK
crates/sych-adapters/  Git snapshot, GitHub, model providers, language servers, syntax/CI facts, sandbox
crates/sych-storage/   SQLite runs and pointers, artifact store, ledger, GC
crates/sych-style/     the house style check rustfmt and Clippy lack
migrations/sqlite/     migrations (applied only by `sych db migrate`)
schemas/               JSON Schema and OpenAPI generated from the Rust DTOs (do not edit)
fixtures/              frozen documents, Git/config/review fixtures
examples/personal/     a working personal sych.yaml and .sych/
eval/                  smoke and labelled quality suites
docs/                  guides, ADRs, output examples, brand
```
