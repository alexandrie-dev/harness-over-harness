# Хост HoH для веб-клиента T3 (HOH-40)

Клиент T3 (`apps/web` + `packages/client-runtime`, без правок) разговаривает со своим сервером только через `effect/rpc` (`RpcClient.make(WsRpcGroup)`) по WebSocket и несколько запросов HTTP до открытия сокета. Здесь этот сервер заменён хостом в самой странице: те же кадры effect/rpc в JSON, но вместо сети — память, вместо сервера T3 — движок HoH через мост Tauri (те же методы, что у нынешнего окна: `session/*`, `hoh/executor/*`, `approval_decide`, `question_answer`).

## Как подставить (без правки файлов T3)

Шов — глобальные `WebSocket` и `fetch`. T3 строит сокет слоем `Socket.layerWebSocketConstructorGlobal` (`apps/web/src/lib/runtime.ts:69`, т.е. `new globalThis.WebSocket(url)`), а HTTP — через `layerRemoteHttpClient((input, init) => globalThis.fetch(input, init))` (там же, стр. 25). Поэтому точка входа окна (u4, `ui/src/t3-entry/**`):

```ts
import { installHohHost } from '../hoh-host/index.ts';
import { tauriEngine } from '../hoh-host/tauri.ts';

installHohHost({ engine: tauriEngine() });   // ДО загрузки T3
await import('../t3/web/main.tsx');           // их main.tsx как есть
```

Сборка T3 — с `VITE_HTTP_URL=http://hoh.local` (адрес, который никто не слушает: его отвечает страница). Без него T3 берёт адрес из `window.location.origin`, а для `tauri://` бросает `PrimaryEnvironmentProtocolUnsupportedError` (`apps/web/src/environments/primary/target.ts:215-239`). `window.desktopBridge` не задавать: режим браузера (`isElectron=false`), без токенов и тикетов. `VITE_CLERK_PUBLISHABLE_KEY` и `VITE_HOSTED_APP_CHANNEL` не задавать.

Что отвечает страница на `http://hoh.local`:
- `GET /api/auth/session` → `authenticated: true` (корневой маршрут ждёт его, `routes/__root.tsx:93`);
- `GET /.well-known/t3/environment` → описание среды `hoh-local`, протокол оркестрации 2;
- `/api/observability/*` → 200 и в никуда; всё остальное → 503 (у T3 для этого запасной путь через сокет: снимки списка и разговора).
- `ws://hoh.local/ws?...` → `HohWebSocket` в памяти. Любой другой адрес сокета — отказ (сети у окна нет). Другие адреса `fetch` (файлы сборки) идут в прежний `fetch`.

Если когда-нибудь понадобится шов глубже — сервис `RpcSessionFactory` (`packages/client-runtime/src/rpc/session.ts:78`). Снаружи его не подменить: `Connection.layerWithOptions` сам подставляет `RpcSession.layer(options)` (`packages/client-runtime/src/connection/layer.ts:76`), так что это была бы правка T3 в этой строке. Нынешний шов правки не требует.

## Файлы

| Файл | Что делает |
|---|---|
| `index.ts` | `installHohHost({ engine })`: хост + сокет + fetch, возвращает `uninstall` |
| `wire.ts` | серверная сторона кадров effect/rpc: Request/Ack/Interrupt/Ping/Eof → Chunk/Exit/Pong; потоки, прерывание, «не поддерживается» |
| `socket.ts` | `HohWebSocket` (WebSocket в памяти), `installHohWebSocket` |
| `http.ts` | ответы HTTP до сокета, `installHohFetch` |
| `host.ts` | `HohHost`: методы WsRpcGroup поверх движка |
| `threads.ts` | разговор HoH → проекция и строка списка T3 |
| `config.ts` | конфиг сервера, провайдер Claude, настройки |
| `errors.ts` | отказ моста по коду (`errors.rs`) → английский текст T3; ошибка провайдера в форме T3 |
| `engine.ts` / `tauri.ts` | интерфейс движка / мост Tauri |
| `methods.ts` | список методов WsRpcGroup (потоковые; без `EnvironmentAuthorizationError`), снят с `packages/contracts/src/rpc.ts` @ 803f21f6 |

## Методы

