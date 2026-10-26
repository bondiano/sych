# Модельный review

Реализация M1.5–M1.7 roadmap: §7.3, §16.1–16.5, §17.1 [спецификации](SYCH_SPEC.md). Модель предлагает
гипотезы, оценки и вердикты; финальные статусы и findings даёт движок и детерминированный reducer M0.

Исторические SHA Sych в измерениях и примерах ниже относятся к истории до squash. Сохранённые отчёты остаются
архивным evidence; для повторения этих запусков нужен архивный клон со старой историей, не текущая ветка.

```sh
sych db migrate --config sych.yaml
sych review --repo . --base origin/main --config sych.yaml --models cheap_api --format markdown
```

`--models fake` — по-прежнему offline fake adapter. `--models roles` ведёт каждую роль через `models.roles` и её
fallback chain; любое другое значение — закреплённый профиль из `models.profiles` (с его собственной цепочкой).
Профили разрешаются так же, как `doctor --probe-model` ([MODELS.md](MODELS.md)): режим, egress allowlist, price
card, credentials и pinned runtime. Transports: OpenRouter, OpenCode Go, GLM Coding Plan через Pi, ChatGPT plan
через Sign in with ChatGPT; router и роли проходов — [MODELS.md](MODELS.md#router-роли-fallback-и-premium-m113).
`--fake-responses` с профилем не сочетается.

## Проходы

Все вызовы идут через общий gateway и ledger одного run: каждый attempt резервирует деньги и токены до
отправки, бюджеты `budgets.analysis` (attempts, tokens, $, deadline) общие для всех проходов и repair.

1. **Assessment** — сначала required, затем по убыванию риска; это и есть резерв verification: discovery не
   может израсходовать бюджет, нужный обязательствам. Если discovery batch (п. 3) показывает verification packet
   обязательства целиком (каждый его span лежит внутри spans batch, ничего обязательного не исключено), оценка
   идёт в том же вызове, что и discovery: `review@4` возвращает `assessments` и гипотезы, и такие вызовы
   выполняются первыми. Для маленького PR это один вызов вместо assessment + discovery. Остальные обязательства
   получают по отдельному `assessment@3`. Assessment, которого нет в ответе `review@4`, — `UNKNOWN/MODEL_FAILED`
   («ob-N was not assessed»). Обязательство, у которого обязательный span не поместился в packet
   (`REQUIRED_OVER_BUDGET`), вызова не получает и остаётся `UNKNOWN/CONTEXT_LIMIT`.
2. **Risk audit** (`risk_audit@4`) — для обязательств риска HIGH/CRITICAL отдельный поиск по verification packet
   обязательства. Он не видит вывод discovery и выполняется даже при пустом discovery; это не обещание
   статистической независимости ошибок.
3. **Discovery** (`discovery@5`) — по discovery packets review units со списком их обязательств. Соседние
   packets объединяются в один вызов, пока их tokens помещаются в `context.max_tokens_per_request`: для
   маленького PR это один discovery вместо вызова на каждый unit.
4. **Verify** (`verify@1`) — вердикт каждому кандидату прохода, на тех же packets плюс ответы на context
   requests.

Проход, который не выполнился (budget, deadline, truncation, ошибка provider, schema после repair), записан в
`analysis` artifact как `FAILED` и попадает в `unresolved_questions`; coverage становится `PARTIAL`
(`LIMIT_REACHED` для бюджета и output cap, `RETRIES_EXHAUSTED` для остального: router уже прошёл все eligible
routes цепочки).

## Nits

Правила discovery для мелких замечаний взяты из [исследования review-комментариев](research/review-comments.md):
nit не выше `LOW`, не меняет поведение и называет место, предотвращаемую ошибку и правку; сильнейшие — согласованность
с уже существующим в repository; ничего из того, что ловят форматтеры, линтеры и компилятор; удаление лучше новой
абстракции; performance — только с аргументом о горячем пути; не больше 3 nits на review unit; при сомнении —
вопрос о конкретном сценарии вместо утверждения.

## Что видит модель

Prompt собирается из доверенных слоёв: роль → output contract → правила → задача; accepted rules target tip —
доверенный текст задачи. Исходный код, gaps packet и тексты гипотез — только в DATA-блоках с fence из hash
содержимого. Каждый span packet показан с номерами строк под локальным id `ref-N`; evidence — `ev-N` (`ev-1` —
source spans этого prompt), обязательства — `ob-N`, критерии — `c-N`, кандидаты — `cand-N`. Модель не пишет
пути, hash или evidence ids: anchor — это `{ref_id, start_line, end_line}` внутри выданного span, и движок
сам строит `CodeRef` через snapshot reader с проверкой hash.

Source spans packet сохраняются как derived evidence `source` (`SOURCE_OBSERVATION`): `COMPLETE_FOR_QUERY` для
ровно этих spans, если обязательный span не исключён, иначе `PARTIAL`. Gaps packet — его limitations.

## Проверки движка

- **Discovery** ([`analysis/discovery.rs`](../crates/sych-core/src/analysis/discovery.rs)): кандидат с хотя бы
  одной выдуманной ссылкой (`ref`, строки вне span, `ev`, `ob`) или без anchor отклоняется целиком, а не
  исправляется; схема требует хотя бы один anchor (`minItems: 1`), чтобы constrained decoding upstream не отдавал
  пустой массив. Больше 20 кандидатов — `CANDIDATE_OVERFLOW`: лишние отбрасываются, это gap и `LIMIT_REACHED`
  (coverage `PARTIAL`); больше 4 context requests — отклонены по счёту. `unresolved_questions` модели попадают
  в `unresolved_questions` результата как `open question: …`, не меняя execution outcome и coverage.
- **Context requests** — закрытый enum `read_lines | find_references | find_callers | find_callees |
  find_implementations | search_source` без hash и команд; переводится в typed operation broker (§9.4), отвечает `Ops` по pinned
  snapshot (fixed-string search до 5 s, secret paths не читаются, до 3 spans на ответ). Ответ добавляется в packet
  прохода verify как новый `ref-N` с причиной `answer to: …`.
- **Redaction**: значения секретов во всех DATA-блоках (spans, ответы broker, тексты кандидатов) заменяются
  `[REDACTED]` при рендере prompt (`Prompt::messages`), изменённый блок помечается `[secret values redacted]`;
  `CodeRef`, hashes и номера строк остаются исходными.
- **Assessment** ([`analysis/assess.rs`](../crates/sych-core/src/analysis/assess.rs)): у каждого критерия
  правила (`acceptance.criteria` или statement) должен быть результат; `MET/NOT_MET` без anchor становится
  `UNKNOWN`. Ответ про чужой `ob-N`, неизвестные `ref`/`ev`/`c-N` или строки вне span дают `UNKNOWN`
  `INVALID_REFERENCE` при любом предложенном статусе: движок не чинит доказательство удалением ссылок. `SUPPORTED` — только если все критерии `MET`; `NOT_MET` при `SUPPORTED` —
  `CONTRADICTORY_ASSESSMENT`; все anchors в тестовых файлах — `TEST_ORACLE_ONLY` (тест того же изменения не
  заменяет accepted requirement); `VIOLATED` — только с `NOT_MET`; `NOT_APPLICABLE` — только с basis. Затем
  reducer M0 ещё раз проверяет evidence kinds, validity и полноту. Отказ модели, бюджет или schema после
  repair — `UNKNOWN` с `MODEL_FAILED`/`BUDGET_EXHAUSTED` и следующим нужным evidence.
- **Verify** ([`analysis/verify.rs`](../crates/sych-core/src/analysis/verify.rs)): кандидат без единственного
  вердикта — `UNRESOLVED`; `CONFIRMED` с хотя бы одной нераспознанной ссылкой (`ref`, строки вне span, `ev`) —
  тоже `UNRESOLVED`. `CONFIRMED` принимается, если есть anchor и основание допустимо:
  `EXECUTED_COUNTEREXAMPLE` невозможен (ничего не исполнялось), `PREFERENCE` не поддерживает defect,
  nit/improvement не выше `LOW`, `HIGH/CRITICAL` требует `STATIC_TRACE` минимум из двух anchors
  (`HIGH_RISK_WITHOUT_TRACE`). Anchor на строке, где уже есть импортированный diagnostic, оставляет кандидата
  `UNRESOLVED` (`DUPLICATES_EXISTING_DIAGNOSTIC`): без сопоставимого baseline новизна не доказана. Отклонённый
  nit уходит в `rejected_candidates`, не повышается.
- **Findings**: severity, support и verification status — разные поля; у каждого finding limitation
  «CONFIRMED by the verification policy on the shown spans; not a proof of the whole program». Findings разных
  проходов с пересекающимися anchors сливаются в самый серьёзный с пометкой `merged an overlapping hypothesis`.
  `VIOLATED` обязательство без связанного finding даёт rule violation finding reducer M0.
- **Основание и baseline.** Виды basis: `STATIC_TRACE` (цепочка anchors от изменения к последствию),
  `SEMANTIC_ARGUMENT` (рассуждение по показанным spans), `EXPLICIT_RULE` (accepted rule), `PREFERENCE` (только
  nit/improvement); `EXECUTED_COUNTEREXAMPLE` появится с исполнением M3.8. Новизна и `RESOLVED` решаются общим
  baseline/revalidation M2 по fingerprint; здесь второго diagnostic lifecycle нет — без baseline совпадение с
  существующим diagnostic остаётся `UNRESOLVED`, а исчезновение diagnostic ничего не закрывает.

## Artifacts и usage

Run сохраняет artifact `analysis`: профиль и модель, проходы (template, packet, обязательство, статус, attempt
ids), записи assessment (obligation → criteria → anchors/evidence, rationale, missing inputs, что движок не
принял), discovery (принятые и отклонённые черновики, context requests и ответы), кандидаты с их статусами и все
attempts (reserved, charge, charge state, tokens, resolved model, upstream, generation id). Prompt text и
chain-of-thought не сохраняются. `usage_summary` результата считается по attempts:

- tokens in/out и `reasoning_tokens` (часть output, не прибавляется); attempts без reported usage —
  `unknown_usage_attempts`, в суммы tokens не входят;
- `cost.known` — фактические деньги (reported или по price card), `cost.reference_estimate` — оценка по
  прайсу, не charge; неурегулированный charge — `unknown_cost_attempts`, не ноль;
- `routes[]` — profile/model, `billing` и `enforcement` (`api_hard` / `subscription_observable`), число
  attempts, `quota_attempts` (подписка без reported money: израсходована включённая квота, не бесплатно) и
  `fixed_fee` месяца (`period`, `amount`, `allocated`) — fee учитывается в месячном пуле один раз, не в
  стоимости run. Остаток квоты и reset пока не сообщает ни один подключённый transport.

`provenance.model_adapters` — `profile:model`.

## Проверки

Реальные snapshots (`cheap_api`, бюджеты personal примера): Vkusori `f1cec4d14` (TS, один файл, fix) —
discovery + verify, один nit отклонён, findings нет, ~$0.002; Sych `6b134cb` (Rust, 8 файлов, 5 units) — 3
discovery в batches и verify, 1 кандидат `REJECTED`, 1 `UNRESOLVED` (тело нового метода не показано),
`COMPLETE_FOR_SCOPE`, ~$0.006. Это проверка пути, а не измерение качества (M1.8).

- `cargo test -p sych-core --test analysis` — validators: выдуманные ids, overflow, критерии, противоречия,
  test oracle, high-risk без trace, nit, дубль diagnostic, слияние.
- `cargo test -p sych-app --test analysis` — проходы на настоящем snapshot со scripted provider: безопасное
  изменение с пустым discovery получает `SUPPORTED`; проза дважды — `MODEL_FAILED`, gaps, без извлечения из
  текста; `max_attempts: 1` — резерв уходит обязательству, остальное `LIMIT_REACHED`; dispatched timeout —
  `UNKNOWN` charge; не dispatched `UNSUPPORTED` — `RELEASED` и `MODEL_FAILED`, не пустой успех; context
  request `read_lines` отвечается через broker и виден verifier как `answer to: …`; маленький PR —
  `review@4` вместо assessment + discovery, а обязательство, которое ни один batch не показывает целиком,
  получает отдельный assessment; `review@4` без assessment оставляет `UNKNOWN/MODEL_FAILED`.
- Реальный smoke `cheap_api` (DeepSeek V4.1 Flash, `reasoning_effort: low`) на паре unsafe/safe через
  `review@2`: удалённый guard — `VIOLATED` и подтверждённый finding; замена guard на `?? []` — `SUPPORTED`, без
  findings; по 3 вызова (review, risk audit, verify), ~$0.0015 за run ([пример](examples/model-review/)).

- `sych eval measure` ([LOCAL_REVIEW.md](LOCAL_REVIEW.md)) на четырёх парах M1.7 — перенос cleanup и invalidation
  в project hook, stale-response защита, invalidation — `cheap_api`, по 2 прогона:
  [отчёт](examples/quality/m1.7-pairs-cheap_api.json). Размеченные defects найдены в 8 из 8 unsafe-прогонов, все
  findings на размеченных местах, на 8 safe-прогонах correctness findings нет (3 nit), ~$0.023 за 16 review.
  До `minItems: 1` у `anchors` кандидата (`discovery@3`, `review@2`, `risk_audit@2`) было 3 из 8: DeepSeek в
  половине ответов называл дефект с пустыми `anchors`, и движок отклонял черновик (`no anchor in the shown
  spans`). Это малая выборка разработки, не оценка качества (M1.8).

- Полный development suite (48 кейсов, включая реальные коммиты Sych и Vkusori), `cheap_api`, по одному прогону:
  вариант C (обычный pipeline) — [отчёт](examples/quality/dev-cheap_api-c.json), вариант B без high-risk audit —
  [отчёт](examples/quality/dev-cheap_api-b.json).

  | | recall | precision | FP на safe | unknown остался UNKNOWN | attempts | стоимость |
  |---|---|---|---|---|---|---|
  | C | 19/22 | 19/27 | 6/25 | 0/3 | 107 | $0.078 |
  | B | 20/22 | 20/27 | 5/25 | 0/3 | 104 | $0.084 |

  На одном прогоне B и C в пределах шума: high-risk audit здесь не добавил найденных defects. Слабые места:
  `unknown`-кейсы модель закрывает уверенным ответом (в т.ч. `VIOLATED` там, где решения нет), а на чистых
  изменениях даёт побочные correctness-замечания. Это development suite, не оценка качества: holdout и критерии
  — owner (M1.8).

- Security-пары (path traversal, SQL/command injection, XSS, SSRF, open redirect, unbounded read; 6 TS, 4 Rust,
  4 Go) и 16 nit-кейсов из [исследования](research/review-comments.md), `cheap_api`, до и после правил для nits:

  | | defects найдены | correctness findings на safe security | всего correctness findings | nits попали в разметку |
  |---|---|---|---|---|
  | до | 12/14 | 10/14 | 25 + 5 на nit-кейсах | 5/15 |
  | после | 12/14 | 8/14 | 20 + 3 на nit-кейсах | 5/15 |

  Отчёты: [до (security)](examples/quality/security-cheap_api-before-nit-rules.json),
  [до (nits)](examples/quality/nits-cheap_api-before-nit-rules.json),
  [после](examples/quality/security-and-nits-cheap_api.json). Go cases пока смотрят только diff: Go semantic/syntax —
  M3, их результаты не засчитываются как поддержка Go. Часть «ложных» замечаний на safe-кейсах содержательна (symlink в
  обход prefix-check, LIKE-метасимволы, клиент без timeout); две фикстуры после замера исправлены — модель нашла в
  них настоящие ошибки (неимпортированный `formatPrice`, удалённый маршрут `PUT /me/avatar`).

## Baselines A/B/C на development suite (M1.8)

Development suite — 103 кейса (`eval/quality/dev`: Rust, TS/React и Go safe/unsafe/UNKNOWN пары, routing/billing/
privacy тройки на Rust, clean, nitpick и реальные коммиты Sych и Vkusori), один прогон каждого варианта,
`cheap_api` (DeepSeek V4.1 Flash через OpenRouter), одинаковые budgets personal примера:

```sh
sych eval measure --suite eval/quality/dev --config examples/personal/sych.yaml --models cheap_api \
  --variant c|b|a --repo sych=PATH --repo vkusori=PATH --output report.json
```

| | defects найдены (recall) | correctness findings на размеченных местах (precision) | safe-кейсы с correctness finding | UNKNOWN остался UNKNOWN | obligations совпали | nits в разметке | attempts | стоимость |
|---|---|---|---|---|---|---|---|---|
| C — pipeline целиком | 34/39 | 39/61 | 22/58 | 2/6 | 10/13 | 4/16 | 226 | $0.168 |
| B — без high-risk audit | 30/39 | 33/51 | 15/58 | 2/6 | 11/13 | 7/16 | 204 | $0.164 |
| A — один reviewer по diff, без semantic, audit и verifier | 39/39 | 59/113 | 34/58 | 0/6 | 10/13 | 13/16 | 109 | $0.078 |

Отчёты: [C](examples/quality/dev-cheap_api-c.json), [B](examples/quality/dev-cheap_api-b.json),
[A](examples/quality/dev-cheap_api-a.json). Стоимость — весь review, включая отклонённые кандидаты, а не только
удачные findings.

Что видно на этом одном прогоне (не оценка качества — критерии принятия задаёт владелец):

- A находит все размеченные дефекты, но ценой шума: 113 correctness-замечаний вместо 61, ложные на 34 из 58
  safe-кейсов и ни одного сохранённого `UNKNOWN` — непроверенные гипотезы модели выдаются за ответы. Verifier и
  контекст C режут шум (precision 64% против 52%, FP на safe 38% против 59%) и теряют 5 дефектов.
- B против C: audit добавил 4 найденных дефекта и 6 размеченных findings, но и 7 safe-кейсов с замечанием.
  Разница одного прогона близка к вариативности (см. повторы ниже).
- `UNKNOWN`-кейсы остаются слабым местом всех вариантов: C/B держат 2 из 6 (обе — новые routing/billing тройки),
  остальные модель закрывает уверенным ответом.
- Go-кейсы смотрят только diff (Go semantic — M3) и в успех M1 не засчитываются; holdout ещё не размечен владельцем.

**Повторы** спорных кейсов (C, по 3 прогона, [отчёт](examples/quality/repeats-cheap_api-c.json), $0.069):
`rust-async-cancellation-unsafe` — найден 2 из 3; `rust-lock-across-await-safe` — ложное замечание 2 из 3;
`react-stale-response-safe` — ложное 3 из 3; `rust-paid-fallback-unknown` и `rust-unknown-charge-unknown` — UNKNOWN
сохранён 3 из 3; `rust-log-secrets-unknown` и `rust-result-propagation-unknown` — закрыты 3 из 3;
`real-sych-context-broker` — без замечаний 3 из 3.

## Rust по категориям (M1.14)

Те же прогоны, Rust-пары (48 кейсов): ячейка — «найден в unsafe · замечание на safe · UNKNOWN сохранён».

| pair | C: unsafe · safe · unknown | B: unsafe · safe · unknown | A: unsafe · safe · unknown |
|---|---|---|---|
| clean-rust-extract-function | — · ok · — | — · ok · — | — · ok · — |
| nit-rust-atomic-ptr | — · ok · — | — · ok · — | — · FP · — |
| nit-rust-history-comment | — · ok · — | — · ok · — | — · FP · — |
| nit-rust-inline-helper | — · ok · — | — · ok · — | — · ok · — |
| nit-rust-must-use | — · FP · — | — · ok · — | — · FP · — |
| nit-rust-reread-in-loop | — · ok · — | — · ok · — | — · ok · — |
| nitpick-doc-typo | — · ok · — | — · ok · — | — · ok · — |
| real-sych-context-broker | — · ok · — | — · ok · — | — · FP · — |
| rust-async-cancellation | 1/1 · FP · — | 0/1 · FP · — | 1/1 · ok · — |
| rust-blocking-in-async | 1/1 · ok · — | 1/1 · ok · — | 1/1 · ok · — |
| rust-cfg-feature | 1/1 · ok · — | 0/1 · ok · — | 1/1 · FP · — |
| rust-cfg-target | — · — · closed | — · — · closed | — · — · closed |
| rust-command-injection | 1/1 · FP · — | 1/1 · FP · — | 1/1 · FP · — |
| rust-error-handling-weakened | 1/1 · ok · — | 1/1 · ok · — | 1/1 · FP · — |
| rust-lock-across-await | 1/1 · ok · — | 1/1 · ok · — | 1/1 · FP · — |
| rust-log-secrets | 1/1 · ok · closed | 1/1 · ok · closed | 1/1 · FP · closed |
| rust-paid-fallback | 1/1 · ok · kept | 1/1 · ok · kept | 1/1 · ok · closed |
| rust-path-traversal | 1/1 · ok · — | 0/1 · ok · — | 1/1 · ok · — |
| rust-raii-guard | 1/1 · ok · — | 1/1 · ok · — | 1/1 · ok · — |
| rust-result-propagation | 1/1 · ok · closed | 1/1 · ok · closed | 1/1 · FP · closed |
| rust-serde-contract | 0/1 · ok · — | 0/1 · ok · — | 1/1 · FP · — |
| rust-sql-injection | 1/1 · FP · — | 1/1 · FP · — | 1/1 · FP · — |
| rust-task-lifetime | 1/1 · FP · — | 1/1 · FP · — | 1/1 · FP · — |
| rust-test-ignored | 1/1 · — · — | 1/1 · — · — | 1/1 · — · — |
| rust-trait-api | 1/1 · ok · — | 1/1 · ok · — | 1/1 · ok · — |
| rust-unbounded-read | 0/1 · FP · — | 1/1 · FP · — | 1/1 · FP · — |
| rust-unknown-charge | 1/1 · ok · kept | 1/1 · ok · kept | 1/1 · ok · closed |

Rust, вариант C: 16 из 18 дефектов найдены (пропущены serde-контракт и unbounded read), ложные correctness-замечания
на 6 из 25 safe/clean кейсов (cancellation, command/SQL injection, task lifetime, unbounded read, must-use nit),
`UNKNOWN` сохранён в 2 из 5 (routing и billing тройки; макрос, cfg target и `Debug` чужого типа модель «решает»).
Error propagation, RAII, lock/blocking через `.await`, trait/API и cfg-feature найдены во всех вариантах, кроме
одиночных промахов B. Это измерение, не доказательство «хорошо ревьюит Rust»: holdout не размечен, прогон один.

**После починки фикстур.** В том замере у 13 Rust-фикстур не было `src/lib.rs`, а у serde-пары — зависимости
`serde`: rust-analyzer не загружал workspace, и semantic facts этих кейсов были `FAILED`. Фикстуры исправлены, safe-
кейсы получили разметку `tolerated` (верное, но не целевое замечание: брошенный stop sender, обязательное поле
`branch`, LIKE-метасимволы, detached flush, 413 на любую ошибку `to_bytes`). Затем — те же 48 кейсов, вариант C,
`cheap_api`, по два прогона: текущие промпты и две попытки правил severity/`UNRESOLVED` (вторая — один прогон).

| | текущие промпты ×2 ([отчёт](examples/quality/rust-cheap_api-c-fixed-fixtures.json)) | правила v1 ×1 ([отчёт](examples/quality/rust-cheap_api-c-severity-rules-v1.json)) | правила v2 ×2 ([отчёт](examples/quality/rust-cheap_api-c-severity-rules-v2.json)) |
|---|---|---|---|
| дефекты найдены | 31/36 (86%) | 12/18 (67%) | 32/36 (89%) |
| precision | 71% | 54% | 65% |
| safe-runs с любым correctness finding | 10/50 | 8/25 | 14/50 |
| из них ложных (не `tolerated`, не завышенный nit) | 1/50 | 5/25 | 4/50 |
| `UNKNOWN` сохранён | 5/10 | 2/5 | 4/10 |
| nits в разметке | 9/12 | 5/6 | 8/12 |
| стоимость | $0.169 | $0.094 | $0.183 |

Главный эффект дали фикстуры, а не промпты. Правило «`DEFECT` только с вредным последствием» и строгий
`UNRESOLVED` (v1) оставили непроверенными найденные discovery дефекты (SQL injection, path traversal, cancellation,
lock across `.await`, cfg feature), а правило о публичной сигнатуре дало новое ложное «сломает вызывающий код» на
четырёх safe-кейсах; v2 без него вернул recall, но ложных замечаний больше, чем у текущих промптов. Обе попытки
откатаны. С текущими промптами пропущены async cancellation (2/2), serde-контракт (2/2: discovery находит,
verifier оставляет `UNRESOLVED`) и unbounded read (1/2); единственное ложное замечание — счётчик hits до `.await` в
`rust-lock-across-await-safe`; `UNKNOWN` теряется в cfg target, result propagation и log secrets. `tolerated`
сопоставляется по строкам, поэтому заголовки этих замечаний проверены вручную: ни одно не утверждает запрещённое
(«wildcard injection» — про LIKE, не SQL injection). Holdout по-прежнему не размечен.

**Фикстуры v2 и движок.** В трёх пропущенных парах не было кода, на котором держится дефект: тела `read_frame`
(async cancellation), публичного маршрута с `OrderView` (serde-контракт), роутера без лимита тела (unbounded read).
Он добавлен в `base` обеих фикстур пары, разметка не менялась. Затем движок: изменённые `select!`/serde/cfg/
нераскрытые макросы подписаны на своём span; verifier видит обязательства фрагмента и один раз запрашивает
контекст; код под неактивным cfg, нераскрытый макрос и `Debug` типа вне snapshot — gaps, при которых обязательство
не бывает `SUPPORTED`, а `VIOLATED` только на таком коде становится `UNKNOWN`; вызовы идут по
`max_parallel_calls`. Те же 47 кейсов (без `real-sych-context-broker`: прогон без `--repo sych=…`), вариант C,
`cheap_api`, по два прогона:

| | фикстуры v2, движок до ([отчёт](examples/quality/rust-cheap_api-c-fixtures-v2.json)) | фикстуры v2, движок после ([отчёт](examples/quality/rust-cheap_api-c-engine-v2.json)) |
|---|---|---|
| дефекты найдены | 33/36 (92%) | 35/36 (97%) |
| precision | 64% | 69% |
| safe-runs с любым correctness finding | 18/48 | 15/48 |
| из них ложных | 6/48 | 4/48 |
| `UNKNOWN` сохранён | 4/10 | 8/10 |
| обязательства совпали с разметкой | 22/26 | 25/26 |
| nits в разметке | 7/12 | 6/12 |
| attempts / стоимость | 225 / $0.176 | 279 / $0.203 |

Фикстуры вернули serde-контракт и async cancellation (2/2 уже до движка). Движок добавил стабильность: unbounded
read, trait/API и SQL injection найдены в обоих прогонах, `UNKNOWN` теперь держится в result propagation и log
secrets (2/2, было 0/2). Пропущен один дефект — cfg feature в одном прогоне. В cfg target `UNKNOWN` теряется на
верном, но не размеченном замечании «каталог конфигурации на Windows сменился без миграции» — решение о
`tolerated` за владельцем. Ложные замечания: error-handling-weakened-safe (2/2), lock-across-await-safe и path-traversal-safe
(по 1/2). Второй раунд verifier и gaps стоят +24% attempts и +15% денег. Holdout не
размечен, по два прогона на кейс — это измерение development suite, не доказательство качества.

**Реальный review Sych** — коммит `26cc0f4` (verification reserve, 6 файлов) против `81ff2a7`, `cheap_api`,
semantic через pinned rust-analyzer:
[result](examples/rust-review/sych-26cc0f4.result.json), [markdown](examples/rust-review/sych-26cc0f4.md).
Coverage `PARTIAL`, recommendation `NEEDS_EVIDENCE`; findings нет, 2 кандидата отклонены verifier'ом, один
`UNRESOLVED` (контракт `router.search` не показан); `TEST-INTEGRITY` — `UNKNOWN/TEST_ORACLE_ONLY`; обязательный
контекст сверх бюджета назван (`REQUIRED_OVER_BUDGET` для router.rs и docs). 4 attempts, $0.0068;
`provenance.engine_version` — `0.1.0+<commit сборки>`. Команды исходного запуска:

```sh
SYCH_DATABASE_URL=… sych db migrate --config examples/personal/sych.yaml
sych review --repo . --base 81ff2a7 --head 26cc0f4 --config examples/personal/sych.yaml --models cheap_api
```

Поддержка ограничена тем, что видят rust-analyzer и syn на pinned commits без исполнения build scripts и
proc-macros: нераскрытые макросы, cfg вне активных features и dynamic dispatch дают gaps, а не доказанное множество.

## Ограничения

- Независимые проходы идут параллельно, не больше `budgets.analysis.max_parallel_calls` (по умолчанию 2) вызовов
  сразу, в две стадии: сначала все вызовы, оценивающие обязательства (обязательные первыми), затем risk audit,
  discovery и verify. Части результата сливаются в порядке плана, так что итог не зависит от того, какой вызов
  вернулся первым; ledger резервирует каждый attempt атомарно, поэтому общий бюджет run соблюдается.
- Context expansion — один раунд на проход: ответы на запросы discovery видит verify, discovery повторно не
  вызывается. Verifier сам может один раз запросить контекст: кандидаты, оставшиеся `UNRESOLVED`, проверяются
  второй раз с ответами; если на второй вызов нет бюджета, остаются вердикты первого.
- Output schema передаётся без `$ref` (все subschemas inline): со ссылками upstream DeepSeek возвращал пустые
  `anchors`, и движок отклонял все гипотезы.
- Reasoning-модели тратят ~1 000–2 400 tokens каждого ответа на reasoning: при §17.1 default 2 000 tokens на
  request discovery и verify обрезаются (`TRUNCATED`). Personal пример поднимает
  `budgets.analysis.max_output_tokens_per_request` до 4 000 и `max_output_tokens` до 24 000; денежный cap
  run ($0.30) не меняется.
- Модель недетерминирована: тот же run может дать `UNKNOWN`, если она не поставила anchor критерию; это
  ожидаемая строгость, а не ошибка.
