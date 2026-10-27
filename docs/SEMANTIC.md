# Semantic backends

`sych semantic --base REF [--head REF] [--diff-base REF] [--repo DIR] [--output FILE]` prints a JSON array of
`SemanticFacts` documents ([schema](../schemas/semantic-facts.schema.json)): one per side
(`DIFF_BASE`, `HEAD`) and language with changed files. The command only reads Git objects. `sych review` collects the
same facts (`--semantic off` disables this) and passes them to the ContextBroker.

## Pinned language servers

Server versions, URLs and SHA-256 are maintained by Sych: [`pinned.json`](../crates/sych-adapters/src/tools/pinned.json).
`PATH` and system-installed servers are not used. On first request a server is downloaded over HTTPS,
verified against the pinned SHA-256, unpacked into a temporary directory and atomically renamed into
`$SYCH_TOOLS_DIR`, otherwise `$XDG_CACHE_HOME/sych/tools`, otherwise `~/.cache/sych/tools`
(`<tool>/<version>/<arch>-<os>/`). A checksum mismatch installs nothing.

| Language | Server | Version selection | Platforms |
|---|---|---|---|
| Rust | rust-analyzer `2026-10-19` (GitHub release) | the single pinned one | linux, macos × x86_64, aarch64 |
| TS/JS/React | TypeScript 7 native `tsc --lsp --stdio` (npm `@typescript/typescript-<platform>`) | exact `typescript` version from `pnpm-lock.yaml` (all importers, multi-document pnpm 11+) or `package-lock.json` | same |
| Go | gopls `v0.24.0`, built with the pinned Go `1.27.3` (go.dev/dl, SHA-256) | the single pinned one | same |
| Python | none: basic M3 fallback — syntax facts without a language server | — | — |

Linters are pinned the same way (GitHub release, SHA-256 from the release digest): ruff `0.17.2`, oxlint `1.89.0`,
actionlint `1.7.12`, zizmor `1.31.1`; see [Pinned linters](#pinned-linters).

gopls is not published as binaries, hence `archive: go-module`: Sych downloads the pinned Go, verifies the `h1:`
hash of the `golang.org/x/tools/gopls@v0.24.0` module (`sha256` in the manifest is its hex) via `go mod download -json`,
then runs `go install -trimpath` with `GOPROXY=proxy.golang.org`, `GOSUMDB=sum.golang.org`, `GOTOOLCHAIN=local`,
`GOENV=off`, `CGO_ENABLED=0`; gopls dependencies are verified by its `go.sum`. The host Go, `GOFLAGS` and credentials are not
used. The build takes about a minute, once per version.

A TypeScript version missing from the manifest, several versions across importers, or TypeScript < 7 (tsserver
protocol only) yield `UNSUPPORTED` with a reason, without substituting another version. A new version is added as a manifest
entry with a checksum verified against upstream (npm `integrity`, GitHub release `digest`).

## What the backend does

1. Each side is written byte-for-byte into a temporary directory from the object database (`ls-tree` + `cat-file
   --batch`): no checkout, filters, `export-subst` or hooks. Secret paths, symlinks, submodules and blobs > 4 MiB
   are not written. Base and head do not share workspace state.
2. The server is started with a cleared environment (`PATH`, `HOME`, `CARGO_HOME`, `RUSTUP_HOME`, `TMPDIR`, `LANG`),
   so SCM/model credentials never reach it.
3. `documentSymbol` of changed files → the innermost named constructs (function, method,
   constructor, class, interface, struct, enum; module-level `const`/`let` — these are React components and hooks)
   that intersect changed lines. Anonymous callbacks and import bindings are skipped; the change is attributed to the
   nearest named construct. At most 60 symbols per document; the rest are listed in limitations.
4. For each symbol: `references`, `implementation` and call hierarchy (`prepare` → `incomingCalls`, call
   sites from `fromRanges`, and the whole calling construct from `from.range` — `calling_constructs`; `outgoingCalls`
   → whole callee constructs from `to.range` — `callees`, calls into dependencies — `external`;
   `definition` for the first 6 type names in the construct's text (capitalized identifiers, except
   `Self/Some/None/Ok/Err` and its own name) — `type_definitions`, the whole definition from `targetRange`) — only when
   the capability is declared, otherwise `UNSUPPORTED`; callers of non-callable symbols are `NOT_APPLICABLE`.
