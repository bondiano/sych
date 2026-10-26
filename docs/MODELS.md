# Model gateway и ledger

Реализация M1.3 roadmap: §16.4–16.7, §17 [спецификации](SYCH_SPEC.md). Transports — OpenRouter (M1.9),
OpenCode Go (M1.12), GLM Coding Plan через Pi (M1.11) и GPT через Sign in with ChatGPT (M1.10), router (M1.13) — ниже.
Внешний вызов модели делают только явный `sych doctor --probe-model PROFILE` и `sych review --models PROFILE`
([ANALYSIS.md](ANALYSIS.md)).

## Контракт

- `sych_core::model::ModelProvider` — один transport: `ModelRequest` → `ModelReply | ProviderError`.
  Adapter не считает бюджет и не повторяет запросы; это делает gateway.
- `Route` фиксирует profile, provider, adapter, запрошенную модель, `billing_kind`, `enforcement` и
  `Capabilities` (native structured output, enforced output cap, usage/cost reporting, hidden upstream calls).
  `ModelReply` хранит фактическую модель, upstream provider и generation id отдельно от запрошенной.
- `ProviderError.kind` различает `AUTH`, `ENTITLEMENT`, `QUOTA_EXHAUSTED`, `RATE_LIMITED`, `TRANSIENT`,
  `TIMEOUT`, `UNSUPPORTED`, `BILLING_MISMATCH`, `CANCELLED`, `CONFIG`; `dispatched` — мог ли запрос дойти до provider.
- `Prompt` собирается из слоёв §16.5: role → output contract → trusted rules (system message) → task →
  data blocks (user message). Каждый data block обрамлён `<<<DATA nonce label>>>…<<<END nonce>>>`, где nonce —
  часть SHA-256 самого содержимого, поэтому текст repository не может закрыть свою рамку. Template id с версией
  (`discovery@1`) и имя схемы сохраняются в каждом attempt; chain-of-thought не запрашивается и не хранится.

## Structured output и repair

- Схема ответа генерируется `schemars` из того же Rust-типа, которым ответ разбирается; provider получает её,
  если поддерживает native structured output.
- Ответ разбирается целиком (`deny_unknown_fields`, без хвостового текста). Допустим только один code fence,
  обрамляющий весь ответ; JSON из прозы не извлекается.
- Невалидный ответ получает максимум один repair: исходные сообщения + ответ модели + ошибка схемы. Repair —
  отдельный attempt с собственной reservation в том же бюджете. Повторная ошибка — `TaskFailure::Schema`.
  `TRUNCATED` и `REFUSED` ответы не считаются результатом.

## Бюджет перед dispatch

Для каждого attempt до отправки:

1. Input bound — число UTF-8 байт сообщений + 16 на сообщение (не меньше числа токенов byte-level BPE).
2. Output bound — 2 000 токенов на request (§17.1); `max_output_tokens` посылается только если route его
   соблюдает (`output_cap_enforced`).
3. Денежная верхняя граница: `api_usage` — по price card из `price_catalog_file`, округление вверх, без скидок
   cache. Нет price card или нет enforced output cap — `NO_PRICE_BOUND`, запрос не отправляется.
   `subscription_quota` — 0 на request (overage выключен), но provider обязан иметь `fixed_fee`, иначе
   `FIXED_FEE_UNKNOWN`.
4. Одна транзакция SQLite проверяет и записывает reservation: attempts/input/output tokens и деньги run
   (по purpose `ANALYSIS | TOUR | FOLLOWUP`), деньги дня и календарного месяца UTC. Output tokens — hard только
   для `api_hard`; для `subscription_observable` upstream output/quota — soft/unknown, а Sych attempts,
   input и деньги остаются hard.
5. Verification reserve (§17.1): чистый поиск (discovery, high-risk audit) резервирует с caps без одной attempt
   и её доли денег run (`cap / max_attempts`), поэтому не может израсходовать то, что нужно verifier; assessment
   и verifier видят полные caps. Retry, repair и fallback тратят тот же reserve-aware остаток.

Invariant: settled charges + удерживаемые reservations + unknown charges ≤ cap.

## Settlement

