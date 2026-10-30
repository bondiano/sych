# Model review

Implements roadmap M1.5–M1.7: §7.3, §16.1–16.5, §17.1 of the specification. The model proposes
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
   spend the budget the obligations need. Nor can the obligations spend all of discovery's: assessment and the
   risk audit leave one attempt per standalone discovery batch, at most half of `max_attempts` less the
   verifier's attempt, so a PR with many required rules is still searched. If a discovery batch (item 3) shows an obligation's verification packet
   in full (each of its spans lies within the batch spans, nothing required is excluded), the assessment
   runs in the same call as discovery: `review@5` returns `assessments` and hypotheses. For a small PR this is
   one call instead of assessment + discovery. The remaining obligations each get a separate `assessment@4`.
   Combined and separate calls share one queue in that obligation order (a combined call ranks by its most
   important obligation). An obligation without exactly one assessment in the `review@5` reply is
   `UNKNOWN/MODEL_FAILED` ("ob-N has no single assessment"); a criterion without exactly one result is `UNKNOWN`. An obligation whose verification packet was split into chunks gets one
   `assessment@4` per chunk; `context_limited` merges them per rule: a violation in any chunk stands, then an
   unknown, then support. The packet holding a required span that fits no chunk (`REQUIRED_OVER_BUDGET`) gets
   no call, and the obligation stays `UNKNOWN/CONTEXT_LIMIT` unless another chunk shows a violation.
2. **Risk audit** (`risk_audit@5`) — for HIGH/CRITICAL risk obligations, a separate search over each chunk of the
   obligation's verification packet.
   It does not see the discovery output and runs even when discovery is empty; this is not a promise of
   statistical independence of errors.
3. **Discovery** (`discovery@6`) — over the discovery packets of review units with the list of their obligations. Adjacent
   packets are merged into one call as long as their tokens fit in `context.max_tokens_per_request`: for a
   small PR this is one discovery instead of a call per unit.
4. **Verify** (`verify@5`) — a verdict for each candidate of a pass, on the same packets plus the answers to context
   requests. When the verifier leaves candidates `UNRESOLVED` without asking anything, the engine itself searches HEAD
   for the identifiers their claims name in backticks (two per candidate, four per call) and runs the second round
   on those answers: a validator or guard outside the diff can then refute the claim. A confirmed verdict may carry `suggested_change`, the full new text of lines of a shown HEAD span; the
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
A further DATA block lists every hunk of each changed file the spans show (`-start,count +start,count` and the lines
removed and added), so the model can tell that a shown hunk is its file's whole change: for example, that a test
file only gained lines, which `TEST-INTEGRITY` otherwise could not settle from the spans alone.
An obligation about the tests themselves (the shipped `TEST-INTEGRITY`) also gets the removed lines of every changed
test and CI file, shown or not, read verbatim from BASE (up to 40 lines per file, the rest counted), and
"0 lines removed" for a file that only gained lines: a weakened assertion or gate is visible even when the file's
spans did not fit the packet.

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
- `cargo test -p sych-app --test analysis_budget` — one run budget across passes: with `max_attempts: 1` the required
  standalone assessment goes before an optional combined review; four required rules over two discovery batches
  leave discovery its two attempts and the verifier its one; a search denied beside an unsettled sibling is retried.
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

## Domain rules on real models (M3.4–M3.8)

M3 cases of the development suite (`eval/quality/dev`): migrations against an old/new reader (`mig-*`, Go + SQL),
an old frontend reading a removed field (`old-frontend-*`), Terraform plans (`tf-replace-db-*`, a CI `terraform show
-json` report per case) and rendered Kubernetes manifests (`k8s-selector-*`). Two runs per case, variant C,
`cheap_api` (DeepSeek V4.1 Flash via OpenRouter), the personal example's budgets:

```sh
sych eval measure --suite eval/quality/dev --config sych.yaml --models cheap_api --variant c --runs 2 \
  --case mig-drop-column-unsafe --case … --output report.json
```

Each cell is "found in unsafe · labelled / all correctness findings · remark on safe · UNKNOWN preserved · obligation
status as labelled".

