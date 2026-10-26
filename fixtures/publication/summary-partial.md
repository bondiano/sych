<!-- sych:summary:7 -->
### Sych review — advisory

Reviewed head `bbbbbbbbbbbb` against target `aaaaaaaaaaaa` · result `01900000-0000-7000-8000-000000000001` (revision 1).
Coverage: **partial** — the gaps below were not checked; this is not "no problems" · recommendation `AttentionRequired` · execution `PlanExecuted`.
Profiles: correctness, side_effects.

#### Problems (1)
| Severity | Finding | Where | Shown |
|---|---|---|---|
| High | Order event is no longer published after commit `EFFECT-01` | `backend/orders/create.go:41` | inline |

#### Unknown and missing checks (2)
- **required** `EFFECT-02` Is cancellation behavior preserved? — SEMANTIC_PROVIDER_UNAVAILABLE; needs: Callers of CreateOrder from gopls references.
- Does any consumer rely on event ordering?

Cost: $0.000000 known · 2 model attempts · 42 s · 1 high or critical.
Full result: https://sych.example/v1/runs/01900000-0000-7000-8000-000000000001/result

_Advisory: Sych never approves, requests changes or gates merges._