| Исход | Состояние | Деньги |
|---|---|---|
| Запрос не ушёл (`dispatched=false`) | `RELEASED` | reservation освобождается |
| Ответ с reported cost или полным usage | `SETTLED` | фактический charge; reference estimate отдельно |
| Подписка без reported money | `SETTLED`, `charge=NULL` | не ноль, а «не сообщено»; fee учтён отдельно |
| Timeout/cancel/обрыв после dispatch, metered ответ без cost и usage | `UNKNOWN` | reservation удерживается (`UNKNOWN_CHARGE`), usage `NULL` |

Settlement идемпотентен по attempt id: `SETTLED` не меняется повторно, `UNKNOWN` можно позже reconcile в
известный charge. Charge выше reservation сохраняется и помечается `billing_mismatch`; для подписки это
блокирует следующие subscription-only reservations этого provider в месяце (`BILLING_MISMATCH`).

## Fixed subscription fee

`models.providers.<name>.fixed_fee: { amount_micro_usd, allocated_micro_usd }` — цена подписки за календарный
месяц UTC и доля, которую владелец относит на модельный pool ($60/month, D08). Первая reservation подписки в
месяце записывает строку `subscription_fees`; доля учитывается в месячном лимите один раз, не на каждый request
и не в дневном/run лимитах. Значения fee/allocation не заполняются по умолчанию: без них subscription profile
не используется.

## Price catalog

```json
{
  "schema_version": 1,
  "revision": "openrouter-2026-10-25",
  "prices": {
    "deepseek/deepseek-v4.1-flash": { "input_per_mtok": 300000, "output_per_mtok": 1200000 }
  }
}
```

Цены — micro-USD за миллион токенов, reasoning оплачивается как output; ключ — slug модели профиля.
Если профиль перечисляет несколько upstreams, цена берётся по самому дорогому из них. `sych doctor` разбирает
catalog и печатает revision; цены вносит operator по актуальному тарифу, Sych их не угадывает. Пример с ценами
OpenRouter на 2026-10-25 — `examples/personal/config/prices.json`.

## OpenRouter (M1.9)

Adapter `sych_adapters::openrouter` — один HTTP `POST {endpoint}/chat/completions`, без SDK и agent loop.

- **Pinned routing.** `model` — точный slug (`vendor/model`); `openrouter/*`, `~…` и `*latest` отклоняются.
  Профиль обязан перечислить `upstreams`; запрос посылает `provider.order = upstreams`,
  `allow_fallbacks: false`, `require_parameters: true`, `data_collection: "deny"` и не посылает `models`
  (fallback-список моделей). Price card профиля должна покрывать самый дорогой из перечисленных upstreams.
- **Reasoning.** `reasoning_effort: low|medium|high` профиля посылается как `reasoning.effort`; без него поле не
  посылается. Reasoning входит в output cap одного request (2 000 tokens, §17.1), поэтому для DeepSeek в примере
  `low`: при default effort verify-ответ обрезался (`TRUNCATED`) после ~1 900 reasoning tokens. Cap одного
  request — `budgets.<purpose>.max_output_tokens_per_request` (по умолчанию 2 000, не больше `max_output_tokens`);
  money upper bound reservation считается от него. Вместе с
  `require_parameters` upstream без поддержки параметра не выбирается.
- **Structured output.** `response_format: json_schema` со `strict: true`; локальная strict-валидация и
  repair выполняются всегда. `require_parameters` не даёт OpenRouter молча выбрать upstream без поддержки схемы.
- **Streaming.** Запрос идёт с `stream: true`: keep-alive комментарии `: OPENROUTER PROCESSING` держат соединение
  во время долгого reasoning, чанки SSE складываются в тот же ответ, что и без streaming (контент, finish reason,
  generation id, модель, upstream; usage и cost — из последнего чанка). Поток, оборвавшийся до `[DONE]` и finish
  reason, — dispatched `TRANSIENT`/`TIMEOUT` с generation id в сообщении, а не частичный ответ; ошибка внутри
  потока разбирается как ошибка после 200. Ответ не 200 приходит обычным JSON.
- **Usage и деньги.** Сохраняются generation id, фактическая модель, upstream provider, prompt/completion/
  reasoning tokens (reasoning входит в completion, не суммируется) и `usage.cost`. USD переводится в micro-USD
  по десятичной записи числа с округлением вверх, без float.
