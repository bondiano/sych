<!-- sych:summary:7 -->
### Sych review — advisory

Reviewed head `bbbbbbbbbbbb` against target `aaaaaaaaaaaa` · result `01900000-0000-7000-8000-000000000001` (revision 1).
Coverage: **partial** — the gaps below were not checked; this is not "no problems" · recommendation `AttentionRequired` · execution `PlanExecuted`.
Profiles: correctness, side_effects.

#### Problems (10)
| Severity | Finding | Where | Shown |
|---|---|---|---|
| Critical | Defect 0 `EFFECT-01` | `backend/orders/create.go:42` | inline |
| Critical | Defect 1 `EFFECT-01` | `backend/orders/create.go:42` | inline |
| High | Defect 4 `EFFECT-01` | `backend/orders/create.go:42` | inline |
| High | Defect 7 `EFFECT-01` | `backend/orders/create.go:42` | inline |
| High | Defect 9 `EFFECT-01` | `backend/orders/create.go:42` | inline |
| High | Defect 8 `EFFECT-01` | `backend/orders/create.go:42` | inline |
| High | Defect 6 `EFFECT-01` | `backend/orders/create.go:42` | inline |
| High | Defect 3 `EFFECT-01` | `backend/orders/create.go:42` | inline |
| High | Defect 2 `EFFECT-01` | `backend/orders/create.go:42` | here (over the 8 new inline comments of a revision) |
| High | Defect 5 `EFFECT-01` | `backend/orders/create.go:42` | here (over the 8 new inline comments of a revision) |

#### Unknown and missing checks (2)
- **required** `EFFECT-02` Is cancellation behavior preserved? — SEMANTIC_PROVIDER_UNAVAILABLE; needs: Callers of CreateOrder from gopls references.
- Does any consumer rely on event ordering?

<details><summary>Improvements and nitpicks (2) — optional</summary>

- Nit · preference: Name the helper after what it returns (`backend/orders/create.go:42`)
- Improvement · STYLE-02: Reuse the existing retry helper (`backend/orders/create.go:42`)

</details>

Cost: $0.000000 known · 2 model attempts · 42 s · 10 high or critical.
Full result: https://sych.example/v1/runs/01900000-0000-7000-8000-000000000001/result

_Advisory: Sych never approves, requests changes or gates merges._
