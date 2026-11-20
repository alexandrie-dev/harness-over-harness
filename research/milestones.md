**Первый рубеж можно отделить от стандартного профиля dsh. Полная цепочка из `first-scenarios.md` для него не нужна; побайтовое сравнение не является условием примера основателя.**

Исследование только чтением: без изменений, сборок и запусков. Ниже — статически обоснованный состав; его работоспособность исполнением **не проверено**.

Обозначения: `P/` — `packages/` клона dsh; `S/` — его `snapshots/`; `M:n` — `docs/porting/MAP.tsv:n`. Имена без префикса означают `hoh-*`; исключение — `apps/hoh`. Статусы взяты из колонки 8 MAP, включая изменения относительно предыдущего отчёта.

**A. Минимальный профиль**

Граница минимизации: сохраняем существующие session/agent/tools, JSONL и replay; в `apps/hoh` пишем короткий драйвер одного запуска. Полный `hoh-headless` не переносим: он действительно импортирует approvals/query и требует `agentDefaultModel` (`P/bundle/headless/src/index.ts:22`, `:26`, `:45`).

Учитываем **используемые импорты значений, точки входа библиотек и обязательные `inject`**, а не все peers. Например, workspace объявлен optional и используется агентом через типы/события; это не обязательный сервис (`P/core/agent/package.json:50`; `P/core/agent/src/archive-admission.ts:12`).

Таблицы одновременно задают очередь писателей: сначала готовые основания, затем волны A1–A7.

| Уже готовые crates: строки; источник статуса | Зачем остаются |
|---|---|
| kernel: 4716; `M:5`, `M:6`, `M:10`–`M:12`; **done** | Сервисы, события, жизненный цикл; статическая композиция предусмотрена `docs/kernel.md:22` |
| scope: 471; **done**, `M:29` | Области session/agent/tools; `P/core/session/src/index.ts:13` |
| llm: 4245; **done**, `M:53`; system-prompt: 699; **done**, `M:31` | Поток модели и сборка контекста; `P/core/agent-loop/src/index.ts:27`, `:291` |
| attachment: 710; **done**, `M:15` | Импортируется библиотекой llm; backend не нужен: `P/llm/llm/src/content.ts:9` |
| session-format: 945; **done**, `M:70`; session-format-v0-to-v1: 2879; **done**, `M:72` | Основание каталога/цепочки кодеков; `P/session/session-format-catalog/src/generated.ts:7`, `:10` |
| subprocess: 576; **done**, `M:93`; timeout: 212; **done**, `M:120` | Контракт процессов и сроки bash; `P/shell/bash-local/src/index.ts:17` |
| fs-patch: 350; **done**, `M:41`; jsdiff: 1421; **done**, `M:409` | Реальные импорты fs-local и diff результата записи: `P/fs/fs-local/src/index.ts:7`; `P/fs/tool-fs/src/diff.ts:7` |
| node-addon-system: 646; **done**, `M:14` | flock журнала; `P/session/session-persistence-jsonl/src/lease.ts:34` |
| brand: 39; util-values: 959; error: 22; **done**, `M:105`, `M:121`, `M:110` | Идентичности, JSON и ошибки; `P/core/session/src/index.ts:11`; `P/llm/llm/src/error.ts:8` |
| util-crypto: 41; package-manifest: 167; **done**, `M:108`, `M:118` | Сообщения и модельные метаданные; `P/llm/llm/src/message.ts:3`; `crates/hoh-llm/Cargo.toml:17` |
| util-time: 1708; util-code-language: 233; **done**, `M:119`, `M:107` | Валидация исторических форматов и оформление read; `P/session/session-format-v0-to-v1/src/payload-validation.ts:2`; `P/fs/tool-fs/src/read-render.ts:182` |
| atomic-write: 293; commander: 4180; home-paths: 238; launch-environment: 137; util-workspace-path: 236; **done**, `M:104`, `M:410`, `M:111`, `M:113`, `M:122` | Уже существующие зависимости Rust loader/session: `crates/hoh-loader/Cargo.toml:14`; `crates/hoh-session/Cargo.toml:19` |

