# hoh-conformance — как dsh воспроизводит и проверяет записанные сценарии

Этот документ описывает **существующий** механизм dsh, чтобы по нему написать Rust-прогонщик. Кода здесь нет.
Источник: клон DeepSeek Harness (коммит `39163dd3`), `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`.
Все пути ниже — от корня клона, если не сказано иное. Ничего из клона не запускалось, не собиралось и не
устанавливалось: всё получено чтением кода и данных. В конце (раздел 8) перечислено, что не проверено.

Охват: три набора сценариев, у каждого каталога есть `snapshot.yml`.

| Набор | Каталог | Сценариев | Профиль процесса | Кто управляет процессом |
|---|---|---|---|---|
| `session` | `snapshots/session` | 192 | `headless` | `snapshots/session/headless.snapshot.ts` (одноразовый запуск, задача аргументом) |
| `sdk` | `snapshots/sdk` | 41 (40 `sdk`, 1 `sdk-minimal`) | `sdk` / `sdk-minimal` | `snapshots/sdk/sdk.snapshot.ts` через `packages/sdk/client` (JSON-RPC по stdio) |
| `acp` | `snapshots/acp` | 9 | `acp` | `snapshots/acp/acp.snapshot.ts` + `packages/test-support/session-snapshot/src/{suite,harness,launcher}.ts` (ACP JSON-RPC по stdio) |

Итого 242 сценария; построчная таблица — `conformance/EXPECTED.tsv`. Набор `snapshots/web` (браузер) в охват не входит.
`llm-mock-server` (`packages/test-support/llm-mock-server`) в снимочных сценариях **не участвует**: это
Messages-совместимый HTTP-сервер сбоев для других тестов (`apps/cli/tests/*`), модель в сценариях подменяется
иначе (п. 1.4).

Общая идея (`snapshots/AGENTS.md`, `docs/testing.md`): committed-журнал сессии `session[.vN].jsonl` — одновременно
**вход** (из него берутся задача пользователя и записанный поток модели) и **ожидаемый результат** (то, что
процесс сам запишет на диск, после нормализации сравнивается с ним). Дополнительно сравниваются дерево файлов
рабочей папки, stdout/stderr (headless), протокольные кадры (ACP, SDK) и «пины» заголовков запроса
(системный промпт и схемы инструментов в отдельных файлах-сателлитах).

---

## 1. Алгоритм воспроизведения

### 1.1. Выбор входных файлов (общий для всех наборов)

Источник: `packages/test-support/session-snapshot/src/session-files.ts`.

1. Имена журналов: родитель `session[.vN].jsonl`, дети `session.<k>[.vN].jsonl`; `v0` без суффикса, `vN` строчными,
   `k` и `N` без ведущих нулей. Регулярка: `^session(?:\.([1-9]\d*))?(?:\.v([1-9]\d*))?\.jsonl$`.
   Имя вида `session*.jsonl`, не подходящее под регулярку, — ошибка.
2. Для каждой роли (0 = родитель, 1.. = дети) берётся **наибольшее** `N`. Старые поколения в каталоге остаются, но
   не используются (`sessionFixtureFiles`). Роли обязаны идти подряд (0,1,2…), иначе ошибка.
3. Имя файла обязано совпасть с `version` в первой строке (заголовке `{"type":"session",…}`);
   у заголовка без `version` поколение 0 (`assertSessionFixtureVersion`).
4. Порядок ролей = порядок «родитель, затем дети». Дети привязываются к живым сессиям по порядку первого вызова
   модели (п. 1.4), а при сравнении логов — по порядку создания (п. 3.1).

В корпусе (три набора) выбранные поколения: v4 — 187 сценариев `session`, 40 `sdk`, 7 `acp`; v3 — 2 `acp` (без
модели, журнал не сравнивается); v0 — 4, v1 — 1 (`session`), v2 — 1 (`sdk`): это «удержанные» исторические
сценарии (п. 3.5). Текущий формат записи — **V4** (`docs/session-format-status.md`, `SESSION_FORMAT_VERSION` в
`packages/core/session/src/types.ts`); процесс под тестом обязан писать заголовок `version: 4` (headless и SDK это
проверяют: `expect(assertPersistedSessionVersion(…)).toBe(SESSION_FORMAT_VERSION)`).

### 1.2. Запуск процесса

Во всех наборах процесс — это штатный `dsh` (`apps/cli/src/bin.ts`) с профилем и накладками (`--patch`). Отдельных
режимов и точек входа для тестов нет (`snapshots/AGENTS.md`). dsh запускается из исходников через `node --import tsx`
(или собранный `lib`); для нас важны **аргументы, окружение и рабочие папки**.

**Слои конфигурации.** Профиль = пакеты-«бандлы» в порядке (`packages/util/official-bundles/src/bundles.ts`,
`PROFILE_TEMPLATES`): `headless` = `dsh-base` + `dsh-headless`; `sdk` = `dsh-base` + `dsh-sdk-app`; `acp` = `dsh-base` +
`dsh-acp-app`; `sdk-minimal` = только `dsh-sdk-minimal`. Файлы: `packages/bundle/{base,headless,acp-app,sdk-app,sdk-minimal}/cordis.patch.yml`.
Затем собственный слой профиля, слой `$DSH_HOME/cordis.patch.yml`, затем `--patch` по порядку argv, затем переключатель
телеметрии (`apps/cli/src/profile-boot.ts`, `composeProfile`). Все слои — это **патчи поверх пустого корня `[]`**
(`vendor/include/src/patch.ts`, `applyPatches`):

- `- insert: [...]` — добавляет строки в конец (или в группу, если указан `id`);
- `- id: X` + поля — находит строку по `id` и **заменяет** перечисленные поля (`config` заменяется целиком, не
  сливается); если указано `name` и оно не совпало с именем строки — патч пропускается с предупреждением;
  неизвестный `id` — предупреждение и пропуск;
- у строки поле `disabled`, `inject`, `group`; значения могут быть выражениями `!!js …` (п. 1.6).

**Какие патчи подключает прогонщик** (воспроизведение, не запись):

| Набор | Патчи в порядке argv |
|---|---|
| `session` (headless) | `snapshots/session/text-turn/cordis.yml` (общая «default» база); `<владелец композиции>/cordis.snapshot.yml`; `snapshots/session/text-turn/model.cordis.yml`; при наличии `<сценарий>/runtime.cordis.yml`. Владелец композиции — единственный каталог с `cordis.yml`, чей `composition` совпал. Для композиции `default` и в режиме воспроизведения тоже берётся `cordis.snapshot.yml` (`headless.snapshot.ts`, `patchSources`). Особый случай `plugin-manager-mcp`: `profile.patch.yml` копируется как слой профиля `.dsh/profiles/headless/cordis.patch.yml`. |
| `sdk` | если композиция начинается с `sdk-`: `<владелец>/cordis.yml`, `<владелец>/cordis.snapshot.yml`; иначе — headless-база `text-turn/cordis.yml`, `<владелец>/cordis.snapshot.yml`, `text-turn/model.cordis.yml` (владельцы ищутся среди `session` и `sdk`). Перед ними `snapshots/sdk/legacy-context.patch.yml` для перечисленных в `legacySdkCompositions` композиций, после — патчи из таблицы `SDK_ASSERTIONS` (`sdk.snapshot.ts`; `capture.cordis.yml`, `runtime.cordis.yml`, `apps/cli/config/examples/team-mode.yml` и т.д.), у `session-resume` ещё `replay.cordis.yml`. |
| `acp` | `snapshots/acp/escalation-approved/cordis.yml` (база для всех) + `cordis.snapshot.yml` **выбранной** композиции: `image-compaction/cordis.snapshot.yml` для `image-compaction`, у остальных — `escalation-approved/cordis.snapshot.yml` (`launcher.ts`, `profileArgs`, `replayPatchPath`). |

Каждый файл-накладка перед запуском «материализуется» (`launcher.ts`, `materializeProfilePatch`): относительные
имена плагинов (`./x.mjs`, `../../scripts/x.ts`) превращаются в `file://`-URL от каталога патча; для голых имён
пакетов (`@deepseek-ai/dsh-…`), разрешимых от файла патча, создаётся симлинк в
`<cwd>/.dsh/profiles/<профиль>/node_modules/<пакет>`. Материализованные копии лежат в `<cwd>/.snapshot-patches/`
(headless, SDK) или `<cwd>/.dsh-profile-patches/launch-*/` (ACP).

