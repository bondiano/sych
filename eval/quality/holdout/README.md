# Holdout quality suite

Holdout cases measure review quality on changes that were never used to tune prompts, rules,
routing or the syntax/semantic passes. The development suite next to it (`../dev`) is the one Sych
is tuned against. The public repository ships the split empty: holdout cases come from your own
repositories, copied from [case.yaml.template](case.yaml.template).

Rules:

- Split by whole PR or scenario, never by fragments of one diff: a scenario's `safe`, `unsafe`
  and `unknown` cases all live here or all live in `../dev`.
- One case per file, named `<id>.yaml`, with `split: holdout`. The format is the development
  format: `pair`/`variant`, `profiles`, `rationale`, inline `base`/`head` files or a
  `repository_fixture`, and `expected` with obligations, findings (`semantic_target`,
  `required_condition`, `evidence_locations`), optional deterministic `facts`/`forbidden_facts`,
  `forbidden_claims` and, for `unknown` cases, what cannot be decided.
- Label meaning, not wording: a review matches a finding by target and condition, never by
  matching comment text.
- A fix commit is a `safe` (or `unknown`) case; the commit that introduced the bug, found with
  `git log -L` or `git blame` on the fixed lines, is the `unsafe` one. Both use `repository_fixture`
  with `base_ref: <commit>~1` and `head_ref: <commit>`.
- Do not read holdout cases while changing prompts, rules or detectors; check only that they
  validate:

```sh
sych eval validate --suite eval/quality/holdout --repo app=../app
```

`validate` proves labels are complete and anchored. Model quality is measured by real-model
runs, not by this command.
