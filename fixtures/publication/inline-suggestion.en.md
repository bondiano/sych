<!-- sych:inline:sha256:3333333333333333333333333333333333333333333333333333333333333333 -->
**[P1] Order event is no longer published after commit** · `EFFECT-01`

The refactor moved the insert out of the transaction that wrote the outbox row. Consumers never receive order-created.

**Suggested:** Write the outbox row in the same transaction as the order.

<details open><summary>📝 Committable suggestion: <code>backend/orders/create.go:41-47</code></summary>

```suggestion
    tx.Insert(order)
    tx.Insert(outbox)
```

</details>

<details><summary>🤖 Prompt for AI agents</summary>

```text
Verify each finding against the current code; fix only those that still apply and skip the rest with a short reason.

In `backend/orders/create.go:41-47`: Order event is no longer published after commit. The refactor moved the insert out of the transaction that wrote the outbox row. Fix: Write the outbox row in the same transaction as the order. Check: A test stops the process between commit and delivery.
```

</details>
