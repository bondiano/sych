# Пример модельного review

Пара изменений одного файла `src/order.ts` под правилом `ORDER-01` (required, HIGH: «total() returns 0 when items
is undefined instead of throwing») из `.sych/rules.yaml` target tip; semantic backend недоступен (нет lockfile),
поэтому packets — diff hunks с тремя строками контекста.

- **unsafe** — guard `if (!items) { return 0; }` заменён на `items!.reduce(...)`: [unsafe.md](unsafe.md),
  [analysis](unsafe.analysis.json).
- **safe** — guard заменён на `(items ?? []).reduce(...)`: [safe.md](safe.md), [analysis](safe.analysis.json).

Профиль `cheap_api` (DeepSeek V4.1 Flash через OpenRouter, `reasoning_effort: low`, шаблон `review@1`), бюджеты по
умолчанию. Это smoke harness и контрактов на реальной модели, а не измерение качества (M1.8).
