<!-- sych:summary:7 -->
### Ревью Sych

> [!CAUTION]
> **Проблемы: 10, 10 high или critical.** Покрытие: **частичное** — пробелы ниже не проверены; это не «проблем нет».

#### 1. 🔴 Critical · Нарушение правила: Defect 0

`EFFECT-01` · `backend/orders/create.go:42` · ↳ показано в коде

#### 2. 🔴 Critical · Нарушение правила: Defect 1

`EFFECT-01` · `backend/orders/create.go:42` · ↳ показано в коде

#### 3. 🟠 High · Нарушение правила: Defect 4

`EFFECT-01` · `backend/orders/create.go:42` · ↳ показано в коде

#### 4. 🟠 High · Нарушение правила: Defect 7

`EFFECT-01` · `backend/orders/create.go:42` · ↳ показано в коде

#### 5. 🟠 High · Нарушение правила: Defect 9

`EFFECT-01` · `backend/orders/create.go:42` · ↳ показано в коде

#### 6. 🟠 High · Нарушение правила: Defect 8

`EFFECT-01` · `backend/orders/create.go:42` · ↳ показано в коде

#### 7. 🟠 High · Нарушение правила: Defect 6

`EFFECT-01` · `backend/orders/create.go:42` · ↳ показано в коде

#### 8. 🟠 High · Нарушение правила: Defect 3

`EFFECT-01` · `backend/orders/create.go:42` · ↳ показано в коде

#### 9. 🟠 High · Нарушение правила: Defect 2

`EFFECT-01` · `backend/orders/create.go:42` · не в коде: сверх 8 новых комментариев в коде за ревизию

https://github.com/acme/shop/blob/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/backend/orders/create.go#L42

The refactor moved the insert out of the transaction that wrote the outbox row.

| Когда | Последствие |
|---|---|
| process stops after commit | Consumers never receive order-created. |

**Что сделать:** Write the outbox row in the same transaction as the order.

#### 10. 🟠 High · Нарушение правила: Defect 5

`EFFECT-01` · `backend/orders/create.go:42` · не в коде: сверх 8 новых комментариев в коде за ревизию

https://github.com/acme/shop/blob/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/backend/orders/create.go#L42

The refactor moved the insert out of the transaction that wrote the outbox row.

| Когда | Последствие |
|---|---|
| process stops after commit | Consumers never receive order-created. |

**Что сделать:** Write the outbox row in the same transaction as the order.

#### Неизвестное и непроверенное (2)

- **обязательное** `EFFECT-02` Is cancellation behavior preserved? — SEMANTIC_PROVIDER_UNAVAILABLE; нужно: Callers of CreateOrder from gopls references.

<details><summary>Прочие проверки и открытые вопросы (1)</summary>

- Does any consumer rely on event ordering?

</details>

<details><summary>Улучшения и мелочи (2) — необязательно</summary>

- Мелочь · предпочтение: Name the helper after what it returns (`backend/orders/create.go:42`)
- Улучшение · STYLE-02: Reuse the existing retry helper (`backend/orders/create.go:42`)

</details>

<details><summary>🤖 Промпт для AI-агента по всем замечаниям</summary>

```text
Проверьте каждое замечание по текущему коду; исправьте только актуальные, остальные пропустите с короткой причиной.

- В `backend/orders/create.go:42`: Defect 0. The refactor moved the insert out of the transaction that wrote the outbox row. Исправить: Write the outbox row in the same transaction as the order. Проверить: A test stops the process between commit and delivery.
- В `backend/orders/create.go:42`: Defect 1. The refactor moved the insert out of the transaction that wrote the outbox row. Исправить: Write the outbox row in the same transaction as the order. Проверить: A test stops the process between commit and delivery.
- В `backend/orders/create.go:42`: Defect 4. The refactor moved the insert out of the transaction that wrote the outbox row. Исправить: Write the outbox row in the same transaction as the order. Проверить: A test stops the process between commit and delivery.
- В `backend/orders/create.go:42`: Defect 7. The refactor moved the insert out of the transaction that wrote the outbox row. Исправить: Write the outbox row in the same transaction as the order. Проверить: A test stops the process between commit and delivery.
- В `backend/orders/create.go:42`: Defect 9. The refactor moved the insert out of the transaction that wrote the outbox row. Исправить: Write the outbox row in the same transaction as the order. Проверить: A test stops the process between commit and delivery.
- В `backend/orders/create.go:42`: Defect 8. The refactor moved the insert out of the transaction that wrote the outbox row. Исправить: Write the outbox row in the same transaction as the order. Проверить: A test stops the process between commit and delivery.
- В `backend/orders/create.go:42`: Defect 6. The refactor moved the insert out of the transaction that wrote the outbox row. Исправить: Write the outbox row in the same transaction as the order. Проверить: A test stops the process between commit and delivery.
- В `backend/orders/create.go:42`: Defect 3. The refactor moved the insert out of the transaction that wrote the outbox row. Исправить: Write the outbox row in the same transaction as the order. Проверить: A test stops the process between commit and delivery.
- В `backend/orders/create.go:42`: Defect 2. The refactor moved the insert out of the transaction that wrote the outbox row. Исправить: Write the outbox row in the same transaction as the order. Проверить: A test stops the process between commit and delivery.
- В `backend/orders/create.go:42`: Defect 5. The refactor moved the insert out of the transaction that wrote the outbox row. Исправить: Write the outbox row in the same transaction as the order. Проверить: A test stops the process between commit and delivery.
```

</details>

---
<sub>Head `bbbbbbbbbbbb` · target `aaaaaaaaaaaa` · результат `01900000-0000-7000-8000-000000000001` (ревизия 1) · рекомендация `AttentionRequired` · выполнение `PlanExecuted` · профили: correctness, side_effects<br>
Стоимость: $0.000000 известная · 2 попыток модели · 42 с · [полный результат](https://sych.example/v1/runs/01900000-0000-7000-8000-000000000001/result)<br>
Рекомендательно: Sych никогда не одобряет, не запрашивает изменения и не блокирует слияние.</sub>
