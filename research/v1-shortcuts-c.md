**Вариант (c): оставить TypeScript рядом с Rust**

Исследован только локальный клон dsh, коммит `39163dd30f703a79079e2d2446545cff3c453738`, версия workspace `0.2.2-rc.3`. Охват — все 65 строк, ровно 69 000 исходных строк из `research/v1-unstarted.tsv:2`. Сеть, установки, сборки и тесты не использовались. Работоспособность предлагаемых мостов — **не проверено**.

Для чтения таблицы:

- `P` = `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/`; `L` = `pnpm-lock.yaml` в корне этого клона.
- Имя crate в первом столбце имеет префикс `hoh-`; имя npm-пакета — `@deepseek-ai/dsh-`. Исключения названы полностью.
- `M:число` означает `package.json:число` в каталоге данной строки; `S:файл:число` — `src/файл:число` там же. Это ссылки на исходные основания строки.
- `D → R` — два замыкания зависимостей: обычные `dependencies`/`optionalDependencies` → те же зависимости **с обязательными `peerDependencies`**, рекурсивно. `N` обозначает точный набор внешних npm-пакетов, `W` — точный набор остальных workspace-пакетов. Наборы раскрыты после основной таблицы.
- При обходе исключены `devDependencies`, необязательные peers и workspace-пакеты со статусом `done` в текущем `docs/porting/MAP.tsv:1`. `review` и `writing` не приравнивались к завершённому переносу.
- Внешние optional-зависимости включены для **всех платформ**. Это счёт записей lock-файла, а не число установленных пакетов на конкретном Linux-хосте.
- В столбце портированных модулей перечислены зависимости от TS-значений/реализаций уже перенесённых dsh-пакетов, помимо Cordis/schemastery. Обычный Rust crate не удовлетворяет такой JS-импорт: нужен сохранённый TS-модуль либо адаптер. Импорт только типа сам по себе мост не требует.
- **Плагин** — отдельно неизменённым процессом не работает: нужны локальный Context, сервисы и lifecycle либо их достаточно полные фасады. **Сервис** — разумная граница RPC после добавления обвязки. **Данные** — возможен сравнительно простой процесс, принимающий и возвращающий данные. Это архитектурные оценки по указанному коду.

