# Roadmap: дашборд и админка Sych

Дата: 2026-10-31. Базовый commit: `d9c65a7f1f3836a95a2706283e32039a982d5e74`. Статус: план, ничего не реализовано.

**Рамки.** Аудитория — команда в одной организации с ролями reader, triage, writer и maintainer. Мультитенантный
SaaS не рассматривается. Админка читает данные и выполняет операции, но не редактирует конфиг ботов: подключения,
модели, бюджеты и API-клиенты остаются в доверенном server YAML, policy репозитория — в `.sych/` на target tip.
UI — отдельный клиент `/v1`; его контракт — DTO в `sych_core::api` и сгенерированный из них
[OpenAPI](../schemas/openapi.json). Язык и стек UI этим документом не задаются. До R1 обратная совместимость API
не обещается.

## Вывод

Главный блокер — бэкенд, а не экраны. Нужные данные уже лежат в БД, но `/v1` умеет только GET по id: нет списков,
поиска и агрегатов. Действуют не люди, а статические API-клиенты. Audit пишет только reconcile, а история
стоимости живёт 90 дней.

У конкурентов обязательный минимум — acceptance или addressed rate с разбивкой, сквозная лента findings, фильтры по
repo и периоду и статистика по правилам. Сильные стороны Sych, которых у конкурентов в найденных docs нет:
явные `UNKNOWN`/`PARTIAL` с причинами gaps, cost ledger на каждый model attempt, waivers со сроком и guided tour с
вопросами. Дашборд должен показывать их, а не прятать за одной «оценкой качества».

Порядок: **D0 → D1 → D2 ∥ D3 → D4 → D5**, D6 — позже. Минимальный полезный срез: D0, D1, обзор и inbox findings
из D2, подключения и waivers из D3.

## 1. Что есть сейчас