| Волна | Crate; исходных строк; статус | Необходимость и источник |
|---|---|---|
| A1 | loader; 8050; **writing:l1**, `M:2`–`M:4`, `M:7`–`M:9` | Загрузить собственный профиль; `docs/kernel.md:52` |
| A1 | session; 3485; **writing:w8**, `M:30` | Журнал и состояние; `P/core/session/src/index.ts:9` |
| A1 | session-format-v1-to-v2; 1256; **review**, `M:73` | Прямой импорт каталога; `P/session/session-format-catalog/src/generated.ts:11` |
| A1 | subprocess-local; 5104; todo, `M:94` | Реальное выполнение процесса; `P/subprocess/subprocess-local/src/index.ts:20` |
| A2 | session-persistence; 636; todo, `M:77` | Контракт долговременного хранения; `P/core/agent-loop/src/index.ts:33` |
| A2 | session-projection; 1031; todo, `M:79` | Обязателен агенту; `P/core/agent/src/index.ts:247` |
| A2 | sandbox; 759; todo, `M:61` | Общие ошибки/helpers инструментов; backend пока отсутствует: `P/shell/tool-bash/src/index.ts:26` |
| A2 | session-format-v2-to-v3; 815; **writing:c4**, `M:74` | Кодеки каталога; `P/session/session-format-catalog/src/generated.ts:12` |
| A2 | compaction-image-offload; 197; todo, `M:21` | **Только чистая projection**; `P/session/session-format-catalog/src/message-projections.ts:3` |
| A3 | session-format-v3-to-v4; 1702; todo, `M:75` | V4-кодек и проверки JSONL; `P/session/session-persistence-jsonl/src/format.ts:27` |
| A3 | fs; 640; todo, `M:39` | Контракт файловой системы; `P/fs/fs-local/src/index.ts:16` |
| A3 | shell; 377; todo, `M:85` | Контракт shell; `P/shell/bash-local/src/index.ts:15` |
| A3 | agent; 1791; todo, `M:26` | Создание и управление агентом; `P/core/agent-loop/src/index.ts:25` |
| A4 | session-format-catalog; 154; todo, `M:71` | Импортируют **и JSONL, и replay**; `P/session/session-persistence-jsonl/src/index.ts:11`; `P/test-support/llm-replay/src/index.ts:17` |
| A4 | fs-local; 1401; todo, `M:40` | Чтение/запись; `P/fs/fs-local/src/index.ts:16` |
| A4 | bash-local; 404; todo, `M:149` | Shell поверх subprocess; `P/shell/bash-local/src/index.ts:98` |
| A4 | tools; 3797; todo, `M:33` | Каталог и pipeline инструментов; `P/core/tools/src/index.ts:100` |
| A5 | session-persistence-jsonl; 5145; todo, `M:78` | Сохранение событий и flush; `P/session/session-persistence-jsonl/src/storage.ts:550` |
| A5 | llm-replay; 1212; todo, `M:99` | Записанный `StreamChunk`; `P/test-support/llm-replay/src/index.ts:1032` |
| A5 | working-directory; 290; todo, `M:83` | Обязателен файловым/bash tools; `P/fs/tool-fs/src/index.ts:24` |
| A5 | agent-loop; 2149; todo, `M:28` | Цикл модель → tools → модель; `P/core/agent-loop/src/index.ts:291` |
| A5 | shell-env; 255; todo, `M:151` | Обязателен bash tool; `P/shell/tool-bash/src/index.ts:36` |
| A5 | tool-executor-native; 35; todo, `M:32` | Передача native tool calls в pipeline; `P/core/tool-executor-native/src/index.ts:5` |
| A6 | tool-fs; 2444; todo, `M:42` | `read` и `write`; `P/fs/tool-fs/src/index.ts:78`, `:99` |
| A6 | tool-bash; 871; todo, `M:152` | Модельный вызов bash; `P/shell/tool-bash/src/index.ts:36` |
| A7 | apps/hoh; 1070; todo, `M:13` | Собственный драйвер: создать сессию, передать prompt, дождаться, flush, завершиться |

Получается **50 crates/приложений: 24 done, 26 незакрытых**. Это минимум выбранной архитектуры, а не доказательство глобального минимума произвольной переписи — **не проверено**.

Эскиз собственного профиля, **псевдо-YAML; синтаксис не проверено**:

```yaml
bundles: []
plugins:
  session: {}
  session-projection: {}
  agent: {}
  llm: {}
  llm-replay:
    file: recording.v4.jsonl
    providers:
      - id: replay
        name: Replay
        models: [{id: recorded, inputModalities: [text], contextWindow: 32768}]
  system-prompt: {includeHarnessIdentity: false, includeRuntimeContext: false}
  tools: {}
  tool-executor-native: {}
  session-persistence-jsonl: {root: /isolated/state/sessions, compression: none}
  fs-local: {}
  working-directory: {defaultDirectory: /isolated/work}
  subprocess-local: {}
  bash-local: {}
  shell-env: {}
  tool-fs: {read: true, write: true, readImage: false, edit: false, applyPatch: false}
  tool-bash: {enableRunInBackground: false, promoteOnTimeout: false}
  agent-loop: {agents: []}
```

