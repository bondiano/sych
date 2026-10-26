# sych-core

Доменные контракты и чистая логика Sych. Здесь нет HTTP, SQL, процессов и SDK моделей: всё, что
трогает внешний мир, живёт в `sych-adapters` и `sych-storage` и подключается через типы и порты этого
crate. Функции детерминированы для одинаковых входов, поэтому ревью воспроизводимо.

## Что внутри

| Модуль | Назначение |
|---|---|
| `snapshot`, `manifest` | `SourceSnapshot`, `DiffManifest`, `CodeRef` и порт `SnapshotReader`: точные координаты кода (side/blob/lines/hash) |
| `config` | server config (`sych.yaml`), repository policy (`.sych/`), effective config с хешами и provenance, `tooling` |
| `plan` | planner: review units, исключения, risk hints и обязательства до анализа |
| `reduce`, `result`, `obligation`, `finding` | детерминированный reducer: assessments + evidence → статусы, coverage, recommendation |
| `evidence`, `freeze` | evidence records, `ExternalInputManifest` и `EvidenceManifest` |
| `semantic`, `syntax`, `ci` | документы фактов: `SemanticFacts`, `SyntaxFacts`, `CiFacts`, их перевод в evidence и `RunFacts` |
| `broker` | ContextBroker (§9): context packets, лимиты §9.3, typed operations §9.4 |
| `model` | ModelGateway, structured output, бюджеты и cost ledger контракты |
| `path_class`, `glob`, `secrets`, `hash`, `version`, `schema` | классы путей, glob, secret paths и redaction, canonical hash, `schema_version`, JSON Schema |

## Инварианты

- Координаты кода строит harness, а не модель: `CodeRef` всегда указывает на pinned blob и hash строк.
- Текст репозитория и ответы модели — данные. Ничто здесь не исполняет их и не трактует как инструкции.
- Внешние документы версионированы (`schema_version`); `decode` отклоняет неизвестную major version.
- JSON Schema в `schemas/` генерируются из типов этого crate: `just schemas`.

## Проверки

```sh
cargo test -p sych-core
just schemas   # после изменения DTO
```
