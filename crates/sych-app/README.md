# sych-app

The `sych` command line. A library with commands and services plus a thin `bin/sych.rs` binary that
only parses arguments and calls a command.

## Layout

- `args` — clap: `Cli`, `Commands` and one `*Arguments` struct per subcommand; a shared `Comparison`
  (`--repo/--base/--head/--diff-base`) and `Output` (`--format/--output`).
- `command` — the trait `Command { name(), run() -> anyhow::Result<ExitCode> }` and `run_command`, which
  prints the error chain (`sych <command>: context: cause`) and returns its exit code.
- `errors` — exit codes: 2 invalid, 3 no result, 4 permission, 10/11 strict policy, 130
  cancelled, 1 failed eval. Errors follow the thiserror + anyhow scheme: libraries return typed
  errors (`SnapshotError`, `StorageError`, `ToolError`, `SandboxError`, `DepsError`, `CiError`), the app returns
  `anyhow::Result` with `.context()` at every step. The code is decided by `Failure` (thiserror: `Invalid`, `NoResult`,
  `Permission`, `Cancelled`), otherwise by the first typed cause in the chain (`SnapshotError::InvalidInput`,
  `StorageError::UnknownRun` → 2), otherwise 3.
- Logs — `tracing`; the `tracing-subscriber` subscriber is installed in `bin/sych.rs`: stderr, no timestamps, color
  only in a terminal. The level is `SYCH_LOG` in `EnvFilter` syntax (by default `info` for Sych crates,
  `warn` for dependencies; `SYCH_LOG=debug`, `SYCH_LOG=sych_adapters=trace`). Secrets, prompts and model
  responses are not logged.
- `commands/` — one file per subcommand (`review`, `semantic`, `show`, `explain`, `tour`, `ask`, `handoff`, `doctor`,
  `config_init`, `config_validate`, `eval_run`, `eval_validate`, `eval_measure`, `db_migrate`, `db_import`,
  `auth_chatgpt`, `cache_prune`, `serve` (also `api` and `worker`), `run_action` (`publish`, `cancel`, `rerun`),
  `reconcile`).
  In each, `run(&XArguments) -> anyhow::Result<ExitCode>` assembles the result from the services.
- Services, by subject area:

| Service | Purpose |
|---|---|
| `review` | the review pipeline: snapshot → trusted policy → facts → plan → model passes → context packets → reduce → save |
| `analysis` | model passes of one review: required assessments, high-risk audit, discovery and verification |
| `backend` | resolving model profiles to a callable transport (mode, egress, model, price, credentials) |
| `semantic` | semantic facts via pinned language servers with a cache |
| `ci` | reading `--ci-facts`/`--ci-manifest` for import |
| `snapshot` | capturing the comparison (`Comparison` → `LocalSnapshot`) |
| `config` | server config, working `.sych/`, effective config, the `config init` example |
| `storage` | DB and artifact paths, `block_on`, reading a saved run |
| `output` | writing JSON/Markdown to a file or stdout |
| `render` | JSON and Markdown renderers with escaping of repository text |
| `fake` | fake model adapter for `--fake-responses` |
| `probe` | opt-in model probe for `doctor --probe-model` |
| `tour`, `tour_reader` | building a guided tour of a saved run; reading it (`show`, `reveal`, `complete`, `dismiss`) without a model |
| `followup` | `sych ask`: a bounded follow-up on one tour stop under `budgets.followup_session` |
| `handoff` | `sych handoff`: selected findings of a saved run as a package for a separate fix workflow |
| `server` | `sych serve`/`api`/`worker`: webhook admission, scheduler, worker, publisher, `/v1` API, health and metrics |
| `eval`, `quality` | the `eval run` suite harness and validation of the labelled quality suite `eval validate` |

## How to add a command

1. An `XArguments` struct in `args.rs` and a variant in `Commands`.
2. `commands/x.rs` with `pub fn run(args: &XArguments)`; reusable logic goes into a service.
3. A branch in `bin/sych.rs`: `Commands::X(args) => run_command("x", || x::run(&args))`.

## Checks

```sh
cargo test -p sych-app      # CLI tests run the built binary on temporary repositories
just run --help
```
