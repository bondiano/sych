# sych-app

Командная строка `sych`. Библиотека с командами и сервисами плюс тонкий бинарь `bin/sych.rs`, который
только разбирает аргументы и вызывает команду.

## Устройство

- `args` — clap: `Cli`, `Commands` и по структуре `*Arguments` на подкоманду; общий `Comparison`
  (`--repo/--base/--head/--diff-base`) и `Output` (`--format/--output`).
- `command` — trait `Command { name(), run() -> anyhow::Result<ExitCode> }` и `run_command`, который
  печатает цепочку ошибки (`sych <команда>: контекст: причина`) и возвращает её exit code.
- `errors` — exit codes (§24.1): 2 invalid, 3 no result, 4 permission, 10/11 strict policy, 130
  cancelled, 1 failed eval. Ошибки по схеме thiserror + anyhow: библиотеки возвращают типизированные
  ошибки (`SnapshotError`, `StorageError`, `ToolError`, `SandboxError`, `DepsError`, `CiError`), app —
  `anyhow::Result` с `.context()` на каждом шаге. Код решает `Failure` (thiserror: `Invalid`, `NoResult`,
  `Permission`, `Cancelled`), иначе первая типизированная причина в цепочке (`SnapshotError::InvalidInput`,
  `StorageError::UnknownRun` → 2), иначе 3.
- Логи — `tracing`, подписчик `tracing-subscriber` ставится в `bin/sych.rs`: stderr, без времени, цвет
  только в терминале. Уровень — `SYCH_LOG` в синтаксисе `EnvFilter` (по умолчанию `info` для crates Sych,
  `warn` для зависимостей; `SYCH_LOG=debug`, `SYCH_LOG=sych_adapters=trace`). Секреты, prompts и ответы
  моделей не логируются.
- `commands/` — по файлу на подкоманду (`review`, `semantic`, `show`, `explain`, `doctor`, `config_init`,
  `config_validate`, `eval_run`, `eval_validate`, `eval_measure`, `db_migrate`, `auth_chatgpt`, `cache_prune`).
  В каждом `run(&XArguments) -> anyhow::Result<ExitCode>` собирает результат из сервисов.
- Сервисы, по предметной области:

| Сервис | Назначение |
|---|---|
| `review` | конвейер review: snapshot → trusted policy → facts → plan → fake assessments → context packets → reduce → сохранение |
| `semantic` | semantic facts через pinned language servers с кешем |
| `ci` | чтение `--ci-facts`/`--ci-manifest` для импорта |
| `snapshot` | захват сравнения (`Comparison` → `LocalSnapshot`) |
| `config` | server config, рабочий `.sych/`, effective config, пример `config init` |
| `storage` | пути БД и artifacts, `block_on`, чтение сохранённого run |
| `output` | запись JSON/Markdown в файл или stdout |
| `render` | JSON и Markdown рендереры с экранированием текста репозитория |
| `fake` | fake model adapter для `--fake-responses` |
| `probe` | opt-in model probe для `doctor --probe-model` |
| `eval`, `quality` | harness suite `eval run` и проверка размеченного quality suite `eval validate` |

## Как добавить команду

1. Структура `XArguments` в `args.rs` и вариант в `Commands`.
2. `commands/x.rs` с `pub fn run(args: &XArguments)`; переиспользуемая логика — в сервис.
3. Ветка в `bin/sych.rs`: `Commands::X(args) => run_command("x", || x::run(&args))`.

## Проверки

```sh
cargo test -p sych-app      # CLI тесты запускают собранный бинарь на временных репозиториях
just run --help
```