5. Pull diagnostics (`textDocument/diagnostic`) of changed files → `diagnostics[]` with `CodeRef`, severity,
   code and source (at most 50 per file). These are server observations, not CI facts.
6. Locations → `CodeRef` with the side's blob oid and SHA-256 of the lines. Locations outside the snapshot (registry, toolchain)
   go into `external` (home → `~`), unmappable ones into `unresolved` with a reason. `CodeRef`s are line-based,
   so the negotiated position encoding (recorded in `position_encoding`) does not affect them.

Every symbol has a `path_class` (`PRODUCTION | TEST | GENERATED | CONFIG | BUILD`) derived from the path name and
the generated flag from the diff manifest. Rust `build_configuration` lists the editions and declared features
of each package (`features:<package>`); only default features and host cfg are analyzed. TS
`build_configuration` lists each `tsconfig*.json` of the side (`tsconfig:<path>`: `extends`, project
`references`, aliases from `paths`, `composite`; JSONC with comments and trailing commas), packages
(`package:<dir>` → name from `package.json`) and `pnpm-workspace.yaml` globs (`workspace`). `extends` from an npm package
without installed dependencies and unparsable files are listed in limitations.

Example: [run facts](examples/semantic/run-facts.json), [semantic evidence](examples/semantic/evidence-semantic-head.json)
and [evidence manifest](examples/semantic/evidence-manifest.json) of a local review of two commits of a TS project.
Behavior on real snapshots without a real server is tested by a scripted LSP
([`tests/semantic.rs`](../crates/sych-adapters/tests/semantic.rs)): separate base/head workspaces, rename of a
Unicode file, a deleted construct, an empty response as `PARTIAL`, missing capabilities as `UNSUPPORTED`.

## Go build configuration

The Go backend looks for `go.work` and all `go.mod` files of the side (excluding `vendor`, `testdata`, hidden directories; depth 6) and
records in `build_configuration`: `toolchain` (pinned Go, `GOTOOLCHAIN=local`), `goos`/`goarch` (host),
`build_tags: none`, `cgo: disabled`, `go.work` (`use`), `module:<path>` (module, `go`, number of requirements,
`toolchain`), `replace:<file>:<module>` and `vendor`. Limitations name a module with a `go` newer than the pinned one, a `replace` pointing to
a path outside the snapshot or without `go.mod`, and the fact that references/call hierarchy cover only the active configuration
(GOOS/GOARCH without additional tags) and do not go through interface dispatch, reflection or function values.
gopls runs with `GOPROXY=off` and a Sych-owned `GOMODCACHE`/`GOCACHE`/`GOPATH` in `~/.cache/sych`; without dependency
installation, packages of external modules are not resolved (limitation `dependencies are not installed`). The producer is
the gopls version (`v0.24.0`), not its build info.

There is one supported profile: host GOOS/GOARCH, no tags, cgo off. Code under other build constraints appears in
syntax facts (`cfg_gate` with `//go:build`), but not in semantic answers.

## Dependencies and sandbox

Without dependencies, imports of npm/workspace packages are not resolved, and offline rust-analyzer cannot see std or
crates that are not in the cache. `tooling.dependency_install` in `sych.yaml` ([CONFIG.md](CONFIG.md)) enables
installation for each side separately, before the server starts:

- TS: pinned pnpm (version from `packageManager` in `package.json`, manifest [`pinned.json`](../crates/sych-adapters/src/tools/pinned.json))
  runs `install --frozen-lockfile --ignore-scripts --ignore-pnpmfile` against the `registry.npmjs.org` registry,
  without the user's `.npmrc`, with the store in `~/.cache/sych/pnpm-store`. pnpm lockfile only.
- Go: `go mod download` with the pinned Go in each module (or in the `go.work` root) via `proxy.golang.org` and
  `sum.golang.org` into `~/.cache/sych/go-mod`; repository code is not executed. Rust-style docker is not used.
- Rust: `cargo fetch` of the workspace and the std toolchain (`RUSTC_BOOTSTRAP=1` only for the std manifest) into
  `~/.cache/sych/cargo-home`; rust-analyzer then runs offline with this `CARGO_HOME`; the host `~/.cargo`
  with credentials is not used.