| Crate / каталог относительно `P` | Прямые внешние npm, версии из lock | `D → R` | Уже портированные runtime-модули | Отдельный Node-процесс: условия и связь | Основания |
|---|---|---|---|---|---|
| `compaction` / `compaction/compaction` | — | N0/W0 → N1/W1 | Нет прямого value-импорта dsh; `llm` используется в типах | **Плагин-интерфейс.** Сам алгоритм не предоставляет; получает Agent/Session и передаёт работу реализации. Выносить вместе с реализацией compaction. | M:41; L:7649; S:index.ts:119 |
| `compaction-basic` / `compaction/compaction-basic` | `zod@4.6.6` | N1/W0 → N1/W2 | `llm`, `util-values`, `brand` | **Плагин.** Нужны живая сессия, projections, token meter, запрос суммаризации и события агента. Проще общий TS-блок compaction, чем отдельный процесс этого пакета. | M:34; L:7667; S:index.ts:120; S:index.ts:172; S:region.ts:10 |
| `compaction-image-offload` / `compaction/compaction-image-offload` | — | N0/W0 → N1/W3 | `llm`, `util-values` | **Плагин.** Перехватывает ошибки запроса и изменяет session surface. За RPC нужны снимок состояния, проверка актуальности и атомарное применение замены. | M:38; L:7734; S:index.ts:19; S:index.ts:27; S:project-message.ts:4 |
| `compaction-tool-result-pruner` / `compaction/compaction-tool-result-pruner` | — | N0/W0 → N1/W2 | `llm`, `util-values` | **Сервис вычисления** возможен для отбора/переписывания сообщений, но текущий Service получает token meter из Context. Передавать сообщения и результаты измерения либо держать meter вместе с ним. | M:34; L:7780; S:index.ts:44; S:index.ts:47 |
| `file-reference` / `context/file-reference` | — | N0/W0 → N1/W4 | — | **Плагин-интерфейс.** Сам не является файловым поисковым сервисом. Вынос отдельно мало что сокращает. | M:41; L:7899; S:index.ts:25 |
| `file-reference-local` / `context/file-reference-local` | — | N0/W0 → N1/W5 | — | **Плагин.** Поиск можно вызвать по RPC; установка prompt-контекста и обработчики Agent/Session остаются интеграцией с Rust. Нужны cwd и доступные инструменты. | M:34; L:7908; S:index.ts:38; S:index.ts:85 |
| `time-context` / `context/time-context` | `zod@4.6.6` | N1/W0 → N1/W4 | `llm`, `util-time`, `util-values` | **Плагин.** Встраивается перед шагом и читает projection сессии. Вынос форматирования времени возможен, но не сохраняет неизменённым весь пакет. | M:34; L:8073; S:index.ts:49; S:index.ts:199; S:request-zone.ts:5 |
| `agent` / `core/agent` | `zod@4.6.6` | N1/W0 → N1/W6 | `scope`, `llm`, `brand` | **Плагин ядра.** Registry, фабрика и process-local scope. За RPC это уже удалённый агент с идентификаторами и командами, а не прежний объект Agent. | M:39; L:8177; S:index.ts:2; S:index.ts:246; S:dispatch.ts:10 |
| `agent-loop` / `core/agent-loop` | `zod@4.6.6` | N1/W0 → N1/W7 | `llm`, `util-values`, `brand`, `scope` | **Целый TS runtime.** Держать вместе Agent, Session, tools, prompt, projections и persistence. Наиболее естественная внешняя граница — SDK, а не RPC каждого шага. | M:33; L:8242; S:index.ts:290; S:index.ts:291; S:agent.ts:370 |
| `tool-executor-native` / `core/tool-executor-native` | — | N0/W0 → N1/W8 | — | **Плагин.** Обвязка над pipeline из `tools`, работающая с Context и функциями инструментов. Отдельно не отделяет исполнение инструментов от их владельца. | M:34; L:8387; S:index.ts:4; S:index.ts:11; S:index.ts:33 |
| `tools` / `core/tools` | — | N0/W0 → N1/W9 | `scope`, `llm`, `util-values` | **Плагин ядра.** Registry хранит определения и исполняемые функции, scoped layers и middleware. Нужны удалённые handles функций либо совместное размещение tools и loop в Node. | M:40; L:8457; S:index.ts:99; S:index.ts:100; S:batch.ts:17 |
| `tool-present` / `deliverables/tool-present` | — | N0/W0 → N1/W10 | — | **Плагин инструмента.** Rust передаёт вызов и контекст, TS проверяет файл; запись события presentation должна попасть в единственную authoritative Session. | M:39; L:8602; S:index.ts:26; S:index.ts:101 |
| `office-kit` / `document/office-kit` | `@deepseek-ai/libreoffice-kit@0.2.3`, `zod@4.6.6` | N2/W11 → N2/W11 | `home-paths`, `brand` | **Хорошая граница сервиса.** RPC convert/render/recalculate, байты или разрешённые пути, cancellation, очередь и освобождение outputs по sessionId. Полный `renderFile` дополнительно требует `workspaceFiles` и `fs`; Typert остаётся TS-зависимостью. | M:52; L:8727; S:index.ts:127; S:index.ts:185; S:index.ts:288 |
| `tool-office` / `document/tool-office` | — | N0/W0 → N2/W12 | `attachment`, `brand`, `error` | **Плагин.** Не только конвертация: инструменты, attachments, fs, cwd, subagents и session events. Дешевле оставить Rust-обвязку инструмента, вынеся `office-kit`, чем переносить всё это через RPC. | M:34; L:8767; S:index.ts:40; S:index.ts:147; S:index.ts:157 |
| `fs` / `fs/fs` | — | N0/W0 → N0/W13 | `llm` — класс ошибки | **Интерфейс сервиса.** Процесс имеет смысл только вместе с backend. Объекты ошибок и target/version должны иметь явное wire-представление. | M:32; L:13140; S:index.ts:104; S:types.ts:8 |
| `fs-local` / `fs/fs-local` | `chokidar@5.0.0`, `koffi@3.1.1` | N3/W0 → N3/W14 | `fs-patch`, `brand` | **Хорошая граница сервиса.** RPC stat/read/write/edit/patch; уведомления watch; сериализация target/version и stale guards. Нужен локальный TS-контракт `fs`. | M:34; L:13159; S:index.ts:7; S:index.ts:75; S:index.ts:76 |
| `tool-fs` / `fs/tool-fs` | `diff@9.0.0` | N4/W0 → N5/W15 | `util-code-language`, `llm`, `fs-patch`, `util-values`, `attachment`, `brand`, `error` | **Плагин.** Помимо файлового backend нужны tools, approval, sandbox policy, attachments и cwd. Сохранить неизменённым можно с этими сервисами или фасадами; один RPC «прочитать файл» пакет не заменяет. | M:34; L:13235; S:index.ts:24; S:read-image.ts:16; S:apply-patch-presentation.ts:2 |
| `commands` / `interaction/commands` | `zod@4.6.6` | N1/W0 → N1/W4 | `util-crypto`, `attachment`, `scope`, `brand` | **Плагин.** Registry хранит function handlers; исполнение связано с Agent/Session и Typert. Wire-команда возможна, но зарегистрированные обработчики должны оставаться у одного владельца. | M:56; L:13977; S:index.ts:7; S:index.ts:263 |
| `permission-presets` / `interaction/permission-presets` | `zod@4.6.6` | N1/W0 → N1/W16 | `brand` | **Плагин.** Меняет shell/approval/sandbox/session state, слушает создание Agent. Нужны согласованные изменения политики и журналирование, а не отдельный RPC выбора строки preset. | M:56; L:14011; S:index.ts:175; S:index.ts:196; S:index.ts:234 |
| `tool-ask-user` / `interaction/tool-ask-user` | — | N0/W0 → N1/W17 | — | **Плагин инструмента.** Возможен вызов через RPC, если `userQuestions` имеет обратный канал ответа и отмены. Сам UI/ответчик этим пакетом не предоставляется. | M:34; L:14060; S:index.ts:29; S:index.ts:34 |
| `user-approval` / `interaction/user-approval` | `zod@4.6.6` | N1/W0 → N1/W4 | `llm`, `scope`, `brand` | **Плагин политики.** Projection и waterfall с продолжением `next`. Нужны двусторонние запросы approval и определённый порядок цепочки. | M:39; L:14094; S:index.ts:168; S:index.ts:169 |
| `user-questions` / `interaction/user-questions` | `zod@4.6.6` | N1/W0 → N1/W4 | `llm`, `scope`, `brand` | **Плагин.** Pending questions связаны с inbox, событиями и projections; нужен RPC ask/answer/cancel с корреляцией, а также TS Typert. | M:52; L:14128; S:index.ts:79; S:index.ts:93; S:index.ts:109 |
| `jobs` / `jobs/jobs` | — | N0/W0 → N1/W4 | `error` | **Плагин-интерфейс.** Registry и admission hooks; выполнение jobs предоставляет backend. Выносить вместе с ним и контроллером уведомлений. | M:41; L:14171; S:index.ts:89; S:archive-admission.ts:25 |
| `jobs-local` / `jobs/jobs-local` | — | N0/W0 → N1/W18 | `scope`, `timeout`, `brand` | **Плагин с памятью процесса.** Хранит jobs, owners, output и обработчики завершения. Нужны handles и подписки; callback-задачу нельзя отправить как JSON без нового протокола. | M:34; L:14192; S:index.ts:184; S:index.ts:201; S:index.ts:215 |
| `llm-retry` / `llm/llm-retry` | `zod@4.6.6` | N1/W0 → N1/W4 | `brand` | **Плагин.** Recovery middleware, durable retry events и cancellable wait. Лучше держать рядом с loop; удалённый расчёт задержки экономит лишь часть пакета. | M:38; L:14559; S:index.ts:23; S:index.ts:40; S:index.ts:241 |
| `token-meter` / `llm/token-meter` | `zod@4.6.6` | N1/W0 → N1/W19 | `util-values`, `llm` | **Сервис вычисления плюс плагин.** Измерение можно вынести по данным; текущий пакет также регистрирует projections и инвалидирует состояние на session events. | M:43; L:14666; S:index.ts:87; S:index.ts:92; S:index.ts:112; S:usage-projection.ts:6 |
| `agent-preset-registry` / `preset/agent-preset-registry` | `zod@4.6.6` | N6/W20 → N7/W21 | `scope` | **Плагин загрузчика.** Монтирует составы в Cordis scopes; требует loader, app-boot, include и Typert. Удалённая декларация preset не заменяет локальное применение его plugin composition. | M:56; L:15082; S:index.ts:53; S:index.ts:54; S:mount.ts:2 |
| `sandbox` / `sandbox/sandbox` | — | N0/W0 → N0/W22 | `llm`, `error`, `util-values` | **Интерфейс и общая логика.** Отдельного sandbox-процесса сам не создаёт. Сохранять вместе с backend; escalation обращается к approval. | M:32; L:15268; S:index.ts:207; S:escalation.ts:19 |
| `sandbox-local` / `sandbox/sandbox-local` | —; через workspace — `koffi`, `yaml` | N8/W23 → N8/W24 | `node-addon-system`, `util-values` | **Сервис возможен.** RPC confine/probe возвращает argv и enforcement facts. Но native launcher, ACL lifecycle и cleanup должны иметь одного владельца. Linux не устраняет статические Windows-зависимости исходного пакета автоматически. | M:34; L:15287; S:index.ts:29; S:index.ts:40; S:index.ts:326; S:index.ts:343 |
| `sandbox-policy` / `sandbox/sandbox-policy` | `zod@4.6.6` | N1/W0 → N1/W25 | — | **Плагин.** Разрешает политику из session projections и текущего контекста. Локальный снимок политики допустим только с определённой актуальностью и порядком обновлений. | M:34; L:15318; S:index.ts:135; S:index.ts:145 |
| `schedule` / `schedule/schedule` | `@js-temporal/polyfill@0.5.1`, `zod@4.6.6` | N9/W0 → N10/W26 | `llm`, `brand`, `util-time`, `storage-domain` | **Сервис возможен, граница большая.** Node владеет scheduler, Rust — активацией Agent; нужны storage-domain RPC, session admission, delivery и recovery. В обязательных peers есть `api-session-controller`, Typert и большой хвост вне 65 строк. | M:52; L:15389; S:index.ts:224; S:index.ts:225; S:index.ts:325; S:storage.ts:8 |
| `tool-schedule` / `schedule/tool-schedule` | — | N0/W0 → N10/W27 | `brand` | **Плагин инструмента.** Команды можно отправлять scheduler по RPC, но регистрация схем и prompt остаётся интеграцией с tools/systemPrompt. | M:34; L:15477; S:index.ts:41; S:index.ts:357 |
| `sdk-protocol` / `sdk/protocol` | — | N0/W0 → N1/W28 | Нет: ссылки на dsh в wire types — `import type` | **Данные/транспорт.** Можно использовать в отдельном Node-процессе без живого ядра. Большой W28 — заявленные peers манифеста, а не необходимость transport-кода. Это не готовый engine service. | M:31; L:15548; S:types.ts:11; S:transport.ts:9; S:transport.ts:65 |
| `sdk-jsonrpc-server` / `sdk/server` | — | N0/W0 → N11/W29 | `brand`, `attachment`, `llm` | **Готовая внешняя граница для TS engine.** Уже stdio JSON-RPC, но server требует Agent/cwd и управляет всем root lifecycle. Для «server в Node, loop в Rust» всё равно нужны фасады сервисов; проще оставить TS runtime целиком за SDK. | M:34; L:15569; S:index.ts:21; S:index.ts:45; S:index.ts:50; S:server.ts:123 |
| `session-query` / `session-query/session-query` | — | N0/W30 → N1/W31 | `llm` — ошибка; транзитивно codecs | **Сервис чтения возможен.** Запросы по данным/журналам подходят RPC; текущий engine также использует SessionStore, title и format catalog. Нужны flush/read consistency и единый владелец форматов. | M:32; L:15751; S:index.ts:98; S:index.ts:99; S:config.ts:3 |
| `session-checkpoint-policy` / `session/session-checkpoint-policy` | — | N0/W0 → N1/W7 | Нет прямых value-импортов портированных dsh; нужна цепочка `llm/stream` | **Плагин порядка исполнения.** Flush должен завершиться до отправки модели и до побочного эффекта инструмента. Асинхронное уведомление через RPC этого не обеспечивает; нужен ожидаемый барьер. | M:34; L:15855; S:index.ts:18; S:index.ts:35; S:index.ts:64; S:index.ts:70 |
| `session-format-catalog` / `session/session-format-catalog` | — | N0/W32 → N1/W33 | `session-format`, `session-format-v0-to-v1`, `brand` | **Данные, но не изолированный codec.** Можно вынести чтение/миграцию целиком; текущая валидация V4 вызывает `Session.fromRestore`, а projections импортируются из других пакетов. Нужна TS Session-библиотека либо её адаптация. | M:37; L:15913; S:generated.ts:6; S:generated.ts:48; S:current.ts:25; S:current.ts:45 |
| `session-format-v3-to-v4` / `session/session-format-v3-to-v4` | — | N0/W34 → N0/W34 | `session-format`; через старые codecs — уже перенесённый V0→V1 | **Хорошая граница данных.** RPC с header/events и историческими фактами детей; сохранять порядок и ошибки. Живой Context не нужен, но TS codec-цепочка нужна. | M:44; L:16001; S:migration.ts:3; S:migration.ts:24; S:migration.ts:34 |
| `session-persistence` / `session/session-persistence` | — | N0/W0 → N0/W22 | `util-values` | **Интерфейс.** Сам storage не предоставляет. Отдельный процесс оправдан только вместе с JSONL backend и договором flush/checkpoint. | M:32; L:16066; S:index.ts:144; S:storage-contract.ts:13 |
| `session-persistence-jsonl` / `session/session-persistence-jsonl` | `koffi@3.1.1`, `p-limit@7.3.3` | N12/W30 → N13/W35 | `session-format`, `brand`, `llm`, `node-addon-system` | **Сервис возможен, интеграция дорогая.** Один writer Node, ordered append/flush RPC, lease и восстановление. Не допускать параллельного Rust-writer того же журнала. Нужны codecs и подписки на события/flush/dispose. | M:35; L:16088; S:storage.ts:550; S:storage.ts:555; S:lease.ts:34; S:generation.ts:30 |
| `session-projection` / `session/session-projection` | `zod@4.6.6` | N1/W0 → N1/W22 | — | **Плагин ядра.** Registry хранит определения projections и функции fold; связан с Fiber и Session. Нужна общая TS Session либо отдельная модель удалённых projections с revisions. | M:39; L:16131; S:index.ts:282; S:index.ts:297; S:index.ts:309 |
| `session-projection-cache` / `session/session-projection-cache` | `zod@4.6.6` | N1/W0 → N1/W36 | `util-values`, `storage-domain` | **Плагин.** Кэширует projections по событиям сессии в storage domain. Отдельный процесс требует согласовать revisions, invalidation и disposal. | M:34; L:16153; S:index.ts:105; S:index.ts:106; S:index.ts:316; S:spec.ts:18 |
| `session-title` / `session/session-title` | `zod@4.6.6` | N1/W0 → N1/W4 | `llm`, `util-values` | **Плагин.** Не только строковое сокращение: session events, модельный поток, фоновые задачи и projections. За RPC нужна согласованная фиксация title events. | M:43; L:16294; S:index.ts:301; S:index.ts:349; S:index.ts:360 |
| `working-directory` / `session/working-directory` | `zod@4.6.6` | N1/W0 → N1/W37 | `llm`, `error` | **Плагин состояния.** Проверяет cwd через fs, читает projections и участвует в prompt assembly. Лучше держать authoritative cwd в Rust и явно передавать его каждому вынесенному сервису. | M:34; L:16469; S:index.ts:76; S:index.ts:77; S:index.ts:116 |
| `shell` / `shell/shell` | — | N0/W0 → N0/W13 | `subprocess` — контракт/значения | **Интерфейс.** Сам shell не запускает. Выносить с bash backend и process backend, сохраняя handles процессов. | M:32; L:16671; S:index.ts:64; S:types.ts:13 |
| `tool-workspace-dependencies` / `skill/tool-workspace-dependencies` | — | N0/W0 → N1/W8 | `error` | **Хорошая граница операции.** Чтение/установка заранее собранного runtime payload по RPC; регистрация инструмента остаётся у Rust. Payload Python/Node/pnpm не входит в npm-счёт этой строки. | M:33; L:17160; S:index.ts:1; S:index.ts:15; S:index.ts:18; S:index.ts:140 |
| `subagent` / `subagent/subagent` | `zod@4.6.6` | N1/W0 → N1/W10 | `llm`, `util-values`, `brand`, `scope`, `util-time`, `chunked-list`, `attachment` | **Плагин ядра.** Хранит children, continuation, scopes, lifecycle и управление агентами. Реалистично оставлять вместе с TS Agent/Session/loop; разрез посередине потребует большого двустороннего протокола. | M:52; L:17624; S:index.ts:194; S:index.ts:199; S:child-agent.ts:190; S:activation.ts:130 |
| `subprocess-local` / `subprocess/subprocess-local` | `koffi@3.1.1`, `node-pty@1.2.0-beta.15` с патчем | N14/W38 → N14/W38 | `subprocess`, `timeout` | **Хорошая, но нетривиальная граница сервиса.** RPC spawn/write/resize/signal/wait и поток stdio. Node владеет process ranges и cleanup. Внутренний runner-протокол уже есть, но не является готовым общим Rust↔Node API. | M:49; L:18239; S:index.ts:55; S:index.ts:77; S:index.ts:105; S:runner-protocol.ts:16 |
| `tool-todo` / `todo/tool-todo` | `zod@4.6.6` | N1/W0 → N1/W8 | — | **Плагин.** Схема инструмента и изменение projection/session state. Отдельный процесс возможен с фасадами tools/projections, но экономия на такой границе сомнительна. | M:39; L:18867; S:index.ts:23 |
| `workspace` / `workspace/workspace` | `zod@4.6.6` | N1/W0 → N1/W39 | `brand`, `storage-domain`; вызывается `storage` | **Плагин состояния.** Registry workspace entities, storage и session persistence. Нужен единый владелец ID, операций и admission; отдельный Node с зеркалом состояния увеличивает мост. | M:39; L:19580; S:index.ts:187; S:index.ts:188; S:spec.ts:11 |
| `agent-instructions` / `context/agent-instructions` | — | N0/W0 → N1/W10 | `llm`, `home-paths`, `util-values` | **Плагин.** Чтение AGENTS.md можно вынести, но пакет также отслеживает версии, session events и pre-step обновления. Нужны cwd, fs и порядок вставки сообщений. | M:34; L:7829; S:index.ts:35; S:index.ts:85; S:index.ts:322; S:index.ts:330 |
| `fs-sandbox` / `fs/fs-sandbox` | — | N0/W0 → N15/W40 | Через `fs-local`: `fs-patch`, `brand` | **Сервис вместе с fs-local.** Наследует локальный backend и проверяет target по per-call sandbox policy. Передавать полную policy и сохранять проверки containment; отдельно от backend почти нечего запускать. | M:34; L:13208; S:index.ts:49; S:index.ts:56; S:index.ts:156 |
| `tool-jobs` / `jobs/tool-jobs` | — | N0/W0 → N1/W41 | `llm`, `output-retention`, `brand` | **Плагин.** Управление jobs и доставка notices в Agent inbox/session. Нужны subscriptions, scope ownership и обработка событий, а не только list/wait RPC. | M:34; L:14229; S:index.ts:34; S:index.ts:241; S:index.ts:271; S:index.ts:287 |
| `agent-preset` / `preset/agent-preset` | — | N0/W0 → N7/W42 | — | **Плагин регистрации.** Всего 30 строк по TSV; монтируется через loader в registry. Вынос отдельным процессом требует больше интеграции, чем содержит исходный пакет. | M:35; L:15051; S:index.ts:13; `research/v1-unstarted.tsv:55` |
| `bash-local` / `shell/bash-local` | — | N0/W0 → N0/W43 | `timeout`; живой сервис `subprocess` | **Сервис вместе с process backend.** RPC исполнения команды, поток/результат, cancellation и deadlines. Текущий класс получает subprocess из Context. | M:34; L:16570; S:index.ts:18; S:index.ts:98 |
| `bash-sandbox` / `shell/bash-sandbox` | — | N0/W0 → N1/W44 | Через `bash-local`: `timeout`, `subprocess` | **Сервис в shell-группе.** Наследует bash-local, применяет policy и хранит enforcement facts по объектам ShellProcess. Не разносить эти объекты между двумя независимыми владельцами. | M:34; L:16592; S:index.ts:45; S:index.ts:46; S:index.ts:59; S:index.ts:85 |
| `shell-env` / `shell/shell-env` | — | N0/W0 → N16/W45 | `home-paths`, `error` | **Плагин.** Registry вычисляемого окружения, deployment/profile context и shell/tools. RPC может вернуть env snapshot, но его актуальность и scope нужно определить. | M:34; L:16683; S:index.ts:31; S:index.ts:101; S:index.ts:225 |
| `tool-bash` / `shell/tool-bash` | — | N0/W0 → N16/W46 | `llm`, `subprocess` | **Плагин инструмента.** Помимо shell нужны approval, sandbox policy, env, cwd, jobs и prompt. Дешевле один TS shell/backend-блок с тонким Rust tool adapter, чем отдельный процесс tool-bash. | M:34; L:16717; S:index.ts:36; S:index.ts:246; S:background.ts:1 |
| `skill-filesystem` / `skill/skill-filesystem` | `chokidar@5.0.0`, `yaml@2.9.0` | N17/W0 → N17/W47 | `home-paths` | **Сервис discovery возможен.** RPC scan/read плюс change notifications. Неизменённый плагин регистрирует provider в `skills` и слушает fs observations; нужны фасады этих точек. | M:34; L:17055; S:index.ts:46; S:index.ts:134; S:index.ts:143 |
| `tool-skill` / `skill/tool-skill` | — | N0/W0 → N1/W48 | `llm` | **Плагин.** Загрузка skill меняет Agent/session context и участвует в pre-step обработке. Один RPC чтения SKILL.md покрывает только часть поведения. | M:34; L:17114; S:index.ts:27; S:index.ts:81; S:index.ts:182 |
| `subagent-spawn-in-process` / `subagent/subagent-spawn-in-process` | — | N0/W0 → N1/W49 | — | **Плагин регистрации, не внешний spawner.** Работает поверх того же TS subagent manager/Agent factory. Отдельный Node-процесс имеет смысл только как контейнер всей этой группы. | M:34; L:18047; S:index.ts:2; S:index.ts:20; S:index.ts:57 |
| `tool-subagent` / `subagent/tool-subagent` | `zod@4.6.6` | N1/W0 → N1/W49 | `brand` | **Плагин.** Регистрирует tools и реагирует на изменение providers, читает projections и presets. Выносить с manager; Rust получает команды и события по ID. | M:41; L:18105; S:index.ts:39; S:index.ts:190; S:index.ts:354; S:model-selection.ts:3 |
| `tool-subagent-control` / `subagent/tool-subagent-control` | — | N0/W0 → N1/W49 | `llm`; `util-values` заявлен в dependencies | **Плагин.** Wait/list/control связаны с inbox и disposal агента. Нужен двусторонний канал событий и отмены ожидания; сам manager остаётся отдельной зависимостью. | M:34; L:18163; S:index.ts:21; S:wait-agent.ts:61; S:wait-agent.ts:64 |
| `tool-web` / `web/tool-web` | `@joplin/turndown-plugin-gfm@1.0.67`, `turndown@7.2.4` | N18/W0 → N19/W50 | `output-retention`, `util-values` | **Плагин; вычислительная часть отделима.** RPC web operation → точный tool result; регистрация tools/prompt и web provider остаётся на одной стороне или требует фасадов. | M:34; L:19201; S:index.ts:26; S:index.ts:88; S:fetch.ts:449; S:search.ts:11 |
| `web-deepseek-api` / `web/web-deepseek-api` | — | N0/W0 → N20/W51 | `credentials`, `launch-environment` | **Сервис HTTP возможен.** Нужны credentials/endpoint/provider config и cancellation. Неизменённый пакет также импортирует DeepSeek-specific helpers и loader; простой универсальный web subprocess из него не получается. | M:34; L:19296; S:index.ts:18; S:index.ts:22; S:index.ts:56; S:index.ts:169 |

