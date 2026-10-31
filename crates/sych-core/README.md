# sych-core

Sych's domain contracts and pure logic. There is no HTTP, SQL, processes or model SDKs here: everything that
touches the outside world lives in `sych-adapters` and `sych-storage` and plugs in through the types and ports of this
crate. Planning and the verdict are deterministic for the same facts and assessments; model answers are not.

## What's inside

| Module | Purpose |
|---|---|
| `snapshot`, `manifest` | `SourceSnapshot`, `DiffManifest`, `CodeRef` and the `SnapshotReader` port: exact code coordinates (side/blob/lines/hash) |
| `config`, `policy` | server config (`sych.yaml`), repository policy (`.sych/`), effective config with hashes and provenance, `tooling`; trusted policy identity |
| `plan` | planner: review units, exclusions, risk hints and obligations before analysis |
| `reduce`, `result`, `obligation`, `finding` | deterministic reducer: assessments + evidence → statuses, coverage, recommendation |
| `evidence`, `freeze` | evidence records, `ExternalInputManifest` and `EvidenceManifest` |
| `semantic`, `syntax`, `ci` | fact documents: `SemanticFacts`, `SyntaxFacts`, `CiFacts`, their translation into evidence and `RunFacts` |
| `broker` | ContextBroker: context packets, their limits and typed operations |
| `model` | ModelGateway, structured output, budget and cost ledger contracts |
| `generic` | the generic rule pack shipped with the engine |
| `deploy`, `migration` | deployment model and transition matrix; `MIG-*` obligations bound to it |
| `terraform`, `kubernetes` | Terraform plans and rendered Kubernetes manifests reduced to allowlisted structure |
| `lint` | diagnostics of the pinned linters Sych runs itself |
| `effects` | effect ledger: bounded base/head table of observable effects from syntax facts |
| `analysis` | model passes without I/O: what each role is shown, its answer contract and the checks on it |
| `status` | independent status dimensions: execution, coverage, recommendation, publication, relevance |
| `scm`, `schedule` | normalized SCM contract (errors, capabilities, webhook signals); the scheduler's pure intent decision |
| `publication`, `lineage` | what an advisory publication shows; finding lineage between revisions |
| `tour`, `handoff` | guided tour of a saved result; fix handoff package of selected findings |
| `api` | `/v1` request/response DTOs and the generated OpenAPI |
| `primitives`, `path_class`, `glob`, `secrets`, `hash`, `version`, `schema` | identifiers, path classes, glob, secret paths and redaction, canonical hash, `schema_version`, JSON Schema |

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