Реализованы (22): `projects.mutate`, `filesystem.browse`, `server.probe`, `server.reportClientActivity`, `server.getConfig`, `subscribeServerConfig`, `server.getSettings`, `server.updateSettings`, `server.refreshProviders`, `subscribeServerLifecycle`, `orchestration.subscribeShell`, `orchestration.subscribeThread`, `orchestration.getThreadProjection`, `orchestration.launchThread`, `orchestration.dispatchCommand`, `orchestration.forceStopThread`, `orchestration.getThreadTitles`, `projects.ensureScratch`, `agentSessions.scan`, `subscribeVcsStatus`, `vcs.refreshStatus`, `vcs.listRefs`; плюс Ping→Pong.

- `subscribeServerLifecycle` → одно событие `welcome` без `bootstrap*`: без него T3 не показывает ничего (`apps/web/src/components/onboarding/FirstRunGate.tsx`).
- Первый запуск — мастер T3 как есть. T3 решает сам (`apps/web/src/onboarding/firstRun.logic.ts`): нет проектов и разговоров — «свежее» место, открывается его страница Welcome (если в его настройках клиента ещё нет `onboardingCompletedAt`). Поэтому у движка без разговоров список пуст; единственный проект — «No project» T3 (его scratch-проект: `scratchWorkspaceRoot` = папка разговоров, название и значок как у `ManagedProjectFolders.ensureScratchProject`) появляется с первым разговором или по `projects.ensureScratch` («Start without a project»). На странице Welcome: Claude — единственный агент, найден/не найден/не вошёл — из `hoh/executor/discover` (не ответил — «не найден» с пояснением, а не вечное «Checking…»); `agentSessions.scan` — пусто (раздел Projects T3 не показывает).
- Папка как проект T3 («Add project» → «Local folder»). T3 в режиме браузера выбирает папку своим `filesystem.browse` (палитра: набранный путь и список подпапок; `apps/web/src/components/CommandPalette.tsx` `startAddProjectBrowse`), системный диалог у них — только кнопка при `desktopBridge`. Хост отвечает на `filesystem.browse` через мост: `browseFolders` → команда Tauri `browse_folders` (имена папок, только главному окну; порт `apps/server/src/workspace/WorkspaceEntries.ts` `browse`). Без этой команды — типизированная `FilesystemBrowseError` «This HoH window cannot list folders. Type the full path of the folder.»: проект добавляется по набранному полному пути. `projects.mutate`: `project.create` (папка должна читаться, если мост умеет её смотреть; та же папка дважды и папка «No project» — `ProjectMutationError`), `project.update` (название, закрепление, архив; папка проекта не меняется), `project.delete` (только без разговоров: HoH разговоры не удаляет). Разговор в проекте: перед первым сообщением хост зовёт `session/working-directory/get`, затем `session/working-directory/set { sessionId, path }` — тот же метод, что выбор папки в прежнем окне (окно защиты хоста Tauri может спросить человека); Claude запускается в этой папке. Набранный `~/…` после первого разговора становится папкой, которую назвал движок. Проекты, место каждого разговора и папка «No project» хранятся в localStorage окна (`hoh-host.projects.v1`) и переживают перезапуск; разговор, в истории которого записана папка проекта (`working-directory/change`), при открытии попадает в этот проект.
- Ошибки — по-английски в форме T3: отказ моста — по его коду (`rejected` → «You declined this in the HoH confirmation window.», `folder-fixed`, `connecting`, …), русские слова хоста не показываются; ход с ошибкой — элемент `error` «Provider error» (`failure.class: provider_error`, текст движка, если он английский, иначе «Provider turn failed.», как `makeProviderFailureTurnItem` T3); неотправленное сообщение — `error` «Message not sent» (`transport_error`).
- `subscribeVcsStatus` / `vcs.*` → «папка не git-репозиторий»: иначе T3 считает её репозиторием (`ChatView.tsx:1777`) и может потребовать базовую ветку перед отправкой.
- Известные пробелы среза: отправка с картинками зовёт `assets.persistChatAttachments` (нет — отправка с вложением падает); `@`-упоминания файлов (`projects.searchEntries`) — пустой ответ с ошибкой в меню.

