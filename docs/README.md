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
- [STORAGE.md](STORAGE.md): PostgreSQL and SQLite, content-addressed artifacts, retention, GC and backup.

## Running a server

- [SERVER.md](SERVER.md): the GitHub App, admission, scheduler, CI evidence, publication, `/v1` API, operations and
  the [self-review of this repository](SERVER.md#self-review-bondianosych).

## Reference

- [adr/](adr/): architecture decision records.
- [examples/](examples/): saved outputs (local review, context packets, semantic facts) and server configuration.
- [brand/](brand/README.md): logo, icons, avatars and colors.
- [../schemas/](../schemas/): JSON Schema and OpenAPI generated from the Rust DTOs.
