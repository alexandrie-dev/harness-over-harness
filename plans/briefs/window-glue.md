# Склейка движка для вехи «окно-чат» (пункт «в»), 19.11 14:30

Цель: `apps/hoh` запускается как движок окна (`hoh` с профилем окна): JSON-RPC 2.0 построчно по stdio,
тот же протокол, что сейчас отдаёт заглушка `crates/hoh-window-stub` (это эталон того, что вызывает окно),
но на настоящих пакетах. Источник контракта: `docs/window.md` §4 (протокол, путь подтверждения),
`crates/hoh-protocol` (типы, PRIVILEGED_METHODS), `crates/hoh-window-stub/src/*` (что именно окно вызывает
и что ждёт в ответ), `apps/hoh-desktop/src/{engine,approval,bridge}.rs` (как ядро Tauri запускает движок,
что шлёт в `initialize`, как считает mac подтверждения).

Сценарий вехи, который движок обязан пройти: `initialize` (ключ подтверждений, `capabilities.approvals:"host"`)
→ первый запуск: `hoh/model/probe`, `hoh/model/key/set`, `hoh/settings/get|set` → `session/prompt` →
потоковые `session.event` от настоящей модели OpenAI-протокола → инструменты bash / read / write /
working-directory / todo / ask-user, шаги в ленте → опасное действие: `approval/request` с mac →
`session/events` (история), `session/list`, `session/cancel` → `shutdown`.

Правило: не ждать соседей. Где сосед ещё не сдал часть — тонкий слой (функция/трейт с тем же
контрактом) в СВОИХ файлах с пометкой `GLUE-SHIM(<часть>)`; при стыковке l1 заменяет его настоящим.
Новые внешние зависимости не добавлять (если без них нельзя — остановиться и описать).

## Части (разные файлы, параллельно)

| Часть | Кто | Файлы (только свои) | Готово, когда |
|---|---|---|---|
| 1. Профиль и композиция сервисов; владелец сборки целого | l1 | `apps/hoh/**` кроме `apps/hoh/tests/window_e2e*.rs` | `hoh` с профилем окна поднимает все плагины ниже; тест `apps/hoh/tests/window_profile.rs`: `initialize` → `session/prompt` на mock → поток событий → `shutdown`, код 0 |
| 2a. Пакет `dsh-sdk-app` → `hoh-sdk-app` | w8 | `crates/hoh-sdk-app` (новый) | порт packages/bundle/sdk-app, тесты |
| 2b. stdio и жизненный цикл сервера | w3 (сразу после текущего крейта) | `crates/hoh-sdk-jsonrpc-server` | `initialize` с `hoh:{protocolVersion}`, `capabilities.approvals:"host"`, ключ подтверждений (повторный `initialize` — ошибка); `session/list`, `session/events`, `session/cancel`; таблица регистрации методов `hoh/*` для других крейтов; `shutdown` |
| 3. Модель и ключ | w7 | новый `crates/hoh-model-host` | плагин: `hoh-llm-openai` как поставщик модели для `hoh-llm`; ключ через `hoh-credentials` (не в открытом конфиге); методы `hoh/model/probe`, `hoh/model/key/set`, `hoh/settings/get|set` (поля модели) — регистрация через таблицу 2b |
| 4. Подтверждения в режиме host | ui1 | `crates/hoh-user-approval` | `approval/request` {approvalId, risk, humanText, actionHash, reason, expiresAt}; ответ `{approvalId, outcome, mac}` проверяется HMAC-SHA256 ключом из `initialize` (каноническая запись как в `apps/hoh-desktop/src/approval.rs`); срок 5 мин → cancelled; решение однократное; уведомления `approval.pending`/`approval.resolved`; инструменты bash/fs идут через этот путь |
| 5. Сквозной тест на mock | ui2 (после «б») | `apps/hoh/tests/window_e2e.rs` | сценарий вехи целиком через stdio собранного `hoh`, модель — mock «б» |
| б. OpenAI-совместимый сценарный mock | ui2 (первым) | новый `crates/hoh-llm-openai-mock` (lib + bin) | `GET /v1/models`, `POST /v1/chat/completions` (SSE, `tool_calls`, usage); сценарии из файла: поток текста, вызов `bash`, `read`, `write`, опасная команда; журнал запросов для проверок |
| г. Окно на настоящем движке | cf1 | `apps/hoh-desktop/**`, `ui/**` | ядро запускает `hoh` (путь из сборки; заглушка только при `HOH_TEST_ENGINE`), первый запуск пишет ключ через `hoh/model/key/set`, поток и подтверждение работают; probe окна PASS на mock |

Инструменты (bash, fs, working-directory, todo, ask-user) уже есть как плагины (`ToolBash`, `ToolFs`, …):
их подключение в профиль — часть 1.

Сдача по частям: каждая часть сдаётся отдельным отчётом, первая строка
`ГОТОВ <крейт>: тесты N/0, clippy, fmt` для крейтов; для частей 1 и 5 (`apps/hoh`) первая строка
`ЧАСТЬ apps/hoh <1|5>: тесты N/0, clippy, fmt` — их ревью запускает ведущий; ревью идёт по мере сдачи. Сборка: `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-<ты>`,
`--offline`, `CARGO_BUILD_JOBS=2`, тесты только под `timeout`. Ничего не устанавливать, не коммитить.
Порт 8000 занят чужим TLS-прокси — не использовать. GPU не трогать.