`dispatchCommand`: `thread.create`, `message.dispatch` (→ `session/prompt`), `thread.stop` / `run.interrupt` (→ `session/cancel`, тот же «Стоп»), `runtime-request.respond` (разрешение → `approval_decide`, вопрос → `question_answer`), `thread.user-input.dismiss`, `thread.runtime-mode.set` (→ `hoh/executor/access/set`, без окна защиты — решение основателя 19:16). Режим — выбор человека в поле ввода T3, и движок получает его до хода, для которого он выбран: в начатом разговоре T3 шлёт `thread.runtime-mode.set` перед сообщением (`ChatView.tsx` `persistThreadSettingsForNextTurn`), у нового — `runtimeMode` запуска (выбор в поле ввода, иначе настройка по умолчанию от хоста; `useComposerSend.ts`); хост ставит его раньше `session/prompt`. Константа T3 `DEFAULT_RUNTIME_MODE = "full-access"` в запрос запуска не попадает: черновой разговор берёт режим из поля ввода или настройки (`ChatView.tsx:995-998`). Режим движка один на все разговоры, каждый разговор показывает действующий. Настройка `defaultRuntimeMode` — с чего начинается новый разговор («подтверждать», пока человек не сменит её в настройках; хранится в localStorage), а не действующий режим: T3 показывает чип режима под полем ввода начатого разговора, пока его режим отличается от этой настройки (`composerControlsOwnership.ts`); в режиме по умолчанию переключатель — в нижней строке меню модели, как у T3, `thread.model-selection.set` (→ `hoh/executor/select`), `thread.metadata.update` (название), `thread.archive`/`unarchive`; отметки чтения/закрепления — приняты без действия. Остальные команды — `OrchestrationV2DispatchCommandError` «Not available in HoH.».

Остальные методы: унарные — типизированная `EnvironmentAuthorizationError` «Not available in HoH.» (если её нет в объединении ошибок метода — дефект с тем же текстом); потоковые — открыты и молчат (без данных, ошибок и повторов). Интерфейс лишнего T3 прячет сам: сервер не объявляет ни одной необязательной возможности (`capabilities` — только `connectionProbe`, `threadForceStop`, `inlineMessageContext`), провайдер один — Claude (`claudeAgent`), режимов два: `approval-required` (HoH «подтверждать», в T3 «Supervised») и `full-access` («Full access»); промежуточные режимы T3 считаются «подтверждать». Проект — scratch, поэтому T3 не предлагает git, ветки и worktree (`ThreadDetailsPanel.tsx`, `offersGit`). Чего нет в `capabilities`, ограничено управляемой политикой сервера (`managedPolicy`, `packages/contracts/src/managedPolicy.ts`): `providers.allowedDrivers: [claudeAgent]` (ни других агентов, ни ACP-реестра на Welcome и в добавлении провайдера), `connect: false` (телефон, гостевые ссылки), `telemetry: false` (вместо «T3 Code collects anonymous usage data…» T3 пишет «HoH turned off anonymous usage data.»; в настройках About — строка «managed by HoH»).

Без флага T3 показывает (хостом не спрятать; прячет вход, `ui/src/t3-entry/features.ts`): «Git URL» в «Add project» — `CommandPalette.tsx:1116-1119` (источник `url` есть всегда; «GitHub repository» выключен настройкой `offeredSourceControlProviders: { github: false }`); «Initialize Git» у проекта-папки (не «No project») — `ThreadDetailsPanel.tsx:110-114` + `GitActionsControl`; «Open in …» — `apps/web/src/components/chat/OpenInPicker.logic.ts:12-17` (всегда для своей среды); «Add project script» — `apps/web/src/components/chat/ThreadDetailsPanel.tsx:173` (у любого проекта);  строка «Add more agents» на Welcome — `WelcomePage.tsx:690` (остаётся и пустой); кнопка голоса — `apps/web/src/speech/useEnvironmentSpeechInput.ts:137,516-517` (пока голос не настроен, предлагается без `voiceTranscription`); Pull requests — `packages/client-runtime/src/state/presentation.ts:331-336` (есть у любой среды с конфигом); Usage — `apps/web/src/components/sidebar/SidebarChrome.tsx:303-307`.

## Разговор

