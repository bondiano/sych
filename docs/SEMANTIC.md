# Semantic backends

`sych semantic --base REF [--head REF] [--diff-base REF] [--repo DIR] [--output FILE]` печатает JSON-массив
документов `SemanticFacts` ([schema](../schemas/semantic-facts.schema.json)): по одному на каждую сторону
(`DIFF_BASE`, `HEAD`) и язык с изменёнными файлами. Команда только читает Git-объекты. `sych review` собирает те
же факты (`--semantic off` отключает) и передаёт их ContextBroker.

## Pinned language servers

Версии, URL и SHA-256 серверов поддерживает Sych: [`pinned.json`](../crates/sych-adapters/src/tools/pinned.json).
`PATH` и установленные в системе серверы не используются. При первом запросе сервер скачивается по HTTPS,
сверяется с закреплённым SHA-256, распаковывается во временный каталог и атомарно переименовывается в
`$SYCH_TOOLS_DIR`, иначе `$XDG_CACHE_HOME/sych/tools`, иначе `~/.cache/sych/tools`
(`<tool>/<version>/<arch>-<os>/`). Несовпадение checksum ничего не устанавливает.

| Язык | Сервер | Выбор версии | Платформы |
|---|---|---|---|
| Rust | rust-analyzer `2026-10-19` (GitHub release) | единственная закреплённая | linux, macos × x86_64, aarch64 |
| TS/JS/React | TypeScript 7 native `tsc --lsp --stdio` (npm `@typescript/typescript-<platform>`) | точная версия `typescript` из `pnpm-lock.yaml` (все importers, multi-document pnpm 11+) или `package-lock.json` | те же |
| Go | gopls `v0.24.0`, собранный закреплённым Go `1.27.3` (go.dev/dl, SHA-256) | единственная закреплённая | те же |
| Python | нет: базовый fallback M3 — syntax facts без language server | — | — |