**Аргументы командной строки.**

- headless: `--profile headless --patch .snapshot-patches/0-… --patch .snapshot-patches/1-… … <задача>`; задача — последний
  позиционный аргумент. Для `provider-cwd` дополнительно `--session-id <id>`, `--json` (`headless.snapshot.ts`,
  `verifyProviderCwdResume`). stdin закрывается сразу, таймаут процесса 30 с (`packages/test-support/loader-smoke/src/index.ts`).
  Ожидаемый код возврата: 0, если последний `turn/end` записанного журнала имеет `reason.kind === 'completed'`, иначе 1.
- sdk: `--profile sdk|sdk-minimal --patch <абс. путь> …` (`packages/sdk/client/src/launch.ts`); дальше JSON-RPC по stdio.
- acp: `--profile acp --patch <база> --patch <реплей-патч>`; дальше ACP по stdio.

**Окружение (воспроизведение).** Процесс наследует `process.env` прогонщика (HOME и прочее **не изолируются**), поверх
него (`harness.ts`, `headless.snapshot.ts`, `sdk.snapshot.ts`):

| Переменная | Значение |
|---|---|
| `DEEPSEEK_API_KEY` | `snapshot-replay-key` (`REPLAY_CREDENTIAL_ENV`); без неё часть инструментов (`web_search`) не появилась бы |
| `DSH_SNAPSHOT` | `replay` |
| `DSH_SNAPSHOT_FILE` | абсолютный путь выбранного журнала родителя |
| `DSH_SNAPSHOT_CHILD_FILES` | пути журналов детей через разделитель путей ОС (`:`), только если дети есть |
| `DSH_SNAPSHOT_OVERRIDE` | путь к `replay.override.json`, если `snapshot.yml: replay.override: true` |
| `DSH_SNAPSHOT_PROVIDER`, `DSH_SNAPSHOT_MODEL` | провайдер и модель из первого `request/header` родительского журнала (`config.provider`, `config.model`); читаются патчем `model.cordis.yml` (плагин `agent-default-model`) |
| `DSH_SNAPSHOT_SPILL_ROOT` | временная папка для файлов «спилла» (`acp-snap-spill-XXXX`) |
| `DSH_SNAPSHOT_SPILL_LOCATOR_ROOT` | `/tmp/dsh-acp-snap-<9 hex>`, где hex = первые 9 символов `sha256(<имя каталога сценария>)` (`snapshotSpillRoot`) |
| `DSH_SNAPSHOT_SESSIONS_ROOT` | только ACP: временная папка корня сессий (`acp-snap-sessions-XXXX`) |
| `DSH_HOME` | `<cwd>/.dsh` |
| `DSH_AGENTS_HOME` | `<cwd>/.agents` |
| `DSH_PERMISSION_MODE` | `snapshot.yml: permission`, если задан |
| `DSH_TELEMETRY_DISABLED` | `1` (headless, SDK; ACP-прогонщик её **не** задаёт — см. раздел 7) |
| `NODE_OPTIONS` | с `--disable-warning=ExperimentalWarning` (headless, SDK) — чтобы stderr был пустым |
| прокси (`HTTP_PROXY`, … из `clearedProxyEnv()`) | удалены |
| всё из `snapshot.yml: environment` | добавляется поверх (в корпусе: `DEEPSEEK_API_KEY=''`, `DEEPSEEK_BASE_URL`, `DSH_MCP_TEMPLATE_TOKEN`, `DSH_SNAPSHOT_FETCH_KEY`, `DSH_SNAPSHOT_SEARCH_KEY`, `DSH_SNAPSHOT_MISSING_SANDBOX_RUNNER`, `DSH_SNAPSHOT_PROVIDER_CWD`, `DSH_SNAPSHOT_FEEDBACK`, `DSH_SUBAGENT_PUBLISHED_FAILURE`, `ELECTRON_RUN_AS_NODE`) |

Для SDK также `DSH_SNAPSHOT_HUMAN_STEER=1` (`subagent-continuable`), `DSH_SYSTEM_PROMPT`, `DSH_TEST_CHILD_*`,
`DSH_TEST_PARENT_PROVIDER` (таблица `SDK_ASSERTIONS`). У headless есть сценарные `DSH_MCP_DEMO_URL`,
`DSH_MCP_RESOURCES_FIXTURE`.

**Рабочая папка.** Каждый прогон создаёт новую временную папку и **это же cwd процесса**:
headless — `mkdtemp(<tmp или «вне tmp»>/dsh-log-snap-)`; ACP — `acp-snap-cwd-`; SDK — `realpath(mkdtemp(tmp/sdk-snapshot-<имя>-))`.
Для `compaction-recovery` (headless), `minimal-image-compaction` и `manual-compact` (SDK) длина cwd **подгоняется под
192 символа JSON-строки**, чтобы оценка токенов от текста cwd не зависела от длины `/tmp` (`compactionWorkspaceOptions`,
`sdk.snapshot.ts` `fixedWidthCwd`). Для `workspace.parent: outside-temp` (4 сценария) cwd создаётся рядом с `/tmp`, но
вне его автоматических прав записи (`scripts/snapshot-workspace-parent.ts`). Перед запуском в папку копируется
`<сценарий>/workspace/` (каждый верхний элемент отдельно, симлинки сохраняются как есть), затем выполняется именованная
подготовка `workspace.setup` (10 штук: `office-skills`, `office-retired-ptc`, `delimiter-path`, `readonly-patch-directory`,
`dsh-doc-resources`, `fixed-search-mtimes`, `cua-script-guidance`, `skill-root-isolation`, `windows-acl-skill`, `git-worktree`;
код — `workspaceSetups` в `headless.snapshot.ts`). Часть подготовок копирует файлы **из самого клона dsh**
(`packages/skill/skill-office/assets`, `packages/preset/agent-preset/skills/dsh-doc`, `packages/experimental/cua*`), т.е. вход
сценария лежит вне каталога сценария. Затем снимается «начальное» состояние рабочей папки (п. 3.3).

Хранилище сессий процесса: `<DSH_HOME>/sessions` (headless, SDK) или `$DSH_SNAPSHOT_SESSIONS_ROOT` (ACP), патч
`session-persistence-jsonl` с `compression: none`. Прогонщик читает его **после** выхода процесса (рекурсивно, по
`<корень>/<проект>/<id-сессии>/session[.vN].jsonl`, берётся наибольшее `N` на каталог; `latestPersistedSessionPaths`).

### 1.3. Откуда берутся задача пользователя и ввод

- **headless (`session`)**: `taskFromSession` (`headless.snapshot.ts`) — **первое** сообщение `agent/inbox/spliced` с
  вставленным сообщением `source.kind === 'user'` и одним текстовым блоком (оригинальный текст с каноническими
  упоминаниями); если такого нет — первое `user/message` с теми же свойствами; если и его нет — `snapshot.yml: input.task`
  (в корпусе у трёх наборов `input.task` нет вовсе). `input.json` у headless запрещён проверкой корпуса
  (`scripts/session-snapshot-corpus.corpus.ts`). Остальные «ходы» в таких сценариях происходят изнутри (цели, подагенты,
  хуки), а не от пользователя.
- **sdk**: `sdkActions` (`sdk.snapshot.ts`) разбирает родительский журнал: на каждый `turn/start` берётся первое
  `user/message` с `source.kind === 'user'` (если оно не принадлежит вызову команды) → `session.run(content)`;
  блоки `image` восстанавливаются из `snapshot.yml: input.attachments` по `attachmentId` (base64 из манифеста);
  в тексте `{{cwd}}` заменяется на живой cwd, `{{session:N}}` — на живой id N-й сессии. Если у сценария
  `replayStandaloneCommands`, то события `command/run` (источник `user`) воспроизводятся как `/<name><args>`. Идентификатор
  корневой сессии фиксирован: `fixture-root-session` (клиент сам выбирает id).
- **acp**: единственный набор с `input.json` (скрипт шагов, не выводится из журнала): операции `initialize`, `newSession`,
  `newSessionExpectError`, `prompt`, `promptContent`, `promptAndCancel` (+`waitForFile`/ожидание `turn/start`), `waitForTurnEnd`,
  плюс `permissionAnswers` (в корпусе `allow_once`, `reject_once`). Полный набор операций — тип `InputStep` в
  `packages/test-support/session-snapshot/src/harness.ts`; в корпусе используются 7 из них.