- bwrap: the host root is read-only; `$HOME`, `/tmp`, `/root`, `/run/user`, `/run/secrets` are replaced with an empty tmpfs;
  only the workspace (rw), the store/`CARGO_HOME` (rw), the tool binary and `RUSTUP_HOME`
  (ro) are mounted back; the environment is cleared down to `PATH`, `LANG`, `NIX_LD*`, `HOME=/tmp`. The network is shared, for the registry.
- docker: `--rm --cap-drop ALL --read-only --security-opt no-new-privileges`, the same set of mounts, the uid
  of the workspace owner. Rust is not installed via docker (the host toolchain does not work in the image) — the next
  backend or the policy applies.
- `build_configuration.dependencies` shows the outcome (`installed (bwrap, pnpm 12.14.0, scripts off)`,
  `fetched (bwrap, sych CARGO_HOME)` or `not installed`). A degraded result is not cached.

Regardless of installation, `.cargo/config*` and `rust-analyzer.toml` are not written to the workspace (they can define
commands for cargo/rust-analyzer), and a `rust-toolchain` with `path` yields `UNSUPPORTED`: otherwise host tools
would execute code from the repository.

## Syntax facts

`sych review` always parses changed `.rs` (syn 2), `.ts/.tsx/.js/.jsx` (oxc 0.139, the latest version with
MSRV ≤ 1.94), `.go` and `.py/.pyi` (tree-sitter) files of both sides in-process, without executing code, and stores `SyntaxFacts`
([schema](../schemas/run-facts.schema.json), together with semantic facts — the `facts` artifact) as derived static
evidence. A fact is a code shape with a `CodeRef`, `in_change` and a detail, not a finding: macros are not expanded, types
are not resolved, and the absence of a fact proves nothing.

- Rust: `async_fn`, `guard_across_await` (a guard from `lock/read/write/borrow(_mut)` is alive across an `.await` of the same block;
  `drop(g)` releases it; closures and `async` blocks are not counted), `select_macro`, `detached_spawn` (handle
  discarded), `blocking_in_async` (`thread::sleep`, `std::fs::*`, `block_on`), `discarded_result` (`let _ =` of a call,
  `.ok();`), `unwrap`, `drop_impl`, `public_item` (signature), `cfg_gate`, `inactive_cfg` (an item or statement under
  a `#[cfg]` that the analyzed build does not compile: not host Linux, not `test`/`debug_assertions`, not a default
  feature from the nearest `Cargo.toml`; an unknown predicate is also inactive), `serde_attribute`, `unexpanded_macro`
  (`tracing`/`log` logging macros are considered known: their arguments are visible), `debug_formatted` (`?value`,
  `{value:?}` or `{:?}` in a logging macro; the type is taken from the function parameter, `related` are its definitions in
  the snapshot, standard types are not flagged), `test_module`, `lint_suppressed` (`#[allow]`/`#[expect]`),
  `test_skipped` (`#[ignore]`).
- React: `effect_with_cleanup` / `effect_without_cleanup`, `unpaired_listener`, `unpaired_timer`,
  `unpaired_subscription`, `async_effect_guarded` / `async_effect_unguarded` (AbortController, `signal` or
  a stale flag `ignore/cancelled/…`), `mutation_call`, `invalidation_call`, `controlled_input`,
  `uncontrolled_input`, `controlled_without_handler`, `form_submit`, `async_state_flag`, `custom_hook_call`,
  `test_skipped` (`it/test/describe.skip|only|todo`).
  For a custom hook, a literal search over the pinned tree finds its definition (`related`), so that the current
  implementation of the wrapper is reviewed, not its name.

In the packet, risky shapes on changed lines go as `REFUTING`; `select_macro`, `serde_attribute`,
`cfg_gate` and `unexpanded_macro` are labeled on their span; definitions of project hooks and `debug_formatted` types
go as `SUPPORTING`. Changed code whose behavior the snapshot does not show is a packet gap: `CFG_INACTIVE`
(`inactive_cfg`), `MACRO_UNEXPANDED` (`unexpanded_macro`) and `DEBUG_UNSEEN_TYPE` (`debug_formatted` without
a type definition). With such a gap an obligation cannot be `SUPPORTED`: `context_limited` turns it into
`UNKNOWN/SEMANTIC_GAP`; so does `VIOLATED` if all HEAD anchors of its NOT_MET criteria lie in such code, while a
violation shown elsewhere in the change remains. For discovery packets the gap makes coverage `PARTIAL`. The
`anyhow` macros (`bail!`, `ensure!`, `anyhow!`, `format_err!`), `serde_json::json!` and `tokio::join!`/`try_join!`/`pin!`
are considered known. References inside `#[cfg(test)]` modules count as tests (`tested_by`). Diagnostics enter the packet
only as `SUPPORTING` (at most 5 per file), marked as possibly being analysis artifacts.