**Что именно посчитано**

Обход начинался с манифеста каждого пакета. Workspace-зависимости разрешались по локальным именам, registry-зависимости — по importer и затем `snapshots` lock-файла. Каждый разрешённый узел учитывался один раз; циклы не суммировались повторно. Для уже портированного workspace-пакета обход останавливался. Внешняя npm-библиотека не исключалась лишь потому, что её функциональность уже перенесена: например, исходный `tool-fs` продолжает требовать `diff@9.0.0`. Основания формата и разрешения: `L:1`, `L:26`, `L:27978`; исключения статусов — `docs/porting/MAP.tsv:1`.

Это **заявленный граф runtime-зависимостей**, а не результат `install --prod` и не измеренный список загруженных модулей. Обязательные peers могут обозначать только типовую связь: показательный случай — `sdk-protocol`, где ссылки на dsh в `types.ts` являются `import type`, а транспорт использует встроенные Node-модули. Обратный случай — сервис, полученный через Context: он может требоваться при работе даже без value-импорта класса. Основания: `P/sdk/protocol/package.json:31`, `P/sdk/protocol/src/types.ts:11`, `P/sdk/protocol/src/transport.ts:9`, `P/document/office-kit/src/index.ts:288`.

**Точные внешние npm-наборы `N`**

