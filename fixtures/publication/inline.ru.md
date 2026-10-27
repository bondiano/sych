<!-- sych:inline:sha256:3333333333333333333333333333333333333333333333333333333333333333 -->
**🟠 High · Нарушение правила** · `EFFECT-01`

#### Order event is no longer published after commit

The refactor moved the insert out of the transaction that wrote the outbox row.

| Когда | Последствие |
|---|---|
| two concurrent requests | Consumers never receive order-created. |

**Что сделать:** Write the outbox row in the same transaction as the order.

<details><summary>🤖 Промпт для AI-агента</summary>

```text
Проверьте каждое замечание по текущему коду; исправьте только актуальные, остальные пропустите с короткой причиной.

В `backend/orders/create.go:41-47`: Order event is no longer published after commit. The refactor moved the insert out of the transaction that wrote the outbox row. Исправить: Write the outbox row in the same transaction as the order. Проверить: A test stops the process between commit and delivery.
```

</details>

<sub>Рекомендательное замечание Sych; не блокирует слияние.</sub>