- Go (tree-sitter-go 0.25): `import`, `cfg_gate` (`//go:build`), `context_param`, `returns_error`, `defer`,
  `detached_spawn` (`go` statement), `channel_op` (send, receive, `close`, `select`), `tx_boundary`
  (`Begin/BeginTx/Commit/Rollback`), `db_call` (`Exec/Query/QueryRow/Prepare[Context]`, sqlx `NamedExec`),
  `write_call` (`os.WriteFile/Create/Remove/…`), `network_call` (`http.Get/Post/…`, `*http.Client`),
  `discarded_result` (`_ = call()`), `dynamic_dispatch` (`reflect.*`), `interface_call` (a call through an interface
  declared in the package), `test_skipped` (`t.Skip*`), `registered_effect_call`. The receiver type of an SQL call (`subject`)
  is taken from parameters, locals `x := db.BeginTx(..)`/`sql.Open` and struct fields of all `.go` files of the same
  directory on the same side, not from the SQL text: `tx.ExecContext` → `*sql.Tx`, `s.db.ExecContext` → `*sql.DB`;
  an unknown receiver stays untyped. `guard` is the condition of the nearest `if` in the function.
- Python (tree-sitter-python 0.25) — M3 fallback: `import` (`related` is the module file in the snapshot: `a/b.py`,
  `a/b/__init__.py`, `src/` layout, relative import, namespace directory; a module outside the snapshot and not from stdlib
  gets `subject: unresolved`), `resource_scope` (`with`/`async with`, `try/finally`), `detached_spawn` /
  `async_task` (`create_task`/`ensure_future` discarded / stored, `Thread(..).start()`), `write_call`
  (`open(.., "w"|"a"|"x"|"+")`, `.write`, `.unlink`, …), `db_call` (`execute*`, `session.add/delete/merge/flush`,
  `.objects.`, `save`), `tx_boundary` (`commit/rollback/begin/atomic`), `network_call` (`requests/httpx/aiohttp/urllib`,
  `*client`/`*session`), `swallowed_exception` (`except` with only `pass/continue/break/return`),
  `dynamic_dispatch` (`getattr/setattr/eval/exec/import_module/__import__/globals`), `monkey_patch`
  (`module.attr = ..`, `setattr(module, ..)`), `runtime_registration` (a decorator not on the wrapper list, for example
  `@app.route`, and module-level `register/connect/subscribe`), `test_skipped` (`pytest.skip`, `@skip*`, `xfail`),
  `registered_effect_call`. Receivers are untyped: matching is by name; this is a shape, not a resolved callee.
- Calls to wrappers from the effect registry (`match.language` = `go|python|typescript`, the name is the last segment of
  `match.symbol`) are `registered_effect_call` with `detail` = registry symbol; `related` are their definitions in the snapshot,
  so that the current implementation is read. In Go the wrapper is also checked against the receiver type; `subject` is the type of the passed
  transaction.

Changed `dynamic_dispatch`, `monkey_patch`, `runtime_registration` yield the `DYNAMIC_UNRESOLVED` gap, a changed
Python import with `subject: unresolved` yields `IMPORT_UNRESOLVED`; both, like `CFG_INACTIVE`, prevent an obligation from being
`SUPPORTED`. An empty AST or search result does not prove the absence of dynamic dependencies.

### Python: basic M3 fallback and what is deferred to M6

Currently Python has only the syntax facts above, a literal search for wrapper definitions, and import of ready-made
typecheck/lint/test reports through the common CI contract (JUnit, SARIF, Cobertura, LCOV). The semantic backend for Python is
`UNSUPPORTED` with the reason «no language server before roadmap M6», and in the packet this is the gap `semantic backend Unsupported`.
Not included until a separate M6 decision based on measured need: a language server (pyright/basedpyright/pylsp),
references/callers/implementations, receiver type inference, resolution of dependency imports, installation of Python
dependencies, and any execution of project code or analyzer plugins.

