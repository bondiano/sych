# Конфигурация, policy и doctor

Реализация M0.3 и M1.4 roadmap: §11, §22–23 и §24.1 [спецификации](SYCH_SPEC.md).

## Файлы

| Файл | Кто владеет | Откуда читается |
|---|---|---|
| `sych.yaml` | владелец процесса / operator | локальный файл, `--config` |
| `.sych/config.yaml` | repository owner | Git-объекты **target tip** (`--base`) |
| `.sych/rules.yaml`, `effects.yaml`, `deploy.yaml` | repository owner | там же; пути задаются в `policy.*_file` |
| `.sych/contracts/**` | repository owner | там же; хешируются как есть |

Рабочие примеры: [`examples/personal/`](../examples/personal/). Секреты в конфигурации не хранятся:
`*_env` содержит только имя переменной окружения (`UPPER_SNAKE_CASE`); значение-секрет на этом месте
отклоняется без вывода самого значения.

## Правила разбора

- Неизвестный ключ — ошибка с точным путём: `sych.yaml: models.providers.openrouter.endpont: unknown field …`.
- Enums в YAML — lowercase `snake_case`; в доменный JSON они преобразуются типизированно
  (например, `severity_on_violation: high` → `Severity::High`).
- `schema_version` обязателен; неизвестная major version отклоняется.
- В `.sych/` допустимы только `config.yaml`, файлы из `policy.*_file` и `contracts/**`; лишний файл,
  symlink, submodule или файл больше 1 MiB — ошибка. Symlink не разрешается.
- Повтор rule ID — конфликт обязательных правил и ошибка конфигурации; модель его не разрешает.
- `contract_refs` должны указывать на файлы в `.sych/contracts/` принятой policy.
- Repository не может превысить server ceilings: `context.max_tokens_per_request` ≤
  `budgets.analysis.max_input_tokens`. Execution включается только в `sych.yaml` вместе с
  `execution.allowed_profiles`; по умолчанию выключен.
- `tooling.dependency_install` (по умолчанию `false`) разрешает установить зависимости проверяемого
  repository для semantic analysis: `pnpm install --frozen-lockfile --ignore-scripts --ignore-pnpmfile` pinned
  pnpm из `packageManager` и `cargo fetch` workspace и std в Sych-owned `CARGO_HOME`. Запуск только в
  `tooling.sandbox.backends` (по порядку: `bwrap`, `docker`; первый доступный используется, остальные —
  fallback). `docker` требует `docker_image` с `@sha256:` digest и пока ставит только pnpm.
  `on_unavailable: fail` (по умолчанию) останавливает review с `TOOLING_UNAVAILABLE` (код 3), если ни один
  backend не доступен или установка упала; `degrade` продолжает без зависимостей с limitation.
- `redaction` дополняет встроенную редакцию секретов (private keys, токены `ghp_`/`sk-`/`AKIA`…,
  значения ключей `password`/`token`/`api_key`…) и никогда её не заменяет. `token_prefixes` — префиксы
  токенов (после префикса нужно 16+ символов `[A-Za-z0-9_-]`), `secret_keys` — подстроки имени ключа
  (без учёта регистра), `patterns` — регулярные выражения (синтаксис crate `regex`, линейное время);
  каждое совпадение заменяется на `[REDACTED]` в DATA-блоках перед отправкой модели. Пустой префикс или
  ключ, невалидное выражение или выражение, совпадающее с пустой строкой, — ошибка конфигурации с путём
  `redaction.patterns[N]`. Задаётся только в trusted `sych.yaml`: repository policy не может ослабить или
  засорить редакцию. Остальные места (логи, ошибки провайдеров, вывод конфигурации) используют
  встроенные правила.

## Приоритет и effective config

Engine defaults → `sych.yaml` → принятая repository policy. Effective config нормализуется (все defaults
заполнены) и получает `config_hash` (server + repository config) и `rules_hash` (rules, effects,
deployment, хеши contracts). `PolicySnapshot.id` зависит от источника, обоих хешей и версии authorization
policy. `provenance` перечисляет слои с SHA-256 исходных файлов; значений секретов в нём нет.

Слой path/domain выражается через `applies_to.paths` правил; generic rules engine идут после repository rules
и заменяются ими по ID. Отдельного файла личных preferences спецификация не задаёт, поэтому такого слоя ещё нет.

## Policy из head не применяется

`sych review` читает `.sych/` только из target tip. Изменения `.sych/` в проверяемом head попадают в
`policy_diff` (файлы, rule IDs, ошибки предложенной policy после redaction) и на текущий run не влияют
(INV-12). Невалидная policy на target tip — exit 2. AGENTS.md, CLAUDE.md и прочие файлы вне `.sych/` policy не
становятся.

Новую trusted revision применяет только отдельный явный run оператора: `sych review … --policy-ref REF` читает
`.sych/` из коммита `REF` вместо target tip; `policy.source` результата — `{ref_name: REF, commit}`, в stderr
пишется предупреждение. Head по-прежнему остаётся только diff.

## Правила, версии и применимость

Rule ID стабилен; изменение смысла повышает `version`. Смысл — `kind`, `required`, `severity_on_violation`,
`applies_to`, `statement`, `acceptance`, `contract_refs`; title, owner, rationale и exceptions меняются в той же
версии. Предложенное в head правило с новым смыслом и прежней версией попадает в `policy_diff.head_errors`
(«changes its meaning without a version bump»).