Линтеры закреплены так же (GitHub release, SHA-256 из digest релиза): ruff `0.17.2`, oxlint `1.89.0`,
actionlint `1.7.12`, zizmor `1.31.1`; см. [Pinned linters](#pinned-linters).

gopls не публикуется бинарями, поэтому `archive: go-module`: Sych скачивает закреплённый Go, проверяет `h1:`
hash модуля `golang.org/x/tools/gopls@v0.24.0` (`sha256` в манифесте — его hex) через `go mod download -json`,
затем `go install -trimpath` с `GOPROXY=proxy.golang.org`, `GOSUMDB=sum.golang.org`, `GOTOOLCHAIN=local`,
`GOENV=off`, `CGO_ENABLED=0`; зависимости gopls проверяет его `go.sum`. Хостовый Go, `GOFLAGS` и credentials не
используются. Сборка занимает около минуты один раз на версию.

Версия TypeScript, которой нет в манифесте, несколько версий в importers или TypeScript < 7 (только tsserver
protocol) дают `UNSUPPORTED` с причиной, без подстановки другой версии. Новая версия добавляется записью в
манифест с checksum, проверенным против upstream (`integrity` npm, `digest` GitHub release).

## Что делает backend

1. Каждая сторона записывается во временный каталог побайтно из object database (`ls-tree` + `cat-file
   --batch`): без checkout, filters, `export-subst` и hooks. Secret paths, symlinks, submodules и blob > 4 MiB
   не пишутся. Base и head не делят workspace state.
2. Сервер запускается с очищенным окружением (`PATH`, `HOME`, `CARGO_HOME`, `RUSTUP_HOME`, `TMPDIR`, `LANG`),
   поэтому SCM/model credentials до него не доходят.
3. `documentSymbol` изменённых файлов → самые внутренние именованные конструкции (function, method,
   constructor, class, interface, struct, enum; module-level `const`/`let` — это React-компоненты и hooks),
   пересекающие изменённые строки. Анонимные callbacks и import bindings пропускаются, изменение относится к
   ближайшей именованной конструкции. Не больше 60 символов на документ, остальные перечислены в limitations.
4. Для каждого символа: `references`, `implementation` и call hierarchy (`prepare` → `incomingCalls`, call
   sites из `fromRanges`, а вызывающая конструкция целиком из `from.range` — `calling_constructs`; `outgoingCalls`
   → вызываемые конструкции целиком из `to.range` — `callees`, вызовы в зависимости — `external`;
   `definition` для первых 6 имён типов в тексте конструкции (идентификаторы с заглавной буквы, кроме
   `Self/Some/None/Ok/Err` и собственного имени) — `type_definitions`, определение целиком из `targetRange`) — только при
   объявленной capability, иначе `UNSUPPORTED`; callers не-callable символов — `NOT_APPLICABLE`.
5. Pull diagnostics (`textDocument/diagnostic`) изменённых файлов → `diagnostics[]` с `CodeRef`, severity,
   code и source (не больше 50 на файл). Это наблюдения сервера, не CI facts.
6. Locations → `CodeRef` с blob oid стороны и SHA-256 строк. Locations вне snapshot (registry, toolchain)
   попадают в `external` (home → `~`), несопоставимые — в `unresolved` с причиной. `CodeRef` построчные,
   поэтому negotiated position encoding (записан в `position_encoding`) на них не влияет.

У каждого символа есть `path_class` (`PRODUCTION | TEST | GENERATED | CONFIG | BUILD`) по имени пути и
флагу generated из diff manifest. Rust `build_configuration` перечисляет editions и объявленные features
каждого пакета (`features:<package>`); анализируются только default features и host cfg. TS
`build_configuration` перечисляет каждый `tsconfig*.json` стороны (`tsconfig:<path>`: `extends`, project
`references`, aliases из `paths`, `composite`; JSONC с комментариями и trailing commas), пакеты
(`package:<dir>` → имя из `package.json`) и globs `pnpm-workspace.yaml` (`workspace`). `extends` из npm-пакета
без установленных зависимостей и неразбираемые файлы перечислены в limitations.

Пример: [run facts](examples/semantic/run-facts.json), [semantic evidence](examples/semantic/evidence-semantic-head.json)
и [evidence manifest](examples/semantic/evidence-manifest.json) локального review двух commits TS-проекта.
Поведение на настоящих snapshots без настоящего сервера проверяет scripted LSP
([`tests/semantic.rs`](../crates/sych-adapters/tests/semantic.rs)): раздельные base/head workspaces, rename
Unicode-файла, удалённая конструкция, пустой ответ как `PARTIAL`, отсутствующие capabilities как `UNSUPPORTED`.

## Go build configuration

Go backend ищет `go.work` и все `go.mod` стороны (без `vendor`, `testdata`, скрытых каталогов, глубина 6) и
записывает в `build_configuration`: `toolchain` (pinned Go, `GOTOOLCHAIN=local`), `goos`/`goarch` (host),
`build_tags: none`, `cgo: disabled`, `go.work` (`use`), `module:<path>` (module, `go`, число requirements,
`toolchain`), `replace:<file>:<module>` и `vendor`. Limitations называют модуль с `go` новее pinned, `replace` на
путь вне snapshot или без `go.mod`, и то, что references/call hierarchy покрывают только активную конфигурацию
(GOOS/GOARCH без дополнительных tags) и не проходят через interface dispatch, reflection и function values.
gopls работает с `GOPROXY=off` и Sych-owned `GOMODCACHE`/`GOCACHE`/`GOPATH` в `~/.cache/sych`; без установки
зависимостей пакеты внешних модулей не разрешаются (limitation `dependencies are not installed`). Producer —
версия gopls (`v0.24.0`), а не его build info.

Поддержанный профиль один: host GOOS/GOARCH, без tags, cgo off. Код под другими build constraints попадает в
syntax facts (`cfg_gate` с `//go:build`), но не в semantic answers.

## Dependencies and sandbox

Без зависимостей imports npm/workspace пакетов не разрешаются, а rust-analyzer offline не видит std и
crates, которых нет в кеше. `tooling.dependency_install` в `sych.yaml` ([CONFIG.md](CONFIG.md)) включает
установку для каждой стороны отдельно, до старта сервера:

- TS: pinned pnpm (версия из `packageManager` в `package.json`, манифест [`pinned.json`](../crates/sych-adapters/src/tools/pinned.json))
  выполняет `install --frozen-lockfile --ignore-scripts --ignore-pnpmfile` с registry `registry.npmjs.org`,
  без user `.npmrc`, store в `~/.cache/sych/pnpm-store`. Только pnpm lockfile.
- Go: `go mod download` закреплённым Go в каждом модуле (или в корне `go.work`) через `proxy.golang.org` и
  `sum.golang.org` в `~/.cache/sych/go-mod`; код репозитория не исполняется. Rust-style docker не используется.
- Rust: `cargo fetch` workspace и std toolchain (`RUSTC_BOOTSTRAP=1` только для std manifest) в
  `~/.cache/sych/cargo-home`; rust-analyzer затем работает offline с этим `CARGO_HOME`, хостовый `~/.cargo`
  с credentials не используется.
- bwrap: корень хоста read-only; `$HOME`, `/tmp`, `/root`, `/run/user`, `/run/secrets` заменены пустым tmpfs;
  обратно монтируются только workspace (rw), store/`CARGO_HOME` (rw), бинарь инструмента и `RUSTUP_HOME`
  (ro); окружение очищено до `PATH`, `LANG`, `NIX_LD*`, `HOME=/tmp`. Сеть общая — для registry.
- docker: `--rm --cap-drop ALL --read-only --security-opt no-new-privileges`, тот же набор mounts, uid
  владельца workspace. Rust через docker не ставится (host toolchain в образе не работает) — следующий
  backend или политика.
- `build_configuration.dependencies` показывает итог (`installed (bwrap, pnpm 12.14.0, scripts off)`,
  `fetched (bwrap, sych CARGO_HOME)` или `not installed`). Деградировавший результат не кешируется.

Независимо от установки в workspace не пишутся `.cargo/config*` и `rust-analyzer.toml` (они могут задать
команды для cargo/rust-analyzer), а `rust-toolchain` с `path` даёт `UNSUPPORTED`: иначе инструменты хоста
исполнили бы код из repository.

## Syntax facts

`sych review` всегда разбирает изменённые `.rs` (syn 2), `.ts/.tsx/.js/.jsx` (oxc 0.139, последняя версия с
MSRV ≤ 1.94), `.go` и `.py/.pyi` (tree-sitter) файлы обеих сторон в процессе, без исполнения кода, и сохраняет `SyntaxFacts`
([schema](../schemas/run-facts.schema.json), вместе с semantic facts — артефакт `facts`) как derived static
evidence. Факт — это форма кода с `CodeRef`, `in_change` и деталью, а не finding: макросы не раскрываются, типы
не разрешаются, отсутствие факта ничего не доказывает.

- Rust: `async_fn`, `guard_across_await` (guard от `lock/read/write/borrow(_mut)` жив на `.await` того же блока,
  `drop(g)` его снимает; замыкания и `async`-блоки не считаются), `select_macro`, `detached_spawn` (handle
  выброшен), `blocking_in_async` (`thread::sleep`, `std::fs::*`, `block_on`), `discarded_result` (`let _ =` вызова,
  `.ok();`), `unwrap`, `drop_impl`, `public_item` (сигнатура), `cfg_gate`, `inactive_cfg` (item или statement под
  `#[cfg]`, который анализируемая сборка не компилирует: не host Linux, не `test`/`debug_assertions`, не default
  feature из ближайшего `Cargo.toml`; неизвестный предикат тоже неактивен), `serde_attribute`, `unexpanded_macro`
  (логирующие макросы `tracing`/`log` считаются известными: их аргументы видны), `debug_formatted` (`?value`,
  `{value:?}` или `{:?}` в логирующем макросе; тип берётся из параметра функции, `related` — его определения в
  snapshot, стандартные типы не отмечаются), `test_module`, `lint_suppressed` (`#[allow]`/`#[expect]`),
  `test_skipped` (`#[ignore]`).
- React: `effect_with_cleanup` / `effect_without_cleanup`, `unpaired_listener`, `unpaired_timer`,
  `unpaired_subscription`, `async_effect_guarded` / `async_effect_unguarded` (AbortController, `signal` или
  stale-флаг `ignore/cancelled/…`), `mutation_call`, `invalidation_call`, `controlled_input`,
  `uncontrolled_input`, `controlled_without_handler`, `form_submit`, `async_state_flag`, `custom_hook_call`,
  `test_skipped` (`it/test/describe.skip|only|todo`).
  Для custom hook literal search по pinned tree находит его определение (`related`), чтобы ревьюировалась
  текущая реализация обёртки, а не её имя.

В packet рискованные формы на изменённых строках идут как `REFUTING`; `select_macro`, `serde_attribute`,
`cfg_gate` и `unexpanded_macro` подписываются на своём span; определения project hooks и типов `debug_formatted`
идут как `SUPPORTING`. Изменённый код, поведение которого snapshot не показывает, — gap пакета: `CFG_INACTIVE`
(`inactive_cfg`), `MACRO_UNEXPANDED` (`unexpanded_macro`) и `DEBUG_UNSEEN_TYPE` (`debug_formatted` без
определения типа). С таким gap обязательство не может быть `SUPPORTED`: `context_limited` переводит его в
`UNKNOWN/SEMANTIC_GAP`; `VIOLATED` тоже, если все HEAD-якоря его NOT_MET-критериев лежат в таком коде, а
нарушение, показанное в другом месте изменения, остаётся. У discovery-пакетов gap делает coverage `PARTIAL`. Макросы
`anyhow` (`bail!`, `ensure!`, `anyhow!`, `format_err!`), `serde_json::json!` и `tokio::join!`/`try_join!`/`pin!`
считаются известными. Ссылки внутри `#[cfg(test)]`-модулей считаются тестами (`tested_by`). Diagnostics попадают в packet
только как `SUPPORTING` (не больше 5 на файл) с пометкой, что они могут быть артефактом анализа.

- Go (tree-sitter-go 0.25): `import`, `cfg_gate` (`//go:build`), `context_param`, `returns_error`, `defer`,
  `detached_spawn` (`go` statement), `channel_op` (send, receive, `close`, `select`), `tx_boundary`
  (`Begin/BeginTx/Commit/Rollback`), `db_call` (`Exec/Query/QueryRow/Prepare[Context]`, sqlx `NamedExec`),
  `write_call` (`os.WriteFile/Create/Remove/…`), `network_call` (`http.Get/Post/…`, `*http.Client`),
  `discarded_result` (`_ = call()`), `dynamic_dispatch` (`reflect.*`), `interface_call` (вызов через interface,
  объявленный в пакете), `test_skipped` (`t.Skip*`), `registered_effect_call`. Тип receiver SQL-вызова (`subject`)
  берётся из параметров, локальных `x := db.BeginTx(..)`/`sql.Open` и полей struct всех `.go` файлов того же
  каталога на той же стороне, а не из текста SQL: `tx.ExecContext` → `*sql.Tx`, `s.db.ExecContext` → `*sql.DB`;
  неизвестный receiver остаётся без типа. `guard` — условие ближайшего `if` в функции.
- Python (tree-sitter-python 0.25) — fallback M3: `import` (`related` — файл модуля в snapshot: `a/b.py`,
  `a/b/__init__.py`, `src/`-layout, относительный импорт, namespace-каталог; модуль вне snapshot и не из stdlib
  получает `subject: unresolved`), `resource_scope` (`with`/`async with`, `try/finally`), `detached_spawn` /
  `async_task` (`create_task`/`ensure_future` выброшен / сохранён, `Thread(..).start()`), `write_call`
  (`open(.., "w"|"a"|"x"|"+")`, `.write`, `.unlink`, …), `db_call` (`execute*`, `session.add/delete/merge/flush`,
  `.objects.`, `save`), `tx_boundary` (`commit/rollback/begin/atomic`), `network_call` (`requests/httpx/aiohttp/urllib`,
  `*client`/`*session`), `swallowed_exception` (`except` только с `pass/continue/break/return`),
  `dynamic_dispatch` (`getattr/setattr/eval/exec/import_module/__import__/globals`), `monkey_patch`
  (`module.attr = ..`, `setattr(module, ..)`), `runtime_registration` (декоратор не из списка обёрток, например
  `@app.route`, и `register/connect/subscribe` на уровне модуля), `test_skipped` (`pytest.skip`, `@skip*`, `xfail`),
  `registered_effect_call`. Receivers не типизированы: совпадение — по имени, это форма, а не разрешённый callee.
- Вызовы wrappers из effect registry (`match.language` = `go|python|typescript`, имя — последний сегмент
  `match.symbol`) — `registered_effect_call` с `detail` = registry symbol; `related` — их определения в snapshot,
  чтобы читалась текущая реализация. В Go wrapper сверяется и с типом receiver, `subject` — тип переданной
  транзакции.

Изменённые `dynamic_dispatch`, `monkey_patch`, `runtime_registration` дают gap `DYNAMIC_UNRESOLVED`, изменённый
Python import с `subject: unresolved` — `IMPORT_UNRESOLVED`; оба, как `CFG_INACTIVE`, не дают обязательству быть
`SUPPORTED`. Пустой AST или search result не доказывает отсутствия динамических зависимостей.

### Python: базовый fallback M3 и что отложено до M6

Сейчас для Python есть только syntax facts выше, literal search определений wrappers и импорт готовых
typecheck/lint/test reports через общий CI-контракт (JUnit, SARIF, Cobertura, LCOV). Semantic backend для Python —
`UNSUPPORTED` с причиной «no language server before roadmap M6», и в packet это gap `semantic backend Unsupported`.
Не включены до отдельного решения M6 по измеренной необходимости: language server (pyright/basedpyright/pylsp),
references/callers/implementations, вывод типов receivers, разрешение импортов зависимостей, установка Python
dependencies и любой запуск project code или analyzer plugins.

## Effect ledger

`sych_core::effects` строит один `EffectLedger` на run из syntax facts всех языков и effect registry
([schema](../schemas/run-facts.schema.json), поле `ledger` артефакта `facts`; в отчёте — `effect_ledger`;
derived static evidence `sych-effect-ledger`, completeness `PARTIAL`). Это ограниченная base/head модель
наблюдаемых эффектов изменённых файлов, не full-program dataflow.

- Эффекты: `db_call`/`tx_boundary` → `db_write`, `write_call` → `filesystem`, `network_call`/`mutation_call` →
  `network`, `invalidation_call` → `cache`, React effects и unpaired listeners/timers/subscriptions →
  `subscription`, async effects → `ui`, `registered_effect_call` → `effect_kind` registry. Ключ — файл (имя на
  head), конструкция, kind и target (операция, hook или registry symbol).
- Occurrence на каждой стороне: `CodeRef`, `payload` (вызов, как его показывают факты), guard, `transaction_scope` (`transaction (*sql.Tx)`, `none: *sql.DB
  outside any transaction`, `none (no transaction passed)`, `unknown (receiver type unresolved)`, Python —
  enclosing `atomic/begin` или последующий `commit`), `lifetime_scope` (`caller`, `detached: go …`, `task: …`,
  `with …`, `try/finally`, для React — cleanup и stale results), `failure_behavior` (ошибка возвращается,
  deferred rollback, detached, swallowed except). Cardinality — число occurrences стороны.
- Match: ключ только на base сначала ищется только на head в другой конструкции (`moved`; scope сравнивается
  только транзакционный и lifetime, guard и обработка ошибок остаются у старой конструкции). Без semantic call
  edge (head callers новой конструкции из старого файла) перенос `partial`. Иначе `removed` с пометкой, что
  альтернативный механизм вне изменённых файлов не исключён. Ключ на обеих сторонах — `preserved` или `changed`
  с notes: transaction scope, lifetime, failure behaviour, guard, multiplicity, порядок относительно других
  эффектов конструкции. Совпадение SQL или имени ничего не доказывает: сравниваются receiver, scope и контракт.
- Resolution: `resolved`; `partial` (перенос без call edge, removed/added, Python, registry без гарантии);
  `dynamic_unknown` (неразрешённый receiver, `dynamic_dispatch`/`monkey_patch` в той же изменённой конструкции) —
  тогда `preserved` становится `unresolved`.
- Registry binding: каждая запись `effects.yaml` связывается со своими `implementation_refs` в этом diff (oid
  base/head). `CURRENT` переносит `durable_with_transaction` в записи ledger; `IMPLEMENTATION_CHANGED` или
  `UNBOUND` — нет, запись получает note «not carried», а packet — gap `REGISTRY_RECHECK`. Для гарантии
  `durable_with_transaction` вызов вне транзакции или в detached goroutine после транзакции на base — `changed` с
  note о том, что recovery path (relay, retry, reconciliation) ещё не установлен: потеря не утверждается.
- `dependencies` — `(side, path, blob_oid)` всех occurrences и implementation refs: ledger переиспользуется, пока
  они совпадают.

Ledger связан с обязательствами: EFFECT-01 активируется изменённым `implementation_refs` или записью не
`preserved`, EFFECT-02 — ещё и записью с изменённым lifetime/failure. В packet unit'а записи не `preserved`/`added`
идут `REFUTING` с notes в причине, остальные `SUPPORTING`; `dynamic_unknown` — gap `EFFECT_UNRESOLVED`. Оба gap
ledger, как `UNSEEN_GAPS`, переводят `SUPPORTED` в `UNKNOWN/SEMANTIC_GAP`; `VIOLATED` остаётся. Посмотреть ledger:
`sych review --format json` (`effect_ledger`), `sych show RUN --format json` или evidence `sych-effect-ledger`.
Пары unsafe/safe §12.4 (Tx→DB и helper с `*sql.Tx`, outbox → goroutine и сохранённый вызов, изменённая
реализация wrapper, React cleanup/stale results, invalidation removed/перенесена в hook, Python write за `with`,
`getattr`) проверяются в [`tests/effect_ledger.rs`](../crates/sych-adapters/tests/effect_ledger.rs). Генерация
runtime-тестов и full-program analysis не требуются.

## Pinned linters

При `--semantic on` Sych сам запускает закреплённые линтеры на изменённых файлах head — только те, что не
исполняют код репозитория и работают без настройки проекта:

| Линтер | Файлы | Вызов | Конфигурация проекта |
|---|---|---|---|
| ruff | `.py`, `.pyi` | `check --output-format sarif --no-cache --exit-zero --force-exclude` | `pyproject.toml`, `ruff.toml`, `.ruff.toml`; без них — правила ruff по умолчанию |
| oxlint | `.js/.jsx/.mjs/.cjs/.ts/.tsx/.mts/.cts` — и в JS-проекте без TypeScript backend | `--format sarif` | `.oxlintrc.json`; если где-то в дереве есть `jsPlugins` или `oxlint.config.*` (JS/TS), проектный конфиг не читается (`--disable-nested-config -c {}`), это limitation |
| actionlint | `.github/workflows/*.y(a)ml` | `-shellcheck= -pyflakes= -format '{{json .}}'` | `.github/actionlint.y(a)ml` |
| zizmor | workflows, `action.y(a)ml`, `.github/dependabot.yml` | `--offline --format sarif` | `zizmor.yml`, `.github/zizmor.yml` |

Head материализуется из Git objects (как для language servers), линтер запускается в нём с очищенным окружением:
пустой `PATH` (actionlint не найдёт shellcheck, oxlint — node), `HOME` — пустой временный каталог (нет
пользовательских конфигов и credentials), без сети, не дольше 120 s. Абсолютные пути вывода приводятся к путям
repository; находка получает `CodeRef`, если её строка есть на head. Результат — `LintFacts` в артефакте `facts` и
в отчёте (`lint_facts`), derived static evidence `PARTIAL` с producer `<linter> <version>`. Если файл
конфигурации линтера изменён в этом diff, limitation говорит, что набор правил — выбор автора изменения.

В packet unit'а находки на изменённых строках head — `REFUTING` (`lint <linter> <level> <rule>: …`); находки на
неизменённых строках не попадают в packet. Линтер, который не удалось скачать или запустить, — gap
`LINT_UNAVAILABLE`: его молчание не читается как чистый результат. Workflow-находки (actionlint, zizmor) идут
в packet любого языка и дополняют TEST-INTEGRITY. ESLint, clippy, golangci-lint, mypy/pyright не запускаются:
они исполняют конфиг/плагины проекта или требуют сборки; их отчёты принимаются из CI (SARIF, cargo JSON и др.).

## Semantic cache

Факты стороны кешируются в `$SYCH_TOOLS_DIR/../semantic` (по умолчанию `~/.cache/sych/semantic`). Ключ — версия
Sych, ревизия алгоритма, snapshot id (он фиксирует оба дерева и diff, а значит и запрошенные файлы), сторона, язык, сервер с версией,
аргументы, initialization options и build configuration. Вместе с записью хранится access partition (repository
identity); при hit она сверяется заново, другой repository с тем же ключом получит промах. Кешируются только
`READY`-ответы. Ошибка записи кеша стоит только повторного вычисления.

Ревизия алгоритма отделена от версии crate: результаты до канонизации корней workspace вычисляются
заново, а старые файлы кеша не удаляются.

## ContextBroker

`sych_core::broker` строит по одному discovery packet на review unit и по verification packet на обязательство
(§9). Packet не содержит исходного текста: только `CodeRef` с hash, причины выбора, приоритет
(`REQUIRED → REFUTING → SUPPORTING`), completeness, связи (`called_by`, `calls`, `uses_type`, `implemented_by`, `tested_by`,
`read_by`, `written_by`, `referenced_by`), записи semantic-запросов, исключённое с причиной и gaps. Текст читается обратно через
`SnapshotReader`, который сверяет hash. Пакеты сохраняются артефактом `context_packets` run. Пример с исключёнными
источниками — [packets-with-exclusions.json](examples/context/packets-with-exclusions.json): Rust crate с
`initial_tokens_per_unit: 450`, construct `save` обеих сторон обязателен и показан, его caller `handle` ушёл в
`excluded` с причиной `BUDGET`.

Порядок сборки: изменённые строки обеих сторон и их enclosing constructs (обязательные) → contracts правил
обязательства из доверенной policy (текстом в prompt под обязательством, с path и hash) → CI diagnostics на
изменённых строках head (`REFUTING`) → error/warning diagnostics rust-analyzer/tsserver (вспомогательные: без
proc-macros и зависимостей часть из них — артефакты анализа) → callers, callees (wrappers и helpers целиком),
определения используемых типов,
implementations, tests и references глубины 1 и блоки imports изменённого файла по syntax facts (Rust `use`,
TS `import`; вспомогательные). Перекрывающиеся и соседние spans одного файла и стороны сливаются с сохранением
всех причин.

Лимиты §9.3: бюджет unit — `context.initial_tokens_per_unit` (6000), verification packet — сумма по units, но не
больше `context.max_tokens_per_request` (14000); токены считаются как байты текста (верхняя граница). Файлов на
run — `context.max_source_files` (80, не касается обязательных spans). Semantic-запросов 24 на unit и 160 на run,
span до 160 строк, текстовый blob до 512 KiB — пока константы §9.3. Обязательный span, который не помещается,
не обрезается: он попадает в `excluded` как `REQUIRED_OVER_BUDGET`, и обязательство получает `UNKNOWN /
CONTEXT_LIMIT` независимо от ответа модели. Syntax risk shapes и CI diagnostics (`REFUTING`) берутся только на
изменённых строках и сливаются с обязательным hunk. Writers изменённого символа вне изменения тоже `REFUTING`:
такой span за бюджетом исключается как `BUDGET`, оставляет gap `REFUTING_OVER_BUDGET`, и packet становится `PARTIAL`.

`packet resolution`: `UNAVAILABLE` — ничего не прочитано; `PARTIAL` — есть gaps (нет фактов для Rust/TS файла,
backend `UNSUPPORTED/FAILED`, неразрешённые locations, лимит запросов, обязательное за бюджетом, исходники языков
без backend — Java, Kotlin, Ruby, C/C++ и др.; Python — `UNSUPPORTED` до M6; изменённые CSS/HTML без browser/accessibility/visual reports — `NO_VISUAL_EVIDENCE`); иначе `SATISFIED`. Это достаточность собранного, а не истинность обязательства.

Typed operations §9.4 (`sych_core::broker::Operation`, закрытый enum без лишних полей): `read_span`, `read_symbol`,
`find_definition`, `find_references`, `find_implementations`, `find_callers`, `find_callees`,
`search_source` (только fixed string, одна строка до 200 символов, `git grep -F` по pinned tree, не дольше 5 s —
иначе `FAILED` с причиной) и
`read_evidence`. Отвеченные semantic-запросы — `PARTIAL`; символ вне изменённых конструкций — `UNSUPPORTED`
(живые запросы к серверу ещё не подключены). Secret paths не читаются и не попадают в результаты поиска.

## Ограничения

- Отвеченный запрос — всегда `PARTIAL`: пустой ответ не доказывает отсутствие callers.
- Rust: build scripts, proc-macros и `cargo check` выключены, cargo работает offline. Generated и
  macro-expanded items отсутствуют; health warning rust-analyzer (например, sysroot metadata offline)
  записывается в limitations. Features — default, target — host.
- Без `tooling.dependency_install` зависимости не устанавливаются: imports npm- и workspace-пакетов не
  разрешаются, их callers отсутствуют. yarn/npm lockfiles пока не устанавливаются.
- rust-analyzer с health `error` (например, нет toolchain для cargo) даёт `FAILED`, а не пустые ответы.
- Call site — одна строка; вызывающая конструкция длиннее лимита span попадает в packet только call site.
  Callee длиннее лимита span в packet не попадает, это записано в gaps.
- Имена типов выбираются по соглашению о заглавной букве (Rust, TS), не по семантике: константы в верхнем регистре
  тоже попадают в запросы, а типы с маленькой буквы — нет.
- Readers и writers: для невызываемого символа `documentHighlight` в позиции каждой из первых 12 references даёт
  kind `Read`/`Write`; остальные и ссылки без kind — в `unresolved` обоих наборов. Сервер без
  `documentHighlightProvider` — `UNSUPPORTED`. Это доступ по имени в коде, не effect ledger (M3).
- Rust offline без установки: sysroot std разрешается, только если его зависимости уже в кеше cargo, иначе
  rust-analyzer даёт health warning, а diagnostics — ложные ошибки типов.
- Go: только host GOOS/GOARCH без tags и с cgo off; gopls без установки зависимостей не видит внешние модули;
  diagnostics — pull (`pullDiagnostics: true`): ошибки компилятора, vet и встроенный в gopls поднабор staticcheck
  (`staticcheck: true` с ST-стилем не включается); при неустановленных зависимостях это ошибки импорта. Interface dispatch и reflection — `interface_call` и
  `dynamic_dispatch`, call hierarchy их не проходит.
- CI facts только импортируются (см. [LOCAL_REVIEW.md](LOCAL_REVIEW.md)); `cargo test` JSON (нестабильный
  libtest формат) не поддерживается — test outcomes берутся из JUnit.
- `rustup` по `rust-toolchain.toml` проверяемого репозитория может скачать указанный toolchain.