- **Ошибки.** 401 → `AUTH`, 402 → `QUOTA_EXHAUSTED`, 403 → `ENTITLEMENT`, 408 → `TIMEOUT`, 429 →
  `RATE_LIMITED` (+`Retry-After`), 503 → `UNSUPPORTED` (нет upstream под заданные требования), прочие 5xx →
  `TRANSIENT`. 400/401/402/403/429/503 и отказ соединения считаются не dispatched (reservation освобождается);
  408, 5xx, обрыв ответа и `error` внутри ответа 200 — dispatched, charge остаётся `UNKNOWN`.
  `finish_reason: length`/неизвестный → `TRUNCATED`, `content_filter`/`refusal` → `REFUSED`.
- **Egress.** Только `endpoint` провайдера из `sych.yaml` (allowlist), без redirects и system proxy, timeout 90 s.
  Ключ — из переменной `api_key_env`, в выводе и ошибках не появляется. Corporate mode с OpenRouter
  отклоняется валидацией конфигурации (D10).

### Fixtures

Deterministic, без сети: capability — metered route без enforced output cap не отправляется
(`crates/sych-core/tests/gateway.rs`); schema — strict разбор и один repair (там же); partial stream — оборванный
и упавший поток, `length` (`crates/sych-adapters/tests/openrouter.rs`); unknown charge — timeout после dispatch и
ответ без cost и usage (`gateway.rs`); provider selection — pinned `model`, `provider.order`, `allow_fallbacks:
false`, `require_parameters` (`openrouter.rs`); весь путь doctor → gateway → ledger → adapter — loopback stub
(`crates/sych-app/tests/probe.rs`). Loopback stubs отвечают 404 на чужие подключения: локальные инструменты
(например, `moshi-hook`) опрашивают каждый новый порт на loopback.

### Ключ для разработки

Dev-ключ хранится вне репозитория в `~/.config/sych/dev.env` (права 600) как
`SYCH_OPENROUTER_API_KEY=…`; `.envrc` чекаута (`dotenv_if_exists "$HOME/.config/sych/dev.env"`, исключён
через `.git/info/exclude`) загружает его direnv. Ключ не коммитится и не пишется в `sych.yaml`.

### Probe

```sh
sych db migrate --config sych.yaml
sych doctor --config sych.yaml --repo . --probe-model cheap_api
```

Делает один маленький платный structured request (echo) через gateway и ledger и печатает JSON: route
(profile, model, upstreams, billing, enforcement, price revision), `valid`, `failure` и attempts (reserved,
charge, charge state, tokens, resolved model, upstream, generation id). Run хранится в ledger под access scope
`model-probe` и закрывается без review result. Exit 0 — валидный ответ, 4 — нет ключа или `AUTH`,
3 — иная ошибка, 2 — конфигурация.

## OpenCode Go (M1.12)