## Effect ledger

`sych_core::effects` builds one `EffectLedger` per run from the syntax facts of all languages and the effect registry
([schema](../schemas/run-facts.schema.json), field `ledger` of the `facts` artifact; in the report — `effect_ledger`;
derived static evidence `sych-effect-ledger`, completeness `PARTIAL`). It is a bounded base/head model of
observable effects of changed files, not full-program dataflow.

- Effects: `db_call`/`tx_boundary` → `db_write`, `write_call` → `filesystem`, `network_call`/`mutation_call` →
  `network`, `invalidation_call` → `cache`, React effects and unpaired listeners/timers/subscriptions →
  `subscription`, async effects → `ui`, `registered_effect_call` → registry `effect_kind`. The key is the file (name on
  head), construct, kind and target (operation, hook or registry symbol).
- An occurrence on each side: `CodeRef`, `payload` (the call as the facts show it), guard, `transaction_scope` (`transaction (*sql.Tx)`, `none: *sql.DB
  outside any transaction`, `none (no transaction passed)`, `unknown (receiver type unresolved)`, for Python —
  the enclosing `atomic/begin` or a subsequent `commit`), `lifetime_scope` (`caller`, `detached: go …`, `task: …`,
  `with …`, `try/finally`, for React — cleanup and stale results), `failure_behavior` (error returned,
  deferred rollback, detached, swallowed except). Cardinality is the number of occurrences on the side.
- Match: a key present only on base is first looked up on head only in another construct (`moved`; only the transaction
  and lifetime scopes are compared; guard and error handling stay with the old construct). Without a semantic call
  edge (head callers of the new construct from the old file) the move is `partial`. Otherwise `removed`, with a note that an
  alternative mechanism outside the changed files is not ruled out. A key on both sides is `preserved` or `changed`
  with notes: transaction scope, lifetime, failure behaviour, guard, multiplicity, order relative to other
  effects of the construct. Matching SQL or names proves nothing: receiver, scope and contract are compared.
- Resolution: `resolved`; `partial` (move without a call edge, removed/added, Python, registry without a guarantee);
  `dynamic_unknown` (unresolved receiver, `dynamic_dispatch`/`monkey_patch` in the same changed construct) —
  then `preserved` becomes `unresolved`.
- Registry binding: each `effects.yaml` entry is bound to its `implementation_refs` in this diff (oids
  base/head). `CURRENT` carries `durable_with_transaction` into the ledger entries; `IMPLEMENTATION_CHANGED` or
  `UNBOUND` does not: the entry gets the note «not carried», and the packet gets the gap `REGISTRY_RECHECK`. For the
  `durable_with_transaction` guarantee, a call outside a transaction or in a detached goroutine after the transaction on base is `changed` with
  a note that a recovery path (relay, retry, reconciliation) has not been established yet: loss is not asserted.
- `dependencies` — `(side, path, blob_oid)` of all occurrences and implementation refs: the ledger is reused while
  they match.

The ledger is linked to obligations: EFFECT-01 is activated by changed `implementation_refs` or by an entry that is not
`preserved`, EFFECT-02 additionally by an entry with changed lifetime/failure. In a unit's packet, entries that are not `preserved`/`added`
go as `REFUTING` with notes in the reason, the rest as `SUPPORTING`; `dynamic_unknown` is the gap `EFFECT_UNRESOLVED`. Both ledger
gaps, like `UNSEEN_GAPS`, turn `SUPPORTED` into `UNKNOWN/SEMANTIC_GAP`; `VIOLATED` remains. To view the ledger:
`sych review --format json` (`effect_ledger`), `sych show RUN --format json` or the `sych-effect-ledger` evidence.
The unsafe/safe pairs of §12.4 (Tx→DB and a helper with `*sql.Tx`, outbox → goroutine and a preserved call, changed
wrapper implementation, React cleanup/stale results, invalidation removed/moved into a hook, Python write outside `with`,
`getattr`) are tested in [`tests/effect_ledger.rs`](../crates/sych-adapters/tests/effect_ledger.rs). Generating
runtime tests and full-program analysis are not required.

## Pinned linters

With `--semantic on`, Sych itself runs pinned linters on the changed head files — only those that do not
execute repository code and work without project setup:

| Linter | Files | Invocation | Project configuration |
|---|---|---|---|
| ruff | `.py`, `.pyi` | `check --output-format sarif --no-cache --exit-zero --force-exclude` | `pyproject.toml`, `ruff.toml`, `.ruff.toml`; without them — ruff default rules |
| oxlint | `.js/.jsx/.mjs/.cjs/.ts/.tsx/.mts/.cts` — also in a JS project without a TypeScript backend | `--format sarif` | `.oxlintrc.json`; if `jsPlugins` or `oxlint.config.*` (JS/TS) appear anywhere in the tree, the project config is not read (`--disable-nested-config -c {}`); this is a limitation |
| actionlint | `.github/workflows/*.y(a)ml` | `-shellcheck= -pyflakes= -format '{{json .}}'` | `.github/actionlint.y(a)ml` |
| zizmor | workflows, `action.y(a)ml`, `.github/dependabot.yml` | `--offline --format sarif` | `zizmor.yml`, `.github/zizmor.yml` |

Head is materialized from Git objects (as for language servers), and the linter runs in it with a cleared environment:
empty `PATH` (actionlint will not find shellcheck, oxlint will not find node), `HOME` is an empty temporary directory (no
user configs or credentials), no network, at most 120 s. Absolute output paths are mapped to
repository paths; a finding gets a `CodeRef` if its line exists on head. The result is `LintFacts` in the `facts` artifact and
in the report (`lint_facts`), derived static evidence `PARTIAL` with producer `<linter> <version>`. If the linter
configuration file is changed in this diff, a limitation states that the rule set is the change author's choice.

In a unit's packet, findings on changed head lines are `REFUTING` (`lint <linter> <level> <rule>: …`); findings on
unchanged lines do not enter the packet. A linter that could not be downloaded or run is the gap
`LINT_UNAVAILABLE`: its silence is not read as a clean result. Workflow findings (actionlint, zizmor) go
into the packet of any language and supplement TEST-INTEGRITY. ESLint, clippy, golangci-lint, mypy/pyright are not run:
they execute the project's config/plugins or require a build; their reports are accepted from CI (SARIF, cargo JSON, etc.).

## Semantic cache

Side facts are cached in `$SYCH_TOOLS_DIR/../semantic` (default `~/.cache/sych/semantic`). The key is the
Sych version, algorithm revision, snapshot id (it pins both trees and the diff, and hence the requested files), side, language, server with version,
arguments, initialization options and build configuration. An access partition (repository
identity) is stored with the entry; on a hit it is re-checked, so another repository with the same key gets a miss. Only
`READY` responses are cached. A cache write error costs only a recomputation.

The algorithm revision is separate from the crate version: results from before workspace root canonicalization are
recomputed, and old cache files are not deleted.

## ContextBroker

`sych_core::broker` builds one discovery packet per review unit and one verification packet per obligation
(§9), split into chunks when its required spans do not fit one request together. A packet contains no source text: only `CodeRef`s with hash, selection reasons, priority
(`REQUIRED → REFUTING → SUPPORTING`), completeness, relations (`called_by`, `calls`, `uses_type`, `implemented_by`, `tested_by`,
`read_by`, `written_by`, `referenced_by`), records of semantic queries, excluded items with reasons, and gaps. Text is read back via
`SnapshotReader`, which verifies the hash. Packets are stored as the run's `context_packets` artifact. An example with excluded
sources is [packets-with-exclusions.json](examples/context/packets-with-exclusions.json): a Rust crate with
`initial_tokens_per_unit: 450`; the `save` construct of both sides is required and shown, its caller `handle` went into
`excluded` with reason `BUDGET`.

Assembly order: changed lines of both sides and their enclosing constructs (required) → contracts of the obligation's rules
from the trusted policy (as text in the prompt under the obligation, with path and hash) → CI diagnostics on
changed head lines (`REFUTING`) → rust-analyzer/tsserver error/warning diagnostics (auxiliary: without
proc-macros and dependencies, some of them are analysis artifacts) → callers, callees (wrappers and helpers in full),
definitions of used types,
implementations, tests and depth-1 references, and import blocks of the changed file from syntax facts (Rust `use`,
TS `import`; auxiliary). Overlapping and adjacent spans of the same file and side are merged, keeping
all reasons.