Один разговор HoH = один thread T3 (id совпадает с `sessionId`). Одно сообщение человека = один run. Каждое изменение отправляется подписчику целым снимком разговора (`kind: "snapshot"`): клиент T3 заменяет им проекцию (`client-runtime/src/state/threads.ts`, `applyItemLocked`), поэтому события оркестрации T3 не воспроизводятся. Поток ответа — `session.assistant-stream` → `assistant_message` со `streaming: true`; шаги — `tool/call`/`tool/result` → `command_execution` / `file_change` / `file_search` / `web_search` / `dynamic_tool`; запрос разрешения — `approval.pending` → `runtimeRequests` (`live`) + `approval_request` в виде запроса Claude у T3 (`ClaudeAdapterV2.ts` `canUseTool`): вид по инструменту (`command`, `file-change`, `file-read`), текст — сама команда, иначе «Write: <файл>»; вызов называет событие `approval/asked { id, callId }`, до него — последний идущий вызов; русский `humanText` движка не показывается; вопрос — `engine-question` → `user_input_request`. История после перезапуска — `session/resume` + `session/events` страницами; ход без конца в истории закрывается как остановленный, пока движок не скажет `session.status: running`. Движок начинает ход раньше, чем записывает его сообщение (`turn/start`, `step/start`, `user/message`): `turn/start` без своего сообщения продолжает прошлый run, а пришедшее сообщение возвращает тому его прежнее состояние и открывает свой run; открытым бывает только последний run (T3 считает разговор работающим, пока открыт любой).

Модели Claude — как у T3: имена из каталога T3 (`apps/server/src/provider/model-manifest.json`, `claudeAgent`, по id или псевдониму движка: `opus` → «Claude Opus 5.5»), в порядке каталога, затем неизвестные каталогу под именем Claude; `default` (указатель Claude Code на свою модель) не показывается; модель нового разговора — модель T3 по умолчанию (`claude-opus-5-5`), если движок её предлагает. Разговор, уже привязанный движком к `default`, показывает `default`. Сообщения о Claude (не найден, не вошёл) — слова T3 (`ClaudeProvider.ts`), не движка.

## Нужно от хоста Tauri (apps/hoh-desktop, u4)

Список папок для «Add project»: команда `browse_folders { partialPath, cwd? } → { parentPath, entries: [{ name, fullPath }] }`, только главное окно; код передан u4 (HOH-40 fix2). Решение ведущего: окно видит имена папок (не файлов) — раньше окно файловую систему не видело вовсе (`ui/src/screens/firstrun/engine.ts`).

Ответ на запрос разрешения идёт из главного окна командой `approval_decide { approvalId, outcome: 'allowed-once' | 'rejected' }`. Сейчас её разрешено звать только окнам `approval-*` (`capabilities/approval.json`, проверка метки в `approvals.decide`). Для ответа в разговоре главному окну нужны `allow-approval-decide` и приём метки `main` в `approvals.decide`.

## Проверка

```sh
cd ui
TMPDIR=/dev/shm/hoh node --test tests/hoh-host/run.mjs
./node_modules/.bin/tsc --noEmit -p tsconfig.json
```

- `projects.test.mjs` — «Add project»: список папок через мост и без него, проект из папки, разговор в нём (`working-directory/set` до `session/prompt`), отказ человека — английский текст, `~/…` → папка движка, перезапуск (новый движок над теми же разговорами, та же память окна), ошибки хода и отправки — элементы `error` T3.
- `host.test.mjs` — хост по его же сокету на поддельном движке (`fake-engine.mjs`): рукопожатие, «не поддерживается», HTTP, список, новый разговор с потоком и шагом, следующее сообщение и «Стоп», разрешение, вопрос, режим, перезапуск, потеря движка, сокет, настройки и папка.
- `effect-client.test.mjs` — первый запуск решает настоящая логика T3 (`firstRun.logic.ts`, `welcomePage.logic.ts`, собираются из исходника вместе с контрактами): движок без разговоров — мастер, Claude готов, «Start without a project» → «No project»; с разговором — сразу приложение; Claude не найден / не проверен — сказано честно. И тот же хост против НАСТОЯЩЕГО клиента T3: `RpcClient.make(WsRpcGroup)` + `makeProtocolSocket` + `RpcSerialization.layerJson` из `effect` 4.1.3 (как в `client-runtime/src/rpc/session.ts`), контракты T3 собираются из исходника (`contracts.mjs`, путь можно задать `HOH_T3_CONTRACTS`). Каждый ответ декодируется схемами T3: конфиг и его поток, настройки, список, проекции, поток ответа, шаги пяти видов, разрешение, вопрос, «Стоп», заголовки, принудительная остановка, режим, модель, типизированные ошибки.
