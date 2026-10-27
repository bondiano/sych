# Sych / Сыч

Инструмент ревью с детерминированным Rust-ядром и ограниченными модельными вызовами.
Источник требований — [SYCH_SPEC.md](docs/SYCH_SPEC.md).
План реализации с зависимостями, Acceptance Criteria и Definition of Done — [SYCH_ROADMAP.md](docs/SYCH_ROADMAP.md).

## Текущий статус

Подготовлены Git-репозиторий, Rust workspace, CLI с `--help`/`--version` и quality-гейты.
Выполнены:

- M0.1 — доменные контракты в `sych-core` (IDs, Git OID с алгоритмом, SHA-256 и canonical JSON, micro-USD,
  `schema_version`, DTO snapshot/policy/evidence/obligation/finding/result) и JSON Schema из тех же типов;
- M0.2 — безопасный Git snapshot в `sych-adapters`: отдельно закреплённые `target_tip`, `diff_base`, `head`,
  полный diff manifest (rename, удаления, Unicode/не-UTF-8, binary, generated, symlink, submodule, LFS,
  слишком большие blobs — с явными limitations) и CodeRef, проверяемый по blob и hash строк;
- M0.3 — типизированные `sych.yaml` и `.sych/*` (неизвестные keys — ошибка с точным путём), effective config
  с hash и provenance, PolicySnapshot только из target tip, `policy_diff` для head без применения,
  default-deny secret paths и redaction, `sych config init --personal | validate` и `sych doctor`.
  Подробности — [docs/CONFIG.md](docs/CONFIG.md), примеры — `examples/personal/`;
- M0.4 — детерминированный planner (review units по каталогам, risk hints, exclusions с причинами,
  obligation на каждое invariant-правило с трёхзначной applicability; лимит `max_source_files` перечисляет
  непроверенные файлы), freeze ExternalInputManifest/EvidenceManifest (позднее внешнее evidence отклоняется)
  и ResultReducer: SUPPORTED требует VALID complete evidence допустимого вида не только от автора,
  VIOLATED без finding превращается в RULE_VIOLATION, recommendation/coverage вычисляются по §5.7;
- M0.5 — `sych-storage`: SQLite (WAL, один writer), content-addressed artifacts с проверкой hash,
  атомарный commit указателя на result, `sych db migrate`, `sych show --run`, `sych cache prune [--dry-run]`
  с retention 7/30 дней и защитой активных runs. Подробности — [docs/STORAGE.md](docs/STORAGE.md);
- M0.6 — законченный локальный путь `review → сохранённый JSON/Markdown → show/explain`: `--format json|markdown`,
  `--output`, `--fake-responses` (evidence и assessments fixture, CodeRef вычисляет harness), `--exit-policy strict`
  (10 — значимое нарушение, 11 — неполный scope), отмена по Ctrl-C (130, run `CANCELLED`),
  `sych explain --run --obligation` и `sych eval run --suite eval/smoke.yaml --models fake`.
  Руководство — [docs/LOCAL_REVIEW.md](docs/LOCAL_REVIEW.md), примеры вывода — `docs/examples/local-review/`.

В работе M1:

- M1.3 (детерминированная часть) — `sych_core::model`: typed `ModelProvider`/`Route`/`Capabilities`, prompt-слои
  с изолированными data blocks, strict structured output с одним repair, денежная upper bound до dispatch и
  SQLite ledger (атомарная reservation run/day/month, идемпотентный settlement, `UNKNOWN_CHARGE`, fixed fee
  подписки раз в месяц, billing mismatch). Usage summary результата разделяет фактические деньги, оценку по прайсу,
  reasoning tokens, attempts на включённой квоте и fixed fee месяца. Подробности — [docs/MODELS.md](docs/MODELS.md).
- M1.9 — OpenRouter adapter с pinned model/upstreams, без hidden fallback, `require_parameters` и
  `data_collection: deny`, streaming, типизированными ошибками и точным cost; opt-in `sych doctor --probe-model
  PROFILE` печатает accounting record. Реальные probes DeepSeek V4.1 Flash, Astra и Fable пройдены.
- M1.10–M1.12 (подписки) — ChatGPT plan через Sign in with ChatGPT (`sych auth chatgpt`, text-only Responses),
  GLM Coding Plan через pinned Pi CLI в изолированном процессе, OpenCode Go по HTTP; quota/allowance отдельно от
  денег, overage подтверждается владельцем. Fixtures и loopback smokes есть, реальные subscription smokes ждут
  аккаунтов владельца. M1.13 — `--models roles`: роли проходов, ограниченный fallback с cooldown в одном budget,
  premium через `risk_auditor`. Подробности — [docs/MODELS.md](docs/MODELS.md).