Применимость к изменению (§11.4) трёхзначна:

- `APPLIES` — старый или новый путь изменённого файла совпал с `applies_to.paths` (перенос файла за пределы
  путей правило не снимает), либо изменённая конструкция вызывается или упоминается из этих путей по semantic
  facts — тогда в limitations записано «applies through a semantic link: `name` is used from path:line».
- `DOES_NOT_APPLY` → `NOT_APPLICABLE` с проверяемым basis (ни один путь, ни semantic link).
- `UNKNOWN` — неполный diff, неразрешённые `change_kinds` или generic rule, чей предметный checker ещё не
  подключён (`PROVIDER_UNAVAILABLE`).

### Generic rule pack

Engine поставляет начальный набор §11.4. Правило включено своим профилем `review.profiles` и создаёт
обязательство только когда изменение его касается; repository rule с тем же ID заменяет generic.

| ID | Профиль | Активация | Оценка сейчас |
|---|---|---|---|
| EFFECT-01 | side_effects | изменён путь из `implementation_refs` effect registry или запись effect ledger не `preserved` | модель по ledger ([SEMANTIC.md](SEMANTIC.md#effect-ledger)) |
| EFFECT-02 | side_effects | syntax shapes ошибок, cancellation и lifetime в изменённых строках (включая `swallowed_exception`) или запись ledger с изменённым lifetime/failure | модель |
| MIG-COMPAT, MIG-RESTART, MIG-CONTRACT, MIG-RECOVERY | migration_compatibility | `**/migrations/**`, `**/migrate/**`, `*.sql` | `UNKNOWN`, migration checker (M3) |
| TF-CHANGE | infrastructure | `*.tf`, `*.tfvars` | `UNKNOWN`, Terraform plan checker (M3) |
| K8S-ROLLOUT | infrastructure | `k8s/`, `helm/`, `charts/` | `UNKNOWN`, rollout checker (M3) |
| TEST-INTEGRITY | correctness | изменены tests или CI gates, либо в изменённых строках новый skip/`#[ignore]` или подавленный lint | модель |

Все generic rules required: недоступный checker не делает правило необязательным, а оставляет `UNKNOWN` с
названием недостающего evidence. Проверка тестов не подменяет accepted requirement: assessment, опирающийся только
на тест того же изменения, — `TEST_ORACLE_ONLY`.

### Preferences и nitpicks

`kind: preference` никогда не required и не создаёт обязательство; nitpick без `rule_id` допустим. Verifier
принимает NIT/IMPROVEMENT не выше `LOW` и только с конкретным местом, поэтому nit не меняет recommendation; ноль
nitpicks — нормальный результат.

### Исключения

`exceptions[]` правила: `paths`, `reason`, `owner`, `author` и `expires_at` или условие `until`. Исключение,
чьи paths касаются изменения и которое в силе на момент run, становится `waivers[]` обязательства со статусом
`VIOLATED` или `UNKNOWN`: статус и rule violation finding сохраняются, но такое обязательство не поднимает
recommendation (не `ATTENTION_REQUIRED` и не `NEEDS_EVIDENCE`), а coverage по-прежнему его учитывает.
Истёкшее исключение не применяется и записывается в limitations.

## Секреты

- `is_secret_path` — default deny для `.env*` (кроме `*.example|sample|template|dist`), ключей
  (`*.pem`, `*.key`, `id_rsa`…), credentials, `.npmrc`/`.netrc`, `*.tfstate*`, каталогов `.ssh/.aws/.gnupg/.terraform`.
  Такие файлы остаются в diff manifest с limitation `SECRET_PATH`; их содержимое не должно уходить в context/model.
- `redact` заменяет блоки private key, известные форматы токенов и значения ключей вида
  `password/secret/token/api_key` на `[REDACTED]`. Применяется к сообщениям об ошибках конфигурации и к выводу
  `config validate`/`doctor`. Это эвристика, не гарантия (§22.3).
- `ServerConfig::egress_allowed` — allowlist по `endpoint` провайдеров (`https://` или loopback `http://`).
  URL разбирается: userinfo запрещён, сравниваются origin (scheme, host, port) и сегменты пути, не строковый префикс.
- `billing_kind`/`enforcement` профиля обязаны совпадать с adapter провайдера: `openrouter`/`openai_compatible` —
  `api_usage` + `api_hard`, подписки — `subscription_quota` + `subscription_observable`.

## Команды

```sh
sych config init --personal                 # пишет ./sych.yaml, существующий файл не трогает (exit 2)
sych config validate --config sych.yaml --repo .
sych doctor --config sych.yaml --repo .
sych review --repo . --base main --head HEAD --config sych.yaml --models fake
```

- `config validate` проверяет схему, пути, ссылки role → profile → provider, fallback chains и лимиты, читает
  `.sych/` из **рабочего дерева** (для авторинга) и печатает effective config в JSON. Ошибки — в stderr, exit 2.
- `doctor` дополнительно проверяет Git repository, наличие переменных окружения (только «set/not set»),
  доступность artifact store на запись и разбор price catalog (формат — [MODELS.md](MODELS.md)). Он не делает deploy, не публикует comment и
  вызывает модель только с `--probe-model PROFILE` (см. [MODELS.md](MODELS.md)). Exit 0 при `ok/warn`,
  4 при `FAIL`, 2 при невалидной конфигурации.
