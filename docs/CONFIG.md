# Configuration, policy and doctor

Implements roadmap M0.3 and M1.4: §11, §22–23 and §24.1 of the specification.

## Files

| File | Owner | Read from |
|---|---|---|
| `sych.yaml` | process owner / operator | local file, `--config` |
| `.sych/config.yaml` | repository owner | Git objects of the **target tip** (`--base`) |
| `.sych/rules.yaml`, `effects.yaml`, `deploy.yaml` ([DEPLOYMENT.md](DEPLOYMENT.md)) | repository owner | same; paths are set in `policy.*_file` |
| `.sych/contracts/**` | repository owner | same; hashed as is |

Working examples: [`examples/personal/`](../examples/personal/). Secrets are not stored in the configuration:
`*_env` holds only the environment variable name (`UPPER_SNAKE_CASE`); a secret value in that position
is rejected without printing the value itself.

## Parsing rules

- An unknown key is an error with the exact path: `sych.yaml: models.providers.openrouter.endpont: unknown field …`.
- Enums in YAML are lowercase `snake_case`; they are converted to domain JSON in a typed way
  (for example, `severity_on_violation: high` → `Severity::High`).
- `schema_version` is required; an unknown major version is rejected.
- Only `config.yaml`, the files from `policy.*_file` and `contracts/**` are allowed in `.sych/`; an extra file,
  a symlink, a submodule or a file larger than 1 MiB is an error. Symlinks are not resolved.
- A duplicate rule ID is a conflict between required rules and a configuration error; the model does not resolve it.
- `contract_refs` must point to files in `.sych/contracts/` of the accepted policy.
- A repository cannot exceed server ceilings: `context.max_tokens_per_request` ≤
  `budgets.analysis.max_input_tokens`. Execution is enabled only in `sych.yaml` together with
  `execution.allowed_profiles`; it is disabled by default (M3.8). An allowed profile must be defined in
  `execution.profiles`: a docker `image` pinned by `@sha256:`, an absolute `executable` inside it, `args` that are
  literals or `{param, one_of: [...]}` choices, `timeout_seconds`, the `report` format of its stdout and limits
  (`memory_mb` 2048, `cpu_millis` 2000, `pids` 512 by default). A request can only pick listed values and cannot add
  arguments; neither the model nor the repository writes a command line.

  When enabled, every allowed profile runs once per review on the pinned head: the head is materialized into a fresh
  workspace (secrets, symlinks, submodules and tool configs left out), mounted as the only volume of a docker
  container with `--network none`, `--cap-drop ALL`, `--read-only`, no-new-privileges and the limits; the environment
  carries no Sych secret. Docker is the only backend for repository code. The stdout is imported as a CI report bound
  to the head (`TrustClass::DerivedLocal`, binding `ENVIRONMENT`, runner `<profile> in <image>`); a non-zero exit is a
  result, not an error. A refused (`disabled`, `not allowed`, `missing` or `invalid` parameter) or failed run is a
  task gap `EXECUTION_REFUSED` / `EXECUTION_FAILED`, never a passed check. No request supplies parameters yet, so a
  profile with choices is refused as missing them.

  A failure/retry/cancel/unmount check prints `sych_manifest` with `assertions: [{name, expected: [event], observed:
  [event]}]`: an assertion passes only when the recorded event order equals the expected one; without `observed` it is
  `SKIPPED` (not run, never a pass), so a matching return value alone proves nothing. With `compare_base: true` (needs
  `report`) the profile also runs on the diff base in its own workspace; every head outcome whose status differs is
  named `BASE_DIFFERS: <suite>/<name> is <base> on the diff base and <head> on the head` (a non-zero base exit or paths left
  out of the base workspace are named `BASE: …` beside it), and a base that could not run is
  `BASE_NOT_RUN`. The base run only labels the head report; it is never evidence of its own. Without runtime evidence the verifier keeps a bounded static
  argument or a concrete `UNKNOWN`; generating new tests is optional and never done by the tour reader.

  ```yaml
  execution:
    enabled: false
    allowed_profiles: [go-test]
    profiles:
      go-test:
        image: golang:1.26@sha256:<digest>
        executable: /usr/local/go/bin/go
        args: [test, -json, ./...]
        timeout_seconds: 300
        report: go_test_json
  ```
