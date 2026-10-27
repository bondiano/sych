# Server: GitHub App, admission, scheduling and publication

`sych serve --config sych.yaml` — один процесс personal sandbox: приём webhooks, scheduler, worker и publisher
(M2). Webhook только принимает durable signal; модель в handler не вызывается. Production —
только self-review `bondiano/sych` закреплённой сборкой ([ниже](#self-review-bondianosych)).

## Подключение GitHub App (M2.1)

Trusted server config задаёт каждое подключение; URL webhook выбирает только `:connection`, но не адрес API,
credentials или tenant:

```yaml
scm:
  github:
    - id: sandbox                 # POST /hooks/github/sandbox
      tenant_id: local            # default
      provider_instance: github.com
      api_base_url: https://api.github.com
      app_id: 123456
      private_key_file_env: SYCH_GITHUB_APP_KEY_FILE   # путь к PEM, не сам ключ
      webhook_secret_env: SYCH_GITHUB_WEBHOOK_SECRET   # отличается от API credential
      repositories: [bondiano/sych-sandbox]            # остальные — receipt без работы
      bot_login: sych-sandbox[bot]                     # его comments никогда не планируют работу
      checks: false                                    # Checks write только при включённых checks
      publish: false                                   # advisory публикация; default выключена
      max_body_bytes: 2097152                          # default 2 MiB
```

Валидатор отвергает повтор `id`, `api_base_url` не https (http — только loopback), одинаковые env для webhook
secret и ключа, пустой список repositories и `bot_login`. Секреты читаются при старте; отсутствующий секрет
останавливает старт, не подменяется.

### Permissions и capabilities

| Capability | Permission App | Endpoint |
|---|---|---|
| change_refs, reconciliation, own_comments | Pull requests: read | `GET /repos/{r}/pulls/{n}`, `GET /repos/{r}/pulls?state=open`, `GET /repos/{r}/issues/{n}/comments` |
| full_diff | Pull requests + Contents: read | `GET /repos/{r}/pulls/{n}/files` (не больше 3000 файлов) |
| blobs | Contents: read | git fetch по installation token |
| ci_runs, ci_artifacts | Actions: read | workflow runs/jobs/artifacts (M2.3) |
| summary_write, inline_write | Pull requests: write | issue comment, review comments (M2.5–M2.6) |
| actor_permissions | Metadata: read | `GET /repos/{r}/collaborators/{login}/permission` |
| checks_write | Checks: write — только при `checks: true` | check runs (M2.6) |

`ProviderCapabilities` вычисляется из `permissions` installation (`GET /repos/{r}/installation`) и
сохраняется в binding; отсутствующая capability даёт limitation, не догадку. Installation token запрашивается
на один repository (`POST /app/installations/{id}/access_tokens` с `repositories: [name]`), живёт в памяти и
обновляется за 5 минут до истечения — отдельно от run. App JWT (RS256) подписывается ключом из PEM (PKCS#1, как
выдаёт GitHub, или PKCS#8), действует 9 минут.

### Admission

`POST /hooks/github/:connection`, по порядку, до разбора JSON:

| Проверка | Ответ |
|---|---|
| нет такого connection | 404 |
| body больше `max_body_bytes` | 413 |
| `Content-Type` не `application/json` | 415 |
| `X-Hub-Signature-256` не совпадает с HMAC-SHA256 raw body (constant-time, `ring`) | 401 |
| нет `X-GitHub-Event`/`X-GitHub-Delivery`, event не из списка, невалидный JSON | 400 |
| ошибка сохранения | 503 — не ACK; GitHub отмечает failed delivery, догоняет reconciliation |
| новая delivery | 202 `{inbox_id, duplicate: false, intents}` |
| повтор `provider_instance + connection + X-GitHub-Delivery` | 200 `{inbox_id, duplicate: true}`, ничего не меняется |

Принимаемые events: `ping`, `pull_request`, `issue_comment`, `workflow_run`, `check_suite`, `installation`,
`installation_repositories`. GitHub не подписывает timestamp, поэтому окна replay нет: повтор распознаёт delivery id.
Ошибки — envelope §25.3 (`error.code/message/retryable/correlation_id/details`), без секретов и payload.

Одна транзакция SQLite сохраняет receipt (`inbox_events`: delivery key, digest payload, нормализованное событие,
significance decision), binding (`repositories`: tenant + connection + instance + external id), change и
intent. Raw payload не хранится: текст PR недоверенный и перечитывается из API. Нормализация:

| Событие | Signal | Intent |
|---|---|---|
| `pull_request` opened/reopened/ready_for_review | `OPENED` (draft → `DRAFT`) | `REFRESH`, debounce 5 s |
| `pull_request` synchronize | `SOURCE_PUSH` | `REFRESH`, 5 s |
| `pull_request` edited с `changes.base` | `TARGET_CHANGED` | `REFRESH`, 5 s |
| `pull_request` converted_to_draft | `DRAFT` | `REFRESH`, 5 s |
| `pull_request` closed | `CLOSED {merged}` | `CLOSE`, сразу |
| `workflow_run`/`check_suite` completed | `CI_COMPLETED` | `CI`, 10 s |
| `issue_comment` `/sych rerun|deep|stop` | `COMMAND` | `COMMAND_*`, сразу; права проверяет scheduler |
| comment от `bot_login` | `BOT_COMMENT` | нет |

Debounce: одно открытое intent на change и вид; каждое новое событие сдвигает срок, но не дальше
`первое событие + 6 × debounce` (30 s для PR, 60 s для CI) — непрерывный поток не откладывает работу бесконечно.
Это выбор Sych, не default spec. Repository вне `repositories` получает receipt `REPOSITORY_NOT_ALLOWED` без
работы. Fork (`head.repo` ≠ base, или удалён) помечается и анализируется только read-only.

Изоляция: access scope = `provider_instance/connection/external_id`, поэтому один repository в двух tenants или
connections имеет раздельные changes, generations, artifacts и publication state; artifact с тем же hash в чужом
scope не читается.

### Ошибки adapter (mapping v1)

| HTTP | Read | Write | Inline comment |
|---|---|---|---|
| 401 | `UNAUTHORIZED` | `UNAUTHORIZED` | `UNAUTHORIZED` |
| 403 | `UNAUTHORIZED` (или `RATE_LIMITED` при `x-ratelimit-remaining: 0`/`Retry-After`) | то же | то же |
| 404, 410 | `NOT_FOUND` | `NOT_FOUND` | `NOT_FOUND` |
| 429 | `RATE_LIMITED` + `retry_after_seconds` | то же | то же |
| 422, 400, 409, 415 | `UNSUPPORTED_CAPABILITY`, не retryable | то же | `INVALID_ANCHOR` |
| 5xx, обрыв | `TRANSIENT` | `PUBLICATION_UNKNOWN` (исход неизвестен) | `PUBLICATION_UNKNOWN` |
| > 3000 файлов | `INCOMPLETE_DIFF` | | |

Retryable только `RATE_LIMITED` и `TRANSIENT`; `PUBLICATION_UNKNOWN` уходит reconciler'у, не в повтор.
Клиент не следует redirects и не использует proxy окружения: credentials уходят только на `api_base_url`.

### Подключение sandbox (протокол владельца)

1. Создать GitHub App на аккаунте владельца: webhook URL `https://HOST/hooks/github/sandbox`, secret,
   permissions из таблицы выше (Checks — только если включены), events: Pull request, Issue comment,
   Workflow run, Check suite. Установить только на sandbox-репозиторий.
2. Сохранить PEM вне репозитория, задать `SYCH_GITHUB_APP_KEY_FILE` и `SYCH_GITHUB_WEBHOOK_SECRET`.
3. `sych db migrate --config sych.yaml`, затем `sych serve --config sych.yaml`.
4. В настройках App отправить `ping` (Redeliver): ожидается 202 и receipt `IGNORED`.
5. Открыть PR в sandbox: receipt `SCHEDULED`, intent `REFRESH`.

## Scheduler, jobs и freeze (M2.2)

`sych serve --config sych.yaml [--models roles|PROFILE|fake] [--semantic on|off]` поднимает в одном процессе
admission, scheduler (раз в секунду), reconcile (при старте и раз в 10 минут), reaper (раз в 15 s) и worker.

**Решение по intent** — чистая функция `sych_core::schedule::decide` над состоянием change из API (webhook
только сигнал) и последней generation:

| Intent / состояние | Решение |
|---|---|
| change закрыт или merged (по API) | `CANCEL`: active runs и их jobs отменяются, committed result остаётся |
| `REFRESH`, draft | `SKIP:DRAFT` (`draft_policy: manual_only`) |
| `REFRESH`, тот же `base_sha..head_sha` | `SKIP:SAME_INPUTS` — generation не растёт, inference нет |
| `REFRESH`, новый head или сдвинутый target | новая generation; active старая — `SUPERSEDED`, её lease отозван |
| `COMMAND_RERUN/DEEP` от actor с `write/maintain/admin` | новая generation с теми же inputs, trigger `MANUAL` (`deep` записывается в trigger; роли стартуют с `models.deep_roles.<role>`, роль без него — с `models.roles`, бюджеты те же; закреплённый `--models PROFILE` сильнее) |
| команда без прав | `SKIP:UNAUTHORIZED_COMMAND`; бот не отвечает |
| `COMMAND_STOP` | `CANCEL` активного run |
| `CI` для старого head | `SKIP:CI_FOR_OLD_HEAD` |
| `CI`, run ждёт evidence | `WAKE` |
| `CI` после freeze, когда ожидаемый source не пришёл до deadline | successor generation с `parent_run_id` |
| `CI` после freeze без пропуска (повтор того же artifact или report, который policy не ожидает) | `SKIP:CI_NOT_SIGNIFICANT`, generation не растёт (`tests/scheduler.rs`) |

Каждое решение пишется в `change_events` (`GENERATED`, `SUPERSEDED`, `CANCELLED`, `SKIPPED:<reason>`, `WOKEN`,
`SCM_READ_FAILED`). Ошибка чтения API: `RATE_LIMITED`/`TRANSIENT` откладывают intent (backoff, `Retry-After`),
остальные — запись и consume, без повтора.

**Jobs и fence.** Generation, supersede старой и job (`ANALYSIS`) коммитятся одной транзакцией. Claim — одна
транзакция: `QUEUED` и due → `CLAIMED`, `lease_epoch + 1`, lease 60 s. Worker продлевает lease каждые 15 s;
потерянный lease (cancel, supersede, reaper) останавливает анализ. Reaper возвращает просроченные leases в очередь с
новым epoch, после `max_attempts` (1 + 2 повтора) или deadline job и run — `FAILED`. Каждая authoritative запись run —
pin artifact, prepare, freeze, result — несёт fence: `job_id + owner + lease_epoch`, живой lease, active state и
`run.generation == change.desired_generation`; поздний worker не пинит artifacts и не коммитит result.

**Worker.** Fetch `head_sha` и `base_sha` в bare mirror (`<data>/mirrors/<connection>/<owner>__<repo>.git`) по
installation token в `GIT_CONFIG_*` env (не argv, не URL), без tags/hooks; ничего не checkout и не исполняется —
fork читается так же. Затем pin snapshot и trusted policy target tip: `ci.expected_reports`, чей `applies_to`
совпал с изменёнными путями, ставят run в `WAITING_EVIDENCE` с deadline `ci.wait_seconds` (default 180 s); job
отпускается до deadline и будится CI-событием. Freeze записывает `expected`, пришедшие CI runs и `missing`;
дальше review идёт в отдельном потоке (без SQL-транзакции через git/модель), heartbeat каждые 15 s. Ошибка
fetch/token — retry по классификатору (`RATE_LIMITED`, `TRANSIENT`; до двух повторов, backoff + jitter,
`Retry-After`, не дальше deadline job), иначе run `FAILED`.

**Reconciliation.** При старте и периодически для каждого repository из `repositories`: installation и
capabilities (фиксируются в binding), список открытых PR → synthetic receipt `reconcile:<n>:<head>:<base>` (повтор —
no-op), change, считавшийся открытым, но не найденный — `reconcile-closed` → `CLOSE` после проверки API. Failed
deliveries GitHub сам не повторяет; reconciliation закрывает этот разрыв.

**Checkpoints и successor (M2.2).** Каждый принятый ответ модели сразу пинится к run как artifact
`checkpoint/<key>` под fence, поэтому работа parent'а переживает его supersede. Dependency record задачи: template,
schema name, role, chain, sha256 отправленных messages (после redaction: все spans, evidence, gaps и ответы на
context asks, то есть и область/completeness поиска для отрицательного вывода), sha256 output schema и digest
`models`; `key` — digest этого record. Successor (`Trigger::Successor`) читает checkpoints parent'а только в своей
partition и берёт ответ без model call лишь при точном совпадении record, затем заново валидирует его на своём
context и facts. Новый CI, policy, source, model config или deep-route меняют prompt или record → вызов заново.
Pass и hop помечаются `reused_from`/`REUSED`, attempts и расход нет; ответ снова пишется в checkpoints successor'а.
Сбой записи checkpoint не роняет задачу (ответ остаётся), а пишется в pass как `checkpoint_error` и в лог; ошибка
store повторяется один раз. Отказ по fence (run superseded/отменён/lease потерян) окончателен: `RunLedger` с этого
момента отказывает в reserve, так что run больше не делает платных вызовов, чей result уже не сможет закоммитить.
Result successor'а ссылается на result parent'а (`parent_result_id`), если тот завершён. Successor заново выводит
derived evidence (syntax/semantic facts, `DERIVED_LOCAL`) из своего snapshot; в external input manifest оно не
попадает, а frozen `evidence_manifest` parent'а не меняется (`tests/ci_evidence.rs`). Ручной rerun/deep
checkpoints не берёт — это осознанный повторный inference. Ограничение: анализ одного run идёт в одном worker.

## CI evidence (M2.3)

Repository policy (`.sych/config.yaml` target tip) перечисляет ожидаемые отчёты (`ci.expected_reports`: id,
`applies_to`, format). Откуда каждый берётся, решает только trusted server config подключения — не имя job:

```yaml
scm:
  github:
    - id: sandbox
      # ...
      ci_reports:
        - report: tests                       # expected_reports[].id
          workflow: .github/workflows/ci.yml  # точный файл workflow
          event: pull_request                 # default
          job: test                           # точное имя job
          artifact: reports                   # точное имя artifact
          path: junit.xml                     # entry в zip
          checkout_manifest: sych-ci.json     # CiManifest с tested_oid; без него checkout не доказан
      artifact_hosts: ["*.blob.core.windows.net", "*.actions.githubusercontent.com"]  # default
```

Worker ищет run по точному `head_sha` (`GET /actions/runs?head_sha=`), workflow и event; job — в jobs этого
`run_attempt`; artifact — среди artifacts этого run. Не «последний зелёный pipeline ветки». Скачивание:
`GET /actions/artifacts/{id}/zip` отвечает redirect'ом на signed storage — туда запрос уходит **без** token, только
по https на host из `artifact_hosts`; не больше 64 MiB.

Zip читается в памяти, на диск ничего не пишется: любое имя с `..`, абсолютным путём, `\`, `:` или symlink
отвергает весь архив; не больше 1000 entries; распаковка одного entry останавливается на 32 MiB (zip bomb);
CRC сверяется; zip64 и шифрование не поддерживаются. Парсеры: `cargo --message-format=json`, JUnit, SARIF 2.1, GitLab Code Quality, LCOV,
Cobertura и project JSON manifest (`{"schema_version": 1, "commands": [{"name", "exit_code"}], "tests": [...]}`);
не больше 2000 записей, XML — глубина 64, внешние entities не разрешаются (quick-xml не читает DTD); пути вне
repository отбрасываются.

**Binding.** `tested_oid` checkout manifest: равен head — `SOURCE_HEAD`; merge-коммит, чей второй родитель —
head (его догружают в mirror и читают родителей) — `MERGE_RESULT` с target-коммитом; иначе или без manifest —
`UNVERIFIED` (evidence `STALE`, gap `CI_BINDING_UNVERIFIED`). `CiOrigin` хранит provider instance/repository,
workflow, run/attempt/job, event, статус и conclusion job, `allow_failure` (GitHub его не сообщает — `null`, не
`false`), execution/source-head/target/tested SHAs, digest checkout manifest и zip, artifact id/name, путь отчёта,
время. Evidence — `AUTHENTICATED_CI`: это происхождение, не независимость oracle (limitation в evidence);
coverage — исполнение, не правильность assertions; тест, изменённый в проверяемом diff, помечен `TEST_CHANGED`.
Conclusion job и результаты тестов учитываются раздельно: failed job не прячет и не заменяет JUnit.

**Gaps** (каждый — task gap: coverage `PARTIAL`, текст в `unresolved_questions`): `CI_NOT_MAPPED`,
`CI_FORMAT_UNSUPPORTED`, `CI_JOB_MISSING`, `CI_JOB_INCOMPLETE`, `CI_JOB_SKIPPED`, `CI_JOB_CANCELLED`,
`CI_ARTIFACT_MISSING`, `CI_ARTIFACT_EXPIRED`, `CI_ARCHIVE_REJECTED`, `CI_REPORT_MISSING`, `CI_REPORT_MALFORMED`,
`CI_BINDING_UNVERIFIED`, `CI_UNAVAILABLE`. До evidence deadline `CI_JOB_MISSING`/`CI_JOB_INCOMPLETE` продолжают
ожидание; после — freeze с gaps, а поздний отчёт даёт successor (M2.2). Freeze записывает expected, импортированные
(run/attempt/job/artifact/binding) и gaps.

Ограничения: сравнение с baseline-диагностиками (исчезновение диагностики между producer/version) не делается —
такие выводы не заявляются; `TerraformPlan`/`KubernetesRendered` — M3 (`CI_FORMAT_UNSUPPORTED`). Go-отчёты
(`go_test_json`, `go_cover`, `go_vet_json`) идут тем же downloader/parser/binding, что и остальные форматы.

## Outbox и сверка публикаций (M2.5)

Финализация с `publish: true` кладёт в outbox одну операцию на ревизию summary (`publication_objects` — слот
`summary` на change с marker `sych:summary:<change>`; `publication_operations` — ревизия с generation). Более новая
ревизия того же объекта, ещё не отправленная, становится `SUPERSEDED`.

```
PENDING ──claim──▶ (gates) ──▶ PUBLISHING ──reply──▶ DELIVERED
   ▲                   │            │ lost reply / 5xx / crash / lease loss
   │ definite refusal  │ gate       ▼
   └── (429, retry) ◀──┘ closed ─▶ UNCERTAIN ──found by id/marker──▶ DELIVERED
                         superseded  │ window exhausted
                         ─▶ CANCELLED/SUPERSEDED   ▼
                                     operator `--allow-create` (audit) ─▶ PENDING
```

- **До сети**: операция `PUBLISHING` с телом, digest запроса, attempt id и fence (owner + `lease_epoch`) —
  в одной записи. Для одного объекта не больше одной операции в полёте.
- **Gates**: локально — generation операции равна `desired_generation` change и change открыт; через API — PR
  открыт сейчас (потерянный close блокирует запись, intent `CANCELLED`), у binding есть `summary_write`.
- **Ответ**: `DELIVERED` и observed remote id/generation/digest тела — одна транзакция. Известный id —
  `PATCH`; первый раз — `POST`. 404 на `PATCH` (комментарий удалён) — id забывается, следующая попытка создаёт
  заново: отсутствие известно, а не предполагается. 429 — `PENDING` с `Retry-After`, не больше 5 попыток.
- **Неизвестный исход** (обрыв, 5xx записи, смерть процесса, потеря lease в `PUBLISHING` — reaper переводит
  только в `UNCERTAIN`, никогда в `PENDING`): reconciler раз в 30 s ищет комментарий по сохранённому id, иначе по
  marker на всех страницах, и только написанный самим ботом (`type: Bot`, `login == bot_login`); скопированный
  человеком marker не даёт прав. Пустой или обрезанный список не разрешает CREATE. Найден — `DELIVERED` (если
  тело другое — восстанавливается `PATCH` по известному id). Окно сверки 1 час; дальше операция остаётся
  `UNCERTAIN` с `RECONCILIATION_WINDOW_EXHAUSTED` и ждёт оператора.
- **Relevance** отдельно от delivery: новая generation помечает объекты change `STALE`; доставка текущей —
  `CURRENT`. Закрытие change отменяет `PENDING`; `UNCERTAIN` остаётся для read-only сверки.
- **Поздний старый UPDATE**: раз в 10 минут доставленные объекты сверяются с последним отправленным телом;
  расхождение в комментарии бота восстанавливается и пишется в `change_events` как `PUBLICATION_REPAIRED`.

### Runbook оператора

1. `sych reconcile --config sych.yaml [--change ID]` — список `PUBLISHING`/`UNCERTAIN`: возраст, попытки,
   «reconciling for N s more» или «operator decision (window exhausted)», последняя ошибка. SCM не вызывается.
2. Пока окно не истекло — ничего не делать: server сверяет сам.
3. После окна: открыть PR и проверить, есть ли комментарий бота с marker `sych:summary:<change>`. Если есть, но
   сверка его не нашла (например, список был обрезан) — оставить как есть и разобраться с доступом.
4. Если комментария точно нет: `sych reconcile --allow-create OP --actor NAME --reason TEXT`. Решение
   пишется в `audit_events` (`PUBLICATION_RECREATE_AUTHORIZED`) с риском дубля; операция возвращается в `PENDING`.
   Для операции с известным remote id это запрещено: там сверка восстанавливает тело по id.

Метрики outbox lag / uncertain age / repairs и alert — см. «Метрики и alerts» ниже.

## Summary, inline и check run (M2.6)

Публикация advisory: Sych никогда не делает approve, request-changes, merge и не становится required check; в
адаптере таких вызовов нет. Всё рендерится из immutable result и diff manifest run'а (`sych_core::publication`),
не из прозы модели; в result попадают только findings, прошедшие проверку движка.

**Summary** — один обновляемый комментарий на change: проверенный head и target, result и revision; coverage
(`partial` прямо говорит, что это не «проблем нет», `unavailable` — что ревью не было); recommendation и execution;
profiles; таблица проблем (Defect/RuleViolation по severity — все HIGH/CRITICAL всегда здесь) с колонкой «где
показано» (`inline` или `here (<причина>)`); конкретные `UNKNOWN` (required помечены) и открытые вопросы, включая
CI gaps; свёрнутые improvements/nitpicks с правилом или «preference»; стоимость (known + unknown attempts), число
attempts, число high/critical; ссылка на полный result, если задан `http.public_base_url`. Golden-файлы:
`fixtures/publication/summary-*.md`.

**Inline** — только Defect и RuleViolation; не больше 8 новых за ревизию (уже опубликованные не считаются), по
severity, затем fingerprint; остальные — в summary с причиной. Позиция — явные `commit_id` (head), `path`, `line`,
`side` и `start_line`/`start_side` для диапазона; устаревший `position` не используется:

| Anchor | Inline |
|---|---|
| head-сторона, тот же blob, строки в hunk ± 3 строки контекста | `RIGHT`, путь head |
| diff-base сторона изменённого/переименованного файла | `LEFT`, путь head (как его показывает GitHub) |
| удалённый файл | `LEFT`, старый путь |
| другой blob, target tip, файл вне diff, строка вне показанного diff | summary с причиной; соседняя строка не подставляется |
| provider diff > 3000 файлов | все — в summary («provider diff is incomplete») |
| GitHub ответил 422 | операция `FAILED (INVALID_ANCHOR)`, summary перерисовывается по известному id с причиной «the provider rejected the inline position» |

Каждый inline — отдельный объект outbox (`inline:<fingerprint>`, marker `sych:inline:<fingerprint>`), с теми же
`PUBLISHING`/`UNCERTAIN`/сверкой (review comments, только комментарии бота).

**Check run** — выключен по умолчанию; `checks: true` (вместе с `publish: true`) требует Checks write. Один check
`Sych (advisory)` на change, закреплённый за head SHA, `external_id` = marker; conclusion всегда `neutral` —
и для результата (`Advisory review`), и для сбоя run (`Sych could not complete this review`), так что сбой
отличим по заголовку, а не выдаётся за success. Сверка неизвестного исхода — по `external_id` и `app.id`.

## Lineage, feedback и waivers (M2.7)

**Fingerprint** модельной находки — категория, путь, длина и hash содержимого закреплённых строк, без номера
строки и формулировки: сдвиг кода выше не создаёт «новую» проблему. Rule violation — rule id, версия и
обязательство.

**Lineage** применяется после каждого committed result (в generation order, раз на run): новые findings — `OPEN`
с событием `found`; каждая ранее открытая находка получает решение `sych_core::lineage::decide`:

| Ситуация | Итог |
|---|---|
| тот же fingerprint найден снова | остаётся `OPEN` |
| файл удалён (не переименован) | `OBSOLETE` |
| файл переименован/перемещён, на новом месте ровно одна находка того же вида | старая `OBSOLETE` «continued as …», у новой `predecessor_id` |
| переименован, кандидатов нет или несколько | остаётся `OPEN` — неоднозначность ничего не закрывает |
| новая ревизия `PARTIAL` (gaps, skipped job, parser error, лимит) | `OPEN` |
| файл не проверялся заново | `OPEN` |
| правило не оценивалось заново (анализатор/правило убрано) | `OPEN` |
| правило оценено заново на новом snapshot и `SUPPORTED` везде | `RESOLVED` с obligation evidence |
| модельная находка: код изменён, unit проверен полностью, такой находки там нет | `RESOLVED` |
| модельная находка: код не изменился | `OPEN` — «нет диагностики» само по себе не исправление |

История (`finding_events`) только дополняется: состояние, причина, актор, evidence.

**Feedback** (`USEFUL | NOT_USEFUL | FALSE_POSITIVE | WONT_FIX`, scope, rationale) хранит актора и policy snapshot;
`FALSE_POSITIVE`/`WONT_FIX` делают находку `DISMISSED` с автором и причиной; повторная находка в следующей ревизии
остаётся `DISMISSED`. Evidence и статусы обязательств не меняются.

**Waivers** (rule id, paths, reason, owner, author, `expires_at` или условие `until`) записываются на repository
binding; worker передаёт действующие в review как исключения правил: они снижают решение о риске, а
`VIOLATED`/`UNKNOWN` остаются как оценены.

| Действие | Кто | Проверка |
|---|---|---|
| `/sych rerun`, `/sych deep`, `/sych stop` в PR | `write`, `maintain`, `admin` | permission API на момент scheduler; иначе `SKIPPED:UNAUTHORIZED_COMMAND`, без ответа |
| комментарий самого бота | никто | `BOT_COMMENT`, работы нет |
| feedback на finding | `triage` и выше | permission API; иначе отказ `Permission` |
| waiver | `maintain`, `admin` | permission API; иначе отказ `Permission` |
| свободный текст PR | — | не меняет policy, budget, credentials и модель |

HTTP-транспорт для feedback и waivers — M2.8.

Реальное подключение ещё не выполнено: нужен App владельца. Тесты — `cargo test -p sych-app --test server`
(подпись, лимит, content/event types, дубли, health), `--test github` (JWT → scoped token, кеш, reads, mapping) на
loopback fake, `-p sych-storage --test admission` (атомарность, coalescing, два tenants), `--test scheduler`
(supersede/fence, reaper, cancel, ожидание evidence), `-p sych-app --test scheduler` (сквозные traces на fake GitHub,
origin по `file://` и fake-модели), `-p sych-adapters --test git_fetch`, `--test ci_reports` (корпус `fixtures/ci`: валидные, повреждённые, XXE,
глубина), `--lib ci::archive` (traversal, symlink, bomb, CRC), `-p sych-app --test ci_evidence` (binding и gaps), `--test outbox` (один CREATE при потерянном ответе,
crash в `PUBLISHING`, решение оператора, пагинация и скопированный marker, потерянный close, поздний UPDATE),
`--test publication` (inline с точными полями, 422 → summary, neutral check), `-p sych-core --test publication`
(anchor corpus и golden summaries), `-p sych-core --test lineage` (lineage corpus), `-p sych-app --test findings`
(feedback/waiver с правами, `DISMISSED` переживает rerun).

## HTTP API, CLI и эксплуатация (M2.8)

Пример конфигурации — [examples/server/sych.yaml](examples/server/sych.yaml); OpenAPI — `schemas/openapi.json`,
генерируется из тех же Rust DTO (`sych_core::api`, `just schemas`).

**Роли процесса.** `sych serve` — всё в одном (personal default); `sych api` — webhooks и `/v1` без claim работы;
`sych worker` — scheduler, worker, publisher и только health/metrics. Bind по умолчанию `127.0.0.1:8080`;
non-loopback bind без `api_clients` и `behind_tls_proxy: true` отвергается валидатором, corporate без
`api_clients` — тоже. SQLite — только локальный файл одного процесса: `api` и `worker` на разных машинах требуют
PostgreSQL (M4).

**`/v1`** требует `Authorization: Bearer <token>` настроенного клиента (сравнение constant-time) и права на
repository (`connection/owner/name` или `*`); чужой repository — 404. Мутирующие запросы требуют
`Idempotency-Key` (scope tenant + actor + endpoint, TTL 24 h): повтор возвращает сохранённый ответ с
`reused: true`, тот же key с другим body — 409 `IDEMPOTENCY_CONFLICT`, незавершённый первый — 409
`REQUEST_IN_PROGRESS`. Уникальность run/publication от TTL ключа не зависит.

| Endpoint | Роль | Ответ |
|---|---|---|
| `POST /v1/reviews` | writer | 202 `{request_id, run_id, status_url, reused}` — durable admission; `publish` default `false` и включается только там, где публикует подключение |
| `GET /v1/runs/:id` | reader | state, stage, coverage, usage, ссылка на result |
| `GET /v1/runs/:id/result` | reader | immutable `ReviewResult`; до commit — 409 `NO_RESULT` |
| `POST /v1/runs/:id/cancel` | writer | активный → `CANCELLED`; committed остаётся в своём terminal state |
| `POST /v1/changes/:id/reruns` | writer | 202, новая generation по текущему состоянию PR |
| `POST /v1/findings/:id/feedback` | triage | 201; `FALSE_POSITIVE`/`WONT_FIX` → `DISMISSED` |
| `POST /v1/waivers` | maintainer | 201; scoped, с expiry или условием |
| `GET /v1/publications/:id` | reader | delivery, relevance, remote id, ошибка |
| `POST /hooks/github/:connection` | подпись webhook | см. M2.1 |
| `GET /health/live` | — | процесс жив; от модели не зависит |
| `GET /health/ready` | — | БД отвечает и artifact store пишется; без config и секретов |
| `GET /metrics` | адрес из `metrics_allow` | Prometheus text; иначе 403 |

Ошибки — `{"error": {code, message, retryable, correlation_id, details}}`: 400 `INVALID_REQUEST`, 401
`UNAUTHENTICATED`, 403 `FORBIDDEN`, 404 `NOT_FOUND`, 409 conflicts, 413, 429 `RATE_LIMITED`, 503
`DEPENDENCY_UNAVAILABLE`; без токенов, prompts, исходников и stack traces.

| CLI | Что делает |
|---|---|
| `sych review --change github:CONN:OWNER/REPO:N --config sych.yaml` | тот же server pipeline в процессе по текущему состоянию PR; ничего не публикует; exit codes как у `review` |
| `sych publish --run ID --as CLIENT --config …` | ставит summary committed run в outbox (writer); отправляет server |
| `sych cancel --run ID --as CLIENT --config …` | отмена до commit — exit 130 и `CANCELLED`; committed — exit 0 и прежний state |
| `sych rerun --change ID [--profile deep] --as CLIENT --config …` | новая generation (writer) |
| `sych reconcile [--change ID] [--allow-create OP --actor … --reason …]` | см. M2.5 |
| `sych serve` / `api` / `worker --config … [--models …] [--semantic …]` | роли процесса |
| `sych db migrate`, `sych cache prune --dry-run` | миграции только явно; план удаления без удаления |

Локальные `publish`/`cancel`/`rerun` действуют от имени настроенного клиента с нужной ролью; неизвестный клиент
или недостаточная роль — exit 4.

### Метрики и alerts

Метрики (только ограниченные метки — состояния, coverage, judgement, причины; никаких путей и id): runs по
state и coverage, `UNKNOWN` обязательства, jobs по state и возраст очереди, model attempts по состоянию ledger,
tokens, known cost, удерживаемые reservations/unknown charges, стоимость superseded runs, публикации по state,
outbox lag, возраст `UNCERTAIN`, repairs, feedback по judgement, bytes и число artifacts, webhooks
(`stored`/`duplicate`/`rejected` по причине). Правила — [examples/server/alerts.yaml](examples/server/alerts.yaml):
сбой сохранения, застрявшая очередь, неизвестные записи и charges, подписи webhooks, credentials, массовое падение
completeness, удерживаемый бюджет. Метрики tour/session добавит M5.

### Runbook personal sandbox

1. **App**: создать GitHub App на аккаунте владельца — permissions из таблицы M2.1 (Checks write — только при
   `checks: true`), events Pull request, Issue comment, Workflow run, Check suite; установить только на sandbox.
2. **Секреты** (вне репозитория и Nix store): `SYCH_GITHUB_APP_KEY_FILE` (путь к PEM), `SYCH_GITHUB_WEBHOOK_SECRET`,
   `SYCH_API_TOKEN_OWNER`, `SYCH_DATABASE_URL`, ключ модели (`SYCH_OPENROUTER_API_KEY`).
3. `sych config validate --config sych.yaml`, затем `sych db migrate --config sych.yaml` — схема меняется только
   этой командой; `serve` на немигрированной базе не стартует (exit 3).
4. `sych serve --config sych.yaml`. Проверка: `curl 127.0.0.1:8080/health/ready`, `curl 127.0.0.1:8080/metrics`.
5. **Остановка** — Ctrl-C/SIGINT: прекращается приём, затем claim; работа в полёте отдаёт lease по истечении,
   reaper следующего старта возвращает job; `PUBLISHING` без ответа становится `UNCERTAIN`, а не «доставлено».
   **Старт** возобновляет scheduler, reconcile пропущенных webhooks и reconciler публикаций.
6. **Credentials**: истёкший/отозванный App key или токен — `SCM_READ_FAILED` в `change_events` и `UNAUTHORIZED` в
   логе; заменить PEM/секрет и перезапустить. Модельный ключ — run `FAILED` с причиной; бюджет не тратится повторно.
7. **Неполный CI**: run с `CI_*` gaps — coverage `PARTIAL`, gap в summary; поздний отчёт создаёт successor сам.
8. **Неизвестные публикации**: `sych reconcile` (см. M2.5); новый CREATE — только `--allow-create` с аудитом.
9. **Хранилище**: `sych cache prune --dry-run`, затем без флага; бэкап — остановить процесс и скопировать
   `.sych-data/` целиком.

Реальный end-to-end на pet repository (форс-push, поздний CI, рестарт, rerun/feedback) ждёт App владельца;
детерминированные сценарии — тесты `-p sych-app`.

## Self-review Sych (M2.9)

Один общий engine и publisher — второго бота нет. `bondiano/sych` (и любая connection с
`environment: production`) принимается валидатором только с `self_review: true` и `reviewer_commit`.

**Доверенный reviewer.** Prompts, schemas и engine вкомпилированы в бинарник; policy берётся из target tip, а
`.sych/` из PR только сравнивается (`policy_diff`). Connection с `self_review: true` требует `reviewer_commit` —
полный commit доверенной сборки, принятой человеком; `sych serve` отказывается стартовать, если собран из другого
commit (`SYCH_BUILD_COMMIT` или `git rev-parse HEAD` при сборке). Каждый result несёт
`provenance.engine_version = <version>+<commit>`. PR не может подменить reviewer, модель, egress или бюджет:
всё это — trusted server config.

```yaml
scm:
  github:
    - id: sych-sandbox
      app_id: 123456
      private_key_file_env: SYCH_GITHUB_APP_KEY_FILE
      webhook_secret_env: SYCH_GITHUB_WEBHOOK_SECRET
      repositories: [bondiano/sych-sandbox]
      bot_login: sych-sandbox[bot]
      publish: true
      self_review: true
      reviewer_commit: "<40-hex commit of the trusted build>"
      ci_reports:
        - report: rust-tests
          workflow: .github/workflows/ci.yml
          job: test
          artifact: reports
          path: outcomes.json
          checkout_manifest: sych-ci.json
```

Repository policy зеркала (`.sych/config.yaml` на target):
`ci: {expected_reports: [{id: rust-tests, applies_to: ["crates/**", "Cargo.lock"], format: sych_manifest}]}`.

**CI** — [examples/server/sandbox-ci.yml](examples/server/sandbox-ci.yml): `pull_request`, а не
`pull_request_target`; без секретов и write-токенов; Sych PR-код не исполняет (build scripts, proc-macros и
изменённые workflows — только в CI зеркала). Отчёт привязывается как `MERGE_RESULT` через родителей
merge-коммита; без checkout manifest — `UNVERIFIED`.

**Модели.** Server run идёт в режиме `personal_service` (corporate — `corporate`): провайдер, разрешённый только
`personal_local` (подписка, в которую владелец вошёл на своей машине), в hosted route не попадает. Маршрут —
role plan M1.13; бюджеты — `budgets` server config; модельные ключи — env-ссылки (`SYCH_OPENROUTER_API_KEY`).

**Сценарии** (детерминированно в тестах `-p sych-app`; на зеркале — после подключения App владельца): force-push
и поздний CI (supersede, successor), дубль webhook, собственный комментарий бота, потерянный ответ публикации и
рестарт — без лишних inference и CREATE; устаревший summary — `STALE`; `RESOLVED` только после перепроверки; бот не
делает approve/request-changes/merge.

### Self-review bondiano/sych

Владелец включил production self-review до R1 (2026-10-27), сразу после открытия исходников: каждый PR Sych, в
том числе агентский, ревьюит сам Sych, и дефекты ревьюера видны на собственных изменениях. Бот advisory: не
approve, не request-changes, не merge. Агенты обязаны дождаться его summary на текущий head
(`scripts/wait-sych`, [AGENTS.md](../AGENTS.md)); `main` защищён — только PR с зелёным CI.

- **Образ.** [image.yml](../.github/workflows/image.yml) на push в `main` собирает
  `ghcr.io/bondiano/sych:<commit>` с `SYCH_BUILD_COMMIT`; PR образ не собирает и не публикует.
- **Pin.** Deployment в petinfra (namespace `sych`) запускает образ по digest, `reviewer_commit` — тот же
  commit; обновление ревьюера — осознанный выпуск человеком, а не merge. Server config, ключ App, webhook
  secret и ключ OpenRouter живут только там.
- **Модели.** Роли по M1.13 без local-only подписок: `reviewer`/`verifier` — DeepSeek через OpenRouter,
  `risk_auditor` и `deep_roles.verifier` — Astra с fallback на Fable; лимиты — `budgets` server config.
- **CI evidence.** Job `check` [ci.yml](../.github/workflows/ci.yml) кладёт в artifact `sych-reports` вывод
  `cargo clippy --message-format=json` (`clippy.json`), итог `just check` (`outcomes.json`) и checkout manifest
  (`sych-ci.json`); [.sych/config.yaml](../.sych/config.yaml) ждёт их до 15 минут, `ci_reports` — в petinfra.