Adapter `sych_adapters::opencode_go` — `POST https://opencode.ai/inference/go/openai/v1/chat/completions`
([Go](https://opencode.ai/v2/docs/console/go/), [inference contract](https://opencode.ai/v2/docs/console/api/inference/)).
Общий с OpenRouter разбор SSE chat completions — `sych_adapters::chat`.

- **Граница `/go`.** `endpoint` провайдера `opencode_go` обязан быть ровно
  `https://opencode.ai/inference/go/openai/v1` (или loopback в тестах); путь без `/go` списывает Console balance
  и отклоняется валидацией конфигурации. Model — точный Go id (`deepseek-v4.1-flash`), без `opencode-go/` и
  `*latest`.
- **Заголовки.** `User-Agent: sych/<version>`; `x-opencode-session` — id задачи gateway (`TaskId`): первый
  attempt и его repair идут в одной сессии, следующая задача получает новую. Ключ — из `api_key_env`,
  auth-файлы OpenCode не читаются.
- **Схема.** Native structured output не заявляется (`response_format` и `max_tokens` не посылаются):
  ответ проверяется локально, repair максимум один. Output cap и quota — soft (`subscription_observable`).
- **Extra Usage.** Sych его не включает и не проверяет через API (его нет). Владелец подтверждает настройку в
  Console и ставит `overage_disabled: true`; без этого profile не резолвится (`sych doctor` об этом пишет).
  DeepSeek в Go требует Global region в Settings › Privacy — это тоже настройка аккаунта, не Sych.
- **Деньги и квота.** `usage.cost` — reported charge (0 внутри лимитов Go — наблюдённый факт, не «бесплатно»);
  `opencode.billing_source` и `opencode.allowance` (расход квоты в долларах плана) сохраняются в attempt
  отдельно; `opencode.list_cost` — только reference. Charge выше нулевой reservation (`balance`/`byok`) ledger
  помечает `billing_mismatch`, и следующие subscription-only calls этого provider в месяце отклоняются.
  Quota windows/reset Go не сообщает в ответе — они остаются неизвестными, не выдумываются.
- **Ошибки** — те же, что у OpenRouter (общий `chat`): 401/403/429/5xx и обрыв потока.

Fixtures: `crates/sych-adapters/tests/opencode_go.rs` (путь, заголовки, session, body без схемы, zero charge +
allowance, balance charge), `crates/sych-core/tests/gateway.rs` (allowance отдельно от charge, repair в той же
задаче), `crates/sych-core/tests/config.rs` (граница `/go`).

## GLM Coding Plan через Pi (M1.11)

Z.ai разрешает Coding Plan в поддерживаемых coding tools ([usage rules](https://docs.z.ai/legal-agreement/subscription-terms),
[Pi](https://docs.z.ai/devpack/tool/pi)), а собственный HTTP/bot — нет. Поэтому adapter `sych_adapters::pi`
запускает настоящий Pi CLI, а не имитирует его запросы.

- **Pin.** Только Pi `1.4.0` (`@earendil-works/pi-coding-agent@1.4.0`, npm integrity
  `sha512-Z41LpZ3yYHSC/UUps6Mr39UMFPwH8elhJBG9DY1P7kuzsE5qSK/6eMj2EHzO5evJRQWLbKSqUMw+4nCNtIyiQg==`): resolve
  выполняет `--version` и отклоняет другой release. Бандл самодостаточен (нужен Node ≥ 22.19):

  ```sh
  npm pack @earendil-works/pi-coding-agent@1.4.0 && tar xzf earendil-works-pi-coding-agent-1.4.0.tgz
  export SYCH_PI_BINARY="$PWD/package/dist/bundle/cli.js"
  ```
- **Endpoint и ключ.** Provider `glm_coding_plan` с `transport: supported_pi_cli` обязан указать
  `endpoint: https://api.z.ai/api/coding/paas/v4` (валидация; Standard API `/api/paas/v4` — другой billing) и
  `executable_env`. Pi получает endpoint как `baseUrl` провайдера `zai` в `models.json`, ключ — только как
  `ZAI_API_KEY` в окружении процесса (не в argv). Тип ключа (Coding Plan, а не Standard API) Sych определить не
  может — это подтверждает владелец; `overage_disabled: true` — подтверждение, что extra usage выключен.
- **Изоляция.** Каждый request — новый процесс во временном каталоге: пустые `HOME`, `PI_CODING_AGENT_DIR` и
  рабочий каталог, `env_clear` (наследуется только `PATH`), флаги `--mode json --no-session --no-tools
  --no-extensions --no-skills --no-prompt-templates --no-themes --no-context-files --no-mcp --no-approve --offline`,
  `PI_SKIP_VERSION_CHECK=1`, `PI_TELEMETRY=0`. `settings.json`: `retry.enabled: false`,
  `retry.provider.maxRetries: 0`, `compaction.enabled: false`, `cacheWarming: off`, `defaultProjectTrust: never`.
  System message — файл `--system-prompt`, остальное — один prompt в stdin (repair передаёт прошлый ответ в том же
  prompt). Timeout 90 s убивает процесс; upstream расход остаётся unknown.
- **Ответ.** Из JSONL берётся единственный assistant `message_end` после `agent_settled`. `tool_execution_start`,
  `auto_retry_start`, `compaction_start`, второй model turn, `toolUse` → `UNSUPPORTED`; поток без `agent_settled` →
  dispatched `TRANSIENT`; `length` → `TRUNCATED`; `aborted` → `CANCELLED`. JSON проверяется общей локальной
  валидацией, native schema не заявляется.
- **Usage и деньги.** Tokens — из Pi usage (input = input + cache read/write; reasoning входит в output), нулевой
  usage — unknown. `usage.cost` Pi считает по своему каталогу — это `reference`, не charge; charge подписки не
  сообщается (`NULL`), fixed fee учитывается раз в месяц. Quota windows Z.ai в ответе не приходят.
- **Ошибки** — по `errorMessage` Pi (`<status>: <body>`) и кодам Z.ai ([api-code](https://docs.z.ai/api-reference/api-code)):
  1000/1001/1003/1005 → `AUTH`; 1308/1310/1316–1321 (5-hour/weekly/monthly limit, сообщение с reset) →
  `QUOTA_EXHAUSTED`; 1113/1309/1311/1313/1314/1315/1220 (нет пакета, план истёк, модель вне плана, fair use) →
  `ENTITLEMENT`; 1302 → `RATE_LIMITED`; 1305/1234/5xx → `TRANSIENT`. Ни одна из них не меняет key/endpoint и не
  включает paid API.

Fixtures: `crates/sych-adapters/tests/pi.rs`; JSONL в `tests/fixtures/pi` записан реальным Pi 1.4.0 против loopback
stub (`ok`, `err401`; `quota` — тот же поток с кодом 1308). Stand-in executable проверяет env, argv, stdin,
пустой рабочий каталог и pinned endpoint. Сквозной `sych doctor --probe-model` через реальный Pi и loopback stub
(2026-10-25): `SETTLED`, 50/7 tokens, charge `NULL`, reference 101 micro-USD; stub получил `User-Agent: pi/1.4.0`,
наш system prompt, fenced packet и тело без `tools`. Реальный Coding Plan smoke ещё не выполнен.

## GPT через ChatGPT plan: Sign in with ChatGPT (M1.10)

Transport — SIWC для open-source local clients ([overview](https://developers.openai.com/siwc/token-sharing-open-source),
[sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in),
[inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference),
[limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations),
[errors](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery)). Codex app-server не
используется: text-only Responses не имеет agent loop, и no-tools boundary не нужно доказывать отдельно.
Обычный OpenAI API key этой подпиской не является: provider с `api_key_env` отклоняется валидацией.

- **Вход.** `sych auth chatgpt --config sych.yaml --provider gpt_subscription` — authorization code + PKCE (S256)
  с loopback redirect `http://127.0.0.1:1455/auth/callback`, dynamic registration (`client_id=dynamic_agent_client`,
  `agent_name_hint=Sych`), scope `openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`,
  `resource=https://api.openai.com/v1`. `ext_agent_host_id` — случайный `urn:uuid`, выбирается один раз (файл
  `<token file>.host`) и переиспользуется вместе с выданным `client_id`. Callback проверяет `state` и выданный
  client id; ID token, полученный напрямую от token endpoint по TLS, проверяется по `iss` (из
  `/.well-known/openid-configuration`), `aud`, `exp` и `nonce` (OIDC Core §3.1.3.7 вместо подписи). Без granted
  `chatgpt.tokens.use.direct` вход не сохраняется. Другой `sub` не перезаписывает файл существующего аккаунта.
  На удалённой машине — `ssh -L 1455:127.0.0.1:1455 HOST` либо вставить адрес, на котором остановился браузер.
  После входа команда печатает модели аккаунта (`GET /v1/models`, `visibility == list`) — inference не делается.
- **Токены.** Файл — путь из `connection_ref_env` (вне репозитория), атомарная запись `0600`; поля по sign-in
  контракту (issuer, subject, client id, host id, id/access/refresh token, expiry, scopes). Перед request токен
  обновляется за 120 s до истечения (`grant_type=refresh_token` с выданным client id и `resource`); refresh
  сериализован lock-файлом, rotated refresh token сохраняется вместе с access token. `invalid_grant` и прочие
  terminal refresh codes → `AUTH` (войти заново), файл не стирается; `invalid_client` → `CONFIG`; сеть → `TRANSIENT`.
  Токены не логируются и не передаются в другие процессы.
- **Request.** `POST https://api.openai.com/v1/responses`: `model`, `instructions` (trusted слои; system items
  route отклоняет), `input` (user/assistant), `store: false`, `stream: true`, `text.format` = strict
  `json_schema`. Не посылаются `max_output_tokens`, sampling, `metadata`, `user`, tools. Endpoint pinned
  валидацией; ChatGPT `backend-api` не используется. Поддержку `text.format` на этом route документация не
  подтверждает: при `subscription_sharing_unsupported_capability` ошибка `UNSUPPORTED` с `param`, параметр молча не
  отбрасывается.
- **Ответ.** Успех — только `response.completed` (text deltas, `usage.input_tokens/output_tokens`,
  reasoning из `output_tokens_details`, response id и фактическая модель). `response.incomplete` → `TRUNCATED`
  (`content_filter` → `REFUSED`), refusal → `REFUSED`, output item кроме `message`/`reasoning` (tool call) →
  `UNSUPPORTED`, обрыв до `completed` → dispatched `TRANSIENT`, `response.failed` → по коду, dispatched.
- **Ошибки** (`error.code`, иначе HTTP): `usage_limit_exceeded` → `QUOTA_EXHAUSTED` (reset не выводится),
  `user_not_eligible`/403 → `ENTITLEMENT`, `invalid_user`/401 → `AUTH`, `unsupported_capability` → `UNSUPPORTED`,
  `route_not_supported`/`chatpass_v2_*` → `CONFIG`, `usage_unavailable`/`user_unavailable`/503 → `TRANSIENT`.
  Ошибки до потока (admission, 4xx, 503) — не dispatched. OpenAI сам billing path не переключает; Sych тоже.
- **Учёт.** `subscription_observable`: charge `NULL`, `billing_source: chatgpt_plan`, tokens — по факту; стандартные
  API prices к плану не применяются; fixed fee — раз в месяц. `allowed_modes` — только `personal_local`: SIWC
  для paid или remotely hosted app требует отдельного admission.

Fixtures: `crates/sych-adapters/tests/siwc.rs` (поля route, completed/incomplete/refusal/cut, tool item, коды
ошибок, refresh с rotation и `0600`, revoked session без inference, нет файла/scope) и `tests/siwc_auth.rs`
(base64url, PKCE RFC 7636, host id, claims, callback). Публичный discovery `auth.openai.com` (2026-10-25):
issuer и endpoints совпадают с константами; authorize открывается только браузером (Cloudflare challenge), поэтому
реальный вход и smoke ждут владельца.

## Router: роли, fallback и premium (M1.13)

`sych_core::model::Router` — один детерминированный loop поверх `Gateway`; отдельного оркестратора нет.

- **План.** На run фиксируется `RoutePlan` (записан в `analysis` artifact с revision-хешем): для ролей
  `reviewer`, `verifier`, `risk_auditor` — primary и одна цепочка. `--models roles`: primary — `models.roles.<role>`
  (нет роли — profile reviewer'а; в `deep` manual run сначала `models.deep_roles.<role>`), цепочка по приоритету `fallback.chains.profiles.<primary>` →
  `chains.roles.<role>` → `chains.providers.<provider primary>`. `--models PROFILE` закрепляет profile для всех
  ролей и берёт только его собственную цепочку (profile, затем provider), без role chains — чтобы сравнение
  cheapest-only не смешивалось с подписками. Цепочки плоские: chain fallback-профиля не подключается, поэтому
  loop не перезапускается (взаимные A → B и B → A заканчиваются после второго hop); дубликат в цепочке,
  self-fallback, неизвестный profile, profile с именем селектора `fake` или `roles` и `wait_for_usage_reset: true`
  отклоняются валидацией. В `analysis` artifact на каждый route записаны model, `reasoning_effort` и `upstreams`.
- **Роли проходов.** Assessment, review и discovery — `reviewer`; obligation-bound high-risk audit — `risk_auditor`
  (там, где `models.roles.risk_auditor` указывает Astra/Fable, это и есть premium escalation: только для
  high-risk obligations, не на весь PR); проверка candidates — `verifier`. Более дорогая модель не заменяет
  недостающие source/environment facts: такие obligations остаются `UNKNOWN`.
- **Hop.** Каждый profile цепочки посещается не больше одного раза за задачу. Перед hop'ом profile резолвится
  заново (mode, billing, egress, credentials, pinned runtime), и route должен совпасть с запланированным в начале
  run; route без structured output пропускается (все роли ждут structured ответ). Ещё проверяются cooldown и —
  внутри gateway — те же run/day/month caps,
  attempts и deadline в одном ledger: смена backend счётчики не сбрасывает, unknown charge прошлого route остаётся
  зарезервированным, repair — отдельный учтённый attempt. Не разрешающийся fallback-профиль остаётся в плане со
  своей причиной и пропускается; явный `--models PROFILE` должен резолвиться сам, `roles` — хотя бы один profile
  цепочки reviewer'а.
- **Когда переходить.** Только по `fallback.on`: `quota_exhausted`, `rate_limited`, `transient`, `timeout`,
  `unsupported_capability`. Ещё — когда route сам не может быть учтён (`NO_PRICE_BOUND`, `FIXED_FEE_UNKNOWN`,
  `BILLING_MISMATCH`): следующий route делает новую reservation. `AUTH`, `ENTITLEMENT`, `CONFIG`, `CANCELLED`,
  общие caps/deadline, schema/truncated/refused ответ — стоп без повтора. Тот же route повторно в задаче не
  вызывается; ожидания reset нет.
- **Cooldown.** Упавший по trigger'у profile отдыхает `max(cooldown_seconds, Retry-After)`; следующие задачи run
  его пропускают (`SKIPPED`, «cooling down until …») и после истечения снова начинают с primary
  (`revert_policy: cooldown_expiry`).
- **Итог.** `hops` в `analysis` artifact: роль, profile и `ANSWERED | FELL_BACK | STOPPED | SKIPPED` с failure/
  причиной. Все skipped → `NO_ELIGIBLE_ROUTE` с причинами; провал pass становится gap'ом result (PARTIAL), не
  пустым успехом. `usage_summary.routes` перечисляет каждый route с attempts, quota attempts и fixed fee отдельно.
- **Границы.** Подписочные adapters (ChatGPT, GLM, OpenCode Go) валидацией допускаются только в
  `personal_local`: личный login не уходит в hosted bot или corporate. Corporate config не может иметь
  OpenRouter или подписку ни как route, ни как fallback — пример `examples/corporate/sych.yaml`.

Fixtures: `crates/sych-core/tests/router.rs` (cheap → subscription → premium в одном ledger, auth-стоп, cooldown
и возврат к primary, исчерпанный cap, unknown charge, ненастроенный trigger, нет eligible route, route без fee
передаёт следующему, приоритет цепочек), `tests/config.rs` (дубликаты, self-fallback, reset wait, corporate
deny-external, subscription только local). Сквозной smoke (2026-10-25, loopback): `review --models roles`, primary
`cheap_go` на закрытом порту → `RELEASED`, fallback `glm_sub` через реальный Pi 1.4.0 → два attempt (stub отвечал
не по схеме) → gap; оба route в `usage_summary.routes`.

## Ограничения

- Реальные subscription smokes (ChatGPT plan, GLM Coding Plan, OpenCode Go) ещё не выполнены: нужны вход
  владельца, ключи, `fixed_fee` и подтверждённый `overage_disabled`. Fixtures и loopback smokes их не заменяют.
- Cooldowns живут в одном run; между процессами они не сохраняются.
- Reconciliation `UNKNOWN` attempts через `GET /generation` ещё не реализован: generation id оборванного потока
  есть в сообщении ошибки attempt, но reservation пока остаётся удержанной.
- Реальные probes со streaming (2026-10-25, price revision `openrouter-2026-10-25`, цены совпали с каталогом
  OpenRouter):

  | профиль | resolved model | upstream | tokens in/out | charge |
  |---|---|---|---|---|
  | `cheap_api` | `deepseek/deepseek-v4.1-flash` | DeepInfra | 127/49 | 39 micro-USD |
  | `premium_astra` | `openai/gpt-6-astra` | OpenAI | 131/14 | 2 010 micro-USD |
  | `premium_fable` | `anthropic/claude-fable-5.1` | Anthropic | 348/15 | 4 230 micro-USD |

  Артефакты: [cheap_api](examples/model-probe/openrouter-cheap_api.json),
  [premium_astra](examples/model-probe/openrouter-premium_astra.json),
  [premium_fable](examples/model-probe/openrouter-premium_fable.json). Reservation premium probe — 204 110 micro-USD
  (верхняя граница при 4 000 output tokens). Первый probe Fable вернул `REFUSED` без usage, и charge остался
  `UNKNOWN`: generation в OpenRouter не записан, повтор прошёл. Сырого ответа первого запроса нет, но теперь ответ,
  который не завершился `stop`, пишет в лог `finish_reason`, `native_finish_reason` и начало refusal.