Число включает прямые и транзитивные внешние зависимости. Формулы — объединение множеств, без повторного счёта. Версии фиксированы данным lock-файлом.

Общие поднаборы:

- `K` — **16**: `koffi@3.1.1` и 15 optional-пакетов `@koromix/koffi-{darwin-arm64,darwin-x64,freebsd-arm64,freebsd-ia32,freebsd-x64,linux-arm64,linux-ia32,linux-loong64,linux-riscv64,linux-x64,openbsd-ia32,openbsd-x64,win32-arm64,win32-ia32,win32-x64}@3.1.1`. Основание: `L:33423`.
- `A` — **11**: `node-addon-require-builtin@0.1.9`, `node-addon-native-custom-loader@0.1.9` и девять optional-вариантов `node-addon-require-builtin-{darwin-arm64,darwin-x64,linux-arm64-gnu,linux-arm64-musl,linux-x64-gnu,linux-x64-musl,win32-arm64-msvc,win32-ia32-msvc,win32-x64-msvc}@0.1.9`. Основание: `L:34106`, `L:34151`.

| Набор | Число | Полный состав или формула | Основания в `L` |
|---|---:|---|---|
| N0 | 0 | ∅ | Importers соответствующих строк основной таблицы |
| N1 | 1 | `zod@4.6.6` | 27957 |
| N2 | 39 | `K`, N1, `@deepseek-ai/libreoffice-kit@0.2.3`, пять optional-вариантов `@deepseek-ai/libreoffice-kit-{darwin-arm64,darwin-x64,wasm,win32-arm64,win32-x64}@0.2.3`; `@swc/helpers@0.5.23`, `base64-js@1.5.1`, `brotli@1.3.3`, `clone@2.1.2`, `dfa@1.2.0`, `fast-deep-equal@3.1.3`, `fflate@0.8.2`, `fontkit@2.0.4`, `pako@0.2.9`, `restructure@3.0.2`, `saxes@6.0.0`, `tiny-inflate@1.0.3`, `tslib@2.8.1`, `unicode-properties@1.4.1`, `unicode-trie@2.0.0`, `xmlchars@2.2.0` | 28578, 28593, 32694, 31476, 35491, 35496 |
| N3 | 18 | `K` + `chokidar@5.0.0`, `readdirp@5.0.0` | 31615, 26729, 33423 |
| N4 | 1 | `diff@9.0.0` | 24527 |
| N5 | 2 | N4 + N1 | 24527, 27957 |
| N6 | 20 | `A` + N1 + `@eslint-community/regexpp@4.12.2`, `ajv@8.20.0`, `fast-deep-equal@3.1.3`, `fast-uri@3.1.8`, `json-schema-traverse@1.0.0`, `require-from-string@2.0.2`, `resolve.exports@2.0.3`, `semver@7.8.5` | 15082, 31194, 21453, 26757, 26774, 34151 |
| N7 | 23 | N6 + `argparse@2.0.1`, `js-yaml@4.3.2`, `yaml@2.9.0` | 33270, 27903 |
| N8 | 17 | `K` + `yaml@2.9.0` | 15287, 33423, 27903 |
| N9 | 3 | N1 + `@js-temporal/polyfill@0.5.1`, `jsbi@4.3.2` | 29444, 25453 |
| N10 | 28 | N7 + `@js-temporal/polyfill@0.5.1`, `jsbi@4.3.2`, `mime-db@1.54.0`, `mime-types@3.0.2`, `ws@8.21.3` | 15389, 29444, 34015, 27848 |
| N11 | 29 | N10 + `eventsource-parser@3.1.0` | 15569, 24785 |
| N12 | 18 | `K` + `p-limit@7.3.3`, `yocto-queue@1.2.2` | 34358, 27940 |
| N13 | 19 | N12 + N1 | 16088, 27957 |
| N14 | 18 | `K` + `node-addon-api@7.1.1`, `node-pty@1.2.0-beta.15` с patch hash `9d30053266d0f39d3debe894c675867f6782da6bcadbc94a693b3cfb9eff8ed0` | 34198, 26150; патч — 24 |
| N15 | 19 | N3 + N1 | 13208, 31615, 27957 |
| N16 | 22 | N7 − `yaml@2.9.0` | 16683, 16717 |
| N17 | 3 | `chokidar@5.0.0`, `readdirp@5.0.0`, `yaml@2.9.0` | 17055, 31615, 27903 |
| N18 | 3 | `@joplin/turndown-plugin-gfm@1.0.67`, `turndown@7.2.4`, `@mixmark-io/domino@2.2.0` | 19201, 21718, 35438 |
| N19 | 4 | N18 + N1 | 19201, 27957 |
| N20 | 3 | `eventsource-parser@3.1.0`, `semver@7.8.5`, `zod@4.6.6` | 19296, 24785, 26928, 27957 |

