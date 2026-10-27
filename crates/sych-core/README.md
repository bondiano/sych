# sych-core

Sych's domain contracts and pure logic. There is no HTTP, SQL, processes or model SDKs here: everything that
touches the outside world lives in `sych-adapters` and `sych-storage` and plugs in through the types and ports of this
crate. Functions are deterministic for identical inputs, so a review is reproducible.

## What's inside

| Module | Purpose |
|---|---|
| `snapshot`, `manifest` | `SourceSnapshot`, `DiffManifest`, `CodeRef` and the `SnapshotReader` port: exact code coordinates (side/blob/lines/hash) |
| `config` | server config (`sych.yaml`), repository policy (`.sych/`), effective config with hashes and provenance, `tooling` |
| `plan` | planner: review units, exclusions, risk hints and obligations before analysis |
| `reduce`, `result`, `obligation`, `finding` | deterministic reducer: assessments + evidence → statuses, coverage, recommendation |
| `evidence`, `freeze` | evidence records, `ExternalInputManifest` and `EvidenceManifest` |
| `semantic`, `syntax`, `ci` | fact documents: `SemanticFacts`, `SyntaxFacts`, `CiFacts`, their translation into evidence and `RunFacts` |
| `broker` | ContextBroker (§9): context packets, §9.3 limits, typed operations §9.4 |
| `model` | ModelGateway, structured output, budget and cost ledger contracts |
| `path_class`, `glob`, `secrets`, `hash`, `version`, `schema` | path classes, glob, secret paths and redaction, canonical hash, `schema_version`, JSON Schema |

## Invariants

- Code coordinates are built by the harness, not the model: a `CodeRef` always points to a pinned blob and a line hash.
- Repository text and model responses are data. Nothing here executes them or treats them as instructions.
- External documents are versioned (`schema_version`); `decode` rejects an unknown major version.
- The JSON Schemas in `schemas/` are generated from this crate's types: `just schemas`.

## Checks

```sh
cargo test -p sych-core
just schemas   # after changing DTOs
```
