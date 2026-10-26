# Holdout quality suite

Holdout cases measure review quality on changes that were never used to tune prompts, rules,
routing or the syntax/semantic passes (spec §27.2). The owner writes and labels them; the
development suite next to it (`../dev`) is the one Sych is tuned against.

Rules:

- Split by whole PR or scenario, never by fragments of one diff: a scenario's `safe`, `unsafe`
  and `unknown` cases all live here or all live in `../dev`.
- One case per file, named `<id>.yaml`, with `split: holdout`. The format is the development
  format (§27.3): `pair`/`variant`, `profiles`, `rationale`, inline `base`/`head` files or a
  `repository_fixture`, and `expected` with obligations, findings (`semantic_target`,
  `required_condition`, `evidence_locations`), optional deterministic `facts`/`forbidden_facts`,
  `forbidden_claims` and, for `unknown` cases, what cannot be decided.
- Label meaning, not wording: a review matches a finding by target and condition, never by
  matching comment text.
- Do not read holdout cases while changing prompts, rules or detectors; check only that they
  validate:

```sh
sych eval validate --suite eval/quality/holdout --repo sych=. --repo vkusori=../vkusori
```

`validate` proves labels are complete and anchored. Model quality is measured by real-model
runs (M1.8), not by this command.