| | Go/SQL migrations | Kubernetes | Terraform | cost |
|---|---|---|---|---|
| [before](examples/quality/m3-domain-cheap_api-c.json): 12 KB request window | 6/6 · 7/10 · 1/6 · 0/2 · 5/12 | 2/2 · 2/2 · 0/2 · — · 1/4 | 0/2 · 0/1 · 0/2 · 1/2 · 3/6 | $0.105 |
| [48 KB window](examples/quality/m3-domain-cheap_api-c-48k.json) (#32) | 4/6 · 5/7 · 0/6 · 0/2 · 6/12 | 2/2 · 2/2 · 0/2 · — · 2/4 | 0/2 · 0/0 · 0/2 · 2/2 · 2/6 | $0.112 |
| [migrations after #33](examples/quality/m3-mig-cheap_api-c.json) (backfill + recovery pairs) | 3/4 · 5/8 · 2/4 · — · 2/8 | | | $0.072 |
| [Terraform after #34](examples/quality/m3-tf-cheap_api-c.json) | | | 2/2 · 2/3 · 0/2 · 1/2 · 4/6 | $0.014 |

What the runs showed and what changed:

- **One finding per revision.** Before #32 a 12 KB request window cut the code the verifier asked for: production
  runs had 5–8 candidates, 1–2 confirmed, the rest `UNRESOLVED` (span not shown) or an assessment skipped over the
  context budget. The ×4 window (48 KB per request, 512 KB per run) trades two migration defects for no remark on a
  safe case and one more preserved UNKNOWN; one run each, so within the variance.
- **Migration obligations stayed UNKNOWN** because the owner's declared transition facts never reached the prompt,
  the earlier migrations that define the schema were filtered out, and a reader hit was one line. #33 puts the
  declared inputs into the MIG criterion, keeps earlier migrations and widens each hit by ±12 lines: the recovery
  unsafe case went from 0/2 to 2/2 `VIOLATED`; the backfill unsafe case is found 1 of 2; `MIG-RESTART` still stays
  `UNKNOWN` on both backfill cases and the recovery safe case is called `VIOLATED` once.
- **TF-CHANGE was always UNKNOWN**: the criterion asked whether a replacement was "intended", which no code shows.
  #34 makes it checkable (a replaced or deleted resource holds no data, or the change keeps it with
  `create_before_destroy`, `prevent_destroy`, a snapshot or a `moved` block): the unsafe plan is `VIOLATED` 2 of 2.
  The safe plan still stays `UNKNOWN`: the model does not take the plan's create-before-delete order as enough.
- **Kubernetes**: the selector mismatch is found 2 of 2; the safe manifest stays `UNKNOWN` (structural evidence of
  a dry-run is never a runtime rollout, by design).

**Whole development suite** after the M3 cases and the 48 KB window (main with #32, before #33/#34; one run,
variant C, [report](examples/quality/dev-cheap_api-c-m3.json); the two real-commit cases need local checkouts and were
skipped): 123 cases, no errors, defects found 42/48 (recall 0.88), labelled correctness findings 58/88 (precision
0.66), a correctness remark on 25 of 66 safe cases (16 of them false), UNKNOWN preserved 4/9, obligations as labelled
16/26, nits matched 4/16; 386 model attempts, $0.327. Against the M1.8 C baseline (34/39, 39/61, 22/58, 2/6, $0.168)
recall and kept UNKNOWNs rose and precision held, at twice the cost: the wider window sends more code per attempt.

**Isolated runner (M3.8).** `execution.profiles` with `image: …@sha256:…` run on docker only: the head is
materialised into a temporary directory and the profile's argv runs with `--network none`, memory, CPU and pid
limits, no credentials; its report is imported like a CI report and bound to the head as `DerivedLocal` evidence
with the runner `<check> in <image>`. Local smoke with a Go test profile: the container had no network interface
(`net=0`), the report bound to the head and reached the obligation. A refused or failing profile is a gap
(`EXECUTION_REFUSED` / `EXECUTION_FAILED`), never a pass.

**Targeted retry trace (M3.8).** Event assertions in `sych_manifest` and `compare_base` run one profile on the diff
base and the head. Real docker run (`cargo test -p sych-adapters -- --ignored execution`, alpine pinned by digest,
2026-10-28): a relay that retries `publish` after an injected crash on the base and drops it on the head. The base
records `commit, crash, publish` (passed); the head records `commit, crash`, so the assertion is `FAILED` and the
report names `BASE_DIFFERS: assertion/publish retried after a crash is Passed on the diff base and Failed on the
head`, bound to the head. The same trace is the quality case `relay-retry-trace-unsafe`: its own profile takes the
script as a `one_of` choice, which the case's request fills (`execution.checks`); reviews and the API pass checks the
same way (`--check`, `checks` in `POST /v1/reviews`).

**Real tour.** `sych review` of the `mig-drop-column-unsafe` repository (one HIGH finding: the migration drops
`orders.status` while `orders/read.go` still selects it; `MIG-RESTART` UNKNOWN), then `sych tour --run …`: `READY`
on the first attempt, two stops, $0.0005. Stop 1 explains the defect with a comprehension question ("Once the
migration has run, what happens to the query in orders/read.go?"); stop 2 records `MIG-RESTART` as undecided, lists
the evidence that would settle it and asks a risk decision ("Should MIG-RESTART be treated as satisfied before that
evidence is gathered?" — "Not on the current evidence"). No status was raised or lowered; the code shown came from
the blobs. The run showed the same lines twice in stop 2 (the obligation's references repeat); fixed in #35.

**Spend** of this round on real models: about $0.95 (1 072 attempts) by the ledger, under the $5 cap.

## Baselines A/B/C on the full feature set (M5.5)

The development suite has grown to 126 cases (Rust, TS/React, Go, Python, migrations, Terraform, Kubernetes, routing/
billing/privacy, clean, nitpick and real commits); 125 ran, `real-sych-context-broker` was skipped without an archived
Sych clone. One run of each variant on the same cases and config (`config_sha256` in each report), `cheap_api`, the
personal example's budgets, 2026-10-28:

```sh
sych eval measure --suite eval/quality/dev --config examples/personal/sych.yaml --models cheap_api \
  --variant a|b|c --repo vkusori=PATH --output report.json
sych eval compare a.json b.json c.json --output comparison.json
```

| | A | B | C |
|---|---|---|---|
| labelled defects found (recall) | 49/49 | 40/49 | 42/49 |
| correctness findings at labelled locations (precision) | 74/137 (54%) | 53/76 (70%) | 51/72 (71%) |
| safe cases with any correctness finding | 38/67 | 19/67 | 17/67 |
| the same without tolerated remarks and overrated nits | 31/67 | 12/67 | 13/67 |
| `UNKNOWN` cases kept unresolved | 0/9 | 6/9 | 4/9 |
| obligations matched | 20/26 | 19/26 | 20/26 |
| labelled nitpicks hit | 12/16 | 4/16 | 5/16 |
| missed defects a dropped candidate reached (`verifier_dropped`) | — | 6 | 7 |
| reviews short of `COMPLETE_FOR_SCOPE` | 32 | 33 | 32 |
| attempts / cost of the whole review | 141 / $0.120 | 292 / $0.280 | 394 / $0.348 |

Reports: [A](examples/quality/m5.5-dev-cheap_api-a.json), [B](examples/quality/m5.5-dev-cheap_api-b.json),
[C](examples/quality/m5.5-dev-cheap_api-c.json), [comparison](examples/quality/m5.5-dev-cheap_api-compare.json): 73 of
125 cases have a different outcome in at least one variant.

What this run shows, within this development sample and one model:

- The verifier and the context trade recall for noise as in M1.8: B and C lose 7–9 of 49 defects and cut false
  remarks on safe cases from 31 to 12–13, precision from 54% to 70%.
- Most of B's and C's misses are not search misses: in 6 of 9 (B) and 7 of 7 (C) a candidate anchored on the labelled
  defect existed, and the verifier rejected it or left it unresolved. The verifier is the place to look for recall.
- C over B: the high-risk search adds 2 defects and costs 24% more; `UNKNOWN` kept 4 vs. 6 of 9. On one run this
  is within the variation below.
- A closes every `UNKNOWN` case with an answer; the verifier is what keeps them open.

**Repeats** of the ten cases where B and C disagreed (C, 3 runs each, [report](examples/quality/m5.5-repeats-cheap_api-c.json),
82 attempts, $0.074): `go-ssrf-unsafe` and `ts-ssrf-unsafe` — found 3 of 3 (C missed `go-ssrf-unsafe` in the main run);
`react-form-submit-unsafe`, `rust-async-cancellation-unsafe`, `rust-blocking-in-async-unsafe` and
`rust-serde-contract-unsafe` — found 2 of 3, and every miss is a `verifier_dropped` candidate; `react-stale-response-safe` —
false remark 3 of 3, `rust-lock-across-await-safe` — 1 of 3; `rust-log-secrets-unknown` — `UNKNOWN` kept 2 of 3;
`ts-dynamic-dispatch-unknown` — closed 0 of 3 without a finding, the obligation answered instead of left `UNKNOWN`. So a
single-run difference of one or two defects between B and C is noise, and the verifier's dropping of true candidates
is the variation itself.
- Not measured here: delivery and anchors on a real pull request (the GitHub self-review of M2.9 records those),
  tour and follow-up cost (`budgets.tour`, `budgets.followup_session`, reported by `sych tour` and `sych ask`), and
  the active human time of reading a review. The holdout is still unlabelled, so these numbers are not a quality
  estimate; acceptance criteria and the usefulness decision are the owner's.

## Scoring by meaning

`eval measure` matches a finding to a label when their anchors overlap. The numbers above are location
matches: a finding that states a different defect on the labelled lines counts, and the right defect anchored
one line off does not. `unknown_kept` only means that there is no correctness finding and something stayed open.

`eval judge` rescores a saved report by the owner's judgments and shows both scores. Each finding in the report
lists the labels it overlaps (`labels`, indexes into the run's `labels`). For `unknown` cases the run also lists what
the review left open (`left_open`).

```yaml
# judgments.yaml: a correctness finding states label N (null: none), or an unknown run kept the labelled question
- { case: vkusori-x-unsafe, run: 1, finding: 0, label: 0 }
- { case: vkusori-x-unsafe, run: 1, finding: 2, label: null }
- { case: vkusori-y-unknown, run: 1, unknown_kept: false }
```

```sh
sych eval judge report.json --judgments judgments.yaml
```

`left_open` also lists unresolved questions, which `unknown_kept` ignores; the flag keeps its old
meaning so new reports compare with archived ones, and an unknown counts by meaning only when both hold. A
correctness finding on a nitpick label counts as labelled by location (as before) but is no judgment
candidate: by meaning it is an overrated nit.

A location match or a kept unknown that nobody judged is listed in `unjudged` and does not count by meaning.
Nitpicks and obligations stay scored by location and status. Reports written before `labels` existed cannot be
judged.

## Holdout, first run

Nine owner-reviewed cases in `eval/quality/holdout` (Vkusori TS/React and one Elixir `unknown` case), one run of
variant C, `cheap_api`, the personal example's budgets: 27 model attempts, $0.057, no errors
([report](examples/quality/holdout-cheap_api-c.json)).

| | defects found | correctness findings at labelled locations | safe cases with a false finding | UNKNOWN kept | nits matched |
|---|---|---|---|---|---|
| labels as measured | 0/2 | 0/1 | 1/6 | 0/1 | 0/1 |
| after relabelling one case | 1/3 | 1/1 | 0/5 | 0/1 | 0/1 |

The one correctness finding was a real defect the labels had missed: in `19ca179d0` the swipe's exit timer
handle is never cleared, so after one animated swipe the always-mounted toast region ignores every later swipe.
That case (`vkusori-swipe-dismiss-ref-unsafe`, measured as `-nit`) was relabelled from the model's answer, so
the second row is not independent of this run; the first row is. The second row is scored by hand: the
finding's anchors (lines 74 and 99) overlap the new label.

What the run shows: the review saw no candidate for the duplicated Faro report (one call); for the sign-in
consent defect a candidate reached the labelled lines and the verifier dropped it; the `unknown` case was closed
as `COMPLETE_FOR_SCOPE`, with open questions about the new test rather than about the batch latency. Nine cases
and one run are too few for a quality acceptance; they show where the pipeline loses defects.

### The two misses, diagnosed

Both cases, three more runs each of variant C on `cheap_api` (19 attempts, $0.035): the labelled defect was found
once in six runs.

- Faro (`vkusori-faro-error-duplicates-unsafe`): discovery proposed no candidate in any run. In two runs the
  required TEST-INTEGRITY assessment was also thrown away as `INVALID_REFERENCE`. It cited `faro.test.ts` lines
  135–137 under `ref-19` (123–131), but the context showed those lines in the next span, `ref-20` (133–139).
  Since then a reference resolves through any shown span of the same file and side that contains its lines.
  Lines no span shows are still rejected. With that change, three more runs gave `COMPLETE_FOR_SCOPE` and still
  no candidate: the missing discovery is the model's miss, and no deterministic change addresses it.
- Sign-in (`vkusori-signin-consent-resume-unsafe`): found once. In the other two runs the labelled chain, where the
  pending effect resumes in the same mount, ended as an UNRESOLVED candidate. The open question was whether
  `onConsentRequired` unmounts the buttons. The verifier rejected a neighbouring claim (a `resumeStarted` ref
  that is never reset). No change was made for this case.

Both cases moved to `eval/quality/dev`, since the pipeline was changed from them.

One run of the development suite with the change (130 cases, the four moved ones included; 394 attempts,
$0.388; one error, `real-sych-context-broker`, whose base needs Sych's archival clone)
([report](examples/quality/dev-cheap_api-c-anchor.json)), beside the archived M3 run of the smaller suite:

| | defects found | correctness findings at labelled locations | safe runs with a false finding | UNKNOWN kept |
|---|---|---|---|---|
| with the change | 47/51 | 57/84 | 15/69 | 5/9 |
| M3 ([report](examples/quality/dev-cheap_api-c-m3.json)) | 42/48 | 58/88 | 16/66 | 4/9 |

Neither run was repeated and the suites differ, so this shows no regression, not an improvement. Both moved
defects were missed again in this run.

## Holdout, second run

Nine cases in `eval/quality/holdout`: the five left after the move and two new owner-reviewed pairs (four cases)
(`vkusori-screen-lead-under-sheet`, `vkusori-guest-profile-identities`). One run of variant C, `cheap_api`, with
the anchor change: 28 model attempts, $0.069, no errors ([report](examples/quality/holdout-cheap_api-c-2.json)).

| defects found | correctness findings at labelled locations | safe cases with a false finding | UNKNOWN kept | nits matched |
|---|---|---|---|---|
| 1/3 | 1/3 | 1/5 | 0/1 | 0/1 |

The one defect found is the swipe exit timer, whose label came from the first run's answer. Of the defects
labelled without the model, none of the two new ones was found, so this independent count is again 0/2.
`vkusori-screen-lead-under-sheet-unsafe` got no finding. `vkusori-guest-profile-identities-unsafe` got one
unlabelled correctness finding (a failed `File.arrayBuffer()` leaves the photo button loading) that nobody has
judged yet. The false finding on a safe case is on `vkusori-swipe-dismiss-ref-safe` (the effect split). Still
too few cases and runs for a quality acceptance.

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
- Context window. `context.max_tokens_per_request` is 48 000 and `initial_tokens_per_unit` 16 000 (bytes of shown
  code), the run's `budgets.analysis.max_input_tokens` 512 000. With 14 000 / 6 000 / 128 000 the reviewer of a
  mid-size PR saw a slice of the change per call: verifiers left real candidates `UNRESOLVED` ("no span shows …"),
  a required assessment was skipped over the context budget, and each new revision surfaced the next one or two
  problems instead of all of them. The money cap stays the guard.
- Context compression. Packets whose shown bytes, sent about twice (search, then verify), exceed
  `budgets.analysis.max_input_tokens` are rebuilt with request budgets scaled down in proportion, to a quarter
  at most and never below the largest required span, so each one still fits a chunk of its own; what no longer fits is cut by priority with the usual `BUDGET`/`REQUIRED_OVER_BUDGET` gaps. A supporting
  construct over budget keeps its first line (`transformation: signature`, excluded as `SIGNATURE_ONLY`) instead of
  disappearing.
- The model is nondeterministic: the same run can yield `UNKNOWN` if it did not attach an anchor to a criterion; this is
  expected strictness, not a bug.

## Guided tour (M5.1)

`sych_core::tour` holds the deterministic part of spec §21 over one saved `ReviewResult`:

- **Plan** (`tour::plan`, `tour-planner@1`): material findings (defects, rule violations) by severity, one stop per
  file, then required `UNKNOWN` obligations that have a location; 3–6 stops from `tour.target_steps`. The overview —
  coverage, every HIGH/CRITICAL finding and every required `UNKNOWN` — is never cut by the route; improvements and
  nits are listed apart.
- **Artifact** (`TourArtifact`, [schema](../schemas/tour-artifact.schema.json)): one result revision, planner/prompt/
  renderer versions and generation provenance, steps with finding/obligation/evidence ids, code refs, free-text
  explanation, an optional `COMPREHENSION | FACT_REQUEST | RISK_DECISION` question and `Example`-labelled snippets.
- **Validation** (`tour::validate`): the tour belongs to the result and revision; the overview equals the typed
  result's (a hidden UNKNOWN is rejected); every id exists; every code ref resolves in the pinned snapshot; every
  illustrative snippet is labelled `Example`. A valid structure is not a true explanation.
- **Rendering** (`tour::render`): the lines shown come from the Git blob, never from the model's text.
- **States** (`tour::next`): `NOT_REQUESTED → GENERATING → VALIDATING → READY | FAILED`, two attempts at most (one
  repair). A tour of an older revision is `STALE` (`tour::stale`) and is never rewritten; a new request makes a new
  `tour_id`.

**Generation** — `sych tour --run <id> --config sych.yaml --repo . [--models PROFILE]`: the saved run's snapshot is
captured again from `--repo` and must have the same id; the plan picks the stops; the guide (`tour@1`, role reviewer,
budget `budgets.tour`, `Purpose::Tour` in the run's ledger) writes only intent, titles, purposes, explanations, an
optional question and `Example` snippets per stop — ids, code references and the overview come from the plan and the
typed result, so the model cannot invent a location or change a status. A schema error gets the gateway's one repair;
an invalid tour (a stop it invented or skipped, a reference that does not resolve) gets one regeneration told what
was wrong, then `FAILED`. A valid tour is stored as artifact `tour:<tour_id>` of the run (`ArtifactKind::Tour`) and
printed with code read from the blobs; the output reports attempts and their cost.

The stored tour is read by `sych tour show|reveal|complete|dismiss` (M5.2, [LOCAL_REVIEW.md](LOCAL_REVIEW.md#reading-a-tour)):
no model, a personal session, `STALE` on a moved head, "source unavailable" once the source is pruned. The server
serves the same tour and a session per API client at `/v1/runs/:id/tour` ([SERVER.md](SERVER.md)), with `code_refs`
instead of source text.

**Follow-up (M5.3)** — `sych ask` ([LOCAL_REVIEW.md](LOCAL_REVIEW.md#asking-about-a-stop)): `followup@1`, role reviewer,
`Purpose::Followup` with the ledger scope `followup:<tour>:<actor>`, so `budgets.followup_session` caps one session
(`model_attempts.scope`) while the tenant day and month pools stay shared. The prompt holds the stop's spans, the
review's findings and obligations of the stop and the overview; context requests are answered once through the broker's
typed operations (`ContextAsk::answer`, shared with analysis). Hypotheses pass the discovery checks and then the regular
verifier (`verify@5`, role verifier) under the same session budget; they stay in the exchange (`tour_exchanges`, one
actor's rows) and never reach the `ReviewResult`, the `TourArtifact` or a publication.
