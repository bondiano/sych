# sych-storage

Долговременное локальное хранилище: SQLite (WAL, одно соединение) для runs, указателей результатов и
model ledger, и content-addressed artifact store для неизменяемых документов run.

## Что внутри

| Модуль | Назначение |
|---|---|
| `lib` | `Storage`: `migrate` (единственная операция, меняющая схему), `open` (отказывает непромигрированной или более новой БД), жизненный цикл run (`begin_run` → `complete_run`/`fail_run`/`cancel_run`), `put_artifact`/`load_artifact` |
| `artifacts` | `ArtifactStore`: файлы по SHA-256 с атомарной записью |
| `ledger` | cost ledger ModelGateway: атомарные reservations, идемпотентный settlement, `UNKNOWN_CHARGE` |
| `gc` | `prune`: orphaned и просроченные artifacts; активные runs и retention защищены |

## Инварианты

- Неудачный или отменённый run никогда не становится `COMPLETED`.
- Artifact неизменяем: одинаковое содержимое — тот же адрес; документ читается через `decode` с проверкой
  `schema_version`.
- Доступ разделён по tenant и access scope (`Partition`); один hash не даёт доступа из другой partition.
- Retention: исходные данные (snapshot, diff) — 7 дней, результаты и evidence — 30 дней.

Ошибки — `StorageError` (`thiserror`); `tracing` пишет миграции и жизненный цикл run (started, completed,
failed).

Миграции лежат в `migrations/sqlite/` в корне репозитория.

## Проверки

```sh
cargo test -p sych-storage
```
