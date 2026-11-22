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
| `engine.ts` / `tauri.ts` | интерфейс движка / мост Tauri |
| `methods.ts` | список методов WsRpcGroup (потоковые; без `EnvironmentAuthorizationError`), снят с `packages/contracts/src/rpc.ts` @ 803f21f6 |

## Методы

Реализованы (16): `server.probe`, `server.getConfig`, `subscribeServerConfig`, `server.getSettings`, `server.updateSettings`, `server.refreshProviders`, `orchestration.subscribeShell`, `orchestration.subscribeThread`, `orchestration.getThreadProjection`, `orchestration.launchThread`, `orchestration.dispatchCommand`, `orchestration.forceStopThread`, `orchestration.getThreadTitles` + Ping.

`dispatchCommand`: `thread.create`, `message.dispatch` (→ `session/prompt`), `thread.stop` / `run.interrupt` (→ `session/cancel`, тот же «Стоп»), `runtime-request.respond` (разрешение → `approval_decide`, вопрос → `question_answer`), `thread.user-input.dismiss`, `thread.runtime-mode.set` (→ `hoh/executor/access/set`, сразу, без окна защиты — решение основателя 19:16), `thread.model-selection.set` (→ `hoh/executor/select`), `thread.metadata.update` (название), `thread.archive`/`unarchive`; отметки чтения/закрепления — приняты без действия. Остальные команды — `OrchestrationV2DispatchCommandError` «Not available in HoH.».

Остальные методы: унарные — типизированная `EnvironmentAuthorizationError` «Not available in HoH.» (если её нет в объединении ошибок метода — дефект с тем же текстом); потоковые — открыты и молчат (без данных, ошибок и повторов). Интерфейс лишнего T3 прячет сам: сервер не объявляет ни одной необязательной возможности (`capabilities` — только `connectionProbe`, `threadForceStop`, `inlineMessageContext`), провайдер один — Claude (`claudeAgent`), режимов два: `approval-required` (HoH «подтверждать») и `full-access` (HoH «полный»); промежуточные режимы T3 считаются «подтверждать».

## Разговор

Один разговор HoH = один thread T3 (id совпадает с `sessionId`). Одно сообщение человека = один run. Каждое изменение отправляется подписчику целым снимком разговора (`kind: "snapshot"`): клиент T3 заменяет им проекцию (`client-runtime/src/state/threads.ts`, `applyItemLocked`), поэтому события оркестрации T3 не воспроизводятся. Поток ответа — `session.assistant-stream` → `assistant_message` со `streaming: true`; шаги — `tool/call`/`tool/result` → `command_execution` / `file_change` / `file_search` / `web_search` / `dynamic_tool`; запрос разрешения — `approval.pending` → `runtimeRequests` (`permission`, `live`) + `approval_request`; вопрос — `engine-question` → `user_input_request`. История после перезапуска — `session/resume` + `session/events` страницами; ход без конца в истории закрывается как остановленный, пока движок не скажет `session.status: running`.

## Нужно от хоста Tauri (apps/hoh-desktop, u4)

Ответ на запрос разрешения идёт из главного окна командой `approval_decide { approvalId, outcome: 'allowed-once' | 'rejected' }`. Сейчас её разрешено звать только окнам `approval-*` (`capabilities/approval.json`, проверка метки в `approvals.decide`). Для ответа в разговоре главному окну нужны `allow-approval-decide` и приём метки `main` в `approvals.decide`.

## Проверка

```sh
cd ui
TMPDIR=/dev/shm/hoh node --test tests/hoh-host/host.test.mjs
./node_modules/.bin/tsc -p <tsconfig только для src/hoh-host>
```

Тесты собирают `src/hoh-host/*.ts` в /dev/shm и гоняют хост по его же сокету на поддельном движке (`tests/hoh-host/fake-engine.mjs`): рукопожатие, «не поддерживается», HTTP, список, новый разговор с потоком и шагом, следующее сообщение и «Стоп», разрешение, вопрос, режим, перезапуск, потеря движка, сокет.

Ограничение: значения собраны вручную в закодированной форме схем `packages/contracts`. Пакета `effect` в окне пока нет, поэтому декодирование схемами T3 тестами не проверено. Первая проверка — живой запуск: если схема не сходится, клиент T3 пишет `SchemaError` в консоль с путём к полю.