Драйвер явно выбирает `replay/recorded`. Основания конфигурации: `P/test-support/llm-replay/src/index.ts:1116`; `P/fs/tool-fs/src/index.ts:27`; `P/shell/tool-bash/src/index.ts:38`; `P/session/session-persistence-jsonl/src/index.ts:94`.

| Можно оставить вне реализации A | Что сохранить |
|---|---|
| Zstd, Windows/macOS, терминалы, background jobs | Linux foreground process, JSONL `none`; отсутствие jobs поддержано явно: `P/shell/tool-bash/src/index.ts:587` |
| Edit/patch/read_image, attachment backend | Настоящие read/write и их результаты; `P/fs/tool-fs/src/index.ts:78` |
| Compaction service, команды, DeepSeek extensions/providers | Replay использует type-only compaction и опционально получает extensions: `P/test-support/llm-replay/src/index.ts:13`, `:1072` |
| Исторические миграционные операции | V4-кодек/валидацию и необходимые общие части прежних crates. Удалять всю цепочку нельзя: прямые импорты в `P/session/session-format-catalog/src/generated.ts:10` |
| Автоматические checkpoints перед каждым действием | Явный итоговый flush и обработку его ошибок; `P/session/session-persistence-jsonl/src/storage.ts:555` |

Доказательство A: наш тест одного запуска проверяет три настоящих инструмента в одной сессии, содержимое выходного файла, stdout/exit bash, завершённый сохранённый журнал и его повторное чтение. Запись модели должна содержать всю последовательность; tool results исполняются, а не воспроизводятся. `session-checkpoint-policy` можно добавить позже: итоговое сохранение само по себе его не требует.

**B. Расширение для примера основателя**

Ниже достаточный состав поверх A для шагов 1–9, 11–13. Шаг 10 в документе явно необязателен (`docs/window.md:101`). Короткий показ ограничивает объём входных текстов контекстным бюджетом; достаточность такого ограничения **не проверено**.

| Очередь после A | Добавляемые crates; строки; статус | Назначение и основания |
|---|---|---|
| B1 | storage 364; storage-domain 910; storage-json 844; **done**, `M:90`, `M:91`, `M:158` | Постоянные сотрудники, задачи, версии; `docs/window.md:120`, `:128` |
| B1 | attachment-local 1559; **review**, `M:16` | Сохранение файлов/фото; `P/attachment/attachment-local/src/index.ts:6` |
| B1 | web 507; **review**, `M:168`; skill 902; **review**, `M:87` | Реестры web providers и инструкций задач; `P/web/web/src/index.ts:9`; `P/skill/skill/src/index.ts:17` |
| B1 | settings 704; **writing:w5**, `M:84`; credentials 409; **done**, `M:34` | Конфигурация модели/защиты; `docs/window.md:134` |
| B1 | sandbox-local 686; todo, `M:62`; session-checkpoint-policy 83; todo, `M:69` | Файловое ограничение процессов; сохранение перед побочными эффектами: `P/sandbox/sandbox-local/src/index.ts:2`; `P/session/session-checkpoint-policy/src/index.ts:70` |
| B2 | web-fetch-http 601; **writing:w1**, `M:170` | Открытые HTML/тексты, шаги 3–4; `P/web/web-fetch-http/src/index.ts:29` |
| B2 | sandbox-policy 328; todo, `M:63` | Права конкретной сессии; `P/sandbox/sandbox-policy/src/index.ts:30` |
| B2 | user-questions 964; user-approval 430; todo, `M:49`, `M:48` | Вопросы и отдельное решение о правах; `P/sdk/server/src/user-questions.ts:5`; `P/sdk/server/src/server.ts:123` |
| B2 | agent-preset-registry 1082; agent-instructions 2114; todo, `M:60`, `M:127`; persona 75; **done**, `M:143` | Композиция builder/сотрудника и правила; `docs/window.md:120`; `P/preset/agent-preset-registry/src/index.ts:54` |
| B2 | commands 734; session-title 1027; todo, `M:45`, `M:81` | SDK command helpers; заголовки истории: `P/sdk/server/src/query-input.ts:4`; `P/session-query/session-query/src/index.ts:16` |
| B3 | tool-web 1054; todo, `M:167`; output-retention 460; **done**, `M:117` | Web tools и ограничение выдачи; `P/web/tool-web/src/index.ts:26`; `P/web/tool-web/src/search.ts:11` |
| B3 | tool-ask-user 332; tool-present 127; todo, `M:47`, `M:36` | Шаги 2, 5, 12; `P/interaction/tool-ask-user/src/index.ts:29`; `P/deliverables/tool-present/src/index.ts:106` |
| B3 | fs-sandbox 257; bash-sandbox 206; todo, `M:133`, `M:150` | Ограничить запись и запуск программы; `P/fs/fs-sandbox/src/index.ts:30`; `P/shell/bash-sandbox/src/index.ts:46` |
| B3 | session-query 2241; sdk-protocol 571; todo, `M:68`, `M:66` | Чтение истории, wire transport; `docs/window.md:116`; `P/sdk/server/src/index.ts:15` |
| B4 | sdk-jsonrpc-server 1049; todo, `M:67` | Окно ↔ движок; `docs/window.md:110` |