- `tooling.dependency_install` (default `false`) allows installing the dependencies of the repository under review
  for semantic analysis: `pnpm install --frozen-lockfile --ignore-scripts --ignore-pnpmfile` with the pinned
  pnpm from `packageManager`, and `cargo fetch` of the workspace and std into a Sych-owned `CARGO_HOME`. It runs only in
  `tooling.sandbox.backends` (in order: `bwrap`, `docker`; the first available one is used, the rest are
  fallbacks). `docker` requires `docker_image` with an `@sha256:` digest and currently installs only pnpm.
  `on_unavailable: fail` (default) stops the review with `TOOLING_UNAVAILABLE` (code 3) if no
  backend is available or the installation failed; `degrade` continues without dependencies, with a limitation.
- `redaction` extends the built-in secret redaction (private keys, `ghp_`/`sk-`/`AKIA`… tokens,
  values of `password`/`token`/`api_key`… keys) and never replaces it. `token_prefixes` are token
  prefixes (16+ `[A-Za-z0-9_-]` characters are required after the prefix), `secret_keys` are key name substrings
  (case-insensitive), `patterns` are regular expressions (`regex` crate syntax, linear time);
  each match is replaced with `[REDACTED]` in DATA blocks before they are sent to the model. An empty prefix or
  key, an invalid expression or an expression that matches the empty string is a configuration error with the path
  `redaction.patterns[N]`. It is set only in the trusted `sych.yaml`: repository policy cannot weaken or
  pollute redaction. All other places (logs, provider errors, configuration output) use the
  built-in rules.

## Precedence and effective config

Engine defaults → `sych.yaml` → accepted repository policy. The effective config is normalized (all defaults
filled in) and gets a `config_hash` (server + repository config) and a `rules_hash` (rules, effects,
deployment, contract hashes). `PolicySnapshot.id` depends on the source, both hashes and the authorization
policy version. `provenance` lists the layers with SHA-256 of the source files; it contains no secret values.

The path/domain layer is expressed through rules' `applies_to.paths`; engine generic rules come after repository rules
and are replaced by them by ID. The specification does not define a separate file for personal preferences, so there is no such layer yet.

## Review language

`review.language` in `.sych/config.yaml` is `en` (default) or `ru`; any other value is a config error. Every model
prompt asks for human-readable fields (titles, claims, explanations, questions, suggestions) in that language, with
code, identifiers, paths, ids and enum values unchanged, and the static text of the summary and inline
comments is rendered in it. The language is part of the policy snapshot and of every prompt, so a change of language
makes a new policy id and new checkpoint keys.

## Policy from head is not applied

`sych review` reads `.sych/` only from the target tip. Changes to `.sych/` in the head under review go into
`policy_diff` (files, rule IDs, errors of the proposed policy after redaction) and do not affect the current run
(INV-12). An invalid policy on the target tip is exit 2. AGENTS.md, CLAUDE.md and other files outside `.sych/` do not
become policy.

A new trusted revision is applied only by a separate explicit operator run: `sych review … --policy-ref REF` reads
`.sych/` from commit `REF` instead of the target tip; the result's `policy.source` is `{ref_name: REF, commit}`, and a warning
is written to stderr. Head still remains only a diff.

## Rules, versions and applicability

A rule ID is stable; a change of meaning bumps `version`. Meaning is `kind`, `required`, `severity_on_violation`,
`applies_to`, `statement`, `acceptance`, `contract_refs`; title, owner, rationale and exceptions may change within the same
version. A rule proposed in head with a new meaning and the previous version goes into `policy_diff.head_errors`
(«changes its meaning without a version bump»).

Applicability to a change (§11.4) is three-valued:

- `APPLIES` — the old or new path of a changed file matched `applies_to.paths` (moving a file outside the
  paths does not lift the rule), or the changed construct is called or referenced from these paths according to semantic
  facts — then limitations record «applies through a semantic link: `name` is used from path:line».
- `DOES_NOT_APPLY` → `NOT_APPLICABLE` with a verifiable basis (no path, no semantic link).
- `UNKNOWN` — incomplete diff, unresolved `change_kinds`, or a generic domain rule without its input
  (`DEPLOYMENT_INPUT_MISSING`, `TERRAFORM_PLAN_MISSING`, `K8S_MANIFESTS_MISSING`).

### Generic rule pack

The engine ships the initial set from §11.4. A rule is enabled by its `review.profiles` profile and creates an
obligation only when the change concerns it; a repository rule with the same ID replaces the generic one.

