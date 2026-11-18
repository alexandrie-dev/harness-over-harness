# Путь к «окну-чату»

Срез: 19.11.2026, MAP и записи ведущего до 23:57 18.11. Только чтение исходников; запусков не было. `P/` ниже означает `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/`; `M:n` — `docs/porting/MAP.tsv:n`, статус из колонки 8. Имена crates сокращены без `hoh-`.

**1. Что отложить из A**

Без переписывания выбранных зависимостей JSONL из 50 позиций A целиком исключается `llm-replay`. Остальные сохраняются, включая небольшой обязательный срез image-offload. `conformance` в эти 50 вообще не входит. Сейчас в A **27 done, 23 непринятых**, а не прежние 24/26: приняты session, shell, v1→v2 (`M:30,85,73`).

| Пакет | Решение и доказательство |
|---|---|
| `llm-replay`, todo (`M:99`) | **Можно отложить.** Загружает записанные сценарии и возвращает их `StreamChunk`, а не обращается к модели: `P/test-support/llm-replay/src/index.ts:1019–1032`. Для живого провайдера нужен OpenAI-адаптер. |
| `session-format-v0-to-v1`, done (`M:72`) | **Целиком нельзя отложить** без изменения каталога: прямой импорт значений в `P/session/session-format-catalog/src/generated.ts:10`. Уже принят, экономии оставшейся работы нет. |
| `session-format-v1-to-v2`, done (`M:73`) | **Нужен** также текущему кодеку: импорт каталога `generated.ts:11`; V4 вызывает V2 encoder (`P/session/session-format-v3-to-v4/src/codec.ts:44,52`), определённый в `P/session/session-format-v1-to-v2/src/codec.ts:25–38`. |
| `session-format-v2-to-v3`, writing:c4 (`M:74`) | **Целиком нельзя отложить:** `generated.ts:12`; V4 импортирует через этот пакет V2 codec (`P/session/session-format-v3-to-v4/src/codec.ts:5`), его реэкспорт — `P/session/session-format-v2-to-v3/src/index.ts:3`. |
| `session-format-v3-to-v4`, writing:w3 (`M:75`) | **Нужен:** здесь V4 encoder и проверки, не только миграция. JSONL напрямую импортирует проверки: `P/session/session-persistence-jsonl/src/format.ts:27`; каталог выбирает V4 encoder: `generated.ts:25`. |
| `session-format-catalog`, writing:w3 (`M:71`) | **Нужен:** JSONL импортирует значения в `P/session/session-persistence-jsonl/src/index.ts:11–15` и `format.ts:26`. Запись заголовка — `format.ts:130`, события — `:323`, чтение — `:369`. |
| `compaction-image-offload`, writing:s11 (`M:21`) | **Нужен частично; целиком нельзя отложить:** `P/session/session-format-catalog/src/current.ts:12,45–51` использует проекции; `message-projections.ts:3,7` импортирует image-offload/projection. Его `projection.ts:6` импортирует `project-message.ts`. **Можно отложить** восстановление после ошибки модели: `P/compaction/compaction-image-offload/src/index.ts:25–40`. |
| `conformance`, review (`M:101`) | **Можно отложить для этой вехи**, сохранив отдельную проверку живого чата при сдаче. Это внешний прогонщик: `P/test-support/session-snapshot/src/harness.ts:6–12` запускает проверяемый процесс и сравнивает результаты; `:24` импортирует Vitest. |

**Да, даже новый V4-журнал подтягивает цепь значениями.** `P/session/session-format-catalog/src/index.ts:3` экспортирует `generated.ts`; там `:10–13` обычные импорты, `:18–30` собирают кодеки и миграции, `:48` сразу создаёт каталог. Функции преобразования старых журналов можно не вызывать, но это не устраняет зависимости crates. Отдельный V4-каталог потребует переделки; её объём не оценён. Нынешний Rust JSONL пока подменяет каталог и V4 проверки (`crates/hoh-session-persistence-jsonl/src/shim.rs:2–4`); этот временный код не доказывает готовность без них.