### 1.4. Поток модели: на каком слое и в каком формате

**Слой — адаптер LLM внутри процесса**, а не HTTP и не провод. Плагин `@deepseek-ai/dsh-llm-replay`
(`packages/test-support/llm-replay/src/index.ts`) монтируется патчем `cordis.snapshot.yml`
(`insert: - id: llm-replay …`), а настоящий адаптер `llm-deepseek` в том же патче отключается (`disabled: true`).
Плагин регистрирует адаптер для провайдеров из своего `config.providers` (в корпусе — 127 из 128 вставок так; единственная
без каталога использует перехват `llm/stream`). Каталог провайдеров несёт свойства маршрута, влияющие на запрос и
заголовок: `models[].contextWindow`, `defaultMaxTokens`, `defaultReasoningEffort`, `reasoningEfforts`,
`inputModalities`, `imageRequestTokens`, `systemPromptUpdate`, `toolUpdate`, `badge`; `retryPolicy` на уровне провайдера;
`paceMs`.

**Формат — внутренние `StreamChunk`** (`packages/llm/llm`): `block-start{index,blockType}`, `text-delta`,
`reasoning-delta`, `tool-call-delta{index,id,name?,argumentsDelta}`, `block-end{index,block}`, `usage{usage}`,
`finish{reason}`. HTTP, SSE и провайдерский провод в воспроизведении не участвуют; поэтому тела запросов к модели
вообще **не сравниваются** (кроме побочных проверок SDK, п. 5).

**Как строится скрипт** (`deriveReplayScript`):

1. Журнал разбирается строгим каталогом форматов (`createSessionFormatCatalogWithChildren…createRestore`,
   `recovery: 'strict'`, `validation: 'current'`) и **мигрируется до текущего V4 в памяти**; файл не меняется.
   Строки тела либо все с `seq`+`time`, либо все без (в зафиксированных фикстурах без; они получают плотные `seq` от 0 и
   `time: 0`); смесь — ошибка. Токен `{{cwd}}` в заголовке временно заменяется на `/dsh-snapshot-cwd`.
   Токен `{{tools}}` в `request/header` временно убирается.
2. По порядку событий: каждое `assistant/message` и `assistant/attempt` даёт один вызов модели из встроенного `data.stream`
   (`expandAssistantStream`: упакованные записи `text-chunks`/`reasoning-chunks`/`tool-call-chunks`
   `{time0,index,dt[],texts|args,id?,name?}` разворачиваются в отдельные дельты с сохранением границ; `chunk` — «сырые»
   `block-start/block-end/usage/finish`). Вызов без завершающего `finish` — ошибка («нужен override»).
3. `compaction/summary` с `llmStreamCall: true` даёт отдельный вызов: по каждому блоку `rawOutput` пара
   `block-start/block-end`, затем `usage` (если есть) и `finish{stop}`.
4. Для детей берутся только события **после** `session/end-seed` (метка границы унаследованного префикса форка;
   вне формата V4 это «inheritedEventCount» декодера каталога): пример — `snapshots/sdk/subagent-fork-in-process/session.1.v4.jsonl`,
   строка с `session/end-seed {"inherited": true}`.

**Привязка к живым сессиям.** Скрипты упорядочены: родитель, затем дети по `(createdAt, id)` заголовка. Живая сессия
(по `options.sessionId` вызова; вызов без id — «анонимная») при **первом** вызове модели занимает следующий
непривязанный скрипт; дальше её курсор идёт по записям строго по порядку. Вызов от «лишней» сессии — ошибка
`a model call arrived from an unrecorded session`; вызов за пределами скрипта — `script exhausted`. Проверка «скрипт
использован целиком» (`assertConsumed`) существует, но снимочные прогонщики её **не вызывают**: недобор вызовов
выявляется только расхождением журнала. SDK-прогонщик отдельно проверяет, что в журналах нет строки
`llm-replay: script exhausted`.

**Подстановки в записях скрипта** (`resolveScriptedEntry`, `materializeSessionTokens`):

- `{{session:N}}` → живой id N-й привязанной сессии;
- `{{fromRequest:<regex>}}` — подстановка из живого запроса: корпус = все строковые листья `messages` запроса, склеенные `\n`;
  берётся **последнее** совпадение, группа 1 (или всё совпадение); не совпало — ошибка. В корпусе — 10 файлов
  `replay.override.json`: `goal-[0-9a-f-]+`, `Full formatted result stored at: (.+-session_event_read[.]txt)[.] Use read`,
  `saved at \"([^\"]+)\"`, `<path>(.+)/red\.png</path>`. Т.е. живой запрос должен содержать эти фрагменты текста.
- `{{cwd}}` внутри записанных аргументов инструментов в headless/ACP **не** подставляется (четыре сценария записаны с таким
  токеном: `sdk/persistent-tools`, `sdk/subagent-dsh-sdk-dynamic-route`, `session/read-image-attachment-path`,
  `session/ptc-read-image-attachment-path`); SDK заранее копирует журналы в `<cwd>/.replay-fixtures/` с заменой `{{cwd}}`→живой cwd
  (`hydrateReplayFixtures`), а эти два headless-сценария используют `fromRequest`.

**Исключительные записи (`replay.override.json`, 43 сценария: 35 headless, 6 SDK, 2 ACP).** Документ — либо массив записей
(заменяет скрипт целиком), либо `{ "patches": [{ "at": i, "entry": … }] }` (заменяет/дописывает вызов i). Виды записи
(`ReplayEntry`): `chunks` (как выведенные), `throw {chunks, message, code, accepted?}` (сначала отдаёт префиксные chunks, затем
`LlmError.fromFailure({message, code})`), `hang {readyFile?}` (отдаёт `block-start`+`text-delta "partial"`, пишет пустой файл
`readyFile` относительно cwd процесса и ждёт отмены). Прогонщик ACP в операции `promptAndCancel` ждёт именно этот файл
(`.dsh-snapshot-stream-ready`). Все вызовы учитывают `AbortSignal` (бросают `Error('aborted')`).

**После «успешного» вызова провайдера `deepseek-official`** воспроизведение выполняет побочные эффекты
`deepseekLlmApiExtensions.prepare({body:{messages:[]}, …}).accept()` — как будто пришёл ответ 2xx (без реального тела).

### 1.5. Выполнение инструментов: по-настоящему

Записанные результаты инструментов **не подставляются**. Реальны: реестр и конвейер инструментов, файловая система,
`bash` (через замену песочницы на `sandbox-local` с «проходным» раннером `bash -c … exec "$@"`, см. любой
`cordis.snapshot.yml`), дочерние процессы, MCP, подагенты и их сессии, цели, сжатие контекста, спилл, хранение, заголовки.
Подменена только модель (и там, где нужно, внешняя среда: плагины-подставки `snapshots/**/*.mjs|*.ts`, локальные HTTP-серверы
на порту `listen(0)`, подмена путей `scripts/snapshot-spill-locators.ts` и `scripts/snapshot-shell-path.ts`). Расхождение
записанного и фактического результата инструмента проявляется как расхождение журнала (п. 3.1), а не отдельной проверкой.
Принцип зафиксирован в `docs/testing.md` («Verify the world, not the self-report»).

### 1.6. Выражения `!!js` в патчах (важно для нас)

Патчи — YAML с вычисляемыми значениями. Словарь, использованный в корпусе и бандлах (подсчёт по `grep '!!js'`):
`process.env.X` (с `??`, `||`, `===`, `!==`), `process.cwd()`, `process.execPath`, `process.platform`,
`process.getBuiltinModule('node:path').join/resolve`, `dshHomePath(...)`, `ctx.get('profileContext')`,
`ctx.headlessStartup.task|sessionId|json|additionalWritableRoots`, `ctx.webStartup.*`, `Number(...)`, `JSON.parse(...)`,
`new URL(..., 'file://' + process.env.DSH_SNAPSHOT_FILE).pathname`, `decodeURIComponent(...)`. Чаще всего
(123 раза) — `dshHomePath('sessions')`. Кто их вычисляет в Rust — решение для `hoh-loader` или предобработки в прогонщике
(раздел 7).

