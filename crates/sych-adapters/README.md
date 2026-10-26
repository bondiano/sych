# sych-adapters

Всё, что Sych делает с внешним миром ради анализа: Git, language servers, парсеры, sandbox, модели и
CI-отчёты. Адаптеры возвращают типы `sych-core` и никогда не исполняют код проверяемого репозитория
привилегированно.

## Что внутри

| Модуль | Назначение |
|---|---|
| `git` | захардененный `git` (очищенное окружение, без hooks/filters/textconv), `LocalSnapshot` (три pinned позиции, diff manifest, `SnapshotReader`: чтение любого файла стороны и `git grep -F`), выгрузка стороны в каталог побайтно из object database |
| `tools` | закреплённые версии language servers и pnpm (`tools/pinned.json`: версия, URL, SHA-256 на платформу), скачивание, проверка и атомарный кеш |
| `semantic` | LSP-клиент, профили rust-analyzer и TypeScript 7 (`tsc --lsp`), изменённые конструкции, references/callers/implementations, pull diagnostics, semantic cache, установка зависимостей через `sandbox` |
| `syntax` | syntax facts из `syn` (Rust) и `oxc` (TS/React) в процессе, без исполнения |
| `sandbox` | запуск установщиков в bwrap (корень read-only, `$HOME`/`/tmp`/секреты скрыты) или docker (pinned digest) |
| `ci` | нормализация `cargo --message-format=json` и JUnit в `CiFacts` с привязкой к tested commit |
| `openrouter` | OpenRouter transport для ModelGateway: pinned model и upstreams, без hidden fallback |

## Ошибки и логи

Ошибки типизированы через `thiserror` (`SnapshotError`, `ToolError`, `LspError`, `SandboxError`, `DepsError`,
`CiError`); причина сохраняется в `source()`. События и спаны — `tracing`: закрепление сравнения, скачивание
инструментов, анализ каждой стороны и языка, установка зависимостей и sandbox jobs, импорт CI, пропущенные
при разборе файлы. Подписчика ставит приложение.

## Границы безопасности

- `.cargo/config*` и `rust-analyzer.toml` не выгружаются, а `rust-toolchain` с `path` отклоняется: они могут
  назначить команды инструментам хоста.
- Language servers и установщики получают только переменные окружения, нужные toolchain; credentials
  модели и SCM до них не доходят.
- Pinned артефакт без совпавшего SHA-256 не устанавливается; версия, которой нет в манифесте, даёт
  `UNSUPPORTED`, а не подстановку.

## Проверки

Для semantic smoke ниже нужен проверяемый репозиторий как минимум с двумя коммитами (`HEAD~1` и `HEAD`).

```sh
cargo test -p sych-adapters
sych semantic --base HEAD~1     # реальный smoke; первый запуск скачивает серверы
```
