# Локальное хранилище

Реализация M0.5 roadmap: §8.2, §20 и §24.4 [спецификации](SYCH_SPEC.md).

## Состав

- **SQLite** (`sych-storage`): WAL, `synchronous=FULL`, одно соединение — один writer и один worker.
  Таблицы `changes`, `runs`, `artifacts`, `run_artifacts`; ledger — `model_attempts`, `subscription_fees`;
  server (M2) — `repositories`, `inbox_events`, `change_intents`, `jobs`, `change_events`, `publication_objects`,
  `publication_operations`, `audit_events` ([SERVER.md](SERVER.md)). Вложенные доменные
  объекты в SQL не раскладываются.
- **Artifacts**: canonical JSON в локальном каталоге, ключ — SHA-256 байтов, путь разделён по tenant/access scope.
  Файл пишется во временный, `fsync`, затем `rename`; при каждом чтении hash проверяется.
- Пути: `storage.database_url_env` (значение `sqlite://PATH`, `sqlite:PATH` или путь) и
  `storage.artifact_store.path` относительно каталога `sych.yaml` (абсолютные остаются абсолютными), не CWD; без них — `./.sych-data/sych.db` и `./.sych-data/artifacts`.

## Инварианты

- Схему меняет только `sych db migrate`. Остальные команды открывают базу только на текущей схеме, иначе exit 3
  с подсказкой; более новую схему они тоже не трогают.
- Run создаётся `RUNNING` с следующей generation своего change. Каждый artifact сначала получает строку и pin
  на run, затем записываются байты.
- Указатель на result, usage и `COMPLETED` фиксируются одной транзакцией и только если bytes result уже
  читаются и проходят hash. Сбой даёт `FAILED`, принятая отмена (SIGINT/Ctrl-C) — `CANCELLED`, никогда не `COMPLETED`.
- Роли artifacts одного review: `source_snapshot`, `diff_manifest`, `policy_snapshot`, `policy_review`
  (provenance и policy diff head), `external_input_manifest`, `evidence/<id>`, `evidence_manifest`, `review_result`, а также
  `checkpoint/<key>` — принятый ответ модели с dependency record (пишется под fence сразу после ответа).
- `sych show --run ID` и `sych explain` читают эти artifacts и не запускают анализ. Удалённый или повреждённый artifact
  явно недоступен (`ArtifactUnavailable`/`Corrupt`), ничего не восстанавливается генерацией. Исключение —
  source snapshot и diff manifest с более коротким retention: после их удаления `show`/`explain` работают,
  а `snapshot`/`diff_manifest` в JSON равны `null` (в Markdown — «unavailable»).

## Финализация server run (M2.4)

Authoritative commit одного run — одна транзакция с предикатом:

- run `RUNNING` с замороженными inputs (`stage = ANALYZE`), tenant и access scope совпадают;
- job `CLAIMED` тем же owner с тем же `lease_epoch`, lease жив (`lease_until ≥ now`);
- `run.generation = change.desired_generation` — push между чтением и commit старый run current не делает;
- bytes result уже сохранены под этим run и читаются с совпадающим hash.

В той же транзакции: result pointer, `COMPLETED`, usage, job `DONE` и outbox intent (если подключение публикует).
Каждый pin artifact server run тоже несёт fence: поздний worker ничего не пинит. Внешние git/LSP/model/SCM
вызовы транзакцию не держат. Cancel/supersede отзывает lease (`lease_epoch + 1`); committed `COMPLETED` не
переписывается.

| Сбой | Итог |
|---|---|
| lease истёк до commit | commit отвергнут, reaper отдаёт job новому owner с новым epoch |
| push во время анализа | старый run `SUPERSEDED`, его commit и pins отвергнуты |
| bytes result потеряны до pointer | `complete_run` отвергнут, run не `COMPLETED`, intent нет |
| inputs не заморожены | commit отвергнут |
| cancel до commit | commit и intent отвергнуты |
| смерть процесса после commit | job `DONE`: reaper не возвращает его, повторного анализа и model attempts нет; intent `PENDING` ждёт publisher |
| сбой БД посреди транзакции (outbox insert) | откат: ни pointer, ни `DONE`, ни intent; run остаётся `RUNNING` |

## Retention и GC

| Что | Срок |
|---|---|
| source snapshot, diff manifest | 7 дней |
| policy snapshot/review, evidence, manifests, review result | 30 дней |
| artifacts активного run (`QUEUED…FINALIZING`) или run с публикацией `PENDING/PUBLISHING/UNCERTAIN` | не удаляются |
| artifacts без completed/активного run (сбой, незавершённый commit) | после grace 1 час |
| mirrors (`<data>/mirrors/…`, source cache) | 7 дней с последнего fetch; не пока repository в активном run |
| `change_intents` (consumed), `inbox_events` без живого intent, `jobs` (terminal) | 30 дней |
| `change_events`, `audit_events`, `model_attempts` (`SETTLED`/`RELEASED`), операции публикации (final) | 90 дней |
| `model_attempts` `RESERVED`/`UNKNOWN`, операции `PENDING/PUBLISHING/UNCERTAIN` | не удаляются по возрасту |
| строки `runs` (usage/audit) | не удаляются; 90 дней — нижняя граница |

`sych cache prune --dry-run` печатает план (artifacts, число строк по таблицам с правилом, mirrors) и ничего не
удаляет; без флага artifacts удаляются по одному под write lock с повторной проверкой pin и retention, строки —
одной транзакцией. Pin или продление retention, принятые другим процессом после первоначального списка, artifact
сохраняют. Raw CI artifacts не хранятся вовсе: zip читается в памяти, сохраняется только нормализованный отчёт.

## Совместимость схемы

Схема меняется только `sych db migrate`. 0004 пересобирает `runs` (snapshot/policy ids стали nullable) по
процедуре SQLite с выключенными foreign keys внутри одной транзакции. `cargo test -p sych-storage --test upgrade`
поднимает базу v2 из `fixtures/storage/v2-seed.sql` (completed run, pin, `UNKNOWN` charge) и проверяет, что после
миграции данные и foreign keys целы. До R1 обратная совместимость не обещается, но данные существующих баз
переносятся.

## Backup

Локально достаточно остановить `sych` и скопировать каталог `.sych-data/` целиком (база, `-wal` и artifacts
вместе). Это описанная процедура для personal mode, а не проверенный corporate restore.