**2. Самая длинная прослеженная цепь до сдачи**

Начало уже принято: session-format, session, v0→v1, v1→v2 (`M:70,30,72,73`). Далее:

1. `session-format-v2-to-v3` — writing:c4; импорт предшественника: `P/session/session-format-v2-to-v3/src/index.ts:3`.
2. `session-format-v3-to-v4` — writing:w3; зависимость от предыдущего: его `src/codec.ts:5`.
3. `session-format-catalog` — writing:w3; `src/generated.ts:12–13`.
4. `session-persistence-jsonl` — todo, пишет c4; `src/format.ts:26–27`. Здесь также нужна приёмка persistence: review, исправляет s12.
5. `agent-loop` с настоящим сохранением — todo, s6; `P/core/agent-loop/src/index.ts:690–696` получает backend и создаёт журнал. Это соединение ветвей, не прямой импорт JSONL.
6. `sdk-jsonrpc-server` и подключение живой модели/подтверждений — todo (`M:67`), назначение исполнителя не найдено; `P/sdk/server/src/index.ts:21`, `server.ts:123–139,592`. Нужны `user-approval` (todo, `M:48`) и решение о запросе разрешения до исполнения: pipeline обращается к approval в `P/core/tools/src/pipeline.ts:457,470`. `sdk-protocol` — todo (`M:66`); HoH-типы уже приняты первым кругом (`plans/session-state.md:750`).
7. Рабочий профиль `apps/hoh` через loader — todo / writing:l1 (`M:13,2`); нынешние зависимости приложения ещё неполны (`apps/hoh/Cargo.toml:12–37`).
8. Подключение движка к `apps/hoh-desktop`, первый ввод адреса/ключа, поток и отдельное подтверждение — новая интеграция без найденной приёмки: `docs/window.md:110,133–146,170–171`. Чат ui2 пока назначен против заглушки (`plans/briefs/window-ui3-chat.md:6,14–26`); ключ хранится через уже done credentials (`M:34`).

До пункта 5 параллельно сходятся: projection (review, s12) → agent (todo, s6), обязательность — `P/core/agent/src/index.ts:247`; tools (review, s7b) → native executor (review, s10), импорт — `P/core/tool-executor-native/src/index.ts:5`; subprocess-local (todo, w5) → bash-local (todo, s9) → tool-bash (todo, s9), обязательные сервисы — `P/shell/bash-local/src/index.ts:98`, `P/shell/tool-bash/src/index.ts:36`. Файловую ветвь fs/fs-local пишет w4, working-directory/tool-fs — s8 (`M:39–42,83`, `plans/session-state.md:724,806`); требования tool-fs — `P/fs/tool-fs/src/index.ts:24`. OpenAI-адаптер начат, s5b отменён (`plans/session-state.md:793–794`); его сдача также блокирует живой профиль.

Это цепь готовности и интеграции. Код можно писать заранее. Длительности неизвестны, поэтому календарный критический путь и дата **не доказаны**.

**3. Объём откладываемого**

По колонке 6 MAP: целиком внутри A — **1 212 строк** replay; вне A — **4 819** conformance; вместе **6 031**. Дополнительно можно отложить **87** строк image-offload (`src/index.ts` — 41, `image-offload.ts` — 46); обязательны оставшиеся 110 (`projection.ts` — 74, `project-message.ts` — 36). Итого **1 299 внутри A**, **6 118 с conformance**. Непринятые v2→v3 + v3→v4 + catalog: **815 + 1 702 + 154 = 2 671**, их целиком в экономию не включать.

Это объём исходника непринятых пакетов, включая уже написанное; не остаток набора кода. Тестовые строки исключены. Сутки экономии без оценки оставшихся исправлений и скорости приёмки не вычисляются; исключение replay само по себе цепь выше не сокращает.

Не проверены исполнение, сборка, тесты, живая модель, полное замыкание всех импортов 50 crates, трудоёмкость переделки под V4-only. Состав расширения поверх A оставлен параллельному исследованию.
