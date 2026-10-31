# sych-adapters

Everything Sych does with the outside world for the sake of analysis: Git, language servers, parsers, sandbox, models and
CI reports. Adapters return `sych-core` types and never execute code of the repository under review
with privileges.

## What's inside

| Module | Purpose |
|---|---|
| `git` | hardened `git` (cleared environment, no hooks/filters/textconv), `LocalSnapshot` (three pinned positions, diff manifest, `SnapshotReader`: reading any file of a side and `git grep -F`), byte-for-byte export of a side into a directory from the object database |
| `tools` | pinned versions of language servers and pnpm (`tools/pinned.json`: version, URL, SHA-256 per platform), download, verification and an atomic cache |
| `semantic` | LSP client, rust-analyzer, TypeScript 7 (`tsc --lsp`), gopls and ty profiles, changed constructs, references/callers/implementations, pull diagnostics, semantic cache, dependency installation via `sandbox` |
| `syntax` | syntax facts from `syn` (Rust), `oxc` (TS/React) and tree-sitter (Go, Python) in-process, without execution |
| `lint` | pinned linters on changed head files (ruff, oxlint, sqruff, mago, jarl, clj-kondo, actionlint, zizmor), never executing repository code |
| `sandbox` | running installers in bwrap (read-only root, `$HOME`/`/tmp`/secrets hidden) or docker (pinned digest) |
| `ci` | normalization of CI reports into `CiFacts` bound to the tested commit: cargo JSON, JUnit, SARIF, GitLab Code Quality, LCOV, Cobertura, Go test/cover/vet, Terraform plans, rendered Kubernetes manifests and the `sych_manifest` outcomes; in-memory artifact zip reading |
| `execution` | targeted checks: allowed profiles run on the pinned head in docker without network or secrets; stdout is imported as a CI report |
| `github` | GitHub App: webhook admission checks, App JWT and installation tokens, normalized reads, comments, reviews, check runs and Actions artifacts |
| `chat` | OpenAI-compatible SSE chat completions shared by OpenRouter and OpenCode Go |
| `openrouter` | OpenRouter transport for ModelGateway: pinned model and upstreams, no hidden fallback |
| `opencode_go` | OpenCode Go subscription transport; local schema validation, one session per task |
| `pi` | GLM Coding Plan through the pinned Pi CLI, one isolated process per request |
| `siwc` | GPT via Sign in with ChatGPT: OAuth sign-in and token refresh, text-only Responses API |

## Errors and logs

Errors are typed via `thiserror` (`SnapshotError`, `ToolError`, `LspError`, `SandboxError`, `DepsError`,
`CiError`); the cause is kept in `source()`. Events and spans use `tracing`: pinning the comparison, downloading
tools, analysis of each side and language, dependency installation and sandbox jobs, CI import, files skipped
during parsing. The application installs the subscriber.

## Security boundaries

- `.cargo/config*` and `rust-analyzer.toml` are not exported, and a `rust-toolchain` with `path` is rejected: they can
  assign commands to host tools.
- Language servers and installers receive only the environment variables the toolchain needs; model and SCM
  credentials do not reach them.
- A pinned artifact without a matching SHA-256 is not installed; a version that is not in the manifest gives
  `UNSUPPORTED`, not a substitution.

## Checks

The Git fixture adds a non-UTF-8 name through the object database and index, without creating such a path on the macOS
file system. Workspace roots for LSP and linters are canonicalized before launch so that absolute paths from
`getcwd()` (on macOS `/private/var/...`) map to repository paths.

The semantic smoke below needs a repository under review with at least two commits (`HEAD~1` and `HEAD`).

```sh
cargo test -p sych-adapters
sych semantic --base HEAD~1     # real smoke; the first run downloads the servers
```
