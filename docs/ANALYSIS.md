# Model review

The model proposes hypotheses, assessments and verdicts; the final statuses and findings come from the engine and the
deterministic reducer.

```sh
sych db migrate --config sych.yaml
sych review --repo . --base origin/main --config sych.yaml --models cheap_api --format markdown
```

`--models fake` is the offline fake adapter. `--models roles` routes each role through `models.roles` and its
fallback chain; any other value is a pinned profile from `models.profiles` (with its own chain).
Profiles are resolved the same way as `doctor --probe-model` ([MODELS.md](MODELS.md)): mode, egress allowlist, price
card, credentials and pinned runtime. Transports: OpenRouter, OpenCode Go, GLM Coding Plan via Pi, ChatGPT plan
via Sign in with ChatGPT; the router and pass roles are in [MODELS.md](MODELS.md#router-roles-fallback-and-premium).
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

The discovery rules for minor remarks: a nit is no higher than `LOW`, does not change behavior, and names the location, the error it prevents and the fix; the strongest ones are about consistency
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
Generated files the change also touches are listed in their own block, by path, status and line counts, at most 50,
and never with their content. A model therefore knows a regenerated file did change, and does not report it as stale.
A context request never reads them either: `read_span` and `read_symbol` answer `UNSUPPORTED`, and a search leaves
out their hits and says how many it dropped.
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
  find_implementations | search_source` without hashes or commands; it is translated into a typed broker operation, answered by `Ops` against the pinned
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
  the reducer checks evidence kinds, validity and completeness once more. A model refusal, budget or schema after
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
  A `VIOLATED` obligation without a linked finding yields a rule violation finding from the reducer.
- **Basis and baseline.** Basis kinds: `STATIC_TRACE` (a chain of anchors from the change to the consequence),
  `SEMANTIC_ARGUMENT` (reasoning over the shown spans), `EXPLICIT_RULE` (accepted rule), `PREFERENCE` (only
  nit/improvement); `EXECUTED_COUNTEREXAMPLE` is never accepted from the model, which executes nothing. Novelty and
  `RESOLVED` are decided by the shared baseline/revalidation by fingerprint; there is no second diagnostic lifecycle here: without a baseline, a match with an
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

## Measured quality

`sych eval measure` ([LOCAL_REVIEW.md](LOCAL_REVIEW.md#measuring-with-a-real-model)) scores reviews against
labelled cases. The latest run of the development suite, on 2026-10-30: 130 cases (five of them from a private
repository, not in the published suite; `real-sych-context-broker` could not run), variant C, one run each,
`cheap_api` (DeepSeek V4.1 Flash via OpenRouter) with the personal example's budgets; 394 model attempts, $0.388.

| defects found | correctness findings at labelled locations | safe runs with a false finding | `UNKNOWN` kept |
|---|---|---|---|
| 47/51 | 57/84 | 15/69 | 5/9 |

This is one run on the suite Sych is tuned against, scored by location: a measurement, not a quality guarantee.
In runs analysed case by case, most misses were candidates that reached the labelled lines and that the verifier
rejected or left `UNRESOLVED` (`verifier_dropped`), and `unknown` cases were often closed with a confident answer.

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
- Reasoning models spend ~1,000–2,400 tokens of each reply on reasoning: with the default of 2,000 tokens per
  request, discovery and verify get truncated (`TRUNCATED`). The personal example raises
  `budgets.analysis.max_output_tokens_per_request` to 4,000.
- Context window. `context.max_tokens_per_request` is 48 000 and `initial_tokens_per_unit` 16 000 (bytes of shown
  code), the run's `budgets.analysis.max_input_tokens` 512 000. Much smaller windows leave the reviewer of a mid-size PR a
  slice of the change per call: verifiers leave real candidates `UNRESOLVED` ("no span shows …"), a required
  assessment is skipped over the context budget, and each new revision surfaces the next one or two problems instead
  of all of them. The money cap stays the guard.
- Context compression. Packets whose shown bytes, sent about twice (search, then verify), exceed
  `budgets.analysis.max_input_tokens` are rebuilt with request budgets scaled down in proportion, to a quarter
  at most and never below the largest required span, so each one still fits a chunk of its own; what no longer fits is cut by priority with the usual `BUDGET`/`REQUIRED_OVER_BUDGET` gaps. A supporting
  construct over budget keeps its first line (`transformation: signature`, excluded as `SIGNATURE_ONLY`) instead of
  disappearing.
- For Rust, support is limited to what rust-analyzer and syn see on pinned commits, without executing build scripts
  and proc-macros: unexpanded macros, cfg outside the active features and dynamic dispatch yield gaps, not a proven
  set. Changed `select!`, serde, cfg and unexpanded macros are flagged on their span; code under an inactive cfg, an
  unexpanded macro and the `Debug` of a type outside the snapshot are gaps under which an obligation is never
  `SUPPORTED`, and a `VIOLATED` resting only on such code becomes `UNKNOWN`.
- The model runs nothing. Runtime evidence comes only from imported CI reports and the isolated execution profiles
  ([CONFIG.md](CONFIG.md#execution-profiles)); a refused or failing profile is a gap (`EXECUTION_REFUSED` /
  `EXECUTION_FAILED`), never a pass.
- The model is nondeterministic: the same run can yield `UNKNOWN` if it did not attach an anchor to a criterion; this is
  expected strictness, not a bug.

## Guided tour

`sych_core::tour` holds the deterministic part of a guided tour over one saved `ReviewResult`:

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

The stored tour is read by `sych tour show|reveal|complete|dismiss` ([LOCAL_REVIEW.md](LOCAL_REVIEW.md#reading-a-tour)):
no model, a personal session, `STALE` on a moved head, "source unavailable" once the source is pruned. The server
serves the same tour and a session per API client at `/v1/runs/:id/tour` ([SERVER.md](SERVER.md)), with `code_refs`
instead of source text.

**Follow-up** — `sych ask` ([LOCAL_REVIEW.md](LOCAL_REVIEW.md#asking-about-a-stop)): `followup@1`, role reviewer,
`Purpose::Followup` with the ledger scope `followup:<tour>:<actor>`, so `budgets.followup_session` caps one session
(`model_attempts.scope`) while the tenant day and month pools stay shared. The prompt holds the stop's spans, the
review's findings and obligations of the stop and the overview; context requests are answered once through the broker's
typed operations (`ContextAsk::answer`, shared with analysis). Hypotheses pass the discovery checks and then the regular
verifier (`verify@5`, role verifier) under the same session budget; they stay in the exchange (`tour_exchanges`, one
actor's rows) and never reach the `ReviewResult`, the `TourArtifact` or a publication.
