<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="brand/lockup-transparent-light.png" />
    <img src="brand/lockup-transparent-dark.png" alt="Sych" width="420" />
  </picture>
</p>

# Documentation

## Using Sych

- [LOCAL_REVIEW.md](LOCAL_REVIEW.md): local `review`, `show` and `explain`, output formats and exit codes.
- [CONFIG.md](CONFIG.md): `sych.yaml` and `.sych/*`, rule packs by profile, waivers, secret paths and redaction.
- [DEPLOYMENT.md](DEPLOYMENT.md): the deployment model `.sych/deploy.yaml`, the transition matrix and `MIG-*` rules.

## How it works

- [ANALYSIS.md](ANALYSIS.md): model review passes (assessment, discovery, risk audit, verifier) and quality measurements.
- [MODELS.md](MODELS.md): model providers and subscriptions, role routing, budgets and the cost ledger.
- [SEMANTIC.md](SEMANTIC.md): pinned language servers and linters, semantic facts and their cache.
- [STORAGE.md](STORAGE.md): SQLite, content-addressed artifacts, retention and GC.

## Running a server

- [SERVER.md](SERVER.md): the GitHub App, admission, scheduler, CI evidence, publication, `/v1` API, operations and
  the [self-review of this repository](SERVER.md#self-review-bondianosych).

## Reference

- [SYCH_ROADMAP.md](SYCH_ROADMAP.md): open architecture-review tasks, decisions, evidence and verification limits.
- [DASHBOARD_ROADMAP.md](DASHBOARD_ROADMAP.md): the dashboard and admin panel plan, with a competitor comparison.
- [adr/](adr/): architecture decision records.
- [examples/](examples/): saved outputs: local review, context packets, model probes, model review, quality runs,
  a real Rust review, semantic facts and server documents.
- [research/](research/): background research behind rules and eval cases.
- [brand/](brand/README.md): logo, icons, avatars and colors.
- [../schemas/](../schemas/): JSON Schema and OpenAPI generated from the Rust DTOs.