Например, `fs-local` имеет две прямые внешние зависимости и **18 записей полного внешнего замыкания**, из которых 15 — платформенные optional-варианты koffi. Из этого нельзя заключать, что Linux-поставка установит 18 внешних пакетов: фактический отбор установщиком **не проверено**. Основания: `P/fs/fs-local/package.json:38`, `L:31615`, `L:33423`.

**Точные workspace-наборы `W`**

Здесь нет корневого пакета данной строки, уже портированных workspace-пакетов и dev-зависимостей. Суффиксы означают `@deepseek-ai/dsh-*`; `cordis-group`, `cordis-include`, `cordis-loader` означают соответственно `@deepseek-ai/cordis-plugin-{group,include,loader}`. `+` и `−` — операции над множествами.

Наборы получены из runtime-разделов манифестов, указанных в основной таблице. Для пакетов вне 65 строк дополнительные основания перечислены ниже.

| Набор | Число | Состав |
|---|---:|---|
| W0 | 0 | ∅ |
| W11 | 1 | `typert-protocol` |
| W20 | 1 | `app-boot` |
| W22 | 1 | `session` |
| W13 | 2 | `sandbox`, `session` |
| W34 | 2 | `session-format-v1-to-v2`, `session-format-v2-to-v3` |
| W36 | 2 | `session`, `session-projection` |
| W38 | 2 | `lazy-require`, `win32-process` |
| W14 | 3 | W13 + `fs` |
| W23 | 3 | W38 + `sandbox-windows-acl` |
| W39 | 3 | `session`, `session-persistence`, `typert-protocol` |
| W43 | 3 | W13 + `shell` |
| W6 | 4 | W36 + `system-prompt`, `typert-protocol` |
| W32 | 4 | W34 + `compaction-image-offload`, `session-format-v3-to-v4` |
| W47 | 4 | W14 + `skill` |
| W4 | 5 | W6 + `agent` |
| W30 | 5 | W32 + `session-format-catalog` |
| W1 | 6 | W4 + `commands` |
| W9 | 6 | W4 + `user-approval` |
| W18 | 6 | W4 + `jobs` |
| W24 | 6 | W23 + `sandbox`, `session`, `skill` |
| W25 | 6 | W4 + `sandbox` |
| W3 | 7 | W1 + `compaction` |
| W8 | 7 | W9 + `tools` |
| W37 | 7 | W25 + `fs` |
| W7 | 8 | W8 + `session-persistence` |
| W17 | 8 | W8 + `user-questions` |
| W41 | 8 | W8 + `jobs` |
| W50 | 8 | W8 + `web` |
| W19 | 9 | W3 + `compaction-image-offload`, `llm-retry` |
| W40 | 9 | W37 + `fs-local`, `sandbox-policy` |
| W44 | 9 | W25 + `bash-local`, `sandbox-policy`, `shell` |
| W2 | 10 | W19 + `token-meter` |
| W10 | 10 | W8 + `fs`, `sandbox`, `working-directory` |
| W16 | 10 | W44 + `commands`, `user-approval` − `bash-local` |
| W5 | 11 | W10 + `file-reference` |
| W15 | 11 | W10 + `sandbox-policy` |
| W33 | 11 | W3 + `compaction-image-offload`, `session-format-v1-to-v2`, `session-format-v2-to-v3`, `session-format-v3-to-v4` |
| W48 | 11 | W10 + `skill` |
| W49 | 11 | W10 + `subagent` |
| W12 | 13 | W48 + `office-kit`, `subagent` |
| W21 | 13 | W8 + `cordis-group`, `cordis-include`, `cordis-loader`, `app-boot`, `config-editor`, `settings` |
| W28 | 13 | W49 + `commands`, `user-questions` |
| W35 | 13 | W33 + `session-format-catalog`, `session-persistence` |
| W45 | 13 | W21 + `sandbox`, `shell` − `config-editor`, `settings` |
| W51 | 13 | W37 + `cordis-loader`, `deepseek-llm-api-extensions`, `llm-deepseek`, `llm-deepseek-account`, `llm-deepseek-api-key`, `web` |
| W42 | 14 | W21 + `agent-preset-registry` |
| W31 | 16 | W33 + `session-format-catalog`, `session-title`, `tool-todo`, `tools`, `user-approval` |
| W46 | 18 | W45 + `fs`, `jobs`, `sandbox-policy`, `shell-env`, `working-directory` |
| W26 | 38 | W31 + `cordis-group`, `cordis-include`, `cordis-loader`, `agent-default-model`, `agent-preset-registry`, `api-gateway`, `api-session-controller`, `app-boot`, `client-connection`, `client-file-upload`, `config-editor`, `file-reference`, `fs`, `sandbox`, `session-persistence`, `session-query`, `settings`, `skill`, `subagent`, `typert-registry`, `working-directory`, `workspace` |
| W27 | 39 | W26 + `schedule` |
| W29 | 45 | W27 + `deepseek-llm-api-extensions`, `goal`, `llm-deepseek`, `llm-deepseek-api-key`, `sdk-protocol`, `user-questions` |

