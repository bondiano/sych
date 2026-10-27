<!-- sych:summary:7 -->
### Sych review

> [!CAUTION]
> **Problems: 10, 10 high or critical.** Coverage: **partial** — the gaps below were not checked; this is not "no problems".

#### 1. 🔴 Critical · Rule violation: Defect 0

`EFFECT-01` · `backend/orders/create.go:42` · ↳ shown inline

#### 2. 🔴 Critical · Rule violation: Defect 1

`EFFECT-01` · `backend/orders/create.go:42` · ↳ shown inline

#### 3. 🟠 High · Rule violation: Defect 4

`EFFECT-01` · `backend/orders/create.go:42` · ↳ shown inline

#### 4. 🟠 High · Rule violation: Defect 7

`EFFECT-01` · `backend/orders/create.go:42` · ↳ shown inline

#### 5. 🟠 High · Rule violation: Defect 9

`EFFECT-01` · `backend/orders/create.go:42` · ↳ shown inline

#### 6. 🟠 High · Rule violation: Defect 8

`EFFECT-01` · `backend/orders/create.go:42` · ↳ shown inline

#### 7. 🟠 High · Rule violation: Defect 6

`EFFECT-01` · `backend/orders/create.go:42` · ↳ shown inline

#### 8. 🟠 High · Rule violation: Defect 3

`EFFECT-01` · `backend/orders/create.go:42` · ↳ shown inline

#### 9. 🟠 High · Rule violation: Defect 2

`EFFECT-01` · `backend/orders/create.go:42` · not inline: over the 8 new inline comments of a revision

https://github.com/acme/shop/blob/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/backend/orders/create.go#L42

The refactor moved the insert out of the transaction that wrote the outbox row.

| When | Consequence |
|---|---|
| process stops after commit | Consumers never receive order-created. |

**Suggested:** Write the outbox row in the same transaction as the order.

#### 10. 🟠 High · Rule violation: Defect 5

`EFFECT-01` · `backend/orders/create.go:42` · not inline: over the 8 new inline comments of a revision

https://github.com/acme/shop/blob/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/backend/orders/create.go#L42

The refactor moved the insert out of the transaction that wrote the outbox row.

| When | Consequence |
|---|---|
| process stops after commit | Consumers never receive order-created. |

**Suggested:** Write the outbox row in the same transaction as the order.

#### Unknown and missing checks (2)

- **required** `EFFECT-02` Is cancellation behavior preserved? — SEMANTIC_PROVIDER_UNAVAILABLE; needs: Callers of CreateOrder from gopls references.

<details><summary>Other checks and open questions (1)</summary>

- Does any consumer rely on event ordering?

</details>

<details><summary>Improvements and nitpicks (2) — optional</summary>

- Nit · preference: Name the helper after what it returns (`backend/orders/create.go:42`)
- Improvement · STYLE-02: Reuse the existing retry helper (`backend/orders/create.go:42`)

</details>

<details><summary>🤖 Prompt for all findings for AI agents</summary>

```text
Verify each finding against the current code; fix only those that still apply and skip the rest with a short reason.

- In `backend/orders/create.go:42`: Defect 0. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
- In `backend/orders/create.go:42`: Defect 1. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
- In `backend/orders/create.go:42`: Defect 4. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
- In `backend/orders/create.go:42`: Defect 7. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
- In `backend/orders/create.go:42`: Defect 9. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
- In `backend/orders/create.go:42`: Defect 8. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
- In `backend/orders/create.go:42`: Defect 6. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
- In `backend/orders/create.go:42`: Defect 3. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
- In `backend/orders/create.go:42`: Defect 2. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
- In `backend/orders/create.go:42`: Defect 5. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
```

</details>

---
<sub>Head `bbbbbbbbbbbb` · target `aaaaaaaaaaaa` · result `01900000-0000-7000-8000-000000000001` (revision 1) · recommendation `AttentionRequired` · execution `PlanExecuted` · profiles: correctness, side_effects<br>
Cost: $0.000000 known · 2 model attempts · 42 s · [full result](https://sych.example/v1/runs/01900000-0000-7000-8000-000000000001/result)<br>
Advisory: Sych never approves, requests changes or gates merges.</sub>
