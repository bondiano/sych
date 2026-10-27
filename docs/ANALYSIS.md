# Model review

Implements roadmap M1.5–M1.7: §7.3, §16.1–16.5, §17.1 of the [specification](SYCH_SPEC.md). The model proposes
hypotheses, assessments and verdicts; the final statuses and findings come from the engine and the deterministic M0 reducer.

Historical Sych SHAs in the measurements and examples below refer to the history before the squash. The saved reports remain
archived evidence; reproducing those runs requires an archived clone with the old history, not the current branch.

```sh
sych db migrate --config sych.yaml
sych review --repo . --base origin/main --config sych.yaml --models cheap_api --format markdown
```

`--models fake` is still the offline fake adapter. `--models roles` routes each role through `models.roles` and its
fallback chain; any other value is a pinned profile from `models.profiles` (with its own chain).
Profiles are resolved the same way as `doctor --probe-model` ([MODELS.md](MODELS.md)): mode, egress allowlist, price
card, credentials and pinned runtime. Transports: OpenRouter, OpenCode Go, GLM Coding Plan via Pi, ChatGPT plan
via Sign in with ChatGPT; the router and pass roles are in [MODELS.md](MODELS.md#router-roles-fallback-and-premium-m113).
`--fake-responses` cannot be combined with a profile.

## Passes

All calls go through the shared gateway and ledger of a single run: each attempt reserves money and tokens before
sending, and the `budgets.analysis` budgets (attempts, tokens, $, deadline) are shared by all passes and repair.

1. **Assessment** — required ones first, then by descending risk; this is the verification reserve: discovery cannot
   spend the budget the obligations need. If a discovery batch (item 3) shows an obligation's verification packet
   in full (each of its spans lies within the batch spans, nothing required is excluded), the assessment
   runs in the same call as discovery: `review@5` returns `assessments` and hypotheses, and such calls
   run first. For a small PR this is one call instead of assessment + discovery. The remaining obligations
   each get a separate `assessment@4`. An assessment missing from the `review@5` reply is `UNKNOWN/MODEL_FAILED`
   ("ob-N was not assessed"). An obligation whose verification packet was split into chunks gets one
   `assessment@4` per chunk; `context_limited` merges them per rule: a violation in any chunk stands, then an
   unknown, then support. The packet holding a required span that fits no chunk (`REQUIRED_OVER_BUDGET`) gets
   no call, and the obligation stays `UNKNOWN/CONTEXT_LIMIT` unless another chunk shows a violation.
2. **Risk audit** (`risk_audit@5`) — for HIGH/CRITICAL risk obligations, a separate search over the obligation's verification packet.
   It does not see the discovery output and runs even when discovery is empty; this is not a promise of
   statistical independence of errors.
3. **Discovery** (`discovery@6`) — over the discovery packets of review units with the list of their obligations. Adjacent
   packets are merged into one call as long as their tokens fit in `context.max_tokens_per_request`: for a
   small PR this is one discovery instead of a call per unit.
4. **Verify** (`verify@5`) — a verdict for each candidate of a pass, on the same packets plus the answers to context
   requests. A confirmed verdict may carry `suggested_change`, the full new text of lines of a shown HEAD span; the
   engine drops it, not the finding, when it names anything else. The inline comment offers it as a GitHub
   suggestion when it covers exactly the commented lines, otherwise as a code block.

A pass that did not complete (budget, deadline, truncation, provider error, schema after repair) is recorded in the
`analysis` artifact as `FAILED` and goes into `unresolved_questions`; coverage becomes `PARTIAL`
(`LIMIT_REACHED` for the budget and output cap, `RETRIES_EXHAUSTED` for the rest: the router has already gone through all eligible
routes of the chain).

## Nits

The discovery rules for minor remarks are taken from the [review comment research](research/review-comments.md):
a nit is no higher than `LOW`, does not change behavior, and names the location, the error it prevents and the fix; the strongest ones are about consistency
with what already exists in the repository; nothing that formatters, linters and the compiler catch; deletion is better than a new
abstraction; performance only with an argument about a hot path; no more than 3 nits per review unit; when in doubt, a
question about a concrete scenario instead of an assertion.

## What the model sees

The prompt is assembled from trusted layers: role → output contract → rules → task; the accepted rules at the target tip are
trusted task text. Source code, the packet gaps and hypothesis texts appear only in DATA blocks with a fence derived from the content
hash. Each packet span is shown with line numbers under a local id `ref-N`; evidence is `ev-N` (`ev-1` is the
source spans of this prompt), obligations are `ob-N`, criteria are `c-N`, candidates are `cand-N`. The model does not write
paths, hashes or evidence ids: an anchor is `{ref_id, start_line, end_line}` within a given span, and the engine
builds the `CodeRef` itself through the snapshot reader with a hash check.

The packet's source spans are stored as derived evidence `source` (`SOURCE_OBSERVATION`): `COMPLETE_FOR_QUERY` for
exactly these spans if no required span was excluded, otherwise `PARTIAL`. The packet gaps are its limitations.

## Engine checks

- **Discovery** ([`analysis/discovery.rs`](../crates/sych-core/src/analysis/discovery.rs)): a candidate with at least
  one fabricated reference (`ref`, lines outside the span, `ev`, `ob`) or without an anchor is rejected entirely, not
  corrected; the schema requires at least one anchor (`minItems: 1`) so that upstream constrained decoding does not return
  an empty array. More than 20 candidates is `CANDIDATE_OVERFLOW`: the extra ones are dropped, which is a gap and `LIMIT_REACHED`
  (coverage `PARTIAL`); more than 4 context requests are rejected by count. The model's `unresolved_questions` go
  into the result's `unresolved_questions` as `open question: …`, without changing the execution outcome and coverage.
- **Context requests** are a closed enum `read_lines | find_references | find_callers | find_callees |
  find_implementations | search_source` without hashes or commands; it is translated into a typed broker operation (§9.4), answered by `Ops` against the pinned
  snapshot (fixed-string search up to 5 s, secret paths are not read, up to 3 spans per answer). The answer is added to the
  verify pass packet as a new `ref-N` with the reason `answer to: …`.
- **Redaction**: secret values in all DATA blocks (spans, broker answers, candidate texts) are replaced with
  `[REDACTED]` when the prompt is rendered (`Prompt::messages`), and a changed block is marked `[secret values redacted]`;
  `CodeRef`, hashes and line numbers stay as they were.
- **Assessment** ([`analysis/assess.rs`](../crates/sych-core/src/analysis/assess.rs)): every criterion of the
  rule (`acceptance.criteria` or statement) must have a result; `MET/NOT_MET` without an anchor becomes
  `UNKNOWN`. A reply about another `ob-N`, unknown `ref`/`ev`/`c-N` or lines outside the span yield `UNKNOWN`
  `INVALID_REFERENCE` regardless of the proposed status: the engine does not repair evidence by deleting references. `SUPPORTED` only if all criteria are `MET`; `NOT_MET` with `SUPPORTED` is
  `CONTRADICTORY_ASSESSMENT`; all anchors in test files are `TEST_ORACLE_ONLY` (a test of the same change does not
  replace an accepted requirement); `VIOLATED` only with `NOT_MET`; `NOT_APPLICABLE` only with a basis. Then
  the M0 reducer checks evidence kinds, validity and completeness once more. A model refusal, budget or schema after
  repair is `UNKNOWN` with `MODEL_FAILED`/`BUDGET_EXHAUSTED` and the next evidence needed.
- **Verify** ([`analysis/verify.rs`](../crates/sych-core/src/analysis/verify.rs)): a candidate without exactly one
  verdict is `UNRESOLVED`; `CONFIRMED` with at least one unrecognized reference (`ref`, lines outside the span, `ev`) is
  also `UNRESOLVED`. `CONFIRMED` is accepted if there is an anchor and the basis is admissible:
  `EXECUTED_COUNTEREXAMPLE` is impossible (nothing was executed), `PREFERENCE` does not support a defect,
  nit/improvement is no higher than `LOW`, `HIGH/CRITICAL` requires a `STATIC_TRACE` of at least two anchors
  (`HIGH_RISK_WITHOUT_TRACE`). An anchor on a line that already has an imported diagnostic leaves the candidate
  `UNRESOLVED` (`DUPLICATES_EXISTING_DIAGNOSTIC`): without a comparable baseline, novelty is not proven. A rejected
  nit goes to `rejected_candidates` and is not promoted.
- **Findings**: severity, support and verification status are separate fields; every finding has the limitation
  "CONFIRMED by the verification policy on the shown spans; not a proof of the whole program". Findings from different
  passes with overlapping anchors are merged into the most severe one, marked `merged an overlapping hypothesis`.
  A `VIOLATED` obligation without a linked finding yields a rule violation finding from the M0 reducer.
- **Basis and baseline.** Basis kinds: `STATIC_TRACE` (a chain of anchors from the change to the consequence),
  `SEMANTIC_ARGUMENT` (reasoning over the shown spans), `EXPLICIT_RULE` (accepted rule), `PREFERENCE` (only
  nit/improvement); `EXECUTED_COUNTEREXAMPLE` will appear with M3.8 execution. Novelty and `RESOLVED` are decided by the shared
  M2 baseline/revalidation by fingerprint; there is no second diagnostic lifecycle here: without a baseline, a match with an
  existing diagnostic stays `UNRESOLVED`, and the disappearance of a diagnostic closes nothing.

## Artifacts and usage

A run stores the `analysis` artifact: profile and model, passes (template, packet, obligation, status, attempt
ids), assessment records (obligation → criteria → anchors/evidence, rationale, missing inputs, what the engine did not
accept), discovery (accepted and rejected drafts, context requests and answers), candidates with their statuses and all
attempts (reserved, charge, charge state, tokens, resolved model, upstream, generation id). Prompt text and
chain-of-thought are not stored. The result's `usage_summary` is computed from attempts:

- tokens in/out and `reasoning_tokens` (part of output, not added); attempts without reported usage are
  `unknown_usage_attempts` and are not included in the token totals;
- `cost.known` is actual money (reported or from the price card), `cost.reference_estimate` is an estimate from the
  price list, not a charge; an unsettled charge is `unknown_cost_attempts`, not zero;
- `routes[]` — profile/model, `billing` and `enforcement` (`api_hard` / `subscription_observable`), the number of
  attempts, `quota_attempts` (subscription without reported money: included quota was spent, not free) and
  the month's `fixed_fee` (`period`, `amount`, `allocated`); the fee is counted in the monthly pool once, not in the
  run cost. No connected transport reports remaining quota or reset yet.

`provenance.model_adapters` is `profile:model`.

## Checks

Real snapshots (`cheap_api`, budgets from the personal example): Vkusori `f1cec4d14` (TS, one file, fix) —
discovery + verify, one nit rejected, no findings, ~$0.002; Sych `6b134cb` (Rust, 8 files, 5 units) — 3
discovery in batches and verify, 1 candidate `REJECTED`, 1 `UNRESOLVED` (the new method body was not shown),
`COMPLETE_FOR_SCOPE`, ~$0.006. This is a path check, not a quality measurement (M1.8).

- `cargo test -p sych-core --test analysis` — validators: fabricated ids, overflow, criteria, contradictions,
  test oracle, high-risk without trace, nit, duplicate diagnostic, merging.
- `cargo test -p sych-app --test analysis` — passes on a real snapshot with a scripted provider: a safe
  change with empty discovery gets `SUPPORTED`; prose twice gives `MODEL_FAILED`, gaps, no extraction from
  text; `max_attempts: 1` — the reserve goes to the obligation, the rest is `LIMIT_REACHED`; a dispatched timeout gives an
  `UNKNOWN` charge; a non-dispatched `UNSUPPORTED` gives `RELEASED` and `MODEL_FAILED`, not an empty success; a context
  request `read_lines` is answered via the broker and visible to the verifier as `answer to: …`; a small PR gets
  `review@5` instead of assessment + discovery, and an obligation that no batch shows in full
  gets a separate assessment; `review@5` without an assessment leaves `UNKNOWN/MODEL_FAILED`.
- Real smoke of `cheap_api` (DeepSeek V4.1 Flash, `reasoning_effort: low`) on an unsafe/safe pair via
  `review@2`: a removed guard gives `VIOLATED` and a confirmed finding; replacing the guard with `?? []` gives `SUPPORTED`, with no
  findings; 3 calls each (review, risk audit, verify), ~$0.0015 per run ([example](examples/model-review/)).

- `sych eval measure` ([LOCAL_REVIEW.md](LOCAL_REVIEW.md)) on the four M1.7 pairs — moving cleanup and invalidation
  into a project hook, stale-response protection, invalidation — `cheap_api`, 2 runs each:
  [report](examples/quality/m1.7-pairs-cheap_api.json). The labeled defects were found in 8 of 8 unsafe runs, all
  findings at labeled locations, no correctness findings on the 8 safe runs (3 nits), ~$0.023 for 16 reviews.
  Before `minItems: 1` on the candidate's `anchors` (`discovery@3`, `review@2`, `risk_audit@2`) it was 3 of 8: in
  half of its replies DeepSeek named the defect with empty `anchors`, and the engine rejected the draft (`no anchor in the shown
  spans`). This is a small development sample, not a quality assessment (M1.8).

- Full development suite (48 cases, including real Sych and Vkusori commits), `cheap_api`, one run each:
  variant C (regular pipeline) — [report](examples/quality/dev-cheap_api-c.json), variant B without the high-risk audit —
  [report](examples/quality/dev-cheap_api-b.json).

  | | recall | precision | FP on safe | unknown stayed UNKNOWN | attempts | cost |
  |---|---|---|---|---|---|---|
  | C | 19/22 | 19/27 | 6/25 | 0/3 | 107 | $0.078 |
  | B | 20/22 | 20/27 | 5/25 | 0/3 | 104 | $0.084 |

  On a single run B and C are within noise: the high-risk audit did not add found defects here. Weak spots:
  the model closes `unknown` cases with a confident answer (including `VIOLATED` where there is no decision), and on clean
  changes it produces side correctness remarks. This is a development suite, not a quality assessment: the holdout and criteria
  belong to the owner (M1.8).

- Security pairs (path traversal, SQL/command injection, XSS, SSRF, open redirect, unbounded read; 6 TS, 4 Rust,
  4 Go) and 16 nit cases from the [research](research/review-comments.md), `cheap_api`, before and after the nit rules:

  | | defects found | correctness findings on safe security | total correctness findings | nits matched labels |
  |---|---|---|---|---|
  | before | 12/14 | 10/14 | 25 + 5 on nit cases | 5/15 |
  | after | 12/14 | 8/14 | 20 + 3 on nit cases | 5/15 |

  Reports: [before (security)](examples/quality/security-cheap_api-before-nit-rules.json),
  [before (nits)](examples/quality/nits-cheap_api-before-nit-rules.json),
  [after](examples/quality/security-and-nits-cheap_api.json). Go cases only look at the diff for now: Go semantic/syntax is
  M3, and their results do not count as Go support. Some of the "false" remarks on safe cases are substantive (a symlink
  bypassing the prefix check, LIKE metacharacters, a client without a timeout); two fixtures were fixed after the measurement because the model found
  real bugs in them (unimported `formatPrice`, removed route `PUT /me/avatar`).

## Baselines A/B/C on the development suite (M1.8)

Development suite: 103 cases (`eval/quality/dev`: Rust, TS/React and Go safe/unsafe/UNKNOWN pairs, Rust routing/billing/
privacy triples, clean, nitpick and real Sych and Vkusori commits), one run of each variant,
`cheap_api` (DeepSeek V4.1 Flash via OpenRouter), identical budgets from the personal example:

```sh
sych eval measure --suite eval/quality/dev --config examples/personal/sych.yaml --models cheap_api \
  --variant c|b|a --repo sych=PATH --repo vkusori=PATH --output report.json
```

| | defects found (recall) | correctness findings at labeled locations (precision) | safe cases with a correctness finding | UNKNOWN stayed UNKNOWN | obligations matched | nits matched labels | attempts | cost |
|---|---|---|---|---|---|---|---|---|
| C — full pipeline | 34/39 | 39/61 | 22/58 | 2/6 | 10/13 | 4/16 | 226 | $0.168 |
| B — without high-risk audit | 30/39 | 33/51 | 15/58 | 2/6 | 11/13 | 7/16 | 204 | $0.164 |
| A — single reviewer on the diff, no semantic, audit or verifier | 39/39 | 59/113 | 34/58 | 0/6 | 10/13 | 13/16 | 109 | $0.078 |

Reports: [C](examples/quality/dev-cheap_api-c.json), [B](examples/quality/dev-cheap_api-b.json),
[A](examples/quality/dev-cheap_api-a.json). The cost covers the whole review, including rejected candidates, not just
successful findings.

What this single run shows (not a quality assessment; the owner sets the acceptance criteria):

- A finds all labeled defects, but at the cost of noise: 113 correctness remarks instead of 61, false ones on 34 of 58
  safe cases and not a single preserved `UNKNOWN`: the model's unverified hypotheses are passed off as answers. The verifier and
  C's context cut the noise (precision 64% vs. 52%, FP on safe 38% vs. 59%) and lose 5 defects.
- B vs. C: the audit added 4 found defects and 6 labeled findings, but also 7 safe cases with a remark.
  The difference of a single run is close to the variance (see the repeats below).
- `UNKNOWN` cases remain a weak spot of all variants: C/B hold 2 of 6 (both are the new routing/billing triples),
  the model closes the rest with a confident answer.
- Go cases only look at the diff (Go semantic is M3) and do not count toward M1 success; the holdout has not yet been labeled by the owner.

**Repeats** of disputed cases (C, 3 runs each, [report](examples/quality/repeats-cheap_api-c.json), $0.069):
`rust-async-cancellation-unsafe` — found 2 of 3; `rust-lock-across-await-safe` — false remark 2 of 3;
`react-stale-response-safe` — false 3 of 3; `rust-paid-fallback-unknown` and `rust-unknown-charge-unknown` — UNKNOWN
preserved 3 of 3; `rust-log-secrets-unknown` and `rust-result-propagation-unknown` — closed 3 of 3;
`real-sych-context-broker` — no remarks 3 of 3.

## Rust by category (M1.14)

The same runs, Rust pairs (48 cases): each cell is "found in unsafe · remark on safe · UNKNOWN preserved".

| pair | C: unsafe · safe · unknown | B: unsafe · safe · unknown | A: unsafe · safe · unknown |
|---|---|---|---|
| clean-rust-extract-function | — · ok · — | — · ok · — | — · ok · — |
| nit-rust-atomic-ptr | — · ok · — | — · ok · — | — · FP · — |
| nit-rust-history-comment | — · ok · — | — · ok · — | — · FP · — |
| nit-rust-inline-helper | — · ok · — | — · ok · — | — · ok · — |
| nit-rust-must-use | — · FP · — | — · ok · — | — · FP · — |
| nit-rust-reread-in-loop | — · ok · — | — · ok · — | — · ok · — |
| nitpick-doc-typo | — · ok · — | — · ok · — | — · ok · — |
| real-sych-context-broker | — · ok · — | — · ok · — | — · FP · — |
| rust-async-cancellation | 1/1 · FP · — | 0/1 · FP · — | 1/1 · ok · — |
| rust-blocking-in-async | 1/1 · ok · — | 1/1 · ok · — | 1/1 · ok · — |
| rust-cfg-feature | 1/1 · ok · — | 0/1 · ok · — | 1/1 · FP · — |
| rust-cfg-target | — · — · closed | — · — · closed | — · — · closed |
| rust-command-injection | 1/1 · FP · — | 1/1 · FP · — | 1/1 · FP · — |
| rust-error-handling-weakened | 1/1 · ok · — | 1/1 · ok · — | 1/1 · FP · — |
| rust-lock-across-await | 1/1 · ok · — | 1/1 · ok · — | 1/1 · FP · — |
| rust-log-secrets | 1/1 · ok · closed | 1/1 · ok · closed | 1/1 · FP · closed |
| rust-paid-fallback | 1/1 · ok · kept | 1/1 · ok · kept | 1/1 · ok · closed |
| rust-path-traversal | 1/1 · ok · — | 0/1 · ok · — | 1/1 · ok · — |
| rust-raii-guard | 1/1 · ok · — | 1/1 · ok · — | 1/1 · ok · — |
| rust-result-propagation | 1/1 · ok · closed | 1/1 · ok · closed | 1/1 · FP · closed |
| rust-serde-contract | 0/1 · ok · — | 0/1 · ok · — | 1/1 · FP · — |
| rust-sql-injection | 1/1 · FP · — | 1/1 · FP · — | 1/1 · FP · — |
| rust-task-lifetime | 1/1 · FP · — | 1/1 · FP · — | 1/1 · FP · — |
| rust-test-ignored | 1/1 · — · — | 1/1 · — · — | 1/1 · — · — |
| rust-trait-api | 1/1 · ok · — | 1/1 · ok · — | 1/1 · ok · — |
| rust-unbounded-read | 0/1 · FP · — | 1/1 · FP · — | 1/1 · FP · — |
| rust-unknown-charge | 1/1 · ok · kept | 1/1 · ok · kept | 1/1 · ok · closed |

Rust, variant C: 16 of 18 defects found (missed: the serde contract and unbounded read), false correctness remarks
on 6 of 25 safe/clean cases (cancellation, command/SQL injection, task lifetime, unbounded read, must-use nit),
`UNKNOWN` preserved in 2 of 5 (the routing and billing triples; the model "decides" the macro, cfg target and `Debug` of a foreign type).
Error propagation, RAII, lock/blocking across `.await`, trait/API and cfg-feature are found in all variants, except for
isolated misses by B. This is a measurement, not proof that it "reviews Rust well": the holdout is not labeled, and there is one run.

**After fixing the fixtures.** In that measurement 13 Rust fixtures had no `src/lib.rs`, and the serde pair lacked the
`serde` dependency: rust-analyzer did not load the workspace, and the semantic facts of these cases were `FAILED`. The fixtures were fixed, and the safe
cases got `tolerated` labels (a correct but non-target remark: a dropped stop sender, the required field
`branch`, LIKE metacharacters, a detached flush, 413 on any `to_bytes` error). Then the same 48 cases, variant C,
`cheap_api`, two runs each: the current prompts and two attempts at severity/`UNRESOLVED` rules (the second with one run).

| | current prompts ×2 ([report](examples/quality/rust-cheap_api-c-fixed-fixtures.json)) | rules v1 ×1 ([report](examples/quality/rust-cheap_api-c-severity-rules-v1.json)) | rules v2 ×2 ([report](examples/quality/rust-cheap_api-c-severity-rules-v2.json)) |
|---|---|---|---|
| defects found | 31/36 (86%) | 12/18 (67%) | 32/36 (89%) |
| precision | 71% | 54% | 65% |
| safe runs with any correctness finding | 10/50 | 8/25 | 14/50 |
| of which false (not `tolerated`, not an inflated nit) | 1/50 | 5/25 | 4/50 |
| `UNKNOWN` preserved | 5/10 | 2/5 | 4/10 |
| nits matched labels | 9/12 | 5/6 | 8/12 |
| cost | $0.169 | $0.094 | $0.183 |

The main effect came from the fixtures, not the prompts. The rule "`DEFECT` only with a harmful consequence" and strict
`UNRESOLVED` (v1) left defects found by discovery unverified (SQL injection, path traversal, cancellation,
lock across `.await`, cfg feature), and the rule about the public signature produced a new false "will break calling code" on
four safe cases; v2 without it restored recall, but with more false remarks than the current prompts. Both attempts
were rolled back. With the current prompts, async cancellation (2/2), the serde contract (2/2: discovery finds it,
the verifier leaves it `UNRESOLVED`) and unbounded read (1/2) are missed; the only false remark is the hits counter before `.await` in
`rust-lock-across-await-safe`; `UNKNOWN` is lost in cfg target, result propagation and log secrets. `tolerated`
is matched by lines, so the titles of these remarks were checked manually: none of them asserts something forbidden
("wildcard injection" is about LIKE, not SQL injection). The holdout is still not labeled.

**Fixtures v2 and the engine.** The three missed pairs lacked the code the defect depends on: the body of `read_frame`
(async cancellation), a public route with `OrderView` (serde contract), a router without a body limit (unbounded read).
It was added to the `base` of both fixtures of the pair; the labels did not change. Then the engine: changed `select!`/serde/cfg/
unexpanded macros are flagged on their span; the verifier sees the fragment's obligations and requests
context once; code under an inactive cfg, an unexpanded macro and the `Debug` of a type outside the snapshot are gaps under which an obligation
is never `SUPPORTED`, and a `VIOLATED` resting only on such code becomes `UNKNOWN`; calls run according to
`max_parallel_calls`. The same 47 cases (without `real-sych-context-broker`: the run was without `--repo sych=…`), variant C,
`cheap_api`, two runs each:

| | fixtures v2, engine before ([report](examples/quality/rust-cheap_api-c-fixtures-v2.json)) | fixtures v2, engine after ([report](examples/quality/rust-cheap_api-c-engine-v2.json)) |
|---|---|---|
| defects found | 33/36 (92%) | 35/36 (97%) |
| precision | 64% | 69% |
| safe runs with any correctness finding | 18/48 | 15/48 |
| of which false | 6/48 | 4/48 |
| `UNKNOWN` preserved | 4/10 | 8/10 |
| obligations matched labels | 22/26 | 25/26 |
| nits matched labels | 7/12 | 6/12 |
| attempts / cost | 225 / $0.176 | 279 / $0.203 |

The fixtures brought back the serde contract and async cancellation (2/2 already before the engine change). The engine added stability: unbounded
read, trait/API and SQL injection are found in both runs, and `UNKNOWN` now holds in result propagation and log
secrets (2/2, was 0/2). One defect was missed: cfg feature in one run. In cfg target, `UNKNOWN` is lost on a
correct but unlabeled remark "the configuration directory on Windows changed without a migration"; the decision on
`tolerated` is up to the owner. False remarks: error-handling-weakened-safe (2/2), lock-across-await-safe and path-traversal-safe
(1/2 each). The second verifier round and gaps cost +24% attempts and +15% money. The holdout is not
labeled, and with two runs per case this is a development suite measurement, not proof of quality.

**Real review of Sych**: commit `26cc0f4` (verification reserve, 6 files) against `81ff2a7`, `cheap_api`,
semantic via pinned rust-analyzer:
[result](examples/rust-review/sych-26cc0f4.result.json), [markdown](examples/rust-review/sych-26cc0f4.md).
Coverage `PARTIAL`, recommendation `NEEDS_EVIDENCE`; no findings, 2 candidates rejected by the verifier, one
`UNRESOLVED` (the `router.search` contract was not shown); `TEST-INTEGRITY` is `UNKNOWN/TEST_ORACLE_ONLY`; the required
context over budget is named (`REQUIRED_OVER_BUDGET` for router.rs and docs). 4 attempts, $0.0068;
`provenance.engine_version` is `0.1.0+<build commit>`. Commands of the original run:

```sh
SYCH_DATABASE_URL=… sych db migrate --config examples/personal/sych.yaml
sych review --repo . --base 81ff2a7 --head 26cc0f4 --config examples/personal/sych.yaml --models cheap_api
```

Support is limited to what rust-analyzer and syn see on pinned commits without executing build scripts and
proc-macros: unexpanded macros, cfg outside the active features and dynamic dispatch yield gaps, not a proven set.

## Limitations

- Independent passes run in parallel, with no more than `budgets.analysis.max_parallel_calls` (default 2) calls
  at once, in two stages: first all calls that assess obligations (required ones first), then risk audit,
  discovery and verify. Result parts are merged in plan order, so the outcome does not depend on which call
  returned first; the ledger reserves each attempt atomically, so the run's shared budget is respected.
- Context expansion is one round per pass: verify sees the answers to discovery requests, and discovery is not
  called again. The verifier itself may request context once: candidates that remain `UNRESOLVED` are checked
  a second time with the answers; if there is no budget for the second call, the first-call verdicts remain.
- The output schema is passed without `$ref` (all subschemas inline): with references, upstream DeepSeek returned empty
  `anchors`, and the engine rejected all hypotheses.
- Reasoning models spend ~1,000–2,400 tokens of each reply on reasoning: with the §17.1 default of 2,000 tokens per
  request, discovery and verify get truncated (`TRUNCATED`). The personal example raises
  `budgets.analysis.max_output_tokens_per_request` to 4,000.
- Context compression. Packets whose shown bytes, sent about twice (search, then verify), exceed
  `budgets.analysis.max_input_tokens` are rebuilt with request budgets scaled down in proportion, to a quarter
  at most; what no longer fits is cut by priority with the usual `BUDGET`/`REQUIRED_OVER_BUDGET` gaps. A supporting
  construct over budget keeps its first line (`transformation: signature`, excluded as `SIGNATURE_ONLY`) instead of
  disappearing.
- The model is nondeterministic: the same run can yield `UNKNOWN` if it did not attach an anchor to a criterion; this is
  expected strictness, not a bug.
