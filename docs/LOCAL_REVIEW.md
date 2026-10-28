# Local review

Implements roadmap M0.6: §24.1, §24.4–24.5 and §27.1 of the specification. Local review reads only
Git objects, stores the run in local storage ([STORAGE.md](STORAGE.md)) and publishes nothing to the SCM.
The fake adapter (`--models fake`) proves that the harness works, not review quality; review with a real model is
`--models PROFILE` ([ANALYSIS.md](ANALYSIS.md)).

## Workflow

```sh
sych db migrate
sych review --repo . --base origin/main --head HEAD --models fake \
  --format json --output .sych-output/review.json
sych review --repo . --base origin/main --models fake --format markdown --output .sych-output/review.md
sych show --run RUN_ID --format markdown
sych explain --run RUN_ID --obligation EFFECT-01 --format markdown
```

- `--base` is the target ref: policy is taken only from its tip, and the diff is computed from the `merge-base`; `--diff-base` sets
  an exact two-dot comparison.
- `--format json|markdown` (JSON by default); `--output PATH` writes a file and creates the directory, otherwise stdout.
- `--semantic on|off` (`on` by default): for Rust, TS/JS, Go and Python (ty) files, pinned language
  servers; the same switch runs the pinned linters ruff, oxlint, sqruff (SQL), mago (PHP), jarl (R), actionlint and
  zizmor on changed head files ([SEMANTIC.md](SEMANTIC.md#pinned-linters)); they produce semantic facts
  ([SEMANTIC.md](SEMANTIC.md)), which are stored as derived evidence, and the ContextBroker builds context packets.
  The first run downloads the servers; a repeat on the same snapshot takes facts from the semantic cache.
- `--ci-facts FILE` (repeatable) imports ready-made CI reports: output of `cargo check/clippy/build
  --message-format=json`, JUnit XML (cargo-nextest, vitest, Jest, gotestsum), SARIF, GitLab Code Quality, LCOV,
  Cobertura, `go test -json` (with `-race`: each race is a `race` diagnostic with its first frame), `go test -coverprofile`
  (import paths of the root `go.mod` → repo paths) and `go vet -json`. Absolute CI checkout paths are matched to the repo
  by suffix. Race and coverage confirm only executed paths. The format is detected from the content; files up to
  32 MiB. `--ci-manifest FILE` describes the job (YAML/JSON, `schema_version: 1`, `job`, `tested_oid` — hex from
  `git rev-parse HEAD`, `toolchain`, `target`, `features`). The report becomes an external input
  (`ExternalInputManifest`) with binding `ci` and `trust_class: AUTHOR_CLAIM`: a local import does not prove
  provenance. Without a manifest it is `UNBOUND`/`STALE`; if `tested_oid` is not equal to head — `STALE`, and its locations are not
  carried over to head. Errors and warnings on changed head lines enter the context packet as
  `REFUTING`, the rest as `SUPPORTING`. Skipped tests, `#[ignore]`, `it.skip` and `#[allow]/#[expect]`
  are recorded separately and are not counted as a fix.
- Progress is written to stderr via `tracing`; verbosity is set by `SYCH_LOG` (`info` by default, `debug`,
  `warn`, or per crate: `SYCH_LOG=sych_adapters=debug`). An error is printed as a single chain
  `sych <command>: context: cause`.
- `show` and `explain` read stored artifacts and do not run analysis; the `show` JSON matches the `review` JSON
  until the 7-day snapshot/diff retention expires; after that those fields are `null`, and the rest is still readable.
  `--obligation` accepts an obligation id or a rule id.
- JSON and Markdown contain the snapshot (target tip, diff base, head), PolicySnapshot, provenance and head policy diff,
  scope, obligations, findings, gaps, evidence, semantic facts, context packets and usage; `explain` shows the
  obligation's context packet. Repository text in Markdown is escaped (HTML, markup);
  terminal control characters and bidi characters are not output verbatim in any format.

Output examples: [review.json](examples/local-review/review.json), [review.md](examples/local-review/review.md),
[explain.md](examples/local-review/explain.md).

## Reading a tour

`sych tour --run RUN --config sych.yaml` generates a tour with a model and stores it ([ANALYSIS.md](ANALYSIS.md)). A
stored tour is read without a model or a config:

```sh
sych tour show --run RUN [--tour ID] [--repo .] [--brief] [--format markdown]   # starts your session
sych tour reveal --run RUN --step stop-1                                        # the answer of a step's question
sych tour complete --run RUN    # or: sych tour dismiss --run RUN
```

- The newest tour of the run is used unless `--tour` names one. Code comes from the run's pinned blobs, read from
  `--repo`, never from the model's text. When the repository has moved past the reviewed head, the tour is `STALE`, but it
  keeps the code the review saw. Once the source snapshot is pruned or the blobs are gone, each location is listed as
  "source unavailable" (`source_unavailable` in JSON) and is not dropped.
- An answer stays hidden in `show` until you `reveal` it. The session goes `IN_PROGRESS → COMPLETED | DISMISSED`, and a
  finished session only reads.
- **Completing a tour is not an approval.** A session is your personal progress (`tour_sessions`): it never changes
  the tour artifact, the result, its recommendation, obligations or a waiver, and progress and answers are never
  published to the PR.

## Fake responses

Without `--fake-responses` the fake adapter assesses nothing: applicable obligations stay `UNKNOWN/NOT_ASSESSED`.
The fixture defines evidence and assessments; the harness computes ids, hashes and CodeRef from the pinned snapshot:

```yaml
schema_version: 1
delay_ms: 0                       # simulated model latency, for testing cancellation
evidence:
  - key: save-keeps-event-in-tx   # name within the fixture
    kind: source
    completeness: COMPLETE_FOR_QUERY
    trust_class: SOURCE_OBSERVATION
    validity: VALID               # default
    code_refs:
      - { path: backend/orders/store.go, start_line: 4, end_line: 9 }   # side: HEAD by default
assessments:
  - rule_id: EFFECT-01
    status: SUPPORTED             # VIOLATED | UNKNOWN (+reason_code, next_evidence_needed) | NOT_APPLICABLE (+basis)
    evidence: [save-keeps-event-in-tx]
```

Unknown fields, evidence keys, paths and lines outside the file — exit 2 before the run is created. The reducer applies
the same rules to the responses as to a model: SUPPORTED only on VALID complete evidence of an allowed kind and not solely on the
author's word, otherwise `UNKNOWN/INSUFFICIENT_EVIDENCE`.

## Exit codes

| Code | When |
|---:|---|
| 0 | result stored (operational mode does not fail because of findings) |
| 2 | invalid flags, refs, config, policy or fixture |
| 3 | no usable stored result: database not migrated, write failure, result could not be output |
| 10 | `--exit-policy strict` and a significant violation (`ATTENTION_REQUIRED`) |
| 11 | `--exit-policy strict` and coverage is not `COMPLETE_FOR_SCOPE`; 10 takes precedence |
| 130 | SIGINT/Ctrl-C during analysis: run `CANCELLED`, result not stored |

Code 4 (credentials) is reserved for the real M1 providers. On exit 3 after storing (`--output` could not be
written), the result is available via `sych show`.

## Fake eval

```sh
sych eval run --suite eval/smoke.yaml --models fake [--output report.json]
```

Each suite case builds a temporary Git repository (`base` — files of the target commit, `head` — changes on top,
`null` deletes a file), runs a review with the fake adapter in temporary storage and compares significant states:
recommendation, coverage, obligation statuses by rule id, the number of findings and units. Runtime ids and timestamps are not
included in the report, so identical fixtures produce an identical report ([example](examples/local-review/eval-smoke.json)).
Exit 0 — all cases matched, 1 — there are mismatches, 2 — invalid suite. There are no external calls: `model_attempts` equals 0.

## Quality suite

`eval/quality/dev/` is a labelled development suite (§27.2–27.3): one `<id>.yaml` per case, `safe/unsafe`
pairs and honest `unknown` cases for Rust (Result/RAII, cancellation, task lifetime, lock across `.await`,
blocking in async, trait API, cfg/features, serde contract, weakened error handling and tests) and TS/React
(cleanup, stale response and preserved guard, timers, subscriptions, invalidation, controlled inputs, form
submit, project hook, API removal), clean changes, nitpick and two real Sych/Vkusori commits
(`repository_fixture`). `eval/quality/holdout/` is labelled by the owner; the rules are in its
[README](../eval/quality/holdout/README.md).

```sh
sych eval validate --suite eval/quality/dev
```

For repository fixtures add `--repo vkusori=PATH` and, if you have an archived pre-squash clone of Sych,
`--repo sych=PATH`. The current Sych branch does not contain the historical refs of the `real-sych-context-broker` case.

`validate` sends nothing to a model: it checks labelling completeness (rationale, profiles, defect for `unsafe`,
no defect for `safe`, target and locations for `tolerated`, a reason for `unknown`, complete pairs, file name = id, one split per directory), that
every evidence location opens as a `CodeRef`, and that expected syntax facts are present (and forbidden ones
absent). Cases with `repository_fixture` without `--repo` are `SKIPPED`. This is a check of labelling and the harness, not a
measurement of review quality: that is done with a real model in M1.8.

A case may carry CI reports as `ci: {<file name>: <text>}` — a Terraform plan (`terraform show -json`), rendered
Kubernetes manifests, test outcomes. They are imported like `sych review --ci-facts`, with a manifest bound to the case
head, so domain rules (`TF-CHANGE`, `K8S-ROLLOUT`) see them; `validate` fails a case whose report does not import.
`expected.obligations` may name a generic rule (`MIG-*`, `TF-CHANGE`, `K8S-ROLLOUT`, `EFFECT-*`, `TEST-INTEGRITY`)
without a `.sych/rules.yaml`; repository rule ids must still be in it. Domain cases: `mig-*`, `old-frontend-*`,
`tf-replace-db-*`, `k8s-selector-*`.

### Measuring with a real model

```sh
sych eval measure --suite eval/quality/dev --config sych.yaml --models cheap_api \
  [--case ID]... [--runs N] [--variant b|c] [--semantic on|off] [--repo NAME=PATH] [--output report.json]
```

Each case goes through a regular `sych review` with the `--models` profile in the storage from `--config`: the ledger is shared, so
run/day/month limits apply to the whole suite, and each review remains a stored run. `--variant c` is the regular
pipeline, `b` — without the additional high-risk search (baseline B §27.5), `a` — a single reviewer over the diff (baseline A):
without semantic facts, high-risk search and verifier; its unverified hypotheses are scored as findings. `--runs` repeats a case for a nondeterministic model.

Report per run: recommendation, coverage, expected and found defects, findings with anchors and a
`labelled` flag, reconciliation of obligation statuses, for `unknown` — whether the case remained unresolved, attempts and money. Totals
(overall and per language): `recall` (labelled defects found), `precision` (correctness findings at labelled
locations), `clean_false_positive_rate` (safe cases with any correctness finding), `clean_false_finding_rate` (the same without
`tolerated_findings` — correctness findings at `expected.tolerated` locations: a correct but non-target remark for the case — and
without `overrated_nits` — correctness findings at the location of a labelled nitpick), `unknown_kept_rate`, `nit_recall` (labelled
nitpicks hit by a nit or improvement), nits, attempts and
cost. A match is an intersection of an anchor with a labelled evidence location: this approximates meaning, and the owner
checks it against finding titles. The numbers are a measurement; acceptance criteria are set by the owner.