Дополнительные манифесты, необходимые для воспроизведения workspace-замыканий:

| Группа | Основания |
|---|---|
| Cordis loader | `vendor/group/package.json:32`, `vendor/include/package.json:32`, `vendor/loader/package.json:32` |
| Boot/config | `P/boot/app-boot/package.json:37`, `P/boot/config-editor/package.json:34`, `P/settings/settings/package.json:39` |
| Core/skills/web | `P/core/session/package.json:47`, `P/core/system-prompt/package.json:34`, `P/core/agent-default-model/package.json:34`, `P/skill/skill/package.json:34`, `P/web/web/package.json:52` |
| API/client/Typert | `P/api/gateway/package.json:65`, `P/api/session-controller/package.json:77`, `P/client/connection/package.json:39`, `P/client/file-upload/package.json:70`, `P/typert/protocol/package.json:41`, `P/typert/registry/package.json:58` |
| Старые форматы | `P/session/session-format-v1-to-v2/package.json:44`, `P/session/session-format-v2-to-v3/package.json:44` |
| DeepSeek и goal | `P/llm/deepseek-llm-api-extensions/package.json:39`, `P/llm/llm-deepseek/package.json:32`, `P/llm/llm-deepseek-account/package.json:34`, `P/llm/llm-deepseek-api-key/package.json:34`, `P/goal/goal/package.json:56` |
| Native/Windows | `P/util/lazy-require/package.json:32`, `P/subprocess/win32-process/package.json:32`, `P/sandbox/sandbox-windows-acl/package.json:39` |