### 1.7. Участие дочерних сессий (`session.<n>.jsonl`)

- Скрипт каждого ребёнка выводится из его журнала (п. 1.4) и привязывается к живому ребёнку по порядку первого вызова модели.
  Дети стартуют синхронно в порядке создания; для **параллельных** детей порядок привязки не гарантирован (в коде
  пометка `XXX(concurrent-subagents)`), поэтому такие сценарии содержат плагины-упорядочиватели
  (`settlement-order.mjs`, `execution-order.mjs`, `subagent-settlement-barrier.ts`, `wait-order.mjs`).
- При сравнении журналов дети сопоставляются ролям `session.<k>`: headless — по порядку записей `subagent/catalog` родителя,
  затем `createdAt` (`persistedSessions`); SDK — по `createdAt` (`orderLogs`); ACP — по `(createdAt, id)` (`harvestSessionLogs`).
  Эти три порядка различаются, их надо повторить для каждого набора отдельно.
- Заголовок ребёнка: `parentSession`, `isSeeded`, `origin: "subagent"`, `delegationDepth`.
  Запись `subagent/catalog.childCreatedAt` обязана **точно** совпасть с `createdAt` заголовка ребёнка до нормализации
  (`reconcileCatalogCreationTimes(…, 'validate')`, ACP и SDK).
- Сателлиты детей: `system-prompt.<k>.expected.md`, `tool-schemas.<k>.expected.json`, перечисленные в
  `header.childSystemPrompts` / `header.childToolSchemas` (`[1]` или `[1, 2]`).

---

## 2. Правила нормализации (исчерпывающе)

Код: `packages/test-support/session-snapshot/src/{normalize,identity}.ts`. Нормализация применяется **к обеим сторонам**
(живой журнал и закоммиченный) одинаково; закоммиченные журналы — «неподвижные точки» нормализации.

### 2.1. Токены, которые стоят в зафиксированных журналах

