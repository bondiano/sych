# Model review example

A pair of changes to a single file `src/order.ts` under rule `ORDER-01` (required, HIGH: "total() returns 0 when items
is undefined instead of throwing") from the target tip's `.sych/rules.yaml`; the semantic backend is unavailable (no lockfile),
so the packets are diff hunks with three lines of context.

- **unsafe** — the guard `if (!items) { return 0; }` is replaced with `items!.reduce(...)`: [unsafe.md](unsafe.md),
  [analysis](unsafe.analysis.json).
- **safe** — the guard is replaced with `(items ?? []).reduce(...)`: [safe.md](safe.md), [analysis](safe.analysis.json).

Profile `cheap_api` (DeepSeek V4.1 Flash via OpenRouter, `reasoning_effort: low`, template `review@1`), default
budgets. This is a smoke test of the harness and contracts on a real model, not a quality measurement (M1.8).
