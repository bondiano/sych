# Локальное ревью

Реализация M0.6 roadmap: §24.1, §24.4–24.5 и §27.1 [спецификации](SYCH_SPEC.md). Локальное ревью читает только
Git objects, сохраняет run в локальное хранилище ([STORAGE.md](STORAGE.md)) и ничего не публикует в SCM.
Fake adapter (`--models fake`) доказывает работу harness, а не качество ревью; review настоящей моделью —
`--models PROFILE` ([ANALYSIS.md](ANALYSIS.md)).

## Сценарий

```sh
sych db migrate
sych review --repo . --base origin/main --head HEAD --models fake \
  --format json --output .sych-output/review.json
sych review --repo . --base origin/main --models fake --format markdown --output .sych-output/review.md
sych show --run RUN_ID --format markdown
sych explain --run RUN_ID --obligation EFFECT-01 --format markdown
```

- `--base` — target ref: policy берётся только с его tip, diff считается от `merge-base`; `--diff-base` задаёт
  точное двухточечное сравнение.
- `--format json|markdown` (по умолчанию JSON), `--output PATH` пишет файл и создаёт каталог, иначе stdout.
- `--semantic on|off` (по умолчанию `on`): для Rust, TS/JS и Go файлов pinned language servers (Python —
  только syntax facts, M3 fallback); тот же переключатель запускает pinned линтеры ruff, oxlint, actionlint и
  zizmor на изменённых файлах head ([SEMANTIC.md](SEMANTIC.md#pinned-linters)) дают semantic facts
  ([SEMANTIC.md](SEMANTIC.md)), они сохраняются как derived evidence, а ContextBroker строит context packets.
  Первый запуск скачивает серверы; повтор того же snapshot берёт факты из semantic cache.
- `--ci-facts FILE` (повторяемый) импортирует готовые CI reports: вывод `cargo check/clippy/build
  --message-format=json`, JUnit XML (cargo-nextest, vitest, Jest, gotestsum), SARIF, GitLab Code Quality, LCOV,
  Cobertura, `go test -json` (с `-race`: каждая гонка — diagnostic `race` с первым кадром), `go test -coverprofile`
  (import paths корневого `go.mod` → пути repo) и `go vet -json`. Абсолютные пути CI checkout сопоставляются с repo
  по суффиксу. Race и coverage подтверждают только исполненные пути. Формат определяется по содержимому, файлы до
  32 MiB. `--ci-manifest FILE` описывает job (YAML/JSON, `schema_version: 1`, `job`, `tested_oid` — hex из
  `git rev-parse HEAD`, `toolchain`, `target`, `features`). Отчёт становится external input
  (`ExternalInputManifest`) с binding `ci` и `trust_class: AUTHOR_CLAIM`: локальный импорт не доказывает
  происхождение. Без manifest он `UNBOUND`/`STALE`; если `tested_oid` не равен head — `STALE`, и его locations не
  переносятся на head. Ошибки и предупреждения на изменённых строках head попадают в context packet как
  `REFUTING`, остальные — `SUPPORTING`. Skipped tests, `#[ignore]`, `it.skip` и `#[allow]/#[expect]`
  фиксируются отдельно и не считаются исправлением.
- Ход работы пишется в stderr через `tracing`; подробность задаёт `SYCH_LOG` (`info` по умолчанию, `debug`,
  `warn`, или по crate: `SYCH_LOG=sych_adapters=debug`). Ошибка печатается одной цепочкой
  `sych <команда>: контекст: причина`.
- `show` и `explain` читают сохранённые artifacts и не запускают анализ; JSON `show` совпадает с JSON `review`,
  пока не истёк 7-дневный retention snapshot/diff; затем эти поля `null`, остальное читается.
  `--obligation` принимает id обязательства или id правила.
- JSON и Markdown содержат snapshot (target tip, diff base, head), PolicySnapshot, provenance и policy diff head,
  scope, obligations, findings, gaps, evidence, semantic facts, context packets и usage; `explain` показывает
  context packet обязательства. Текст репозитория в Markdown экранирован (HTML, разметка),
  управляющие символы терминала и bidi-символы не выводятся как есть ни в одном формате.

Примеры вывода: [review.json](examples/local-review/review.json), [review.md](examples/local-review/review.md),
[explain.md](examples/local-review/explain.md).

## Fake responses

Без `--fake-responses` fake adapter ничего не оценивает: применимые обязательства остаются `UNKNOWN/NOT_ASSESSED`.
Fixture задаёт evidence и assessments; ids, hashes и CodeRef вычисляет harness по закреплённому snapshot:

```yaml
schema_version: 1
delay_ms: 0                       # имитация задержки модели, для проверки отмены
evidence:
  - key: save-keeps-event-in-tx   # имя внутри fixture
    kind: source
    completeness: COMPLETE_FOR_QUERY
    trust_class: SOURCE_OBSERVATION
    validity: VALID               # по умолчанию
    code_refs:
      - { path: backend/orders/store.go, start_line: 4, end_line: 9 }   # side: HEAD по умолчанию
assessments:
  - rule_id: EFFECT-01
    status: SUPPORTED             # VIOLATED | UNKNOWN (+reason_code, next_evidence_needed) | NOT_APPLICABLE (+basis)
    evidence: [save-keeps-event-in-tx]
```

Неизвестные поля, ключи evidence, пути и строки вне файла — exit 2 до создания run. Reducer применяет к ответам
те же правила, что и к модели: SUPPORTED только на VALID complete evidence допустимого вида и не только со слов
автора, иначе `UNKNOWN/INSUFFICIENT_EVIDENCE`.

## Exit codes

| Code | Когда |
|---:|---|
| 0 | result сохранён (operational mode не проваливается из-за findings) |
| 2 | неверные flags, refs, config, policy или fixture |
| 3 | нет пригодного сохранённого result: не мигрирована БД, сбой записи, не удалось вывести result |
| 10 | `--exit-policy strict` и значимое нарушение (`ATTENTION_REQUIRED`) |
| 11 | `--exit-policy strict` и coverage не `COMPLETE_FOR_SCOPE`; 10 имеет приоритет |
| 130 | SIGINT/Ctrl-C во время анализа: run `CANCELLED`, result не сохраняется |

Код 4 (credentials) зарезервирован для реальных providers M1. При exit 3 после сохранения (не удалось записать
`--output`) result доступен через `sych show`.

## Fake eval

```sh
sych eval run --suite eval/smoke.yaml --models fake [--output report.json]
```

Каждый случай suite собирает временный Git-репозиторий (`base` — файлы target commit, `head` — изменения поверх,
`null` удаляет файл), запускает review с fake adapter во временное хранилище и сравнивает значимые состояния:
recommendation, coverage, статусы obligations по rule id, число findings и units. Runtime ids и timestamps в отчёт
не входят, поэтому одинаковые fixtures дают одинаковый отчёт ([пример](examples/local-review/eval-smoke.json)).
Exit 0 — все случаи совпали, 1 — есть расхождения, 2 — неверная suite. Внешних вызовов нет: `model_attempts` равно 0.

## Quality suite

`eval/quality/dev/` — размеченный development suite (§27.2–27.3): по одному `<id>.yaml` на кейс, пары
`safe/unsafe` и честные `unknown` для Rust (Result/RAII, cancellation, task lifetime, lock across `.await`,
blocking в async, trait API, cfg/features, serde contract, ослабленная обработка ошибок и тесты) и TS/React
(cleanup, stale response и сохранённая защита, timers, subscriptions, invalidation, controlled inputs, form
submit, project hook, удаление API), чистые изменения, nitpick и два реальных коммита Sych/Vkusori
(`repository_fixture`). `eval/quality/holdout/` размечает владелец, правила — в его
[README](../eval/quality/holdout/README.md).

```sh
sych eval validate --suite eval/quality/dev
```

Для repository fixtures добавьте `--repo vkusori=PATH` и, при наличии архивного клона Sych до squash,
`--repo sych=PATH`. Текущая ветка Sych не содержит исторических refs кейса `real-sych-context-broker`.

`validate` ничего не отправляет модели: проверяет полноту разметки (rationale, profiles, defect у `unsafe`,
отсутствие defect у `safe`, target и locations у `tolerated`, причина у `unknown`, полные пары, имя файла = id, один split на каталог), что
каждая evidence location открывается как `CodeRef`, а ожидаемые syntax facts присутствуют (и запрещённые
отсутствуют). Кейсы с `repository_fixture` без `--repo` — `SKIPPED`. Это проверка разметки и harness, не
измерение качества review: оно делается реальной моделью в M1.8.

### Измерение реальной моделью

```sh
sych eval measure --suite eval/quality/dev --config sych.yaml --models cheap_api \
  [--case ID]... [--runs N] [--variant b|c] [--semantic on|off] [--repo NAME=PATH] [--output report.json]
```

Каждый кейс проходит обычный `sych review` с профилем `--models` в хранилище из `--config`: ledger общий, поэтому
лимиты run/дня/месяца действуют на весь suite, а каждый review остаётся сохранённым run. `--variant c` — обычный
pipeline, `b` — без дополнительного high-risk поиска (baseline B §27.5), `a` — один reviewer по diff (baseline A):
без semantic facts, high-risk поиска и verifier; его непроверенные гипотезы оцениваются как findings. `--runs` повторяет кейс для nondeterministic модели.

Отчёт по каждому run: recommendation, coverage, ожидаемые и найденные defects, findings с anchors и признаком
`labelled`, сверка статусов obligations, для `unknown` — остался ли кейс неразрешённым, attempts и деньги. Totals
(всего и по языкам): `recall` (найдены размеченные defects), `precision` (correctness findings на размеченных
местах), `clean_false_positive_rate` (safe-кейсы с любым correctness finding), `clean_false_finding_rate` (то же без
`tolerated_findings` — correctness findings на местах `expected.tolerated`: верное, но не целевое замечание кейса — и
без `overrated_nits` — correctness findings на месте размеченного nitpick), `unknown_kept_rate`, `nit_recall` (размеченные
nitpicks, на которые попал nit или improvement), nits, attempts и
стоимость. Совпадение — пересечение anchor с размеченной evidence location: это приближение смысла, owner
проверяет его по заголовкам findings. Числа — измерение, критерии принятия задаёт owner.