| Токен | Что заменяет | Где определён |
|---|---|---|
| `{{session:N}}` | id сессии (`N` — порядок первого появления; родитель и дети в одном пространстве) | `identity.ts` (`CANONICAL_TOKEN_RE`, `redactSessionSnapshotIds`) |
| `{{message:N}}` | id сообщения (`id` у объектов с `role`, `content[]`, `source{}`; плюс «as message <uuid>» в тексте) | там же |
| `{{approval:N}}` | `id` в `approval/asked`, `approval/decided` | там же |
| `{{workflow:N}}` | ключ `runId` | там же |
| `{{command:N}}` | ключ `commandId` (любое непустое значение) | там же |
| `{{rpc:N}}` | ключ `rpcId` | там же |
| `{{retry:N}}` | ключ `retryId` | там же |
| `{{id:N}}` | прочие ключи `id`, `*Id`, `*Ids`, если значение похоже на UUID-фрагмент или завершается `-<16 hex>` (идентификатор задачи), а также `Anonymous user: <uuid>`, `feedback/message-put.data.item.version` | там же (`ID_KEY_RE`, `UUID_FRAGMENT_RE`, `SHORT_JOB_ID_RE`) |
| `{{cwd}}` | сгенерированная рабочая папка (и её псевдонимы: `realpath`, нативный путь, с `/` вместо `\`; на macOS ещё `/private…`) | `normalize.ts` (`cwdSpellings`, `replaceCwd`); для записи фикстур — `tokenizeSessionFixtureCwd` |
| `{{system}}` | текст первого блока `system/message.data.message.content[0].text` | `normalize.ts` (`scrubModelRequestContent`, `scrubSystemPrompts`) |
| `{{tools}}` | поле `request/header.data.header.tools` (список схем). Если в журнале есть событие `developer/message`, вместо токена остаётся **упорядоченный список имён** инструментов | `normalize.ts` (`scrubToolSchemas`) |
| `{{spillLocator:<имя>}}` | путь файла спилла, оставляя только имя файла: `{{cwd}}/.spill/session-<12hex>/<12hex>-<имя>` и `/tmp/dsh-acp-snap-<9hex>/session-<12hex>/<12hex>-<имя>` (`/t/…` на Windows) | `normalize.ts` (`LOCAL_SPILL_PATH_RE`, `SNAPSHOT_SPILL_PATH_RE`) |
| `{{eventTime}}`, `{{eventOmittedBytes}}` | в результате инструмента чтения событий: `"time": <n>` внутри фрагмента ```json …``` целевого события; число «(Omitted N bytes.)» | `normalize.ts` (`EVENT_READ_TARGET_REGION_RE`, `EMBEDDED_EVENT_TIME_RE`, `EVENT_READ_OMITTED_BYTES_RE`) |
| `{{sourceSessionFormatVersion}}` | `session-log-deepseek/delivery-accepted.data.sessionFormatVersion`, если равно версии исходного журнала (только при сравнении выходных файлов записи, `nativeWriterOutput`) | `normalize.ts` (`normalizeSessionFormatMetadata`) |
| `{{usedTokens}}` | поле `used` в ACP-обновлении `usage_update` | `normalize.ts` (`scrubValue`) |
| `{{sessionId}}`, `{{messageId}}` | старый («legacy») режим: id известных сессий и **любая** UUID-подобная строка → `{{sessionId}}`; значение под ключом `messageId` → `{{messageId}}` | `normalize.ts` (`UUID_RE`, `scrubString`, `scrubValue`); используется для ACP stdout и для нормализации промптов/заголовков, но **не** для сравнения журналов целиком (там режим `preserve`) |
| `{{fromRequest:…}}`, `{{session:N}}` в записях скрипта | входные токены воспроизведения, не часть нормализации; см. п. 1.4 | `llm-replay/src/index.ts` |

В JSON-RPC-кадрах (ACP) числовой `id` заменяется порядковым номером первого появления по ключу `JSON.stringify(id)`
(`normalizeStdout`); запросы клиента и агента с одинаковым `id` получают общий номер.

### 2.2. Зануляемые значения (время и длительности)

`normalizeSessionLog` (`normalize.ts`): `createdAt` заголовка → 0; `time` каждого события → 0; в упакованных записях
`time0` → 0 и каждый элемент `dt` → 0 (длина массива сохраняется); внутри `assistant/message|assistant/attempt.data.stream[]`
— `time`, `time0`, `dt[]`; `hook/result.data.durationMs` → 0; `job/registered|settled.data.startedAt|finishedAt` → 0;
`goal/change.data.createdAt|updatedAt`; `subagent/catalog.data.childCreatedAt`;
`feedback/message-put.data.item.createdAt|updatedAt`; `plugin:timeline/tree` (`tree.createdAt`, `lastTaskUpdate`,
`tasks[].createdAt|updatedAt`, `receipt.at|previousAt`); `tool/result.data.meta.timeline.at|previousAt`.
Прочие числа **не** трогаются: токены в `usage` (берутся из записанного потока), размеры, счётчики, номера `turn`/`step`.

### 2.3. Конверты и ссылки на номера событий

`scrubSessionSnapshot`/`projectSessionSnapshot`: в каждой строке тела удаляются `seq`, `time`, `seq0`, `time0`
(заголовок остаётся как есть); `sourceEventSeqs` разворачивается в плоский массив (`decodeSeqRanges`). Ссылки на номера
событий **внутри данных** остаются и сравниваются: `sourceEventSeqs`, `headerSeq`, `messageSeqs`,
`surfaceOp {op:'replace', startSeq, endSeq}`, `shadowedRange`, `compaction/summary.sourceEventSeq`, `command/done.sourceEventSeq`.
Следствие: нумерация и число событий должны совпасть с dsh.

### 2.4. Пути

В режиме `canonical` (по умолчанию) разделители `\` заменяются на `/` только в путях, начинающихся с `{{cwd}}`, внутри
`<path>…</path>`, после `Additional instructions from: ` и в значениях ключа `path`. Совпадение cwd учитывается только на
границе пути (`isCwdMatch`), чтобы не затрагивать более длинные имена с тем же префиксом. На Linux прогон идёт в режиме
`canonical`; нативный Windows-вариант (`stdout.expected.windows.jsonl`) в этих трёх наборах не используется.

### 2.5. Сателлиты заголовков (системный промпт, схемы инструментов)

Читаемый текст не лежит в журналах — он вынесен в файлы рядом (`suite.ts`, `headless.snapshot.ts`):

- `system-prompt.expected.md` — текст промпта (узел 0 поверхности) + для каждого последующего `system/message`
  блок `\n<!-- system/message change N -->\n\n<текст>`; заканчивается `\n`;
- `tool-schemas.expected.json` — `{"initial":[…схемы…],"changes":[[…],…]}`, `JSON.stringify(…, null, 2)` + `\n`;
- `system-prompt.<k>.expected.md`, `tool-schemas.<k>.expected.json` — то же для ребёнка `k`.

Ровно одна «закрепляющая» (`header.pin: true`) запись на пару `composition/header.class` владеет сателлитами; остальные
ссылаются через `systemPromptSource` / `toolSchemasSource` (имя `сценарий` или `набор/сценарий`; три сателлита — симлинки,
цель проверяется корпусной проверкой). Разные содержимое: в корпусе 81 различных файла промптов и 77 различных
файлов схем.

### 2.6. Что нормализация НЕ делает

Не маскирует произвольный пользовательский или инструментальный текст («Never redact arbitrary user or tool text merely
because it resembles an identifier», `snapshots/AGENTS.md`); не сортирует события; не отбрасывает неожиданные поля
заголовка запроса (даже `system`); не нормализует имена и коды ошибок инструментов.

---

## 3. Сравнение

### 3.1. Журнал сессии

Общая функция `normalizeSessionSnapshots(rawLogs, ctx, {nativeWriterOutput: true})`
(`normalize.ts`), порядок для **каждой стороны** (живая и ожидаемая):

1. `hasSessionFormatVersion` — если в заголовке нет `version`, пропуск пункта 2 (v0-«проекция»).
2. `prepareSessionSnapshotFixtureForComparison` (`llm-replay/src/index.ts`): строгий разбор и миграция в V4 → снова
   кодирование V4 по одному событию на строку; `{{cwd}}` в заголовке и `{{tools}}` в `request/header` сохраняются.
   Живой журнал тоже проходит эту строгую проверку формата — **он обязан быть корректным V4 по схеме каталога**.
3. `normalizeSessionFormatMetadata` — у заголовка удаляется поле `version`; у `session-log-deepseek/delivery-accepted`
   версия исходника заменяется на `{{sourceSessionFormatVersion}}`.
4. `redactSessionSnapshotIds` над **всеми** журналами сценария вместе (родитель + дети) — типизированные токены с общим
   пространством номеров, порядок первого появления.
5. `normalizeSessionLog` в режиме `preserve` (без замены UUID в тексте, `ctx.sessionIds = []`): зануление времён (п. 2.2),
   замена cwd, спилл-локаторы, окна времени чтения событий.
6. `scrubSessionSnapshot` (`{{system}}`, `{{tools}}`/имена, удаление конвертов, разворот `sourceEventSeqs`).

Затем **сравнение распарсенных записей** `toEqual` (порядок ключей не важен, порядок записей важен; значения чисел и
строк — точные), отдельно для каждой роли `session`, `session.1`, …. Число журналов должно совпасть (`persisted session
count`). Источник ожидаемого: для обычного сценария — сам закоммиченный журнал выбранного поколения; для «удержанного»
(п. 3.5) — `writer[.k].expected.jsonl`.

Дополнительные проверки на живых журналах: перед первым `request/header` стоит `system/message`
(`systemPromptPrecedesRequests`, все три набора); число `system/message` = `1 + header.promptChanges` у родителя и 1 у
детей (все три набора). Только ACP-набор (`suite.ts`) ещё требует: ни одного результата с кодом `UNKNOWN_TOOL`
(`unknownToolCallIds`; headless и SDK этого не проверяют, так что у них `retired-tools` держится на сравнении с
`writer*.expected.jsonl`), и число `request/header` с `reason: "change"` равное `header.changes` (значения `reason`:
`initial`, `change`, `series`, `resume`; закрепляющие заголовки не включают `series`).

### 3.2. Заголовки запросов и сателлиты (`verifyHeaders`)

Для каждого журнала живого прогона: все `request/header` после нормализации (`normalizeSessionLog` в режиме **legacy**,
т.е. с заменой UUID и id сессий) сравниваются `toEqual` с заголовком из закрепляющего сценария своего класса, в который
подставлены полные схемы из `tool-schemas.expected.json` (`restorePinnedToolSchemas`: токен `{{tools}}` либо точный
порядок имён). Для детей с `childToolSchemas` подставляются их схемы. Для каждого `system/message` строка
`formatSystemPromptSnapshot(промпт…)` обязана **побайтно** совпасть с содержимым `system-prompt.expected.md`
(или `.<k>.` у ребёнка). Схемы сравниваются как значения JSON (порядок ключей объекта не важен, порядок массивов —
важен; строки, в том числе описания инструментов, — точные). Это единственное место, где проверяется текст
системного промпта и схем (в журнале они токенизированы).

### 3.3. Дерево рабочей папки

`captureWorkspaceSnapshot` (`workspace.ts`): рекурсивный обход, элементы сортируются побайтно по имени; запись для каждого
файла — `text` (если байты без `\0` и туда-обратно совпадают с UTF-8) с точным содержимым, иначе `binary` (base64);
симлинк — текст ссылки без разыменования; пустой каталог — `empty-directory`. Режимы доступа, время изменения и владелец
**не** сравниваются. Из корня исключаются только служебные элементы прогонщика:
headless — `.agents`, `.dsh`, `.snapshot-patches` (для `git-worktree` вместо `.agents` исключаются `.dsh`,
`.snapshot-patches`, `.git`; из итога убирается `.agents/worktrees/isolated/.git`), ACP — `.agents`, `.dsh`,
`.dsh-profile-patches`, `.dsh-snapshot-stream-ready`, SDK — `.agents`, `.child-dsh`, `.dsh`, `.replay-fixtures`, `.snapshot-patches`.

- `workspace.final: true` (41 сценарий) — итоговое дерево должно **равняться** `workspace.expected/`
  (`captureExpectedWorkspaceSnapshot`: тот же обход; корневой `.empty` игнорируется, вложенный `<каталог>/.empty`
  означает пустой каталог; `.empty` в живой папке — обычный файл). Для пустого итога каталог содержит только `.empty`
  (корпусная проверка `session-snapshot-corpus.corpus.ts`).
- иначе итоговое дерево должно **равняться начальному** (после копирования `workspace/` и `setup`): сценарий не имеет права
  менять рабочую папку.
- Запись/обновление `workspace.expected/` при record/refresh не выполняется (это независимый оракул).

### 3.4. Что сравнивается ещё, по наборам

| Набор | Дополнительно к журналу и дереву |
|---|---|
| headless | `stdout === <текст последнего assistant/message родителя (склейка text-блоков)> + "\n"`; `stderr` — **точное** равенство реконструкции из журнала: блоки `dsh: reasoning:\n<текст>\n` для каждой серии reasoning-дельт и, если последний `turn/end.reason.kind === 'error'`, строка `dsh: <code>: <message>\n`; код возврата 0/1 (п. 1.2). Никаких других записей в stderr быть не должно. |
| sdk | см. раздел 5 |
| acp | `stdout` целиком, кадр за кадром (раздел 5) |

### 3.5. Форматы и платформы

**`sessionFormat.version` и покрытие.** Без блока `sessionFormat` сценарий пишется текущим писателем; в воспроизведении
допустим либо «базовый» V3, либо текущий V4 для каждой роли независимо (`scripts/session-snapshot-corpus-policy.ts`:
`RETAINED_BASELINE_VERSION = 3`). Блок `sessionFormat: {version, coverage[]}` фиксирует «удержанный» сценарий: **все** роли
обязаны иметь именно эту версию; воспроизведение идёт с этого файла (проходит миграцию до V4 при разборе); ожидаемое —
**не** он, а `writer[.k].expected.jsonl` (нормализованный выход текущего писателя, заголовок которого ≤ V4 и тоже
мигрируется); проверка «исторический вход не изменён». Допустимые `coverage` (`manifest.ts`, `SNAPSHOT_SESSION_FORMAT_COVERAGE`):
`multi-hop`, `packed-row`, `retry-failure`, `shipped-profile` (всё это — v0), `adjacent-migration` (v1, v2…), `retired-tools`
(любая версия, в том числе текущая V4; закрепляет **снятые** инструменты независимо от миграции: в `office-render-tools` вызов снятого инструмента даёт
ровно `Error: unknown tool "<имя>"` с `error: {name: "ToolNotFoundError", code: "UNKNOWN_TOOL"}`, см.
`snapshots/session/office-render-tools/writer.expected.jsonl`; `office-render-tools-ptc` не разбирался). В трёх наборах удержаны 8 сценариев:
`session/empty-response-retry`, `session/error-finish`, `session/packed-chunks`, `session/subagent-multi` (v0);
`session/text-turn` (v1); `sdk/multi-turn` (v2); `session/office-render-tools`, `session/office-render-tools-ptc` (v4, `retired-tools`).
Корпус целиком (с `web`) удерживает 13 ролей в 10 сценариях и требует покрытия v0–v3 (`assertSnapshotCorpusPolicy`).
Отдельно `tool-call-turn` в headless запускается **дважды**: из обычного v4 и из удержанного `session.v3.jsonl` с проверкой
`verifyToolResultWriterParity` (сообщения `tool/result` собственного писателя совпадают с миграцией V3 без поля `id`).
Для упакованных строк v0 (`text-chunks` и т.д. на уровне журнала) нужна миграция формата v0→v4 (`packed-chunks` проверяет,
что упакованный и распакованный журналы нормализуются одинаково).

**`platform`.** `posix` (33 сценария: 32 headless + `acp/cancel-tool-calls`): на Windows пропуск. `pwsh` (2:
`persistent-pwsh-tool-turn`, `pwsh-tool-turn`): пропуск, если нет работающего `pwsh` (проба
`pwsh -NoLogo -NoProfile -NonInteractive -Command $true`). Без значения — переносимый. Дополнительные пропуски:
`workspace.setup: readonly-patch-directory` при uid 0 (каталог `0555` не запрещает запись root-у). На Linux правила для
`win32` (разделители, `/t`, `stdout.expected.windows.jsonl`, `junction`) не применяются.

**`recording`.** `live` — перезаписывается живой моделью командой record; `authored` — рукописный, не перезаписывается.
На воспроизведение влияния нет.

---

## 4. Поля `snapshot.yml`

Схема закрыта: неизвестное поле — ошибка (`manifest.ts`, `parseSnapshotManifest`). Значения «в корпусе» — по трём наборам
(session 192, sdk 41, acp 9).

| Поле | Смысл | Значения в корпусе |
|---|---|---|
| `version` | версия манифеста | всегда `1` |
| `scenario` | имя каталога (kebab-case) | у всех равно имени каталога |
| `profile` | профиль процесса | `headless` (192), `sdk` (40), `sdk-minimal` (1), `acp` (9) |
| `composition` | id композиции; единственный каталог с `cordis.yml` владеет её патчами | session: 102 различных (`default` 57, `ptc` 12, `image` 5, `team-targets` 4, …); sdk: 24 (`default` 8, `sdk-desktop` 8, `sdk-default` 3, `ptc` 2, …); acp: `acp-default` 8, `image-compaction` 1 |
| `recording` | `live` / `authored` | session 50 / 142; sdk 8 / 33; acp 3 / 6 |
| `header.class` | класс заголовка; все члены класса побайтно (после нормализации) делят заголовок запроса | session 115 различных; sdk 29; acp 2 |
| `header.pin` | сценарий владеет токенизированной последовательностью заголовка класса | `true` (session 119, sdk 23, acp 2) |
| `header.systemPromptSource`, `header.toolSchemasSource` | чей сателлит использовать | имена сценариев (`text-turn`, `session/text-turn`, …) |
| `header.childSystemPrompts`, `header.childToolSchemas` | индексы детей со своими сателлитами | `[1]` (session 4+4, sdk 7+7), `[1, 2]` (session 2+1) |
| `header.changes` | число `request/header` с `reason: "change"` после первого | 1, 2, 4 |
| `header.promptChanges` | число дополнительных `system/message` после первого (замена узла 0 или добавление «в истории») | 1, 2, 4 |
| `replay.override` | рядом лежит `replay.override.json` | `true` (35 / 6 / 2) |
| `platform` | требование к хосту | `posix` (33), `pwsh` (2), иначе нет |
| `permission` | предустановка прав (`DSH_PERMISSION_MODE`) | `workspace-write`, `danger-full-access`, `read-only` (session 14/9/7; acp 3/2/0) |
| `environment` | строковые переменные окружения процесса (имена `[A-Z][A-Z0-9_]*`) | ключи см. п. 1.2 |
| `workspace.setup` | именованная подготовка папки | 10 имён (п. 1.2) |
| `workspace.final` | дерево `workspace.expected/` — полный итог | `true` (session 36, sdk 2, acp 3) |
| `workspace.parent` | `outside-temp`: cwd вне автоматических прав записи временных папок | session 4 |
| `input.task` | задача, если в журнале нет принятого ввода | в трёх наборах не используется |
| `input.attachments` | `[{id: "sha256:…", mediaType, data(base64)}]` — байты картинок, которых нет в журнале | acp 1, sdk 2 |
| `session.source` | заимствование чужого журнала | только `web`; в трёх наборах нет |
| `sessionFormat.version`, `sessionFormat.coverage` | удержанное поколение и закрытый список покрытий | session: v0 ×4, v1 ×1, v4 ×2; sdk: v2 ×1 |

Файлы каталога сценария (все варианты): `snapshot.yml`; `session[.k][.vN].jsonl`; `system-prompt.expected.md`,
`tool-schemas.expected.json` (+ `.k.`); `cordis.yml` / `cordis.snapshot.yml` / `model.cordis.yml` / `runtime.cordis.yml` /
`capture.cordis.yml` / `child.cordis.yml` / `replay.cordis.yml` / `profile.patch.yml` / `legacy-context.patch.yml` (последний —
`snapshots/sdk`); `replay.override.json`; `workspace/` (вход), `workspace.expected/` (итог); `writer[.k].expected.jsonl`
(удержанные); ACP: `input.json`, `stdout.expected.jsonl`; SDK: `notifications.expected.jsonl`, `result.expected.json`
(14 сценариев; у удержанного SDK — `notifications.current.expected.jsonl`); локальные `*.mjs`, `*.ts`, `*.d.mts` —
плагины-подставки и серверы.

---

## 5. Различия наборов sdk и acp

### 5.1. SDK (`snapshots/sdk/sdk.snapshot.ts`)

- **Протокол**: JSON-RPC 2.0 по stdio, клиент `@deepseek-ai/dsh-sdk-client` (`DeepSeekHarness`). Методы клиент→сервер
  (`packages/sdk/protocol/README.md`): `initialize`, `session/prompt`, `session/query`, `session/resume`, `session/wait`,
  `session/working-directory/get|set`, `shutdown`; серверные запросы `approval/request`, `user-question/ask`;
  уведомления `session.event`, `session.status`, `session.query.completed`, `subagent.started`, `subagent.finished`.
  `initialize` несёт cwd, маршрут (`provider`, `model`), необязательные `reasoningEffort`, `maxTokens`.
- **Что делает прогонщик**: на каждое действие журнала (п. 1.3) вызывает `session.run(...)`, который ставит ввод в очередь
  (`session/query`), ждёт, пока `messageId` не появится в журнале (`agent/inbox/spliced`), и собирает уведомления до
  `session.status = idle` корня (`packages/sdk/client/src/api.ts`, `run`); затем ждёт `turn/end` с нужным номером.
  Для команд (`/compact`, `/plan`, `/goal`, `/permission`, …) ждёт `session.query.completed` с тем же `commandId`.
  Особые сценарии: `session-resume` (перезапуск процесса, `resumeSession`, второй запуск с моделью `deepseek-v4-pro`),
  `approval` (клиентские ответы `allowed-once`, `rejected`), `plan-review` (ответы на вопросы `keep`/`approve`/`dismiss`),
  `subagent-continuable`, `serial-created`, `team-default-enabled` (шесть запросов к модели) и т.д.
- **Сравнивается**: (1) журналы (п. 3.1) с `orderLogs`; (2) заголовки/сателлиты (п. 3.2), у `subagent-dsh-sdk-dynamic-route`
  заголовок ребёнка дополняется `config` из `SDK_ASSERTIONS`; (3) для 14 сценариев — `notifications.expected.jsonl`:
  нормализация (`normalizeNotifications`) прогоняет вложенные `session.event` через ту же нормализацию журнала, затем
  `normalizeStdout`; сравнение распарсенных записей; (4) `result.expected.json` — `{"sessionId":…,"finalResponse":…}` как
  **строка**; (5) дерево папки; (6) десятки сценарных проверок в теле теста (число событий, порядок уведомлений, тексты).
- **Запросы к модели**: у шести сценариев патч `capture.cordis.yml` подключает плагин `capture-requests.mjs`, который
  пишет запросы к модели в `<DSH_HOME>/<имя>.jsonl`; тест проверяет их: число запросов, имена инструментов,
  что в сообщениях встречаются точные подстроки (`Agent Teams is available in this session.`, `You are in plan mode.`,
  `<goal_complete>`, `You are now acting as a compaction engine`, `Current DSH file policy: read-only.` и др.).
- **Тексты, зашитые в тест** (`SDK_ASSERTIONS`): точный системный промпт `You are the environment-selected minimal
  software engineer.` и точное описание `bash` (`MINIMAL_BASH_DESCRIPTION`) для `persistent-tools`; список инструментов и их
  обязательных аргументов для `minimal-image-compaction`, `ptc-turn`, `persistent-tools`; ожидаемые итоговые ответы
  (`TEAM_CONFIG_OK`, `SDK_COMPACT_OK`, …).

### 5.2. ACP (`snapshots/acp/acp.snapshot.ts`, `suite.ts`, `harness.ts`, `launcher.ts`)

- **Протокол**: ACP (`@agentclientprotocol/sdk`, `PROTOCOL_VERSION = 1`), NDJSON по stdio; stdout процесса зарезервирован
  под ACP, любая невалидная JSON-строка в stdout — ошибка («проверка чистоты stdout»).
  Клиент: `initialize {protocolVersion, clientCapabilities: {}}`, `session/new {cwd, mcpServers: []}`,
  `session/prompt`, `session/cancel`, ответ на `session/request_permission` по **виду** опции (`allow_once`, `reject_once`, …:
  `optionId` у агента случайны; без очереди ответ `cancelled`; запрошенный, но не предложенный вид — ошибка прогона).
- **Ожидание завершения шагов** опрашивает **живой журнал** на диске каждые 10 мс (таймаут 10 с по умолчанию) поиском
  подстрок `\n{"type":"turn/start",`, `\n{"type":"turn/end",`, `\n{"type":"session/title",`, `\n{"type":"<тип>",` в последней
  полной строке: **строки журнала обязаны быть компактным JSON с `type` первым ключом, без пробелов**, и записываться на диск
  в процессе работы, не только при выходе.
- **Сравнивается**: (1) `stdout.expected.jsonl` — каждый кадр после `normalizeStdout` (разбор, номера `id`, замена токенов,
  `JSON.stringify` с сохранением порядка ключей) сравнивается с файлом **как текст** через `toMatchFileSnapshot`;
  (2) журналы (п. 3.1) для сценариев с моделью; (3) заголовки/сателлиты; (4) дерево папки. Кадры содержат константы
  протокола: `agentInfo {name: "deepseek-harness-acp", version: "0.0.1"}`, `agentCapabilities`, `authMethods: []`,
  `configOptions` (выбор модели из каталога провайдеров), тексты обновлений `session/update`, `stopReason`.
- Обновления сессии привязаны к `messageId` и т.п.; все id нормализованы в `{{sessionId}}`, `{{messageId}}`, `{{usedTokens}}`.

---

## 6. Что обязан воспроизвести Rust-прогонщик, а что можно делать иначе

### 6.1. Обязано совпасть побайтно с dsh (по содержимому после нормализации)

Фикстуры токенизируют **только** идентификаторы, времена, cwd, пути спилла и (в журнале) промпты/схемы. Всё остальное —
буквальный текст, который наш бинарник обязан породить сам.

1. **Системные промпты** — 81 различных текста (`system-prompt*.expected.md`). Токен `{{system}}` скрывает их в журнале, но
   `verifyHeaders` сравнивает их побайтно с файлами. Пример: `snapshots/session/text-turn/system-prompt.expected.md`
   (3 617 байт; содержит правила по инструментам, цели, подагентам).
2. **Схемы и описания инструментов** — 77 различных наборов (`tool-schemas*.expected.json`): имена, порядок инструментов,
   `description`, JSON-схема `parameters` (порядок `required`, `enum`, значения по умолчанию). Пример: 25 инструментов
   у `text-turn`, описание `workflow` — 1 667 символов.
3. **Заголовок запроса**: `config` (`provider`, `model`, и всё прочее, что в нём есть), `reason`, число и порядок
   `request/header`, `request/context`.
4. **События журнала V4**: типы, порядок, число, значения полей, ссылки на номера событий (п. 2.3), `surfaceOp`,
   `turn`/`step`, `usage` (из записанного потока), `session/title` (заголовок-«запасной» из первых слов задачи формируется
   нашим кодом — `source.kind: "fallback"`), `request/context`, `permission/preset`, `sandbox/mode`, `approval/policy`,
   `job/*`, `goal/change`, `subagent/catalog`, `subagent/descriptor`, `compaction/*`.
5. **Тексты, не токенизируемые и входящие в журнал**: результаты инструментов (например `TERMINAL_OK\n`, формат результата `bash`
   с `[exit code: N]`, `[status: failed, …]`), сообщения об ошибках (`Error: unknown tool "…"`,
   `Error: tool call aborted`, `subagent limit reached (active child limit: 1)`, `MCP fixture rejected Bearer [redacted]` …),
   имена/коды ошибок (`ToolNotFoundError`/`UNKNOWN_TOOL`, `AbortError`/`ABORTED`, `AutoReviewDeniedError`), служебные
   сообщения пользователю (`<system-reminder>…`, `Current runtime context…`, `The working directory changed from … to …`),
   уведомления подагентов, форматы вывода списков и поиска.
6. **Поток записи упакованных дельт** (`AssistantStreamAccumulator`, `packages/llm/llm/src/assistant-stream.ts`):
   правила склейки в записи `text-chunks`/`reasoning-chunks`/`tool-call-chunks` (тот же `type` и `index` подряд,
   одинаковые `id` и наличие/значение `name` у вызова инструмента; пустой `id` или пустое `name` → сырой `chunk`). Времена
   зануляются, но **число записей и длины массивов** сравниваются.
7. **Длины, влияющие на поведение**: путь спилла `…/session-<12hex>/<12hex>-<имя>` и логический корень
   `/tmp/dsh-acp-snap-<9hex>` фиксированной длины (чтобы предпросмотры укладывались в записанные бюджеты); оценка токенов
   (сжатие в `compaction-*`, `manual-compact`, `minimal-image-compaction` зависит от **точной** оценки текста запроса,
   поэтому cwd подгоняется до 192 символов).
8. **headless**: точный stdout/stderr/код возврата (п. 3.4); пустой stderr при успехе.
9. **ACP**: кадры stdout побайтно по значениям (константы протокола — п. 5.2).
10. **SDK**: константы протокола и содержимое уведомлений; `serverInfo.name` «остаётся устойчивым»
    `deepseek-harness-sdk-runtime` (`packages/sdk/protocol/README.md`).
11. **Формат файла журнала**: V4, заголовок `{"type":"session","version":4,"id":…,"createdAt":…,"cwd":…,…}`, затем по одному
    событию на строку с конвертом `seq`/`time` (в живых файлах; разбор требует «все с конвертом или все без»), LF,
    `type` первым ключом (п. 5.2), раскладка `<корень>/<проект>/<id>/session.v4.jsonl`.

### 6.2. Можно делать иначе без ослабления проверки

- Реализовать прогонщик на Rust вместо vitest; не использовать `tsx`, `src`/`lib`-режимы, `vitest`, `pnpm`.
- Сравнивать **распарсенные** значения JSON (порядок ключей объекта не важен) и для ACP-кадров: dsh сравнивает текст, но
  порядок ключей — не смысл; в журналах и уведомлениях dsh сам сравнивает значения. Строки и числа сравнивать точно.
- Иначе называть и размещать временные папки (кроме правил п. 1.2: подгонка длины cwd до 192 символов, фиксированный
  логический корень спилла, «вне tmp» для четырёх сценариев), иное число параллельных прогонов, иные таймауты.
- Реализовать воспроизведение модели как плагин `hoh-llm-replay` на слое чанков (рекомендуется: тот же слой, что у dsh;
  это единственный способ сохранить границы дельт и поля `usage`/`finish`, записанные в журнале). Прогон через
  HTTP-заглушку проверял бы ещё и наш адаптер, но требует обратного перевода чанков в провод и не повторяет побочные
  эффекты `deepseek-official`; как дополнение допустимо, как замена — нет.
- Принимать патчи в собственном формате профиля (при сохранении семантики `id`/`insert`/`disabled`/`config`),
  заранее вычисляя `!!js` (п. 1.6). Пока `id` строк и имена плагинов в наших пакетах другие, прогонщик обязан
  переписывать патчи (раздел 7, вопрос 1).
- Реализовать сценарные проверки из тела `headless.snapshot.ts` / `sdk.snapshot.ts` постепенно; сценарий без реализованной
  проверки не может получить статус «проходит».
- Строгую схему V4 из каталога форматов заменить собственной, **если** она отвергает то же, что и dsh (иначе ослабление).

### 6.3. Чего делать нельзя (ослабит проверку)

Игнорировать `request/header`, `system/message`, `request/context`; нормализовать больше, чем нормализует dsh (маскировать
тексты результатов, числа в `usage`, номера ссылок); сравнивать только «итоговый ответ»; подставлять записанные результаты
инструментов вместо реального выполнения; пропускать сравнение дерева папки; объявлять исторический сценарий «проходящим»,
не выполнив миграцию его входа.

### 6.4. Что токенизируют, а что нет — прямой ответ

- Системные промпты, описания и схемы инструментов: **токенизируются в журнале**, но **сравниваются побайтно** через
  сателлиты. Наш бинарник обязан выдавать те же тексты (или сценарий получает письменную причину в `EXPECTED.tsv`).
- Сообщения об ошибках и тексты результатов инструментов: **не токенизируются**, сравниваются побайтно.
- Идентификаторы, времена, cwd, пути спилла: токенизируются (п. 2).

---

## 7. Открытые вопросы (из кода не решены)

1. **Идентичность конфигурации.** Патчи сценариев адресуют строки по `id` (`llm-deepseek`, `sandbox`, `session-persistence-jsonl`,
   `system-prompt`, `tool-subagent`, …) и именам пакетов `@deepseek-ai/dsh-*`. Сохраняет ли `hoh-loader` те же `id` и имена,
   или прогонщик переписывает патчи по таблице соответствия `MAP.tsv`? От этого зависит, нужны ли наши пакеты-«двойники»
   (`@deepseek-ai/dsh-llm-replay` и др.) под теми же именами.
2. **Вычисляемые значения `!!js`.** Кто их вычисляет — `hoh-loader` (нужен интерпретатор подмножества выражений) или
   прогонщик (предобработка)? Вычисления зависят от окружения процесса (`process.env`, `process.cwd()`) и от сервисов
   (`ctx.headlessStartup`, `ctx.get('profileContext')`).
3. **Плагины-подставки сценариев** (локальные `*.mjs`/`*.ts`): в `EXPECTED.tsv` у 134 из 242 сценариев в `plugins` есть
   записи `local:…` (кроме общих `scripts/snapshot-spill-locators.ts`, `scripts/snapshot-shell-path.ts`); их коду нужна среда
   Cordis/TS. Либо переписывать подставки (HTTP-серверы, фальшивые провайдеры, барьеры порядка) на Rust, либо присваивать
   сценариям письменную причину. Без `local:`-плагинов осталось 93 сценария `session`, 9 `sdk` и ни одного `acp`.
4. **Миграции формата V0–V3 → V4 и строгая схема V4** живут в `packages/session/*` (каталог форматов; не читался). Нужны для
   разбора входов (v0 `packed-chunks`, v1, v2, v3) и для проверки «живой журнал — корректный V4». Точные правила строгой
   проверки неизвестны.
5. **Откуда берётся `inheritedEventCount` ребёнка в V4** — из метки `session/end-seed` (п. 1.4); правила разбора каталога
   форматов не читались.
6. **ACP-прогонщик не задаёт `DSH_TELEMETRY_DISABLED`.** Монтируется ли `otel` и ходит ли в сеть в ACP-прогонах — не выяснено
   (`resolveTelemetryPatch` в `packages/boot/app-boot/src/profile-context.ts` не читался).
7. **Изоляция окружения.** Дочерний процесс наследует весь `process.env` и реальный `HOME`; зависимость сценариев от
   `HOME`, `XDG_*`, установленных `git`, `bash`, `pwsh`, `node` не описана. Для Rust-прогонщика нужно решить, какие
   инструменты хост обязан иметь (`git-worktree` использует `git`, `office-*` — Python/Node по `workspace.setup`).
8. **Параллельные подагенты.** Порядок привязки скриптов детям — по первому вызову модели; для одновременных детей
   детерминированность обеспечивают плагины-упорядочиватели. Как это воспроизвести в Rust — открыто.
9. **Часть сценарных проверок не разобрана построчно**: `provider-cwd` (3 запуска, `--session-id`, `--json`,
   `DSH_SNAPSHOT_PROVIDER_CWD`), `session-reference-spill` (`reference-source`), `session-query-spill`, `mcp-resources*`,
   `plugin-manager-mcp`, `worktree`, `working-directory`, `team-*`, `advanced-toolchain`, `office-progressive-*`,
   `toolcall-unused-args`, `empty-response-retry-current`, `background-confinement-failure`, `subagent-depth-two-rejection`.
   Каждая добавляет проверки внешних эффектов или ход процесса; их надо перечитать перед портированием.
10. **Оценка токенов и порядок параллельных задач** (`job/*`, `session/title` и др.) — места, где побайтная идентичность
    зависит от внутренних алгоритмов dsh, которые здесь не разбирались.
11. **Нужен ли нам SDK/ACP-клиент в прогонщике.** Для 41+9 сценариев прогонщик должен говорить по SDK-JSON-RPC и ACP.
    Сохраним ли мы протоколы (`deepseek-harness-acp`, `deepseek-harness-sdk-runtime`) без изменений — решает план HoH,
    а не код dsh.
12. **Стабильность 30-секундного таймаута** для headless и 10-секундных ожиданий ACP на медленной машине (Spark) — в dsh
    значения фиксированы; допустимы ли у нас другие, не проверено.

## 8. Что не проверено

- Ничего не запускалось и не собиралось; поведение — по коду и данным, не по прогону.
- Не читались: `packages/session/*` (каталог форматов и миграции), `packages/boot/app-boot` (раскладка профиля, телеметрия,
  вычисление `!!js`), `packages/sdk/server`, реализации инструментов, плагинов `compaction-*`, `llm-retry`, `goal-*`,
  локальные подставки `snapshots/**/*.mjs`, `scripts/snapshots/python-sdk-single-exe/`, `scripts/migrate-packed-session-fixtures.ts`,
  `docs/cookbook/adding-a-session-format-version.md`.
- `snapshots/web` и `apps/web/tests/*.snapshot.ts` вне охвата.
- Столбец `plugins` в `conformance/EXPECTED.tsv` получен статическим применением патчей (алгоритм п. 1.2) к YAML бандлов и
  сценариев. Допущения: профиль не `desktop`, `DSH_PRIMARY_RUNTIME` не задан, платформа Linux; из `!!js`-условий
  `disabled` разобраны только 8 встреченных выражений. Плагины, которые другие плагины монтируют во время работы, и
  включения через `plugin-manager` в списке отсутствуют. Локальные плагины записаны как `local:<путь от корня клона>`.
  Для `sdk`-сценариев с композицией без префикса `sdk-` слои собраны по `authoredPatches`; список патчей `SDK_ASSERTIONS`
  извлечён регулярным выражением из текста `sdk.snapshot.ts`.
- Столбец `session_format` — выбранные версии ролей (`v4`, `v3`, …) и, для удержанных, `;retained=<coverage>`.
- Все числа в разделе 4 получены разбором `snapshot.yml` и каталогов этого коммита; число «различных файлов промптов/схем»
  (81/77) посчитано по хешу содержимого файлов `*.expected.md` / `*.expected.json` всего `snapshots/` (включая `web`).