- M1.1/M1.14 (semantic backends, первая часть) — `sych semantic` собирает definitions, references, callers и
  implementations изменённых конструкций для base и head раздельно через pinned rust-analyzer и TypeScript 7
  `tsc --lsp`; Sych сам скачивает, проверяет SHA-256 и кеширует серверы по своему манифесту. Подробности и
  ограничения — [docs/SEMANTIC.md](docs/SEMANTIC.md). Добавлены path classes, pull diagnostics и Cargo
  editions/features.
- M1.2 (ContextBroker) — `sych review` превращает semantic facts в derived evidence и строит context packets
  (§9.1–9.5): exact `CodeRef`, reason/priority, completeness, links, исключённое и gaps, лимиты §9.3 и
  `UNKNOWN/CONTEXT_LIMIT` для обязательного контекста за бюджетом; callers, callees, imports и tests; typed
  operations §9.4 с поиском, ограниченным по времени, и semantic cache §9.6.
- M1.4 (правила) — generic rule pack §11.4 по профилям, трёхзначная применимость с переносом и semantic links,
  waivers без переписывания статуса, `--policy-ref` для явного run с новой policy. Подробности —
  [docs/CONFIG.md](docs/CONFIG.md).
- M1.5–M1.7 (модельный review) — `sych review --models PROFILE`: assessment обязательств (для маленького PR вместе с
  discovery в одном вызове), high-risk audit, discovery с typed context requests и verifier; финальные статусы даёт
  движок. Подробности — [docs/ANALYSIS.md](docs/ANALYSIS.md).
- Syntax facts (M1.1/M1.14) — syn и oxc разбирают изменённые Rust и TS/React файлы: guards через `.await`,
  detached tasks, discarded results, cfg/serde/public API; effects и cleanup, listeners/timers, stale async,
  mutation/invalidation, controlled inputs, form submit, project hooks со ссылкой на определение.
- CI facts (M1.14) — `sych review --ci-facts FILE --ci-manifest FILE` импортирует cargo/clippy JSON и JUnit как
  external input с binding к tested commit; локальный импорт остаётся author claim.
- Isolated tooling profile (M1.1/M1.14) — `tooling.dependency_install` ставит зависимости pinned pnpm и
  `cargo fetch` в bwrap или docker (fallback и `fail|degrade` настраиваются), без scripts и host credentials;
  `.cargo/config*`, `rust-analyzer.toml` и toolchain `path` из repository не исполняются.
