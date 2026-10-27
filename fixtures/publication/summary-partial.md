<!-- sych:summary:7 -->
### Sych review

> [!CAUTION]
> **Problems: 1, 1 high or critical.** Coverage: **partial** — the gaps below were not checked; this is not "no problems".

#### 1. 🟠 High · Rule violation: Order event is no longer published after commit

`EFFECT-01` · `backend/orders/create.go:41-47` · ↳ shown inline

#### Unknown and missing checks (2)

- **required** `EFFECT-02` Is cancellation behavior preserved? — SEMANTIC_PROVIDER_UNAVAILABLE; needs: Callers of CreateOrder from gopls references.

<details><summary>Other checks and open questions (1)</summary>

- Does any consumer rely on event ordering?

</details>

<details><summary>🤖 Prompt for all findings for AI agents</summary>

```text
Verify each finding against the current code; fix only those that still apply and skip the rest with a short reason.

- In `backend/orders/create.go:41-47`: Order event is no longer published after commit. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
```

</details>

---
<sub>Head `bbbbbbbbbbbb` · target `aaaaaaaaaaaa` · result `01900000-0000-7000-8000-000000000001` (revision 1) · recommendation `AttentionRequired` · execution `PlanExecuted` · profiles: correctness, side_effects<br>
Cost: $0.000000 known · 2 model attempts · 42 s · [full result](https://sych.example/v1/runs/01900000-0000-7000-8000-000000000001/result)<br>
Advisory: Sych never approves, requests changes or gates merges.</sub>
