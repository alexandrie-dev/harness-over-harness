# Пакет 4 «окно-чат»: native bridge

## Границы

Задание: `plans/briefs/window-pkg4-bridge.md` и `plans/briefs/interface-first.md`.
Изменяется только `apps/hoh-desktop`; `ui/src/**`, `docs/window.md`, `hoh-protocol`,
`hoh-window-stub` и другие crates не менялись. Это отдельный Cargo workspace со своим
`Cargo.lock`. Проверки идут `--locked --offline`, `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-cf1/desktop` (tmpfs symlink).
Приостановленные регулятором T/Tl-процессы вручную не возобновлялись.

## Дочерний движок и bridge

- `engine.rs` задаёт заменяемые `Connector`/`Transport`; первый транспорт — отдельный
  stdio-процесс с JSON-RPC framing. Native initialize требует `protocolVersion: 1` и
  `approvals: host`; legacy handshake не открывает bridge.
- IDs связаны с generation. После падения незавершённые вызовы не повторяются (эффект мог
  уже произойти), stale frames игнорируются, окно получает понятный status, процесс
  перезапускается. На exit отправляется shutdown, затем process group ограниченно ждётся,
  завершается и reap-ится.
- `bridge.rs` содержит явный allowlist UI-методов и валидацию `hoh-protocol`.
  `initialize`, `shutdown`, `approval/*`, reverse user questions и неизвестные методы
  недоступны JS. Метка caller проверяется в Rust.
- `request_with_approval` — единственный privileged путь Tauri. Его callback должен
  закончить native preflight до передачи исходного RPC транспорту; отказ оставляет frame
  count неизменным. Непривилегированные вызовы идут напрямую.
- Уведомления проходят typed deserialize/serialize; stderr движка, engine error data и
  аргументы запросов не журналируются. `pick_folder` использует системный GTK chooser.

## Host approval и ключ модели

- При запуске ядро читает 256 бит из `/dev/urandom`. Ключ остаётся в Rust и уходит движку
  только в initialize. JS не получает key/MAC/action/actionHash.
- Literal no-send реализован двумя fail-closed стадиями: сначала native preflight, затем
  отправка RPC и отдельный engine-authored `approval/request`. Это необходимо, потому что
  текущий protocol не имеет preview endpoint, а stub создаёт challenge только после RPC.
- `approval.rs` хранит не более одного immutable pending request. Каждое окно получает
  новый label `approval-N`; engine destination также связан с generation и reverse RPC id.
  Поэтому delayed IPC старого окна не может подтвердить новый запрос с повторившимся
  `approvalId` после restart.
- Capability `approval` применима только к `approval-*` и содержит две команды:
  `approval_current` и `approval_decide`. Capability `main` их не содержит; Rust повторно
  проверяет точный активный label. У approval-окна нет engine, event, filesystem, shell,
  network или window-creation permissions.
- Ядро валидирует canonical action/hash/text/risk/expiry, хранит собственную запись,
  запрещает allowed-once первую секунду, подписывает HMAC через `hoh-protocol`, проверяет
  подпись и потребляет решение один раз. Close даёт signed `rejected`; expiry/crash/build
  failure дают fail-closed error. Навигация разрешена только на bundled `approval.html`,
  `window.open` запрещён.
- Для `hoh/model/key/set` preflight action содержит только `keyHash` и `baseUrl`; raw
  `apiKey` живёт в аргументе main invoke до передачи stdio и нигде desktop-ядром не
  сохраняется. `hoh/settings/rules/set` проходит тот же privileged gate.

## Проверки 19.11

- Финальный набор содержит **23 теста**. Помимо bad MAC, one-use, stale label,
  no-send, transport/restart/shutdown и реального `hoh-window-stub`, он проверяет все
  privileged preview, отсчёт секунды от первого `approval_current`, Destroyed/status/
  expiry cleanup, initialize-send backoff, capability isolation, exact navigation origin,
  production probe isolation и параллельные запросы при занятом approval-слоте.
- Реальный stub сначала получил корректно сформированный decision с неверным MAC и
  отклонил original key-set, затем принял HMAC, созданный `Broker`, и завершил key-set.
