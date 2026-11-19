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
