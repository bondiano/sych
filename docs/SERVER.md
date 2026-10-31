# Server: GitHub App, admission, scheduling and publication

`sych serve --config sych.yaml` runs webhook intake, the scheduler, the worker and the publisher in a single process.
A webhook only accepts a durable signal; the model is not called in the handler. This repository reviews its own pull
requests with a pinned build ([below](#self-review-bondianosych)).

## Connecting a GitHub App

Trusted server config defines each connection; the webhook URL selects only the `:connection`, not the API address,
credentials or tenant:

```yaml
scm:
  github:
    - id: sandbox                 # POST /hooks/github/sandbox
      tenant_id: local            # default
      provider_instance: github.com
      api_base_url: https://api.github.com
      app_id: 123456
      private_key_file_env: SYCH_GITHUB_APP_KEY_FILE   # path to the PEM, not the key itself
      webhook_secret_env: SYCH_GITHUB_WEBHOOK_SECRET   # differs from the API credential
      repositories: [your-org/your-repo]               # others get a receipt without work
      bot_login: your-app[bot]                         # its comments never schedule work
      checks: false                                    # Checks write only with checks enabled
      publish: false                                   # advisory publication; off by default
      max_body_bytes: 2097152                          # default 2 MiB
```

The validator rejects a duplicate `id`, a non-https `api_base_url` (http only for loopback), the same env for the webhook
secret and the key, an empty repositories list and an empty `bot_login`. Secrets are read at startup; a missing secret
stops startup and is not substituted.

### Permissions and capabilities

| Capability | App permission | Endpoint |
|---|---|---|
| change_refs, reconciliation, own_comments | Pull requests: read | `GET /repos/{r}/pulls/{n}`, `GET /repos/{r}/pulls?state=open`, `GET /repos/{r}/issues/{n}/comments` |
| full_diff | Pull requests + Contents: read | `GET /repos/{r}/pulls/{n}/files` (no more than 3000 files) |
| blobs | Contents: read | git fetch with the installation token |
| ci_runs, ci_artifacts | Actions: read | workflow runs/jobs/artifacts |
| summary_write, inline_write | Pull requests: write | issue comment, review comments |
| actor_permissions | Metadata: read | `GET /repos/{r}/collaborators/{login}/permission` |
| checks_write | Checks: write — only with `checks: true` | check runs |
| events `issue_comment`, `check_suite` | Issues: read, Checks: read | without them GitHub does not allow subscribing to these events |

`ProviderCapabilities` is computed from the installation `permissions` (`GET /repos/{r}/installation`) and
stored in the binding; a missing capability yields a limitation, not a guess. The installation token is requested
for a single repository (`POST /app/installations/{id}/access_tokens` with `repositories: [name]`), lives in memory and
is refreshed 5 minutes before expiry — independently of the run. The App JWT (RS256) is signed with the key from the PEM (PKCS#1, as
GitHub issues it, or PKCS#8) and is valid for 9 minutes.

### Admission

`POST /hooks/github/:connection`, in order, before JSON parsing:

| Check | Response |
|---|---|
| no such connection | 404 |
| body larger than `max_body_bytes` | 413 |
| `Content-Type` is not `application/json` | 415 |
| `X-Hub-Signature-256` does not match the HMAC-SHA256 of the raw body (constant-time, `ring`) | 401 |
| no `X-GitHub-Event`/`X-GitHub-Delivery`, event not in the list, invalid JSON | 400 |
| storage error | 503 — no ACK; GitHub marks the delivery failed, reconciliation catches up |
| new delivery | 202 `{inbox_id, duplicate: false, intents}` |
| repeat of `provider_instance + connection + X-GitHub-Delivery` | 200 `{inbox_id, duplicate: true}`, nothing changes |

Accepted events: `ping`, `pull_request`, `issue_comment`, `workflow_run`, `check_suite`, `installation`,
`installation_repositories`. GitHub does not sign a timestamp, so there is no replay window: a repeat is recognized by the delivery id.
Errors use the API error envelope (`error.code/message/retryable/correlation_id/details`), without secrets or payload.

A single storage transaction stores the receipt (`inbox_events`: delivery key, payload digest, normalized event,
significance decision), the binding (`repositories`: tenant + connection + instance + external id), the change and the
intent. The raw payload is not stored: PR text is untrusted and is re-read from the API. Normalization:

| Event | Signal | Intent |
|---|---|---|
| `pull_request` opened/reopened/ready_for_review | `OPENED` (draft → `DRAFT`) | `REFRESH`, debounce 5 s |
| `pull_request` synchronize | `SOURCE_PUSH` | `REFRESH`, 5 s |
| `pull_request` edited with `changes.base` | `TARGET_CHANGED` | `REFRESH`, 5 s |
| `pull_request` converted_to_draft | `DRAFT` | `REFRESH`, 5 s |
| `pull_request` closed | `CLOSED {merged}` | `CLOSE`, immediately |
| `workflow_run`/`check_suite` completed | `CI_COMPLETED` | `CI`, 10 s |
| `issue_comment` `/sych rerun|deep|stop` | `COMMAND` | `COMMAND_*`, immediately; the scheduler checks permissions |
| comment from `bot_login` | `BOT_COMMENT` | none |

Debounce: one open intent per change and kind; each new event moves the deadline, but no further than
`first event + 6 × debounce` (30 s for PRs, 60 s for CI) — a continuous stream does not postpone work indefinitely.
A repository outside `repositories` gets a `REPOSITORY_NOT_ALLOWED` receipt without
work. A fork (`head.repo` ≠ base, or deleted) is flagged and analyzed read-only.

Isolation: access scope = `provider_instance/connection/external_id`, so the same repository in two tenants or
connections has separate changes, generations, artifacts and publication state; an artifact with the same hash in a foreign
scope is not read.

### Adapter errors

| HTTP | Read | Write | Inline comment |
|---|---|---|---|
| 401 | `UNAUTHORIZED` | `UNAUTHORIZED` | `UNAUTHORIZED` |
| 403 | `UNAUTHORIZED` (or `RATE_LIMITED` with `x-ratelimit-remaining: 0`/`Retry-After`) | same | same |
| 404, 410 | `NOT_FOUND` | `NOT_FOUND` | `NOT_FOUND` |
| 429 | `RATE_LIMITED` + `retry_after_seconds` | same | same |
| 422, 400, 409, 415 | `UNSUPPORTED_CAPABILITY`, not retryable | same | `INVALID_ANCHOR` |
| 5xx, connection drop | `TRANSIENT` | `PUBLICATION_UNKNOWN` (outcome unknown) | `PUBLICATION_UNKNOWN` |
| > 3000 files | `INCOMPLETE_DIFF` | | |

Only `RATE_LIMITED` and `TRANSIENT` are retryable; `PUBLICATION_UNKNOWN` goes to the reconciler, not to a retry.
The client does not follow redirects and does not use environment proxies: credentials go only to `api_base_url`.

## Scheduler, jobs and freeze

`sych serve --config sych.yaml [--models roles|PROFILE|fake] [--semantic on|off]` starts in a single process
admission, the scheduler (once per second), reconcile (at startup and every 10 minutes), the reaper (every 15 s) and the worker.

**Intent decision** is the pure function `sych_core::schedule::decide` over the change state from the API (the webhook
is only a signal) and the latest generation:

| Intent / state | Decision |
|---|---|
| change closed or merged (per the API) | `CANCEL`: active runs and their jobs are cancelled, the committed result remains |
| `REFRESH`, draft | `SKIP:DRAFT` (`draft_policy: manual_only`) |
| `REFRESH`, same `base_sha..head_sha` | `SKIP:SAME_INPUTS` — generation does not grow, no inference |
| `REFRESH`, new head or moved target | new generation; the active old one is `SUPERSEDED`, its lease revoked |
| `COMMAND_RERUN/DEEP` from an actor with `write/maintain/admin` | new generation with the same inputs, trigger `MANUAL` (`deep` is recorded in the trigger; roles start with `models.deep_roles.<role>`, a role without it — with `models.roles`, budgets are the same; a pinned `--models PROFILE` takes precedence) |
| command without permissions | `SKIP:UNAUTHORIZED_COMMAND`; the bot does not respond |
| `COMMAND_STOP` | `CANCEL` of the active run |
| `CI` for an old head | `SKIP:CI_FOR_OLD_HEAD` |
| `CI`, run waiting for evidence | `WAKE` |
| `CI` after freeze, when an expected source did not arrive before the deadline | successor generation with `parent_run_id` |
| `CI` after freeze with nothing missing (a repeat of the same artifact or a report the policy does not expect) | `SKIP:CI_NOT_SIGNIFICANT`, generation does not grow (`tests/scheduler.rs`) |

Every decision is written to `change_events` (`GENERATED`, `SUPERSEDED`, `CANCELLED`, `SKIPPED:<reason>`, `WOKEN`,
`SCM_READ_FAILED`). API read error: `RATE_LIMITED`/`TRANSIENT` postpone the intent (backoff, `Retry-After`),
others are recorded and consumed, without retry.

**Jobs and fence.** The generation, the supersede of the old one and the job (`ANALYSIS`) are committed in one transaction. Claim is one
transaction: `QUEUED` and due → `CLAIMED`, `lease_epoch + 1`, lease 60 s. The worker renews the lease every 15 s;
a lost lease (cancel, supersede, reaper) stops analysis. The reaper returns expired leases to the queue with a
new epoch; after `max_attempts` (1 + 2 retries) or the job deadline, the job and run are `FAILED`. Every authoritative run write —
artifact pin, prepare, freeze, result — carries the fence: `job_id + owner + lease_epoch`, a live lease, an active state and
`run.generation == change.desired_generation`; a late worker does not pin artifacts and does not commit a result.

**Worker.** Fetches `head_sha` and `base_sha` into a bare mirror (`<data>/mirrors/<connection>/<owner>__<repo>.git`) with the
installation token in `GIT_CONFIG_*` env (not argv, not the URL), without tags/hooks; nothing is checked out or executed —
a fork is read the same way. Then it pins the snapshot and the trusted target tip policy: `ci.expected_reports` whose `applies_to`
matched the changed paths put the run into `WAITING_EVIDENCE` with deadline `ci.wait_seconds` (default 180 s); the job
is released until the deadline and woken by a CI event. Freeze records `expected`, the CI runs that arrived and `missing`;
after that, review runs in a separate thread (without an SQL transaction across git/model). The lease is renewed every
`workers.heartbeat_seconds` for the whole claim, from token and fetch through CI collection to the end of the analysis; a
lost lease abandons the job to its new holder. A fetch/token error is retried per the classifier (`RATE_LIMITED`,
`TRANSIENT`; up to two retries, backoff + jitter, `Retry-After`, not past the job deadline), otherwise the run is
`FAILED`. The job deadline starts at 180 + 300 + 120 s and PREPARE moves it to the evidence deadline + 300 + 120 s, so
a longer `ci.wait_seconds` still leaves room for retries.

**Reconciliation.** At startup and periodically, for each repository in `repositories`: installation and
capabilities (recorded in the binding), the list of open PRs → synthetic receipt `reconcile:<n>:<head>:<base>` (a repeat is a
no-op); a change considered open but not found → `reconcile-closed` → `CLOSE` after an API check. GitHub does not
retry failed deliveries itself; reconciliation closes this gap.

**Checkpoints and successor.** Each accepted model response is immediately pinned to the run as artifact
`checkpoint/<key>` under the fence, so the parent's work survives its supersede. The task's dependency record: template,
schema name, role, chain, sha256 of the sent messages (after redaction: all spans, evidence, gaps and answers to
context asks, i.e. including the search scope/completeness for a negative conclusion), sha256 of the output schema and the digest of
`models`; `key` is the digest of this record. A successor (`Trigger::Successor`) reads the parent's checkpoints only in its own
partition and takes a response without a model call only on an exact record match, then re-validates it against its own
context and facts. New CI, policy, source, model config or deep-route change the prompt or the record → a fresh call.
The pass and hop are marked `reused_from`/`REUSED`, with no attempts and no spend; the response is written again to the successor's checkpoints.
A checkpoint write failure does not fail the task (the response is kept); it is written to the pass as `checkpoint_error` and to the log; a
store error is retried once. A fence rejection (run superseded/cancelled/lease lost) is final: from that
point `RunLedger` refuses reserve, so the run makes no more paid calls whose result it could no longer commit.
The successor's result references the parent's result (`parent_result_id`) if the parent completed. The successor re-derives
derived evidence (syntax/semantic facts, `DERIVED_LOCAL`) from its own snapshot; it does not enter the external input manifest,
and the parent's frozen `evidence_manifest` does not change (`tests/ci_evidence.rs`). A manual rerun/deep
does not take checkpoints — it is a deliberate repeat inference. Limitation: analysis of a single run happens in a single worker.

## CI evidence

Repository policy (the `.sych/config.yaml` of the target tip) lists the expected reports (`ci.expected_reports`: id,
`applies_to`, format). Where each one comes from is decided only by the connection's trusted server config — not by the job name:

```yaml
scm:
  github:
    - id: sandbox
      # ...
      ci_reports:
        - report: tests                       # expected_reports[].id
          workflow: .github/workflows/ci.yml  # exact workflow file
          event: pull_request                 # default
          job: test                           # exact job name
          artifact: reports                   # exact artifact name
          path: junit.xml                     # entry in the zip
          checkout_manifest: sych-ci.json     # CiManifest with tested_oid; without it the checkout is not proven
      artifact_hosts: ["*.blob.core.windows.net", "*.actions.githubusercontent.com"]  # default
```

The worker looks up the run by the exact `head_sha` (`GET /actions/runs?head_sha=`), workflow and event; the job — among the jobs of that
`run_attempt`; the artifact — among the artifacts of that run. Not "the latest green pipeline of the branch". Download:
`GET /actions/artifacts/{id}/zip` responds with a redirect to signed storage — the request goes there **without** a token, only
over https to a host from `artifact_hosts`; no more than 64 MiB.

The zip is read in memory and nothing is written to disk: any name with `..`, an absolute path, `\`, `:` or a symlink
rejects the whole archive; no more than 1000 entries; decompression of a single entry stops at 32 MiB (zip bomb);
CRC is verified; zip64 and encryption are not supported. Parsers: `cargo --message-format=json`, JUnit, SARIF 2.1, GitLab Code Quality, LCOV,
Cobertura and the project JSON manifest (`{"schema_version": 1, "commands": [{"name", "exit_code"}], "tests": [...]}`);
no more than 2000 records, XML — depth 64, external entities are not resolved (quick-xml does not read DTDs); paths outside the
repository are dropped.

**Binding.** The checkout manifest's `tested_oid`: equal to head — `SOURCE_HEAD`; a merge commit whose second parent is
head (it is fetched into the mirror and its parents are read) — `MERGE_RESULT` with the target commit; otherwise, or without a manifest —
`UNVERIFIED` (evidence `STALE`, gap `CI_BINDING_UNVERIFIED`). `CiOrigin` stores the provider instance/repository,
workflow, run/attempt/job, event, the job's status and conclusion, `allow_failure` (GitHub does not report it — `null`, not
`false`), execution/source-head/target/tested SHAs, the digests of the checkout manifest and the zip, artifact id/name, report path and
time. Evidence is `AUTHENTICATED_CI`: this is provenance, not oracle independence (a limitation in the evidence);
coverage is execution, not correctness of assertions; a test changed in the reviewed diff is flagged `TEST_CHANGED`.
The job conclusion and the test results are counted separately: a failed job neither hides nor replaces JUnit.

**Gaps** (each is a task gap: coverage `PARTIAL`, text in `unresolved_questions`): `CI_NOT_MAPPED`,
`CI_JOB_MISSING`, `CI_JOB_INCOMPLETE`, `CI_JOB_SKIPPED`, `CI_JOB_CANCELLED`,
`CI_ARTIFACT_MISSING`, `CI_ARTIFACT_EXPIRED`, `CI_ARCHIVE_REJECTED`, `CI_REPORT_MISSING`, `CI_REPORT_MALFORMED`,
`CI_BINDING_UNVERIFIED`, `CI_UNAVAILABLE`. Before the evidence deadline, `CI_JOB_MISSING`/`CI_JOB_INCOMPLETE` keep
waiting; after it — freeze with gaps, and a late report yields a successor. Freeze records the expected, the imported
(run/attempt/job/artifact/binding) and the gaps.

Limitations: no comparison with baseline diagnostics (a diagnostic disappearing between producer/version) is made —
such conclusions are not claimed. Go reports (`go_test_json`, `go_cover`, `go_vet_json`), `terraform_plan` and
`kubernetes_rendered` go through the same downloader/parser/binding as the
other formats.

**Terraform plan** (`terraform_plan`, `terraform show -json` of a saved plan, `format_version` 1.x; another major is
`CI_REPORT_MALFORMED`). It is reduced on import to allowlisted structure — address, actions, replacement order
(`delete_before_create` / `create_before_delete`), `replace_paths`, the *names* of changed, unknown-after-apply and
sensitive attributes, `previous_address`, root-module `depends_on`, `terraform_version`, the plan `timestamp`, provider
version constraints and a digest of the variables. Provider lock hashes are not part of plan JSON: a limitation says so. No attribute or variable
value is kept, so none reaches a prompt, log or artifact; a resource change without address or actions is a
limitation, not an empty plan. `TF-CHANGE` uses only a plan bound to the head: each deleted or replaced resource
becomes a criterion; without one the obligation stays `UNKNOWN`/`TERRAFORM_PLAN_MISSING`. Workspace and environment are
not part of the plan JSON: they are proven by the job binding (`ci_reports` selector + checkout manifest).

**Rendered Kubernetes manifests** (`kubernetes_rendered`: `helm template` / `kustomize build` output, multi-document
YAML, `*List` kinds unpacked). For `Deployment`, `StatefulSet`, `DaemonSet`, `Job`, `CronJob` and `Pod` it keeps:
selector vs pod-template labels, containers without a readiness probe, replicas, strategy, termination grace and
`preStop`, persistent claims, the *names* of referenced Secrets/ConfigMaps, hook and sync-wave annotations
(`helm.sh/hook`, `argocd.argoproj.io/hook`, `argocd.argoproj.io/sync-wave`) and a keep/no-prune policy. Env values and
ConfigMap/Secret data are never read. Passive built-ins (Service, Ingress, RBAC, …) are skipped; any other kind (CRDs)
is listed as not normalized. The report's evidence stage is read from the documents — `rendered` (plain YAML),
`dry_run` (`uid`/`creationTimestamp` without `status`, as a server dry-run returns them), `runtime` (objects with
`status`) — and the weakest document wins. `K8S-ROLLOUT` uses manifests bound to the head: each risky object becomes a
criterion (a persistent claim notes that a Pod-template rollback does not restore its data), with a limitation for the
stage — rendered is structure only, a dry-run is admission, not a successful rollout, runtime objects are a snapshot of
the live state — and one that controller-specific ordering and inventory are not known. Without them: `UNKNOWN`/`K8S_MANIFESTS_MISSING`. The reviewer never deploys or
asks for cluster credentials; StatefulSet- and controller-specific profiles are not claimed.

## Outbox and publication reconciliation

Finalization with `publish: true` puts one operation per summary revision into the outbox (`publication_objects` — the
`summary` slot on a change with marker `sych:summary:<change>`; `publication_operations` — a revision with a generation). A newer
revision of the same object that has not been sent yet makes the older one `SUPERSEDED`.

```
PENDING ──claim──▶ (gates) ──▶ PUBLISHING ──reply──▶ DELIVERED
   ▲                   │            │ lost reply / 5xx / crash / lease loss
   │ definite refusal  │ gate       ▼
   └── (429, retry) ◀──┘ closed ─▶ UNCERTAIN ──found by id/marker──▶ DELIVERED
                         superseded  │ complete read empty after 5 min (audit) ─▶ PENDING
                         ─▶ CANCELLED/SUPERSEDED   │ window exhausted without one
                                                   ▼
                                     operator `--allow-create` (audit) ─▶ PENDING
```

- **Before the network**: the `PUBLISHING` operation with the body, request digest, attempt id and fence (owner + `lease_epoch`) —
  in a single write. No more than one operation in flight per object.
- **Gates**: locally — the operation's generation equals the change's `desired_generation` and the change is open; via the API — the PR
  is open right now (a lost close blocks the write, intent `CANCELLED`), and the binding has `summary_write`.
- **Reply**: `DELIVERED` and the observed remote id/generation/body digest — one transaction. A known id —
  `PATCH`; the first time — `POST`. A 404 on `PATCH` (comment deleted) — the id is forgotten, and the next attempt creates it
  anew: the absence is known, not assumed. 429 — `PENDING` with `Retry-After`, no more than 5 attempts.
- **Unknown outcome** (connection drop, 5xx on write, process death, lease loss in `PUBLISHING` — the reaper moves it
  only to `UNCERTAIN`, never to `PENDING`): every 30 s the reconciler looks up the comment by the stored id, otherwise by
  marker across all pages, and only one written by the bot itself (`type: Bot`, `login == bot_login`); a marker copied by a
  human grants nothing. Found — `DELIVERED` (if the body differs, it is restored with `PATCH` by the known id). A
  comment with no stored id whose list was read to the end without a match, at least 5 minutes after the outcome became
  unknown (time for a write applied despite the 5xx to show up), is known absent: the reconciler returns it to `PENDING`
  with an `audit_events` row `PUBLICATION_RECREATE_AUTOMATIC`, within the same 5 attempts. A failed or truncated read,
  a check run (one page, no completeness) or a known id never permit CREATE. The reconciliation window is 1 hour; after
  that the operation stays `UNCERTAIN` with `RECONCILIATION_WINDOW_EXHAUSTED` and waits for the operator.
- **Relevance** is separate from delivery: a new generation marks the change's objects `STALE`; delivery of the current one —
  `CURRENT`. Closing the change cancels `PENDING`; `UNCERTAIN` stays for read-only reconciliation.
- **Late old UPDATE**: every 10 minutes delivered objects are checked against the last sent body;
  a divergence in the bot's comment is repaired and written to `change_events` as `PUBLICATION_REPAIRED`.

### Operator runbook

1. `sych reconcile --config sych.yaml [--change ID]` — list of `PUBLISHING`/`UNCERTAIN`: age, attempts,
   "reconciling for N s more" or "operator decision (window exhausted)", the last error. The SCM is not called.
2. While the window has not expired — do nothing: the server reconciles on its own, including a new CREATE once a complete
   read shows the comment absent.
3. After the window: open the PR and check whether there is a bot comment with marker `sych:summary:<change>`. If there is, but
   reconciliation did not find it (for example, the list was truncated) — leave it as is and investigate access.
4. If the comment is definitely absent: `sych reconcile --allow-create OP --actor NAME --reason TEXT`. The decision
   is written to `audit_events` (`PUBLICATION_RECREATE_AUTHORIZED`) with the risk of a duplicate; the operation returns to `PENDING`.
   This is forbidden for an operation with a known remote id: there reconciliation restores the body by id.

Outbox lag / uncertain age / repairs metrics and the alert — see "Metrics and alerts" below.

## Summary, inline and check run

Publication is advisory: Sych never approves, requests changes, merges or becomes a required check; the
adapter has no such calls. Everything is rendered from the run's immutable result and diff manifest (`sych_core::publication`),
not from model prose; only findings that passed engine verification enter the result.

**Summary** is one updatable comment per change: the reviewed head and target, result and revision; coverage
(`partial` states plainly that this is not "no problems", `unavailable` — that there was no review); recommendation and execution;
profiles; a table of problems (Defect/RuleViolation by severity — all HIGH/CRITICAL are always here) with a "where
shown" column (`inline` or `here (<reason>)`); specific `UNKNOWN`s (required ones are marked) and open questions, including
CI gaps; collapsed improvements/nitpicks with a rule or "preference"; cost (known + unknown attempts), the number of
attempts, the number of high/critical; a link to the full result if `http.public_base_url` is set. Golden files:
`fixtures/publication/summary-*.md`.

**Inline** — only Defect and RuleViolation; no more than 8 new ones per revision (already published ones are not counted), by
severity, then fingerprint; the rest go to the summary with a reason. Position — explicit `commit_id` (head), `path`, `line`,
`side` and `start_line`/`start_side` for a range; the deprecated `position` is not used:

| Anchor | Inline |
|---|---|
| head side, same blob, lines in a hunk ± 3 lines of context | `RIGHT`, head path |
| diff-base side of a modified/renamed file | `LEFT`, head path (as GitHub shows it) |
| deleted file | `LEFT`, old path |
| different blob, target tip, file outside the diff, line outside the shown diff | summary with a reason; a neighbouring line is not substituted |
| provider diff > 3000 files | all go to the summary ("provider diff is incomplete") |
| GitHub responded 422 | operation `FAILED (INVALID_ANCHOR)`, the summary is re-rendered by the known id with the reason "the provider rejected the inline position" |

Each inline is a separate outbox object (`inline:<fingerprint>`, marker `sych:inline:<fingerprint>`), with the same
`PUBLISHING`/`UNCERTAIN`/reconciliation (review comments, bot comments only).

**Check run** — off by default; `checks: true` (together with `publish: true`) requires Checks write. One check
`Sych (advisory)` per change, pinned to the head SHA, `external_id` = marker; the conclusion is always `neutral` —
both for a result (`Advisory review`) and for a run failure (`Sych could not complete this review`), so a failure
is distinguishable by the title rather than passed off as success. Reconciliation of an unknown outcome is by `external_id` and `app.id`.

## Lineage, feedback and waivers

The **fingerprint** of a model finding is the category, path, length and the hash of the content of the pinned lines, without the line
number or wording: shifting code above does not create a "new" problem. A rule violation — rule id, version and
obligation.

**Lineage** is applied after each committed result (in generation order, once per run): new findings are `OPEN`
with a `found` event; each previously open finding gets a decision from `sych_core::lineage::decide`:

| Situation | Outcome |
|---|---|
| the same fingerprint found again | stays `OPEN` |
| file deleted (not renamed) | `OBSOLETE` |
| file renamed/moved, exactly one finding of the same kind at the new location | the old one `OBSOLETE` "continued as …", the new one has `predecessor_id` |
| renamed, no candidates or several | stays `OPEN` — ambiguity closes nothing |
| new revision is `PARTIAL` (gaps, skipped job, parser error, limit) | `OPEN` |
| file not re-reviewed | `OPEN` |
| rule not re-assessed (analyzer/rule removed) | `OPEN` |
| rule re-assessed on the new snapshot and `SUPPORTED` everywhere | `RESOLVED` with obligation evidence |
| model finding: code changed, unit fully reviewed, no such finding there | `RESOLVED` |
| model finding: code unchanged | `OPEN` — "no diagnostic" by itself is not a fix |

History (`finding_events`) is append-only: state, reason, actor, evidence.

**Feedback** (`USEFUL | NOT_USEFUL | FALSE_POSITIVE | WONT_FIX`, scope, rationale) stores the actor and the policy snapshot;
`FALSE_POSITIVE`/`WONT_FIX` make the finding `DISMISSED` with the author and reason; a repeat finding in the next revision
stays `DISMISSED`. Evidence and obligation statuses do not change.

**Waivers** (rule id, paths, reason, owner, author, `expires_at` or an `until` condition) are recorded on the repository
binding; the worker passes the active ones into the review as rule exceptions: they lower the risk decision, while
`VIOLATED`/`UNKNOWN` stay as assessed.

| Action | Who | Check |
|---|---|---|
| `/sych rerun`, `/sych deep`, `/sych stop` in a PR | `write`, `maintain`, `admin` | permission API at scheduler time; otherwise `SKIPPED:UNAUTHORIZED_COMMAND`, no reply |
| the bot's own comment | nobody | `BOT_COMMENT`, no work |
| feedback on a finding | `triage` and above | permission API; otherwise a `Permission` refusal |
| waiver | `maintain`, `admin` | permission API; otherwise a `Permission` refusal |
| free PR text | — | does not change policy, budget, credentials or the model |

Tests — `cargo test -p sych-app --test server`
(signature, limit, content/event types, duplicates, health), `--test github` (JWT → scoped token, cache, reads, mapping) on a
loopback fake, `-p sych-storage --test admission` (atomicity, coalescing, two tenants), `--test scheduler`
(supersede/fence, reaper, cancel, waiting for evidence), `-p sych-app --test scheduler` (end-to-end traces on fake GitHub,
origin via `file://` and fake models), `-p sych-adapters --test git_fetch`, `--test ci_reports` (the `fixtures/ci` corpus: valid, corrupted, XXE,
depth), `--lib ci::archive` (traversal, symlink, bomb, CRC), `-p sych-app --test ci_evidence` (binding and gaps), `--test outbox` (a single CREATE on a lost reply,
crash in `PUBLISHING`, a first-publish 5xx re-created after a complete empty read, operator decision, pagination and a copied marker, a lost close, a late UPDATE),
`--test publication` (inline with exact fields, 422 → summary, neutral check), `-p sych-core --test publication`
(anchor corpus and golden summaries), `-p sych-core --test lineage` (lineage corpus), `-p sych-app --test findings`
(feedback/waiver with permissions, `DISMISSED` survives a rerun).

## HTTP API, CLI and operations

Example configuration — [examples/server/sych.yaml](examples/server/sych.yaml); OpenAPI — `schemas/openapi.json`,
generated from the same Rust DTOs (`sych_core::api`, `just schemas`).

**Process roles.** `sych serve` — everything in one (personal default); `sych api` — webhooks and `/v1` without claiming work;
`sych worker` — scheduler, worker, publisher and only health/metrics. The default bind is `127.0.0.1:8080`;
a non-loopback bind without `api_clients` and `behind_tls_proxy: true` is rejected by the validator, as is corporate without
`api_clients`. Use PostgreSQL for the server via `storage.database_url_env`; the local CLI still defaults to
SQLite. All writers serialize under a database lock. Separate machines also need the same artifact files;
PostgreSQL alone does not make local artifact directories shared.

**`/v1`** requires `Authorization: Bearer <token>` of a configured client (constant-time comparison) and access to the
repository (`connection/owner/name` or `*`); a foreign repository — 404. Mutating requests require an
`Idempotency-Key` (scope tenant + actor + endpoint, TTL 24 h): a repeat returns the stored response with
`reused: true`, the same key with a different body or target resource (path parameters) — 409 `IDEMPOTENCY_CONFLICT`, an unfinished first request — 409
`REQUEST_IN_PROGRESS`. Run/publication uniqueness does not depend on the key TTL.

Lists show only repositories in the caller's scope and never changes without a repository (local
runs). They are newest first by id: `limit` is 1–200 (50 by default) and a page's `next_before`, passed
as `before`, gives the next one. Times are Unix seconds, money micro-USD.

| Endpoint | Role | Response |
|---|---|---|
| `POST /v1/reviews` | writer | 202 `{request_id, run_id, status_url, reused}` — durable admission; `publish` defaults to `false` and is enabled only where the connection publishes; optional `checks: [{profile, params}]` run only those execution profiles ([CONFIG.md](CONFIG.md)) |
| `GET /v1/runs/:id` | reader | state, stage, coverage, usage, link to the result |
| `GET /v1/runs/:id/result` | reader | immutable `ReviewResult`; before commit — 409 `NO_RESULT`; once retention deleted it — 409 `RESULT_PRUNED` |
| `GET /v1/runs/:id/attempts` | reader | the run's model attempts as the ledger recorded them; an unknown charge is `null`, never 0 |
| `POST /v1/runs/:id/cancel` | writer | active → `CANCELLED`; a committed one stays in its terminal state |
| `POST /v1/changes/:id/reruns` | writer | 202, a new generation from the current PR state |
| `POST /v1/findings/:id/feedback` | triage | 201; `FALSE_POSITIVE`/`WONT_FIX` → `DISMISSED` |
| `POST /v1/waivers` | maintainer | 201; scoped, with an expiry or a condition |
| `GET /v1/waivers[?repository_id=&state=]` | reader | waivers, newest first; `state` is `active`, `expired` or `revoked` |
| `POST /v1/waivers/:id/revoke` | maintainer | 200 with the waiver; the next review no longer applies it; a revoked one keeps its time |
| `GET /v1/publications/:id` | reader | delivery, relevance, remote id, error |
| `GET /v1/repositories` | reader | repositories the caller may read |
| `GET /v1/changes[?repository_id=&state=&before=&limit=]` | reader | pull requests, newest first, with the latest run and open findings |
| `GET /v1/changes/:id` | reader | every generation's run, the event log and publication attempts; findings are `GET /v1/findings?change_id=` |
| `GET /v1/findings[?repository_id=&change_id=&state=&category=&severity=&rule_id=&before=&limit=]` | reader | findings, newest first, with the `id` feedback is posted to |
| `GET /v1/findings/:id` | reader | the lineage row, its history, feedback and the finding as the latest result states it (`null` once that result was pruned) |
| `GET /v1/summary[?repository_id=&since=]` | reader | since 30 days ago by default: runs by state and coverage, unknown obligations, finding transitions, known cost and unknown-cost attempts; the queue and `UNCERTAIN` publications are current |
| `GET /v1/usage[?repository_id=&from=&to=]` | reader | model usage per UTC day (`YYYY-MM-DD`, 30 days by default); the tenant's subscription fees only for a caller of every repository without `repository_id` |
| `POST /v1/runs/:id/handoff` | reader | the run's own findings (`{finding_ids, source_snapshot_id?}`) as a `handoff@1` package ([LOCAL_REVIEW.md](LOCAL_REVIEW.md#handing-off-a-fix)); no model, no change, nothing published; a foreign finding — 400 `FOREIGN_FINDING`, another snapshot — 409 `SNAPSHOT_MISMATCH` |
| `GET /v1/runs/:id/tour[?tour=]` | reader | the newest (or named) tour as the caller's own session sees it: unrevealed answers blanked, `stale` once the result revision or the change's generation moved on; source is not included, only `code_refs`; a pruned tour — 409 `TOUR_UNAVAILABLE` |
| `POST /v1/runs/:id/tours/:tour/reveal\|complete\|dismiss` | reader | the caller's session (the actor is the API client), never published and never an approval; `reveal` takes `{step_id}`; finishing a finished session — 409 `SESSION_FINISHED` |
| `POST /v1/runs/:id/tours/:tour/questions` | reader; `RISK_DECISION` maintainer | a follow-up on one step (`{step_id, kind, question, waiver?}`), stored as the caller's private exchange and listed in their `GET …/tour` (`exchanges`). `COMPREHENSION` asks the engine's models over the mirror's pinned blobs under `budgets.followup_session` per session (503 without an engine, 409 `SOURCE_UNAVAILABLE` / `NO_ANSWER`); `FACT_REQUEST` records the caller's statement; `RISK_DECISION` records a waiver like `POST /v1/waivers` (terms in `waiver`, 403 below maintainer) and never rewrites a status |
| `POST /hooks/github/:connection` | webhook signature | see [Admission](#admission) |
| `GET /health/live` | — | the process is alive; does not depend on the model |
| `GET /health/ready` | — | the DB responds and the artifact store is writable; without config or secrets |
| `GET /metrics` | address from `metrics_allow` | Prometheus text; otherwise 403 |

For Kubernetes scraping, `SYCH_METRICS_BIND=0.0.0.0:9464` enables a **separate private**
listener serving only `GET /metrics`; unset means no additional listener. An invalid or
occupied socket fails startup. This listener has no application API or peer allowlist:
restrict it to Prometheus with a NetworkPolicy and never attach a public ingress.
The main listener's `/metrics` still requires `metrics_allow`, so a proxy cannot bypass
that check using forwarded headers. It exports the same bounded storage/webhook metrics,
without tokens, paths or payload labels.

Errors — `{"error": {code, message, retryable, correlation_id, details}}`: 400 `INVALID_REQUEST`, 401
`UNAUTHENTICATED`, 403 `FORBIDDEN`, 404 `NOT_FOUND`, 409 conflicts, 413, 429 `RATE_LIMITED`, 503
`DEPENDENCY_UNAVAILABLE`; without tokens, prompts, sources or stack traces.

| CLI | What it does |
|---|---|
| `sych review --change github:CONN:OWNER/REPO:N --config sych.yaml` | the same server pipeline in-process from the current PR state; publishes nothing; exit codes as for `review` |
| `sych publish --run ID --as CLIENT --config …` | puts the summary of a committed run into the outbox (writer); the server sends it |
| `sych cancel --run ID --as CLIENT --config …` | cancellation before commit — exit 130 and `CANCELLED`; committed — exit 0 and the previous state |
| `sych rerun --change ID [--profile deep] --as CLIENT --config …` | a new generation (writer) |
| `sych reconcile [--change ID] [--allow-create OP --actor … --reason …]` | see [Operator runbook](#operator-runbook) |
| `sych serve` / `api` / `worker --config … [--models …] [--semantic …]` | process roles |
| `sych db migrate`, `sych cache prune --dry-run` | migrations only explicitly; a deletion plan without deleting |

Local `publish`/`cancel`/`rerun` act on behalf of a configured client with the required role; an unknown client
or an insufficient role — exit 4.

### Metrics and alerts

Metrics (only bounded labels — states, coverage, judgement, reasons; no paths or ids): runs by
state and coverage, `UNKNOWN` obligations, jobs by state and queue age, model attempts by ledger state,
tokens, known cost, held reservations/unknown charges, cost of superseded runs, publications by state,
outbox lag, `UNCERTAIN` age, repairs, feedback by judgement, bytes and number of artifacts, webhooks
(`stored`/`duplicate`/`rejected` by reason). Rules — [examples/server/alerts.yaml](examples/server/alerts.yaml):
storage failure, a stuck queue, unknown writes and charges, webhook signatures, credentials, a mass drop in
completeness, held budget. Tour and session metrics are not exported.

### Personal sandbox runbook

1. **App**: create a GitHub App with webhook URL `https://HOST/hooks/github/<connection>` and a webhook secret,
   permissions from the [permissions table](#permissions-and-capabilities) (Checks write — only with `checks: true`),
   events Pull request, Issue comment, Workflow run, Check suite; install it only on the repositories you list.
2. **Secrets** (outside the repository and the Nix store): `SYCH_GITHUB_APP_KEY_FILE` (path to the PEM), `SYCH_GITHUB_WEBHOOK_SECRET`,
   `SYCH_API_TOKEN_OWNER`, `SYCH_DATABASE_URL`, the model key (`SYCH_OPENROUTER_API_KEY`).
3. `sych config validate --config sych.yaml`, then `sych db migrate --config sych.yaml` — the schema changes only
   through this command; `serve` does not start on an unmigrated database (exit 3).
4. `sych serve --config sych.yaml`. Check: `curl 127.0.0.1:8080/health/ready`, `curl 127.0.0.1:8080/metrics`.
   In the App settings send a `ping` (Redeliver): expect 202 and an `IGNORED` receipt. Open a PR: receipt `SCHEDULED`,
   intent `REFRESH`.
5. **Provider smoke**: `sych doctor --config sych.yaml --scm-smoke <connection> --pr N` writes, reads back, updates,
   finds by marker and deletes one marked comment on PR `N` of the connection's first repository and prints a JSON
   report; exit 0 means the App can publish.
6. **Shutdown** — Ctrl-C/SIGINT: intake stops, then claiming; in-flight work gives up its lease on expiry,
   and the reaper on the next start returns the job; `PUBLISHING` without a reply becomes `UNCERTAIN`, not "delivered".
   **Startup** resumes the scheduler, reconcile of missed webhooks and the publication reconciler.
7. **Credentials**: an expired/revoked App key or token — `SCM_READ_FAILED` in `change_events` and `UNAUTHORIZED` in the
   log; replace the PEM/secret and restart. Model key — run `FAILED` with a reason; the budget is not spent again.
8. **Incomplete CI**: a run with `CI_*` gaps — coverage `PARTIAL`, the gap in the summary; a late report creates a successor on its own.
9. **Unknown publications**: `sych reconcile` (see the [operator runbook](#operator-runbook)); a new CREATE — automatically after a complete empty read, otherwise
   only `--allow-create`; both with an audit.
10. **Storage**: `sych cache prune --dry-run`, then without the flag; backup — see [STORAGE.md](STORAGE.md#backup).

Late CI, restarts and rerun/feedback with the usage ledger are covered by deterministic `-p sych-app` tests, not by a
recorded live end-to-end run.

## Self-review

A repository can be reviewed by a Sych built from that repository. It is the same engine and publisher — there is no
second bot. A connection with `environment: production` is accepted by the validator only with `self_review: true` and
`reviewer_commit`.

**Trusted reviewer.** Prompts, schemas and the engine are compiled into the binary; policy is taken from the target tip, and
`.sych/` from the PR is only compared (`policy_diff`). A connection with `self_review: true` requires `reviewer_commit` —
the full commit of a trusted build accepted by a human; `sych serve` refuses to start if it was built from a different
commit (`SYCH_BUILD_COMMIT` or `git rev-parse HEAD` at build time). Every result carries
`provenance.engine_version = <version>+<commit>`. A PR cannot substitute the reviewer, model, egress or budget:
all of these are trusted server config.

```yaml
scm:
  github:
    - id: self
      app_id: 123456
      private_key_file_env: SYCH_GITHUB_APP_KEY_FILE
      webhook_secret_env: SYCH_GITHUB_WEBHOOK_SECRET
      repositories: [your-org/your-repo]
      bot_login: your-app[bot]
      publish: true
      self_review: true
      reviewer_commit: "<40-hex commit of the trusted build>"
      ci_reports:
        - report: rust-tests
          workflow: .github/workflows/ci.yml
          job: test
          artifact: reports
          path: outcomes.json
          checkout_manifest: sych-ci.json
```

The repository policy (`.sych/config.yaml` on the target):
`ci: {expected_reports: [{id: rust-tests, applies_to: ["crates/**", "Cargo.lock"], format: sych_manifest}]}`.

**CI** — [examples/server/sandbox-ci.yml](examples/server/sandbox-ci.yml): `pull_request`, not
`pull_request_target`; no secrets or write tokens; Sych does not execute PR code (build scripts, proc-macros and
changed workflows run only in the repository's CI). The report is bound as `MERGE_RESULT` via the parents of the
merge commit; without a checkout manifest — `UNVERIFIED`.

**Models.** A server run runs in `personal_service` mode (corporate — `corporate`): a provider allowed only for
`personal_local` (a subscription signed into on a personal machine) does not enter a hosted route. The route is the
role plan ([MODELS.md](MODELS.md#router-roles-fallback-and-premium)); budgets — `budgets` in the server config; model
keys — env references (`SYCH_OPENROUTER_API_KEY`).

**Scenarios** (deterministic in the `-p sych-app` tests): force-push and late CI (supersede, successor), a duplicate
webhook, the bot's own comment, a lost publication reply and a restart — without extra inference or CREATE; a stale
summary — `STALE`; `RESOLVED` only after re-verification; the bot does not approve/request-changes/merge.

### Self-review bondiano/sych

Every pull request to this repository, including agent-authored ones, is reviewed by Sych itself, so reviewer defects
show up on its own changes. The bot is advisory: it does not approve, request changes or merge. Agents wait for its
summary on the current head (`scripts/wait-sych`, [AGENTS.md](../AGENTS.md)); `main` is protected — PRs with green CI
only.

- **Image.** [image.yml](../.github/workflows/image.yml) on push to `main` builds `ghcr.io/bondiano/sych:<commit>` with
  `SYCH_BUILD_COMMIT`; a PR neither builds nor publishes the image.
- **Pin.** The reviewer runs that image by digest from a separate deployment repository, with `reviewer_commit` set to
  the same commit; updating the reviewer is a deliberate release by a human, not a merge. The server config, App key,
  webhook secret and model keys live only there, so a PR cannot change the reviewer that reviews it.
- **CI evidence.** The `check` job in [ci.yml](../.github/workflows/ci.yml) puts into the `sych-reports` artifact the
  output of `cargo clippy --message-format=json` (`clippy.json`), the results of `just check` and `just test-postgres`
  (`outcomes.json`) and the checkout manifest (`sych-ci.json`); [.sych/config.yaml](../.sych/config.yaml) waits for them
  up to 15 minutes.