- M2.1 (GitHub App, начало) — `sych serve`: `POST /hooks/github/:connection` с проверкой подписи по raw body,
  лимитом 2 MiB, content/event types и атомарной записью receipt + binding + intent; повтор delivery распознаётся,
  bot comments не планируют работу; App JWT → token на один repository, capabilities из permissions, mapping
  ошибок v1. M2.2 — scheduler (решение по API state: same inputs, supersede, draft, команды с правами, close,
  CI wake/successor), durable jobs с lease/fence, reaper, reconcile пропущенных webhooks, fetch pinned commits в
  mirror. M2.3 — CI evidence по trusted mapping (точные run/attempt/job/artifact), безопасный zip в памяти,
  SARIF/Code Quality/LCOV/Cobertura/manifest, binding `SOURCE_HEAD`/`MERGE_RESULT`/`UNVERIFIED` и конкретные gap-коды.
  M2.4 — fenced финализация (frozen inputs, lease, generation, bytes до pointer) вместе с job и outbox intent,
  fault-матрица, retention записей и mirrors без удаления unknown charges/незавершённых публикаций, тест апгрейда
  схемы ([docs/STORAGE.md](docs/STORAGE.md)). M2.5 — durable outbox: `PUBLISHING` до сети, `UNCERTAIN` вместо
  слепого повтора, сверка по id/marker только комментариев бота, gates (generation, открытый PR), починка позднего
  UPDATE, `sych reconcile` с аудируемым разрешением нового CREATE. M2.6 — summary из immutable result (coverage,
  проблемы, UNKNOWN, свёрнутые nits, стоимость), до 8 inline на ревизию по explicit commit/path/line/side, 422 →
  summary, neutral check run по желанию. M2.7 — lineage findings (RESOLVED только после сопоставимой перепроверки,
  rename не закрывает), feedback и waivers по правам репозитория. M2.8 — `/v1` (identity, repository scope,
  idempotency, envelope ошибок), роли `serve`/`api`/`worker`, health/metrics/alerts, `review --change`,
  `publish`/`cancel`/`rerun`, OpenAPI из DTO. M2.9 — self-review с pin доверенной
  сборки, production (`bondiano/sych`) — только так ([SERVER.md](docs/SERVER.md#self-review-bondianosych)), server-маршруты без local-only подписок, пример CI
  зеркала без `pull_request_target`. Реальное подключение ждёт App владельца —
  [docs/SERVER.md](docs/SERVER.md).
- Quality suite (M1.8/M1.14, разметка) — 103 development cases в `eval/quality/dev` (Rust, TS/React и Go
  safe/unsafe/UNKNOWN пары, routing/billing/privacy тройки, clean, nitpick, реальные коммиты) и `sych eval validate`;
  holdout размечает владелец. `sych eval measure` на DeepSeek V4.1 Flash, baselines A/B/C: полный pipeline C —
  recall 34/39, precision 39/61, ложные замечания на 22 из 58 safe, $0.17 за suite; один reviewer A — 39/39, но
  59/113 и 34/58 (одиночный прогон и повторы, не оценка качества — [docs/ANALYSIS.md](docs/ANALYSIS.md)).

M0 закрыт на fake adapter: внешних API-вызовов нет, качество ревью не измерялось.
`sych review --base REF [--head REF] [--diff-base REF] [--config sych.yaml] --models fake` требует предварительного
`sych db migrate`, иначе код 3. Без `--fake-responses` применимые обязательства остаются `UNKNOWN/NOT_ASSESSED`.
Невалидная policy на target tip — код 2.
Порог значимости пока фиксирован (`MEDIUM`); `change_kinds` без semantic provider дают `UNKNOWN`.
Неверные flags, refs или путь к репозиторию дают код 2; отсутствие merge-base — код 3 с причиной
(`NO_MERGE_BASE`, `SHALLOW_HISTORY`, `AMBIGUOUS_MERGE_BASE`), без сравнения с произвольным commit.
Тесты покрывают контракты `sych-core`, Git snapshot, storage и реальный CLI smoke на временных репозиториях (нужен `git` в `PATH`); зелёный тестовый запуск не подтверждает acceptance cases спецификации.

Согласованный scope включает качественный Rust review, OpenRouter и GPT/GLM/OpenCode subscription access, дешёвый DeepSeek V4.1 Flash и bounded fallback с эскалацией на Astra/Fable. Это требования к будущей реализации, не готовые возможности CLI. GitHub self-review Sych описан в M2.9 roadmap и [SERVER.md](docs/SERVER.md#self-review-bondianosych): CI выполняет quality-проверки, `image.yml` собирает образ ревьюера из `main`, а сам бот работает в petinfra.

## Локальная разработка

Нужны [rustup](https://rustup.rs/) и [just](https://just.systems/) (в CI — just 1.59.0).
Rust 1.94.1, rustfmt и Clippy зафиксированы в `rust-toolchain.toml`; зависимости — в `Cargo.lock`.
Для поиска неиспользуемых зависимостей нужен [cargo-machete](https://github.com/bnjbvr/cargo-machete) 0.9.2; в CI закреплена та же версия.

```sh
rustup show active-toolchain
cargo install cargo-machete --version 0.9.2 --locked
just check
just run
```

`rustup` использует toolchain из файла и устанавливает его при необходимости.
`just run` по умолчанию показывает справку; аргументы передаются напрямую, например `just run --version`.
Список рецептов выводится командой `just`.

## Quality-гейты

- `just fmt` — форматирование Rust-кода.
- `just lint` — rustfmt, Clippy для всего workspace со всеми targets и features (warnings — ошибки), `sych-style`
  и `cargo machete` для поиска неиспользуемых зависимостей.
- `just test` — тесты workspace со всеми features.
- `just check` — lint и test; тот же рецепт запускается в GitHub Actions.
- `just build` — сборка приложения.
- `just schemas` — перегенерация `schemas/*.schema.json` из Rust DTO; `just test` падает, если закоммиченные схемы устарели.

Cargo-команды сборки и тестов, а также Clippy используют `--locked`: изменения зависимостей требуют явного обновления lockfile.
Общие lints наследуются каждым пакетом через `[lints] workspace = true`.
Как в flick-lang, запрещены unsafe-код и игнорирование обязательных результатов;
включены Clippy all/pedantic, проверки `unwrap` и абсолютных путей.
`.editorconfig` и `.gitattributes` задают единые окончания строк и сохраняют DOCX как бинарные файлы.

[CI](.github/workflows/ci.yml) проверяет pull requests и push в `main`, также доступен ручной запуск.
Workflow имеет только `contents: read` и не сохраняет Git credentials в checkout.
После публикации репозитория job `check` нужно отдельно назначить обязательным в branch protection/ruleset.

## Правила разработки и выпуска

- Придерживаемся [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/): `feat`, `fix`, `docs` и другие подходящие типы; breaking changes обозначаем `!` или footer `BREAKING CHANGE:`.
- Стиль кода: после `if`-блока, за которым в том же блоке идёт другой оператор, ставится пустая строка. Проверяет `sych-style` в `just lint`, исправляет `just style-fix`.
- До полного feature scope и прохождения финального release gate из roadmap **ничего не выкатываем в production**. Промежуточные серверы и проверки интеграций — только test/staging/sandbox; боевое подключение бота, включая shadow mode, тоже считается production. Локальное read-only ревью само по себе не является выкладкой.

## Структура

```text
crates/sych-app/       CLI: lib (args, commands/, сервисы) и тонкий bin/sych.rs
crates/sych-core/      доменные типы, canonical hashing, schema_version, порт SnapshotReader
crates/sych-adapters/  Git snapshot, OpenRouter, pinned language servers, semantic/syntax/CI facts, sandbox; позже SCM
crates/sych-storage/   SQLite runs/pointers, artifact store, GC
crates/sych-style/     проверка стиля, которой нет в rustfmt/clippy (пустая строка после `if`)
migrations/sqlite/    миграции SQLite (только через `sych db migrate`)
schemas/              JSON Schema, сгенерированные из sych-core (не править вручную)
fixtures/core/        замороженные v1-документы, несовместимая версия, эталонный canonical hash
fixtures/git/         эталонный diff manifest сценарного репозитория (перегенерация: SYCH_BLESS=1 just test)
fixtures/config/      ошибочные config/policy: unknown key, секреты, конфликт правил, неверный путь, ослабление
fixtures/review/      сохранённый VIOLATED run: scope, policy, external inputs, evidence manifest и result
examples/personal/    рабочий personal sych.yaml и .sych/ (его же пишет `sych config init --personal`)
eval/                 fake eval suite `smoke.yaml` и fake responses fixtures (`sych eval run`)
.github/workflows/    CI
docs/                 спецификация, руководства и примеры вывода (`docs/examples/`)
```

У каждого crate есть свой README с назначением, картой модулей и инвариантами.

Внешние JSON-документы содержат `schema_version`; `sych_core::decode` отклоняет неизвестную major version
до разбора полей. Новые поля добавляются с default, чтобы `fixtures/core/v1` продолжали читаться.
Canonical hash: ключи объектов сортируются, вывод компактный, порядок массивов значим — наборы сортируются до хеширования.
Git вызывается с очищенным окружением, без system/global config, replace refs и lazy fetch,
только через `--git-dir` и с `--no-ext-diff --no-textconv`: hooks, filters, textconv, fsmonitor и external diff
из repository не выполняются, work tree и index не читаются, symlinks не разрешаются на host.
`ExecutionOutcome` (`PLAN_EXECUTED | LIMIT_REACHED | RETRIES_EXHAUSTED | INPUT_UNAVAILABLE`) — значения,
не перечисленные в §5.7 спецификации явно; решение — [ADR 0001](docs/adr/0001-execution-outcome-values.md).

Остальные crates и библиотеки из §4.2 спецификации добавляются вместе с использующим их кодом,
без пустых пакетов и неиспользуемых зависимостей. Подключены Clap, а в `sych-core` — Serde, serde_json, serde_yaml_ng, serde_path_to_error, Schemars, jiff, sha2, uuid и thiserror; в `sych-storage` — SQLx (SQLite, bundled) и Tokio (однопоточный runtime в CLI, signal/time для отмены и fake delay); tempfile — в тестах, для временных репозиториев `sych eval run` и workspace semantic backends; в `sych-adapters` — flate2 и tar для распаковки pinned language servers toml для Cargo manifests, syn/quote/proc-macro2 и oxc для syntax facts, quick-xml для JUnit.
Будущий `sych-core` не должен зависеть от HTTP, SQL или конкретного LLM SDK.
Публикация Cargo-пакетов отключена; лицензия проекта пока не выбрана.