| Область | Есть | Нет |
|---|---|---|
| Данные | changes, runs (state, coverage, tokens, cost, failure), `model_attempts` (day, month, purpose, model, ledger state), findings и `finding_events`, feedback, waivers, jobs, `change_events`, `inbox_events`, `publication_*`, `audit_events`, `tour_*` — [схема](../migrations/postgres/0001_storage.sql) | таблиц людей, сессий, лога API-запросов |
| HTTP чтение | `GET /v1/runs/{id}`, `…/result`, `…/tour`, `/v1/publications/{id}` — [router](../crates/sych-app/src/server/mod.rs#L141-L155) | list, search, aggregate; feedback требует `findings.id`, который не отдаёт ни один endpoint |
| HTTP действия | reviews, cancel, rerun, feedback, waiver, handoff, tour reveal/complete/dismiss/questions | отзыв waiver: `revoked_at` только читается ([lineage.rs](../crates/sych-storage/src/lineage.rs#L287)); reconcile только из CLI |
| Storage без HTTP | `open_changes`, `change_events` ([scheduler.rs](../crates/sych-storage/src/scheduler.rs#L63)), `findings_of`, `finding_history`, `active_waivers` ([lineage.rs](../crates/sych-storage/src/lineage.rs#L129)), `operations` ([outbox.rs](../crates/sych-storage/src/outbox.rs#L127)), `jobs_of` ([jobs.rs](../crates/sych-storage/src/jobs.rs#L321)), `unresolved_publications` ([publish.rs](../crates/sych-storage/src/publish.rs#L444)) | — |
| Identity | статические `api_clients` из YAML с ролью и списком repo ([http.rs](../crates/sych-core/src/config/http.rs#L55-L64)); actor — имя клиента | вход человека; `Engine::feedback`/`waive` с проверкой GitHub permission ([findings.rs](../crates/sych-app/src/server/findings.rs#L87-L115)) из HTTP не вызываются |
| Audit | `audit_events` — только `--allow-create` у reconcile ([recreate.rs](../crates/sych-storage/src/recreate.rs#L79)), `--actor` свободный текст | записи о feedback, waiver, cancel, rerun, publish; CLI `publish` проверяет клиента, но не сохраняет actor |
| Метрики | глобальные gauges ([metrics.rs](../crates/sych-app/src/server/metrics.rs)), [alerts.yaml](examples/server/alerts.yaml) | разбивка по repo; tour-метрики ([SERVER.md](SERVER.md#metrics-and-alerts)) |
| Конфиг | читается при старте; `sych config validate` печатает эффективный конфиг с hashes | просмотр через API |
| Retention | runs, findings, feedback, waivers не удаляются; `model_attempts`, `change_events`, `audit_events` — 90 дней; jobs, `inbox_events` — 30; results (artifacts) — 7–30 ([retention.rs](../crates/sych-storage/src/retention.rs)) | долгие тренды стоимости; результат старого run |

## 2. Что у конкурентов

Исследование 2026-10-31 по официальным docs. «Не найдено» означает, что функции нет в прочитанных страницах, а не
что её нет в продукте. Тарифные ограничения почти не проверялись.

| Возможность | У кого | Sych сейчас | План |
|---|---|---|---|
| Acceptance/addressed rate по severity, category, repo | CodeRabbit (Quality), Greptile (Addressed Rate), Qodo (high-severity acceptance, показывается от 5 исходов), Copilot (applied by comment type), Bugbot, Graphite, Bito, Baz | глобальная `sych_feedback{judgement}` | D4 |
| Сквозная лента findings | Greptile (Bugs Caught), Graphite (Comment feed), Qodo (CSV до 10 000) | нет list API | D0, D2 |
| Статистика по правилам | Greptile, Graphite (и по exclusions), Qodo (passed, violated, merged with violation; сам отключает редко исправляемые), Bugbot | нет | D4, без автоотключения |
| Правила и learnings в UI, очередь pending | CodeRabbit (learnings, approval delay), Greptile (suggested rules), Qodo (Rule Miner), Bugbot, Graphite | policy в `.sych/` на target tip | D6, только через PR |
| Шум, строгость, effort tiers | Greptile, Qodo, Copilot (Lite/Balanced/Max), Bugbot, Graphite (exclusions), Macroscope, CodeAnt | профиль `deep`, nitpicks в `.sych/config.yaml` | D3 показывает применённое |
| Стоимость за review | Ellipsis (в UI), Bugbot (API `cost_cents`), Copilot (оценка по tier), Macroscope (по repo, feature) | ledger на каждый attempt, без API | D0, D4 |
| Бюджеты, лимиты и алерты | Ellipsis (уровни, алерты 80/100%), Macroscope (на review, PR, месяц), Copilot и Greptile (остановка при нуле) | run, session, tenant daily/monthly в YAML; alert `SychBudgetHeld` | D4 прогноз, D5 алерты |
| Audit log | CodeRabbit (Enterprise), CodeAnt (API) | только reconcile | D1 |
| Кто может запускать и «учить» бота | Greptile, Copilot, Bugbot (allow/deny), Qodo | GitHub permission для `/sych`-команд | D1 для UI |
| Запланированные отчёты | CodeRabbit (Slack, email), Macroscope (weekly digest) | нет | D5 |
| Причины пропуска PR | Bito (отдельный график) | `change_events` (`SKIPPED:*`) без API | D3, D4 |
| Merged with open findings | Qodo | `changes.scm_state = MERGED` + `OPEN` findings | D4 |
| Live preview, playground | CodeRabbit (пример комментария при правке настроек), Baz (prompt на реальном PR) | `sych review --change` без публикации | D6 |
| Waivers со сроком и scope | не найдено | API создания | D0 отзыв, D3 экран |
| Guided tour с вопросами | не найдено (ближе всего Sourcery reviewer's guide и чат `/ask`) | API | D2 |
| Явные `UNKNOWN`/`PARTIAL` и gaps | не найдено | в result | D2, D4 |

**Выводы.**

- Обязательный минимум: acceptance-метрика, лента findings, фильтры по repo и периоду, правила со статистикой.
- Повторять стоит статистику по каждому правилу, порог выборки перед показом доли (Qodo) и стоимость рядом с
  результатом (Ellipsis).
- Не повторять: «time saved» и рейтинги авторов. Первое не проверяется, второе превращает метрики бота в оценку людей.
- Автоотключение правил (Qodo) не подходит: policy доверенная и живёт в репозитории, её меняет человек через PR.

**Источники:**
[CodeRabbit dashboard](https://docs.coderabbit.ai/guides/dashboard),
[CodeRabbit learnings](https://docs.coderabbit.ai/knowledge-base/learnings.md),
[CodeRabbit audit logs](https://docs.coderabbit.ai/management/audit-logs.md),
[CodeRabbit scheduled reports](https://docs.coderabbit.ai/guides/scheduled-reports.md),
[Greptile analytics](https://www.greptile.com/docs/account/analytics.md),
[Greptile learning](https://www.greptile.com/docs/code-review/training-the-learning-system.md),
[Qodo analytics](https://docs.qodo.ai/governance/code-review-analytics),
[Qodo rule analytics](https://docs.qodo.ai/governance/rule-enforcement/analytics),
[Copilot code review](https://docs.github.com/en/copilot/concepts/agents/code-review),
[Copilot metrics](https://docs.github.com/en/copilot/concepts/copilot-usage-metrics/copilot-metrics),
[Cursor Bugbot](https://cursor.com/docs/bugbot),
[Graphite AI reviews](https://www.graphite.com/docs/ai-review-comments),
[Graphite insights](https://www.graphite.com/docs/insights),
[Sourcery analytics](https://docs.sourcery.ai/reviews/analytics/),
[Bito analytics](https://docs.bito.ai/ai-code-reviews-in-git/code-review-analytics.md),
[Ellipsis budgets](https://www.ellipsis.dev/docs/budgets),
[Baz evaluation](https://baz.ai/docs/insights/evaluation),
[Macroscope pricing](https://docs.macroscope.com/pricing).

## 3. Принципы

- **UI — только клиент `/v1`.** Каждая цифра на экране приходит из API. Prometheus остаётся для алертов и не
  получает id и путей в labels.
- **Честные метрики.** У каждой доли есть формула, знаменатель и `n` на экране. Долю не показывать, пока исходов
  меньше 5. `UNKNOWN`, `PARTIAL` и неизвестная стоимость показываются отдельно и не превращаются в ноль или успех.
- **Policy только для чтения.** UI показывает применённую policy, правка — через PR в `.sych/`.
- **Операции — через существующие доменные пути.** Каждая записывает `audit_events` с проверенным actor.
- **Минимум исходного кода.** UI не показывает исходники и промпты сверх того, что уже есть в result и в tour
  (`code_refs`).

## 4. Этапы

### D0 — API чтения

Блокер для всех экранов.

- **Общие правила.**
  - Курсорная пагинация по `(created_at, id)` и фильтры.
  - Видимость по repo scope клиента: в списках чужого нет, запрос по id — 404, как сейчас.
  - DTO в `sych_core::api`, OpenAPI через `just schemas`.
- **Endpoints.**
  - `GET /v1/repositories`.
  - `GET /v1/changes`: последний run, coverage, число `OPEN` findings, стоимость.
  - `GET /v1/changes/{id}`: ревизии и runs, `change_events`, `operations`, `findings_of`.
  - `GET /v1/runs` с фильтрами state, coverage и периода.
  - `GET /v1/runs/{id}/attempts`: purpose, profile, модели, ledger state, tokens, стоимость.
  - `GET /v1/findings`: с числовым `id`, это закрывает дыру с feedback; фильтры state, category, rule, repo.
  - `GET /v1/findings/{id}`: `finding_history` и feedback.
  - `GET /v1/waivers` (active, expired, revoked) и `POST /v1/waivers/{id}/revoke` (maintainer).
  - `GET /v1/usage?group_by=day|repository|model|purpose`: known cost и число unknown-cost attempts отдельно,
    `subscription_fees` отдельной строкой.
- **Удалённый result.** Run, чей result удалён по retention, отвечает явным кодом (`RESULT_PRUNED`), как tour
  отвечает `TOUR_UNAVAILABLE`, а не 404.
- **Готово, когда:**
  - тесты проходят на SQLite и PostgreSQL;
  - клиент с узким scope не видит чужие repo ни в одном списке;
  - страницы курсора стабильны при параллельной вставке.

### D1 — люди, роли, audit

- **Вход через GitHub.** Используется user authorization той же GitHub App. User token нужен только чтобы узнать
  login, хранить его не нужно.
- **Сессия.** Хэш токена хранится в новой таблице, actor записывается как `github:<login>`.
- **Роль на каждый repo** — по GitHub permission, той же шкалой, что для `/sych`-команд
  ([SERVER.md](SERVER.md#lineage-feedback-and-waivers-m27)): read → reader, triage → triage, write → writer,
  maintain и admin → maintainer. Проверка идёт через существующий permission check installation-токеном
  ([findings.rs](../crates/sych-app/src/server/findings.rs)) с коротким кэшем.
- **Статические `api_clients` остаются** для машин и CLI.
- **`audit_events`** записывается на feedback, waiver create и revoke, cancel, rerun, publish, reconcile и
  `RISK_DECISION`.
  - Заодно сохранять actor у CLI `publish`.
  - `reconcile --actor` должен быть configured client, а не свободный текст.
- **Журнал.** `GET /v1/audit` для maintainer. `audit_events` больше не удаляются по retention: строк мало, а журнал
  без истории бесполезен.
- **Готово, когда:**
  - роль из GitHub применяется к каждому repo отдельно;
  - пользователь без доступа к repo не видит его ни в списках, ни по id;
  - отозванный на GitHub доступ перестаёт действовать после истечения кэша.

### D2 — дашборд, MVP

Только чтение и feedback.

- **Обзор за период** с фильтром по repo:
  - runs по state;
  - доля `COMPLETE`/`PARTIAL`, `UNKNOWN` obligations;
  - findings opened, resolved, dismissed;
  - known cost и число attempts с неизвестной стоимостью;
  - возраст очереди;
  - `UNCERTAIN`-публикации.
- **Список PR и страница PR:**
  - ревизии: force-push, supersede, successor после позднего CI;
  - recommendation, obligations, findings, gaps и причины coverage;
  - CI evidence, состояние публикаций, стоимость каждого run, ссылка на PR в GitHub.
- **Inbox findings.** Фильтры, feedback (`USEFUL`, `NOT_USEFUL`, `FALSE_POSITIVE`, `WONT_FIX`) от triage и выше,
  история finding.
- **Run trace.** Attempts по purpose: модель, tokens, стоимость, ledger state, отказ. Отвечает на вопрос «почему
  `UNKNOWN`» без чтения логов; у конкурентов аналога не найдено.
- **Tours.** Шаги по `code_refs`, reveal, complete, dismiss и вопросы поверх существующего API.

### D3 — админка операций

Для writer и maintainer.

- **Боты и подключения** — только просмотр:
  - `app_id`, `bot_login`, repos, `publish`, `checks`, `environment`;
  - `reviewer_commit` рядом с commit работающей сборки (`provenance.engine_version`);
  - последнее `SCM_READ_FAILED` как признак проблемы с credentials;
  - решения webhooks по `inbox_events`.
- **Эффективный конфиг** — `GET /v1/admin/config` для maintainer: то же представление, что печатает
  `sych config validate`, с hashes и только именами env, без значений.
- **Очередь:** jobs (`jobs_of`), упавшие runs с причинами, cancel и rerun.
- **Журнал доставок webhooks:** decision и причина отказа; хранится 30 дней.
- **Публикации:** `PUBLISHING` и `UNCERTAIN` (`unresolved_publications`), reconcile через
  `POST /v1/publications/{id}/reconcile` с `allow_create` и причиной, с записью в audit.
- **Waivers:** список, создание, отзыв, «скоро истекают».
- **Policy repo** — только просмотр. PolicySnapshot последнего run хранит только hashes и source, поэтому
  содержимое `.sych/` читается из mirror на этом source, в том же виде, что печатает `sych config validate --repo`.
- **Журнал audit.**
- **Готово, когда:** каждое действие проходит через тот же доменный путь, что CLI или API, и оставляет запись в
  audit. Записи в конфиг нет.

### D4 — аналитика качества и стоимости

**Полезность**

- Распределение feedback по judgement с разбивкой по rule, category, severity и repo.
- Доля ложных = `FALSE_POSITIVE` ÷ все оценки.
- **Addressed rate** = `RESOLVED` ÷ (`RESOLVED` + `DISMISSED` + `OPEN`) по findings завершённых PR (`MERGED`,
  `CLOSED`). `OBSOLETE` не учитывается. `RESOLVED` у Sych требует повторной проверки, поэтому метрика строже,
  чем «комментарий привёл к изменению» у конкурентов.
- **Merged with open findings** — PR в `MERGED` с `OPEN` findings выше заданной severity.

**Шумные правила.** Высокая доля `FALSE_POSITIVE` при достаточной выборке даёт список кандидатов на правку
`.sych/rules.yaml`. Автоотключения нет.

**Метрики честности**

- Доля `PARTIAL`.
- Причины gaps: `CI_*`, `REQUIRED_OVER_BUDGET`, `BUDGET_EXHAUSTED`, ограничения контекста.
- `UNKNOWN` obligations по причинам. Тренд показывает, где ревьюеру не хватает данных или бюджета.

**Стоимость**

- По дням, repo, модели и purpose.
- На PR и на finding с оценкой `USEFUL`.
- Superseded cost — потери на force-push и сменённые ревизии.
- Unknown charges.
- Расход относительно `tenant_daily` и `tenant_monthly` с прогнозом до конца месяца.

**Задержка**

- От создания run до `DELIVERED`-публикации: `runs.created_at` → `publication_operations.finished_at`.
- Длительность run и ожидание в очереди: jobs, окно 30 дней.

**Заранее.** Отдельная настройка retention для settled `model_attempts` и `change_events`, по умолчанию год.
Дневной rollup — только если объём этого потребует.

### D5 — отчёты и алерты

Маленький этап.

- Недельный digest JSON и Markdown через `GET /v1/reports/weekly`, собранный из агрегатов D4.
- Метрика доли израсходованного tenant-бюджета (label — только scope) и правила алертов на 80 и 100% в
  [alerts.yaml](examples/server/alerts.yaml).
- Tour-метрики, обещанные в [SERVER.md](SERVER.md#metrics-and-alerts).
- Доставку digest в Slack или почту — только отдельным решением.

### D6 — позже или под вопросом

- **Предложения правил из feedback.** Очередь pending, применение только через PR в `.sych/`, который открывает
  человек. Риск: prompt injection через текст rationale.
- **Dry-run playground.** Через существующий `sych review --change` без публикации. Каждый запуск стоит денег:
  нужны роль maintainer и отдельный бюджет.
- **Jira, Linear, Slack-агент, экспорт CSV.**

## 5. Не делаем

- Правку подключений, моделей, бюджетов и API-клиентов из UI: это доверенная конфигурация self-review.
- Seats, биллинг, мультитенантность.
- Approve, request changes, merge.
- «Time saved».
- Рейтинги авторов.

## 6. Открытые решения

- **Как доставляется UI.** Статика с того же origin через `sych serve` или отдельный deploy за тем же proxy. От
  этого зависит, нужна ли сессия в cookie с CSRF-защитой или достаточно bearer.
- **Несколько инстансов.** Одна UI на prod и sandbox через переключатель base URL. Агрегация между инстансами не
  планируется.
- **Длина истории.** Сколько хранить `model_attempts` и `change_events` для трендов (D4).