§9.3 limits: the unit budget is `context.initial_tokens_per_unit` (16000), a verification packet gets the sum over units, but no
more than `context.max_tokens_per_request` (48000); tokens are counted as bytes of text (an upper bound). Files per
run: `context.max_source_files` (80, does not apply to required spans). 24 semantic queries per unit and 160 per run,
spans up to 160 lines, text blobs up to 512 KiB — constants of §9.3 for now. Spans of a verification packet are
packed first-fit into up to four chunks of the request budget, each chunk its own packet and assessment. A
required span larger than the budget, or beyond the four chunks, is not truncated: it goes into `excluded` as
`REQUIRED_OVER_BUDGET` on a packet of its own, and the obligation gets `UNKNOWN / CONTEXT_LIMIT` unless a chunk
that fit shows a violation. Syntax risk shapes and CI diagnostics (`REFUTING`) are taken only on
changed lines and are merged with the required hunk. Writers of a changed symbol outside the change are also `REFUTING`:
such a span over budget is excluded as `BUDGET`, leaves the gap `REFUTING_OVER_BUDGET`, and the packet becomes `PARTIAL`.

`packet resolution`: `UNAVAILABLE` — nothing was read; `PARTIAL` — there are gaps (no facts for a Rust/TS file,
backend `UNSUPPORTED/FAILED`, unresolved locations, query limit, required content over budget, sources in languages
without a backend — Java, Kotlin, Ruby, C/C++, etc.; Python — `UNSUPPORTED` until M6; changed CSS/HTML without browser/accessibility/visual reports — `NO_VISUAL_EVIDENCE`); otherwise `SATISFIED`. This is the sufficiency of what was collected, not the truth of the obligation.

§9.4 typed operations (`sych_core::broker::Operation`, a closed enum without extra fields): `read_span`, `read_symbol`,
`find_definition`, `find_references`, `find_implementations`, `find_callers`, `find_callees`,
`search_source` (fixed string only, one line of up to 200 characters, `git grep -F` over the pinned tree, at most 5 s —
otherwise `FAILED` with a reason) and
`read_evidence`. Answered semantic queries are `PARTIAL`; a symbol outside the changed constructs is `UNSUPPORTED`
(live server queries are not connected yet). Secret paths are not read and do not appear in search results.

## Limitations

- An answered query is always `PARTIAL`: an empty response does not prove the absence of callers.
- Rust: build scripts, proc-macros and `cargo check` are disabled; cargo runs offline. Generated and
  macro-expanded items are absent; a rust-analyzer health warning (for example, sysroot metadata offline)
  is recorded in limitations. Features are default, target is host.
- Without `tooling.dependency_install`, dependencies are not installed: imports of npm and workspace packages are not
  resolved, and their callers are absent. yarn/npm lockfiles are not installed yet.
- rust-analyzer with health `error` (for example, no toolchain for cargo) yields `FAILED`, not empty responses.
- A call site is one line; for a calling construct longer than the span limit, only the call site enters the packet.
  A callee longer than the span limit does not enter the packet; this is recorded in gaps.
- Type names are selected by the capitalization convention (Rust, TS), not by semantics: uppercase constants
  also end up in queries, and lowercase types do not.
- Readers and writers: for a non-callable symbol, `documentHighlight` at the position of each of the first 12 references yields
  kind `Read`/`Write`; the rest and references without a kind go into `unresolved` of both sets. A server without
  `documentHighlightProvider` is `UNSUPPORTED`. This is name-based access in code, not the effect ledger (M3).
- Rust offline without installation: sysroot std is resolved only if its dependencies are already in the cargo cache; otherwise
  rust-analyzer emits a health warning, and diagnostics show false type errors.
- Go: only host GOOS/GOARCH without tags and with cgo off; without dependency installation gopls cannot see external modules;
  diagnostics are pull (`pullDiagnostics: true`): compiler errors, vet and the staticcheck subset built into gopls
  (`staticcheck: true` with ST style is not enabled); with uninstalled dependencies these are import errors. Interface dispatch and reflection are `interface_call` and
  `dynamic_dispatch`; call hierarchy does not go through them.
- CI facts are only imported (see [LOCAL_REVIEW.md](LOCAL_REVIEW.md)); `cargo test` JSON (unstable
  libtest format) is not supported — test outcomes are taken from JUnit.
- `rustup` may download the toolchain specified by the reviewed repository's `rust-toolchain.toml`.
