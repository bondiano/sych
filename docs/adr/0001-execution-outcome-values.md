# ADR 0001. Значения `ExecutionOutcome` в ReviewResult

**Статус:** принято, 25 октября 2026. **Затрагивает:** SYCH_SPEC v1.2 §5.7, §8.1–8.2; roadmap M0.1, M0.4; INV-02, INV-03.

## Контекст

§5.7 требует поле `execution_outcome` в каждом ReviewResult, но не перечисляет его значения.
`ExecutionState` из §8.1 для этого не подходит: результат сохраняется только при переходе
`FINALIZING → COMPLETED`, поэтому внутри неизменяемого result state всегда был бы `COMPLETED`
и не сообщал бы, как run дошёл до финализации. Coverage и recommendation — отдельные измерения (INV-02)
и тоже не заменяют это поле.

## Решение

`execution_outcome` описывает, каким переходом §8.1 run попал в FINALIZING:

| Значение | Переход §8.1 / случай §8.2 | Смысл |
|---|---|---|
| `PLAN_EXECUTED` | RUNNING → FINALIZING, план завершён | Все запланированные задачи выполнены |
| `LIMIT_REACHED` | RUNNING → FINALIZING, достигнут лимит | Бюджет/размер остановил план; пропущенное перечислено как gaps |
| `RETRIES_EXHAUSTED` | RETRY_WAIT → FINALIZING | Повторы исчерпаны, сохранён честный partial result |
| `INPUT_UNAVAILABLE` | §8.2, diff не получен | Содержательный анализ невозможен; coverage `UNAVAILABLE` |

`CANCELLED`, `SUPERSEDED` и `FAILED` не являются значениями: в этих состояниях ReviewResult не сохраняется.

## Последствия

- Значение не влияет на recommendation само по себе: её считает reducer по obligations и findings (§5.7).
  `PLAN_EXECUTED` не означает `COMPLETE_FOR_SCOPE`, а `LIMIT_REACHED`/`RETRIES_EXHAUSTED` не допускают его (M0.4).
- Источник истины — `sych_core::ExecutionOutcome`; JSON Schema генерируется из него.
  Добавление значения — изменение enum по §25.4: нужен путь совместимости и тест старого fixture.
- В следующей редакции спецификации §5.7 должен сослаться на эту таблицу.