Таким образом, заявленный runtime-граф с обязательными peers после отсечения `done` содержит **92 workspace-пакета: исходные 65 и ещё 27**. Это не означает, что все 92 надо активировать как плагины: часть нужна как библиотека или указана peer-зависимостью для типов. Основания — объединение W-наборов и корней основной таблицы; различие между импортом и активацией видно, например, в `P/session/session-format-catalog/src/generated.ts:3` и `P/sdk/protocol/src/types.ts:11`.

**Где действительно нужен мост к уже портированному Rust**

У такого моста три разных задачи, и их стоимость нельзя складывать в одну строку «JSON-RPC»:

1. **Обычные функции и значения.** `brandString`, ошибки, JSON helpers, форматирование времени, codecs, patch helpers. Наименее затратный вариант (c) — оставить небольшие TS-копии этих зависимостей внутри Node-поставки. Асинхронный RPC не заменяет синхронный импорт класса ошибки, constructor или schema. Если принципиально исключить эти TS-модули, придётся менять импортирующий код или добавлять специальный binding. Примеры: `P/core/agent/src/model-selection.ts:7`, `P/fs/tool-fs/src/apply-patch-presentation.ts:2`, `P/session/session-format-v3-to-v4/src/migration.ts:3`.
2. **Состоящие из данных сервисы.** Credentials, storage, storage-domain, subprocess и filesystem. Здесь RPC реалистичен, но требует собственных handles, транзакций/flush, версий, потоков, отмены и subscriptions. Примеры: `P/schedule/schedule/src/storage.ts:8`, `P/document/office-kit/src/index.ts:288`, `P/subprocess/subprocess-local/src/index.ts:210`.
3. **Cordis/Scope и живое ядро.** Context, Fiber, effects, scoped dispatch, function handlers и `next` middleware. Уже портированный `hoh-kernel` не позволяет просто удалить TS Cordis из Node. Для неизменённых плагинов нужен местный TS runtime; для Rust-владельца состояния — новые фасады и явный протокол. Особо жёсткая граница — синхронное создание Session с возможностью veto и публикация batch events: `P/core/session/src/index.ts:43`, `P/core/session/src/index.ts:67`. Process-local scope прямо обозначен в `P/core/agent/src/index.ts:2`.

При сохранении всех исходных TS-зависимостей, включая уже портированные, тот же заявленный граф всех 65 корней имеет **129 workspace-пакетов и 79 внешних npm-пакетов**. При отсечении `done` внешнее объединение составляет **76**, без обязательных workspace-peers — **70**. Это объединения, а не сумма столбца по строкам.

Три внешних пакета, добавляющиеся при сохранении портированных TS-зависимостей: `@standard-schema/spec@1.1.0`, `rrule@2.8.1`, `undici@8.11.2`. Основания: runtime-манифесты перечисленных графов; `vendor/cordis/package.json:50`; `L:30382`, `L:34857`, `L:35489`. Состав фактической установки и загрузки модулей **не проверено**.

**Как соединить Node и Rust**

Для нескольких вынесенных сервисов предлагается один долгоживущий Node-процесс с двусторонним NDJSON JSON-RPC: request ID, result/error, notifications для потоков и изменений, отдельные операции отмены и закрытия handles. Это проектное предложение; готовая обвязка для всех строк **не проверено**. В клоне уже есть пригодная транспортная основа — `JsonRpcLineTransport`, включая pending requests и cancellation; но она не предоставляет engine/service API сама по себе. Основание: `P/sdk/protocol/src/transport.ts:34`, `P/sdk/protocol/src/transport.ts:65`.

Для сохранения значительной части ядра дешевле другая граница: **Rust управляет целым TS engine через существующий SDK**, а Agent/Session/loop/tools/projections остаются совместно в Node. Протокол уже имеет prompt/query, notifications и обратные пользовательские запросы; server подключён к session events, approval и lifecycle root runtime. Это сокращает разработку общего kernel-моста, но откладывает использование соответствующих Rust-портов в рабочем engine. Основания: `P/sdk/protocol/src/types.ts:17`, `P/sdk/protocol/src/types.ts:59`, `P/sdk/server/src/server.ts:123`, `P/sdk/server/src/server.ts:164`, `P/sdk/server/src/index.ts:50`.

Для гибридного engine нужно заранее выбрать владельца каждого вида состояния:

- Rust Session и journal writer либо TS Session и journal writer.
- Rust process supervisor либо Node subprocess backend с собственными handles.
- Rust scheduler/storage либо Node scheduler с подтверждаемыми storage/delivery RPC.

Это вывод из наличия in-memory registries, scoped lifecycle, file leases и ожидаемых checkpoint-барьеров; готовой реализации такого разделения в исследованных пакетах **не проверено**. Основания: `P/jobs/jobs-local/src/index.ts:184`, `P/subprocess/subprocess-local/src/index.ts:77`, `P/session/session-persistence-jsonl/src/lease.ts:20`, `P/session/session-checkpoint-policy/src/index.ts:35`.

**Фиксированная стоимость Node в продукте**

Даже один оставленный пакет требует JS runtime. Корневой manifest dsh задаёт **Node `>=24.9.0`**, ESM и `pnpm@11.28.5`; проверка работы выбранного поднабора на более старом Node **не проверено**. Основание: корневой `package.json:6`.

Практическая стоимость поставки:

| Статья | Что придётся поставлять или реализовать | Что измерено |
|---|---|---|
| JS runtime | Зафиксированный Node для каждой поддерживаемой платформы либо обязательный внешний Node | Размер бинарника, архива и RSS — **не проверено** |
| Исполняемые модули | Скомпилированные JS, locale/assets и нужные npm/workspace dependencies. Манифесты экспортируют `lib`; исходный TS использует, в частности, Typert decorators | Размер production-артефакта — **не проверено**. Основания: `P/document/office-kit/package.json:18`, `P/typert/protocol/src/index.ts:295`, `tsconfig.base.json:6` |
| Сборка поставки | Отдельный JS build на этапе разработки/релиза. `typescript`, `tsx`, pnpm не обязаны становиться runtime-зависимостями готового sidecar | Такая поставка — **не проверено**. В клоне build/runtime разделены манифестами и `lib` exports; корневой `package.json:20`, `P/sdk/protocol/package.json:18` |
| Управление процессом | Запуск, readiness, stderr/stdout, завершение, очистка ресурсов, обработка падения и совместимость версий RPC | Время старта и RPC latency — **не проверено**. Точки lifecycle: `P/sdk/server/src/index.ts:45`, `P/sdk/server/src/index.ts:69` |
| Native-компоненты | Для выбранных строк — node-pty, koffi, dsh system addon/launcher; для Office — Kit engine/assets и шрифты | Размер и наличие нужных binaries — **не проверено**. Основания: `P/subprocess/subprocess-local/src/index.ts:55`, `P/sandbox/sandbox-local/src/index.ts:29`, `P/document/office-kit/src/index.ts:346` |
| Сопровождение | Версии Node/npm-пакетов, patches и third-party notices становятся частью выпуска вместе с Rust | Трудозатраты — **не проверено**; зафиксированные patches находятся в `L:12` |
| Внешние утилиты | Shell/sandbox/backend могут дополнительно требовать системные executable; они не входят в число npm-зависимостей | Доступность на целевой машине — **не проверено**. Например, bwrap probe: `P/sandbox/sandbox-local/src/index.ts:86` |
| Workspace runtime payload | Пакет `tool-workspace-dependencies` может предоставлять отдельно собранные Python/Node/pnpm payloads | Объём — **не проверено**; это отдельная стоимость, а не транзитивный npm-хвост sidecar. Основание: `P/skill/tool-workspace-dependencies/src/index.ts:18` |

Один общий Node-процесс позволяет разделить стоимость runtime и зависимостей между пакетами. Отдельный процесс на каждую строку добавляет границы между тесно связанными сервисами и не соответствует нынешней модели общего Context; количество процессов и их ресурсный эффект **не проверено**. Основания связанности: `P/core/agent-loop/src/index.ts:291`, `P/core/tools/src/index.ts:99`, `P/subagent/subagent/src/index.ts:199`.

**Что выбрать для ускорения**

По исследованным границам наиболее убедительные кандидаты на ограниченный вариант (c):

- **`office-kit`**: уже выраженные операции конвертации и рендеринга; дорогой внешний engine остаётся на привычной JS-стороне. Нужны cancellation, bytes/paths и output lifecycle. Основание: `P/document/office-kit/src/index.ts:185`.
- **Исторические codecs и миграции**, прежде всего V3→V4: передавать целый артефакт и факты детей. Сохранять маленькие TS codec-зависимости проще, чем связывать синхронные функции с Rust через RPC. Catalog требует дополнительной Session-библиотеки для current validation. Основания: `P/session/session-format-v3-to-v4/src/migration.ts:34`, `P/session/session-format-catalog/src/current.ts:45`.
- **Filesystem и subprocess/shell группами**, если именно эти порты задерживают engine: один владелец watchers, stale guards, process handles, streams и cleanup. Нужна полноценная сервисная обвязка; её готовность **не проверено**. Основания: `P/fs/fs-local/src/index.ts:76`, `P/subprocess/subprocess-local/src/index.ts:77`, `P/shell/bash-sandbox/src/index.ts:59`.
- **Целый TS engine за SDK**, если приоритет — получить рабочий продукт быстрее использования Rust-ядра. Это наиболее широкое откладывание переноса с уже существующей внешней границей. Основания: `P/sdk/server/src/index.ts:2`, `P/sdk/server/src/index.ts:58`.

Для коротких регистрационных/интерфейсных пакетов отдельный процесс выглядит невыгодно: `tool-executor-native` — 35 строк, `agent-preset` — 30, `subagent-spawn-in-process` — 59. Это оценка масштаба интеграции, а не измеренные сроки; фактическая экономия **не проверено**. Основание: соответствующие строки `research/v1-unstarted.tsv` и их реализации `src/index.ts`, указанные в таблице.

**Приёмка гибридного решения**

Сам по себе Node-процесс не делает записанные сценарии непригодными. Если запускается итоговый гибридный бинарник, реальные инструменты выполняются, а все прежние сравнения сохраняются, сценарии продолжают доказывать наблюдаемую эквивалентность **гибридного продукта**. Они не доказывают, что оставленное поведение реализовано в Rust. Это вывод из существующего критерия приёмки: `docs/porting/PORTING.md:8`, `crates/hoh-conformance/README.md:515`, `crates/hoh-conformance/README.md:580`.

Особенно полезны для проверки новых границ уже существующие каталоги:

| Граница | Сценарии под `snapshots/` |
|---|---|
| Compaction и meter | `session/compaction-recovery`, `session/compaction-output-reserve`, `sdk/manual-compact`, `sdk/minimal-image-compaction` |
| Office и файлы | `session/office-render-tools`, `session/office-progressive-native`, `session/office-progressive-ptc` |
| Shell/process/jobs | `session/background-confinement-failure`, `session/background-job-admission`, `session/pty-tools-sandbox-backend`, `sdk/background-wait-steer` |
| Session/cwd/query | `session/working-directory`, `session/working-directory-admission`, `session/session-query-spill`, `sdk/session-title-stability` |
| Subagents | `session/subagent-parallel`, `session/subagent-spawn-in-process`, `sdk/subagent-send-message`, `sdk/subagent-continuable` |
| Skills/scheduler | `session/skill-load`, `session/skill-root-isolation`, `sdk/tool-scheduler-recovery` |

Основание существования каждого сценария — `snapshot.yml:1` в соответствующем каталоге клона. Их выполнение на гибридном продукте **не проверено**.

К прежним 242 сценариям понадобятся дополнительные проверки именно моста: отмена и поздний ответ; падение Node во время append/flush; смерть владельца subprocess и cleanup потомков; stream backpressure; сохранение missing/null, bytes и кодов ошибок; порядок session events; checkpoint до побочного эффекта; единоличное владение journal lease. Они дополняют прежнюю приёмку. Подмена реального результата инструмента записанным результатом или ослабление нормализации остаются недопустимыми. Основания: `P/sdk/protocol/src/transport.ts:39`, `P/subprocess/subprocess-local/src/index.ts:105`, `P/session/session-persistence-jsonl/src/lease.ts:20`, `P/session/session-checkpoint-policy/src/index.ts:52`, `crates/hoh-conformance/README.md:580`. Эти дополнительные проверки не реализованы и не запускались — **не проверено**.