- `cargo clippy --all-targets --locked --offline -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- `scripts/run-approval-probe.sh` теперь сам staging-ит production
  `ui/dist/approval.html` рядом с tiny probe main, собирает stub и desktop именно с
  `probe-dist` и feature `probe`, запускает их в одноразовом HOME/Xvfb/D-Bus без IP-сокетов и нормализует
  любой отрицательный исход в result line и exit 1. `--build-only` полностью прошёл в
  managed sandbox; в ELF проверены встроенные probe HTML/JS. Runtime остаётся внешней
  проверкой: первый прямой запуск ведущего завершился timeout, потому что после probe
  бинарь был пересобран тестами с production frontend. Новый единый сценарий исключает
  подмену артефакта между сборкой и запуском.

## Решение и остаточные ограничения

Официальная модель Tauri использована без shim: label-scoped capabilities,
`AppManifest::commands`, Rust caller-label check и `on_navigation`. Пересекающихся main/
approval capabilities нет. Независимый deep plan review после исправления no-send и stale
window P1 завершился `ready`, findings отсутствуют.

Главное окно создаётся штатным Tauri auto-create до `setup`; зарегистрированный заранее
plugin применяет к нему exact-origin navigation allowlist, а WebKit `create` handler
отклоняет `window.open`. Это сохраняет раннюю загрузку UI/probe без ослабления политики.

Проверен Linux/WebKitGTK путь. На других платформах системный folder chooser пока возвращает
unsupported; это существующее ограничение пакета. Persistence API-ключа принадлежит движку;
stub использует свою process-local credential implementation.

### Watchdog живого движка

Зависший, но не завершившийся процесс после состояния `ready` автоматически не убивается.
Это сознательное fail-visible ограничение текущей версии: протокол пока не определяет
аутентифицированный heartbeat, а локальный таймер не отличит зависание от допустимого
долгого модельного шага и может оборвать уже начатый эффект. Запрос всё равно ограничен
таймаутом и возвращает ошибку пользователю. Перед подключением production-движка нужно
добавить в протокол отдельный liveness/heartbeat с deadline; только его пропуск должен
перезапускать процесс без переигрывания незавершённых запросов.

## Стык с настоящим движком (sg6, 19.11)

- Запуск: `HOH_TEST_ENGINE` (только заглушка, без аргументов и `provider: mock`) либо путь из сборки
  (`option_env!("HOH_ENGINE_PATH")`, при сборке `build.rs` следит за переменной) или `HOH_ENGINE_PATH`
  при запуске. Для настоящего `hoh`: аргументы `--profile window`, `DSH_HOME=<app_data>/engine`,
  `provider: openai-compatible`, `model: local` (модель потом задаёт `hoh/settings/set`).
  Путь, аргументы и окружение берутся только из сборки/переменных, не из записываемого `engine.json`.
- `DSH_HOME` создаётся ядром с правами 0700 (и принудительно выставляется на уже существующей папке):
  credentials-local иначе откажет в `hoh/model/key/set`.
- `capabilities.userQuestions: true`. `user-question/ask` не идёт через окно подтверждения:
  `questions.rs` хранит исходный вопрос, главное окно получает событие `engine-question`, отвечает командой
  `question_answer` (только метка `main`), Rust проверяет id, подписи вариантов и размер ответа по хранимому
  вопросу и один раз отправляет ответ движку. `user-question.cancelled`, смена статуса движка и перезапуск
  закрывают вопросы. Ответ вопроса ничего не разрешает — это текст для модели.
- Рабочая папка сессии: `workspace.rs`. Старт — папка, выбранная на шаге «Правила» (`workspace.json` в
  config-каталоге), иначе `Документы/HoH` (создаётся), но не HOME. `hoh/settings/rules/set` из главного окна
  получает настоящий абсолютный путь вместо подписи «Документы/HoH» до подтверждения и до движка; после
  разрешённого вызова ядро запоминает папку и перезапускает движок (`Engine::restart_in`) в ней. Незавершённые
  вызовы при перезапуске заканчиваются ошибкой и не повторяются.
- Подтверждение остаётся двухступенчатым (ядро, затем `approval/request` движка): на первом запуске окно
  подтверждения появляется для `key/set`, `settings/set` и `rules/set`, по два на действие.
- Движок на этой стороне не знает `hoh/employee/open` и `hoh/security/summary`: окно получает от ядра
  маркер `method-unavailable` (только для JSON-RPC -32601; другой текст ошибки наружу не идёт), чат
  тогда берёт сессию `window-main` и определяет «данные на этом компьютере» по адресу модели (`hoh/settings/get`).
- Проверки: `tests/real_engine.rs` (нужны `HOH_REAL_ENGINE`, `HOH_MOCK_BIN`) — первый запуск через настоящие
  ключ/адрес с MAC от `Broker`, поток ответа, bash/read/write и опасная команда, `session/events|list`,
  вопрос ассистента и ответ в запросе модели, рабочая папка сессии. Запуск: `target-cf1/run-desktop-tests.sh`.

## Известные ограничения

Два решения приняты осознанно и записаны здесь, чтобы их не принимали за недоработку.

1. **Подтверждение и «двойник» запроса.** Человек один раз подтверждает действие окна (например, ключ или
   рабочую папку), и ядро подписывает ответ на тот самый запрос движка без второго окна. Пока этот запрос
   в пути, любой запрос движка с тем же методом, действием и сессией неотличим от настоящего без изменения
   протокола — он получит это одно подтверждение. По смыслу это то же действие, которое человек уже
   разрешил; действия модели, чужие сессии, чужие запуски движка и повторы окно получают как обычно.
   Подтверждение живёт не дольше 30 секунд с момента нажатия и сгорает после первого использования.
2. **Сон компьютера.** Если окно подтверждения осталось без ответа и таймер ядра проснулся уже после срока
   (например, ноутбук спал), подписать «отменено» нельзя — протокол запрещает подпись после срока. Движок сам
   отменяет запрос по своему сроку, а журнал ядра честно пишет «истёк без ответа». Таймер срабатывает с запасом
   5 секунд, поэтому в обычной работе это не случается.

## Окно на веб-клиенте T3 (HOH-40, 22.11.2026, основатель 19:16)

- **Страница.** Главное окно открывает `index.html` — с HOH-40 это веб-клиент T3 (`ui/t3.html`, сборка
  `vite build --mode t3`, см. `ui/src/t3-entry`). Прежняя оболочка — `legacy.html`; её открывает переменная
  `HOH_WINDOW_SHELL=legacy` (`native::legacy_shell_requested`), пока оболочку не удалят. Имя окна — «HoH».
- **Навигация главного окна** разрешает ещё `legacy.html` и пути роутера T3 без файлов (`/settings/general`,
  `/<среда>/<разговор>`): после перезагрузки на таком пути Tauri отдаёт `index.html`. Файлы (`approval.html`,
  `firstrun.html`, `assets/*`) и чужие адреса — как раньше, нельзя.
- **CSP:** `style-src 'unsafe-inline'`, `img-src blob:`, `font-src data:` — стили, картинки и шрифты T3. Скрипты — по-прежнему
  только `'self'`; встроенный сценарий темы T3 вынесен в файл.
- **CSP проверяется сборкой** (fix2, решение CEO 22.11 20:00): `ui/scripts/check-dist.mjs` читает CSP из `tauri.conf.json`;
  у каждой директивы — только перечисленные источники, наружу ничего; тесты — `ui/tests/chat/dist-check.test.mjs`.
- **Ссылки** (fix2): запрос нового окна из главного (T3: `target=_blank`, `window.open`) окно не создаёт никогда; http(s)-ссылку
  получает `xdg-open` (`opener::clicked_link`), только если WebKit отметил нажатие человека (`is_user_gesture`). Тесты —
  `tests/opener.rs`, `tests/security.rs`.
- **Режим доступа Claude** (`hoh/executor/access/set`), **выбор Claude** (`hoh/executor/select` с `executor:
  "claude"`, fix3) и **папка разговора Claude** (`session/working-directory/set`, если движок по
  `hoh/executor/access/get` называет исполнителем разговора Claude, fix3) — без окна подтверждения: действие
  человека в главном окне и есть подтверждение (`approval::main_window_confirms`, `Broker::confirm_from_main_window`).
  Принимается только от окна `main` (`bridge::check_main`); журнал — `who: human, via: main-window`; запрос движка
  на то же действие подписывает ядро по разрешению «в полёте», как после окна настройки. Без этого действия
  (исполнитель, модель, SDK-сервер) запрос движка получает окно. Встроенный помощник (`select` без исполнителя,
  папка его разговора, движок не ответил), ключ, правила — с окном, как были.
- **Папки для «Add project»** (`browse_folders`, fix3, разрешено ведущим 22.11 20:43): порт `browse` из
  `apps/server/src/workspace/WorkspaceEntries.ts` T3 (`src/folders.rs`) — только имена подпапок (тип записи без
  перехода по ссылкам, файлов нет), только окну `main` (право `allow-browse-folders`). Отличие: путь без `/`, `~`,
  `./` T3 берёт от рабочей папки своего сервера, окно — от домашней (своей рабочей папки у окна нет).
- **Запрос разрешения в разговоре.** В окне T3 запрос разрешения движка не открывает `approval.html`: окно T3
  показывает его по уведомлению `approval.pending`, ответ — `approval_decide` из главного окна, только на этот
  запрос по `approvalId` (`Broker::show_in_chat`/`decide_in_chat`; права `main` — `allow-approval-decide`). Проверки
  те же (не раньше 1 с после показа, вложенная команда закрывает «разрешить»), подпись та же, журнал —
  `who: human, via: chat`, срок — отмена с подписью. В прежней оболочке — окно подтверждения, как было.
- Проверки: `cargo test` (94, с `HOH_TEST_ENGINE`/`HOH_REAL_ENGINE`/`HOH_MOCK_BIN`), clippy (и `--features probe`), fmt.