`skill` нужен выбранному контракту `hoh-task` (`docs/window.md:123`). `skill-filesystem`/`tool-skill` не обязательны: задачи могут предоставлять инструкции через собственный provider. Автопоиск `SKILL.md`, встроенный `agent-preset`, file-reference/time-context и Office нужны расширенному продукту, не данной последовательности (`research/v1-engine-scope.codex.md:15`, `:19`).

**Одних портов недостаточно:**

| Следующий писатель / новая работа | Зависимости и результат |
|---|---|
| Локальный OpenAI-compatible adapter; предлагаемое имя `hoh-llm-openai` — **не проверено** | Над llm; SSE/tool-call deltas → StreamChunk. DeepSeek adapter отправляет `/messages`, одной замены URL недостаточно: `P/llm/llm-deepseek/src/adapter.ts:142` |
| Сетевой контроль и загрузка фото, расширения web/sandbox | **В dsh sandbox сеть вне контракта**, bwrap не отключает её: `P/sandbox/sandbox/src/index.ts:32`; `P/sandbox/sandbox-local/src/profiles.ts:17`. Нужен принудительный запрет обхода разрешённых адресов, общий с fetch/download; проект реализации **не проверено** |
| Search provider | Fetch не выполняет поиск. Нужен отдельно настроенный источник, например новый provider над web; локальная Qwen его не заменяет: `research/v1-engine-scope.codex.md:13` |
| `hoh-protocol` → расширение SDK и host approvals | Типы/реестр методов, ключ, actionHash, MAC, срок, однократность; `docs/window.md:112`, `:141`–`:146`, `:168` |
| `hoh-employee`, затем `hoh-task` | Registry/persona/instructions/storage; память, builder-session, draft → example → yes → повтор → неизменяемая версия; кнопки и запуск: `docs/window.md:119`–`:128` |
| Artifact API → `apps/hoh-desktop` и UI | Авторизованное чтение, сохранённые результаты, изолированный просмотр, мост и отдельное окно approvals; `docs/window.md:130`, `:170`–`:173` |
| `hoh-window-stub`; отдельно `hoh-tools-docs` | Stub — вспомогательная проверочная инфраструктура; docs — для Office/PDF/таблиц, **не обязательного** текстового результата с фото: `docs/window.md:132`, `:169` |

У новых частей нет измеренного объёма/статуса в MAP — **не проверено**. HTTP-fetch отвергает бинарные MIME, поэтому скачивание фотографий необходимо добавить отдельно (`P/web/web-fetch-http/src/provider.ts:129`).

**Все 13 звеньев прежнего критического пути:**

| Звено | Для A | Для цели 2 |
|---|---|---|
| session | Да | Да: чаты builder/сотрудника |
| session-persistence | Да | Да: история и возобновление |
| workspace | Нет | Не требуется описанному показу; cwd обеспечивает working-directory. Архивирование workspace — позже |
| agent | Да | Да |
| commands | Нет | Библиотечные SDK helpers; registry для slash-команд. Кнопка задачи сама вызывается через task API |
| compaction | Нет | Не обязательна ограниченному показу; для длинных исследований потребуется отдельно |
| compaction-image-offload | Только projection | Projection остаётся; recovery-плагин не требуется фото в панели |
| session-format-catalog | Да | Да: JSONL и чтение |
| session-query | Нет | Срез списка/истории; SQLite/full-text не нужен |
| subagent | Нет | Builder — отдельная обычная сессия, сотрудник — preset; дерево делегирования не требуется |
| schedule | Нет | Только необязательные шаг 10 и повтор из шага 13 |
| sdk-jsonrpc-server | Нет | Да: выбранный транспорт окна |
| apps/hoh | Да | Да: композиция SDK-режима |