| ID | Profile | Activation | Current assessment |
|---|---|---|---|
| EFFECT-01 | side_effects | a path from effect registry `implementation_refs` is changed, or an effect ledger entry is not `preserved` | model, based on the ledger ([SEMANTIC.md](SEMANTIC.md#effect-ledger)) |
| EFFECT-02 | side_effects | syntax shapes of errors, cancellation and lifetime in changed lines (including `swallowed_exception`), or a ledger entry with changed lifetime/failure | model |
| MIG-COMPAT, MIG-RESTART, MIG-CONTRACT, MIG-RECOVERY | migration_compatibility | `**/migrations/**`, `**/migrate/**`, `*.sql` | `UNKNOWN`, migration checker (M3) |
| TF-CHANGE | infrastructure | `*.tf`, `*.tfvars` | `UNKNOWN`, Terraform plan checker (M3) |
| K8S-ROLLOUT | infrastructure | `k8s/`, `helm/`, `charts/` | `UNKNOWN`, rollout checker (M3) |
| TEST-INTEGRITY | correctness | tests or CI gates changed, or a new skip/`#[ignore]` or suppressed lint in changed lines | model |

All generic rules are required: an unavailable checker does not make a rule optional but leaves `UNKNOWN` with
the name of the missing evidence. Checking tests does not substitute for an accepted requirement: an assessment that relies only
on a test from the same change is `TEST_ORACLE_ONLY`.

### Preferences and nitpicks

`kind: preference` is never required and never creates an obligation; a nitpick without a `rule_id` is allowed. The verifier
accepts NIT/IMPROVEMENT at no higher than `LOW` and only with a concrete location, so a nit does not change the recommendation; zero
nitpicks is a normal result.

### Exceptions

A rule's `exceptions[]`: `paths`, `reason`, `owner`, `author` and `expires_at` or an `until` condition. An exception
whose paths touch the change and which is in force at the time of the run becomes a `waivers[]` entry of an obligation with status
`VIOLATED` or `UNKNOWN`: the status and the rule violation finding are kept, but such an obligation does not raise the
recommendation (neither `ATTENTION_REQUIRED` nor `NEEDS_EVIDENCE`), while coverage still counts it.
An expired exception is not applied and is recorded in limitations.

## Secrets

- `is_secret_path` — default deny for `.env*` (except `*.example|sample|template|dist`), keys
  (`*.pem`, `*.key`, `id_rsa`…), credentials, `.npmrc`/`.netrc`, `*.tfstate*`, `.ssh/.aws/.gnupg/.terraform` directories.
  Such files remain in the diff manifest with the `SECRET_PATH` limitation; their contents must not reach the context/model.
- `redact` replaces private key blocks, known token formats and values of keys like
  `password/secret/token/api_key` with `[REDACTED]`. It is applied to configuration error messages and to the output of
  `config validate`/`doctor`. This is a heuristic, not a guarantee (§22.3).
- `ServerConfig::egress_allowed` — an allowlist by provider `endpoint` (`https://` or loopback `http://`).
  The URL is parsed: userinfo is forbidden; origin (scheme, host, port) and path segments are compared, not a string prefix.
- A profile's `billing_kind`/`enforcement` must match the provider adapter: `openrouter`/`openai_compatible` —
  `api_usage` + `api_hard`, subscriptions — `subscription_quota` + `subscription_observable`.

## Commands

```sh
sych config init --personal                 # writes ./sych.yaml, leaves an existing file untouched (exit 2)
sych config validate --config sych.yaml --repo .
sych doctor --config sych.yaml --repo .
sych review --repo . --base main --head HEAD --config sych.yaml --models fake
```

- `config validate` checks the schema, paths, role → profile → provider references, fallback chains and limits, reads
  `.sych/` from the **working tree** (for authoring) and prints the effective config as JSON. Errors go to stderr, exit 2.
- `doctor` additionally checks the Git repository, the presence of environment variables (only «set/not set»),
  write access to the artifact store and parsing of the price catalog (format — [MODELS.md](MODELS.md)). It does not deploy, does not publish a comment, and
  calls a model only with `--probe-model PROFILE` (see [MODELS.md](MODELS.md)). Exit 0 on `ok/warn`,
  4 on `FAIL`, 2 on an invalid configuration.
- `doctor --scm-smoke CONNECTION --pr N` is the one explicit write: through the connection's first repository it
  creates a comment marked `<!-- sych:smoke:<uuid> -->`, reads it back by id, updates it, reads it again, finds it by
  marker as the App's own and deletes it, then prints a JSON report (installation id, permissions, capabilities,
  comment id, body hashes; no tokens). Local checks are skipped, so it runs in the server image. Exit 0 when the
  comment was found and is gone.
