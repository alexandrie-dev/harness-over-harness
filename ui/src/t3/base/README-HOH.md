# Основа HOH-40: веб-клиент T3 в окне HoH — как подключить

Для u3 (хост данных, `ui/src/hoh-host`) и u4 (вход окна, `ui/src/t3-entry`). Что перенесено и что изменено — `ui/T3-MAP.md`
(раздел «Основа HOH-40»), пакеты — `ui/T3-DEPS.md`. Всё в этой папке, кроме перечисленного ниже как «наше», пишет
`ui/scripts/t3-sync.py`; руками файлы T3 здесь не правятся (правки — `../base-patches/*.patch` через манифест `../manifest.json`).

## Вход и монтирование

- Вход — их загрузчик `web/bootstrap.ts`: `import("./main")`, при ошибке запуска — их страница ошибки (`web/lib/bootError.ts`).
- `web/main.tsx`: `startAppServices()` (`web/appInit.tsx`, маршрутизатор: `createBrowserHistory()` вне Electron) →
  `ReactDOM.createRoot(document.getElementById("root"))` → `<AppRoot router={router} />` (`web/AppRoot.tsx`). Экспортирует `startup`.
- Странице нужен только `<div id="root">` и модуль `web/bootstrap.ts` (образец — `index.html` здесь). Стили T3 (`web/index.css`,
  шрифт Inter) подключает сам `main.tsx`.
- Хост должен встать ДО загрузки `web/bootstrap.ts` (образец — `qa/empty-host.ts`: `installHohHost(...)`, затем `import(bootstrap)`).

## Где T3 строит протокол (шов для хоста)

Шов, выбранный u3, — глобальные `WebSocket` и `fetch`, правки T3 не нужны (подробно — `ui/src/hoh-host/README.md`):

| Что | Файл T3 (здесь) | Имя |
|---|---|---|
| сокет RPC | `web/lib/runtime.ts:69` | `Socket.layerWebSocketConstructorGlobal` (`new globalThis.WebSocket(url)`) |
| HTTP до сокета | `web/lib/runtime.ts:24` | `layerRemoteHttpClient((input, init) => globalThis.fetch(input, init))` |
| клиент протокола | `client-runtime/rpc/protocol.ts:5` | `makeWsRpcProtocolClient = RpcClient.make(WsRpcGroup)` (группа — `contracts/rpc.ts`) |
| сессия RPC | `client-runtime/rpc/session.ts:78` | сервис `RpcSessionFactory`; слой `RpcSession.layer(options)` |
| сборка соединения | `client-runtime/connection/layer.ts:76` | `Connection.layerWithOptions` сам подставляет `RpcSession.layer(options)` — снаружи не подменить без правки |
| адрес среды | `web/environments/primary/target.ts:187` | `import.meta.env.VITE_HTTP_URL`, иначе `window.location.origin` |
| сессия входа | `web/environments/primary/auth.ts` | `fetchSessionState()` → `GET /api/auth/session` (корневой маршрут ждёт его) |

## Сборка (что нужно vite)

- Псевдонимы — `aliases.json` (пути от `ui/`; `~` → `src/t3/base/web`, остальные — точные спецификаторы, включая `?url`).
- `react({ compiler: true })` (как у T3: без React Compiler ChatView зацикливает отрисовку), `@tailwindcss/vite`.
- `define`: переменные сборки T3 пустые (`VITE_CLERK_PUBLISHABLE_KEY`, `VITE_RELAY_OTLP_TRACES_URL`, `T3CODE_FEATURE_SECRETS` и др.
  — части выключены), `VITE_HTTP_URL` — адрес хоста; `import.meta.env.APP_VERSION`.
- `worker: { format: 'es' }`, `assetsInclude: ['**/*.wasm']`.
- Образец целиком — `vite.config.ts` здесь (наш): `node_modules/.bin/vite build -c src/t3/base/vite.config.ts` из `ui/`
  (папка — `T3_BASE_OUT`; `T3_BASE_QA=1` добавляет `qa.html`). Снимки: `node scripts/t3-snapshot.mjs <сборка> <папка> qa.html`.

## Что уже выключено в основе (не дублировать или не мешает)

Правками файлов T3 (`../base-patches`): маршруты скрытых частей не регистрируются (`web/routeTree.gen.ts`), разделы настроек — только
общие, вид, чат, провайдеры, доступ (`web/components/settings/settingsSearch.ts`, `HOH_SETTINGS_PATHS`), терминал не открывается
(`web/terminalUiStateStore.ts`, кнопки нет — `web/components/chat/PanelLayoutControls.tsx`), импорта тем нет
(`web/components/settings/ThemeSettings.tsx`), имя продукта HoH (`web/branding.ts`, `web/components/sidebar/SidebarChrome.tsx`),
языки — английский и русский (`gt.config.json`). Подмена скрытых маршрутов при сборке (u4, `features.ts`) с этим совместима.
Остальное прячет хост, не объявляя возможностей (`capabilities`) и областей (`AuthTerminalOperateScope` и т.п.).

## Наше в этой папке

- `shims/gt-react.tsx`, `shims/gt-i18n.ts`, `shims/gt-i18n/types.ts` — i18n вместо General Translation, без сети: английские строки T3
  как есть с подстановкой ICU (`{n, plural, ...}`, `select`, `#`); `useGT()`/`useMessages()` возвращают одну и ту же функцию, пока
  не сменился язык; перевод — `registerDictionary(locale, {английский: текст})`, словарей пока нет.
- `shims/jszip.ts` — типы JSZip для офисной части (скрытой).
- `stubs/` — создаёт `t3-sync.py`: значения пакетов скрытых частей (`stubs/absent.ts`: вызов даёт объект, любое свойство — функция,
  возвращающая `undefined`; образец — пробная сборка u4).
- `types/` — `vite-plus/client` (их типы сборки → `vite/client`), `trusted-types` (пока у окна нет `@types/trusted-types`).
- `web/components/T3Wordmark.tsx`, `web/components/T3AppIcon.tsx` — эмблема HoH вместо знака T3.
- `*.hoh.d.ts` / `*.hoh.ts` рядом с файлами скрытых частей на заглушках — объявления для tsc (vite их не берёт).
- `vite.config.ts`, `index.html`, `qa.html`, `qa/empty-host.ts`, этот файл.

## Обновление

Из `ui/`: `python3 scripts/t3-sync.py` (перенос, правки, заглушки, tsconfig, aliases), затем `python3 scripts/t3-deps.py`,
`python3 scripts/t3-map.py`, `python3 -I scripts/t3-check.py`; tsc — `node_modules/.bin/tsc --noEmit -p src/t3/base/tsconfig.json`.