Основания исключений: optional workspace выше; type-only compaction в replay; чистая offload projection; SDK импортирует schedule/subagent только как типы (`P/sdk/server/src/server.ts:23`). Создание SDK-сессии прямо использует agents (`P/sdk/server/src/server.ts:578`). Для Qwen надо **удалить статический DeepSeek fallback**, а не просто не вызывать его (`:25`, `:233`; `research/v1-engine-scope.codex.md:59`). Typert glue заменяется Rust-сервисами, не включается скрытой зависимостью (`research/v1-engine-scope.codex.md:58`).

**C. Нет, цель 2 не требует предварительного прохождения всех 242 сравнений.**

Это проверка другого состава, собственного протокола задач и настоящего Qwen. Нужны наши сквозные проверки примера, сохранения/повтора, approvals, сетевых ограничений и отображения результатов. Полное сравнение остаётся отдельной целью совместимости; новые employee/task/host-approval контракты им всё равно не покрываются (`docs/window.md:176`; `research/v1-engine-scope.codex.md:84`).

**D. Объём и завершение очереди**

| Рубеж | Полный исходный объём выбранных пакетов MAP | Объём незакрытых пакетов |
|---|---:|---:|
| A: 50 crates/приложений | 71 193 | **45 070** |
| Расширение B: ещё 28 | 20 620 | **17 558** |
| Цель 2 суммарно | 91 813 | **62 628 + новый код HoH** |

Это сумма `src_lines` при `status != done`: review/writing учитываются полностью. **Точный остаток написания срезов не проверено**; числа не являются оценкой Rust LOC или трудозатрат.

После B4: новый adapter/сетевой контроль → protocol/SDK approvals → employee/task/artifacts → окно → настоящий сквозной пример. UI можно готовить со stub раньше (`docs/window.md:163`).

Для необязательного расписания добавить `schedule` 3569, `tool-schedule` 496 и импортируемый ими `subagent` 5057 — все todo (`M:64`, `M:65`, `M:92`); `chunked-list` 58 уже done (`M:106`). Получится **71 750 незакрытых исходных строк + новый код**. Причина subagent — реальный импорт `delegationDepthOf`, не необходимость делегирования (`P/schedule/schedule/src/index.ts:10`; `P/schedule/tool-schedule/src/index.ts:16`). Выделение depth-helper сократит эту ветку, но это отдельная адаптация — **не проверено**.

**E. Какие dsh-сценарии возможны раньше**

| Кандидат | Вывод |
|---|---|
| `sdk/minimal-image-compaction` | Да, отдельный ранний кандидат **без стандартного профиля**: `S/sdk/minimal-image-compaction/snapshot.yml:3`; standalone дерево — `P/bundle/sdk-minimal/cordis.patch.yml:1`. Требует сверх A attachment backend, SDK, compaction/token-meter, image-offload, persistent terminal и сценарные проверки |
| `session/fs-read`, `fs-write`, `bash-tool-turn` | Полезные записи для наших ранних тестов, но сокращение профиля **не нейтрально для сравнения**: меняются request/header, схемы и runtime context. Пример: `S/session/fs-read/session.v4.jsonl:11`, `:12` |
| `sdk/persistent-tools` | Название composition `sdk-minimal` обманчиво: профиль остаётся `sdk`, следовательно обычная база: `S/sdk/persistent-tools/snapshot.yml:3`; `crates/hoh-conformance/README.md:58` |

Для minimal-image нельзя убрать compaction: assertions требуют ровно summary/offload и их уведомления (`S/sdk/sdk.snapshot.ts:1493`, `:1502`). Сейчас SDK assertions и strict V4 gate прогонщика не завершены (`crates/hoh-conformance/src/runner.rs:379`, `:425`).

**Подтверждённого сценария, проходящего ровно на A, нет.** Нейтральное сокращение стандартного профиля не установлено; изменять сравниваемые промпты/схемы или ослаблять проверки нельзя (`crates/hoh-conformance/README.md:580`, `:589`).