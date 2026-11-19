# hoh-sdk-jsonrpc-server ← packages/sdk/server @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs, src/transport.rs | Plugin `sdk-jsonrpc-server`, stdio JSONL, readiness, shutdown/flush/exit request |
| src/server.ts | src/server.rs, src/error.rs, src/shim.rs | сессии, route, prompt/query/resume/wait, events, approval, teardown |
| src/query-input.ts | src/query_input.rs, src/command_helpers.rs | JS whitespace, aliases и builtin dispatch; helper из interaction/commands |
| src/query-content.ts | src/query_content.rs | строгая wire-валидация и картинки команд |
| src/query-activity.ts | src/query_activity.rs | наблюдение command/input/goal до терминального события |
| src/user-questions.ts | src/user_questions.rs, src/user_question_helpers.rs | bridge, parser, cancellation; `UserQuestionError` из interaction/user-questions |
| locale/{de,en,es,fr,ja,pt,ru,zh}.json | locale/ | скопированы побайтно |
| tests/*.spec.ts | tests/*.rs | native Rust fixtures; учёт исключений ниже |
| README*, package.json | эти заметки, Cargo.toml | прочитаны; собственного LICENSE нет |

Скелета не было. Он создан неизменённым `scripts/new-crates.sh`; для discovery
передана только назначенная строка, потому что MAP уже имел status `writing:w4`.
Скрипт, MAP, workspace manifest, lockfile и чужие crates вручную не менялись.

## Отличия от образца

- `hoh-protocol` заменяет sdk-protocol: его framing, wire types, request correlation и
  classification используются без дублирования. Асинхронные waiter/executor,
  которых нет в protocol crate, живут в `JsonRpcLineTransport` этого host crate.
- Командный каталог, descriptor/builtin vocabulary, submitted attachments и execute
  берутся напрямую из `hoh-commands`. Query fixture регистрирует настоящие
  `CommandDefinition` в `CommandRuntime`, поэтому service key, admission и lifecycle
  проверяются с owner TypeId, а wire result явно проецируется в `hoh-protocol`.
- Goal availability и `goal/activation-changed` берутся напрямую из `hoh-goal`;
  query-activity проверяет `Goals::get` и typed `GoalActivationChanged`. Тесты
  монтируют настоящий `GoalServicePlugin` и вызывают owner transitions; ошибка
  чтения owner projection не подавляется, как и исключение `goals.get()` в source.
- При отсутствии `deepseek-official` adapter сервер монтирует настоящий
  `hoh_llm_deepseek_api_key::DeepSeekApiKey`, как `ctx.plugin(LlmDeepSeek)` в source;
  заранее зарегистрированный adapter по-прежнему побеждает.
- TypeScript runtime hooks `input/output/exit` не сериализуются в Config: Rust хост
  предоставляет `JsonRpcRuntimeHooks`. Библиотека не зовёт `process::exit`:
  публикует `SdkProcessExit`, а app завершает процесс после disposal. Тестовый
  callback сохранён. Стандартный stdin/stdout остался default. Необязательный
  `JsonRpcRuntimeHooks::with_exiting` (Rust-добавка, не из source) вызывается один раз после сброса
  ответа на `shutdown` и до disposal корня; `exit` по-прежнему идёт после него. Хост, ещё грузивший дерево,
  так узнаёт, что корень закрыт по запросу выхода.
- `Context`/Rust events заменяют JS declaration merging. Открытая граница
  `contentBlocks` до адмиссии удерживает `serde_json::Value`, потому что закрытый
  protocol enum не может сохранить чужой plugin block. Далее используются
  реальные `hoh-llm::ContentBlock` и durable attachments. Поэтому блок вроде
  `{type:"x-plugin",a:1}` после адмиссии отвергается при преобразовании в
  `ContentBlock`; образец передаёт такой объект без изменений. Это осознанное
  ограничение текущего закрытого enum, а не строгая wire-валидация source.
- Promise команды в образце eager. Rust job явно владеет продолжением,
  shutdown его дожидается, а отмена request-handler не обрывает native execution.
- `QUERY_INVOCATION` использует Tokio `task_local!`: значение доступно только в
  текущей task и, в отличие от Node `AsyncLocalStorage`, не наследуется задачей,
  которую command handler сам создаёт через `tokio::spawn`. События из такой
  дочерней задачи не считаются принадлежащими текущему query activity.
- `ForeignErrorValue` — Rust carrier для JS-значения, брошенного владельцем
  optional subagent API. Он нужен, чтобы точно сохранить структурную
  проверку `{name:'SubagentError', code:'UNAUTHORIZED'}` наряду с HarnessError.
- Добавлены Rust-имена для анонимных source shapes/service keys:
  `JsonRpcRuntimeHooks`, `SdkSessionActivation`, `SdkProcessExit`, `SdkServerError`,
  `CommandDescriptor`, `BuiltinCommandName` и контракты `integration`. Новых
  wire методов, настроек и session events они не вводят.

## Не перенесено

- `built-scope-carrier.e2e.ts` — npm/node packaging e2e, исключён PORTING §8/§10.
- `compact/permission/plan/goal-parity.spec.ts` зависят от конкретных command
  plugins, не от registry owner. Вместо копии этих plugins tests/commands.rs
  регистрирует fixture definitions в настоящем `CommandRuntime` и проверяет
  dispatch, aliases, аргументы, image admission, lifecycle и результат.
- Пять owner-integration сценариев не изображают отсутствующие packages:
  command image-admission до handler; plan за paused-goal cancellation; goal reference за
  maintenance; independently resumed paused goal; disposal до выдачи command id. Их
  наблюдаемые server-side инварианты (admission/liveness, isolation, event correlation,
  shutdown cleanup) покрыты на узких fixtures; саму owner state machine переносить
  в этот crate запрещено.
- Одна ветвь shutdown aggregate подменяет в образце возвращаемый
  `AgentHandle.dispose` и заставляет root teardown упасть. Текущий concrete `Agents`
  не имеет test seam для подмены выданного handle. Ветвь одной drain-ошибки,
  disposal root после неё и код aggregate для двух failures сохранены; двойная
  ошибка остаётся непроверенной в этом crate.
- Сценарий `continues teardown after a subscription disposer fails` подменяет
  prototype и возврат disposer на лету; это исключённая PORTING §8 замена кода.
  Native `Disposer::dispose` не возвращает user error; reverse disposal и continuation
  после ошибки drain проверяются.
- В образце JS может бросить `undefined` во время terminal publication.
  Rust `BoxError` не имеет беззначного значения; ветвь с именованным
  sentinel проверяет ту же retention/clear последовательность.

## Не проверено

- Настоящее окно, stdin/stdout процесса и `process::exit` не запускаются.
  `server.spec.ts:280` с живым OpenAI-compatible HTTP endpoint отложен до
  регистрации `hoh-llm-openai`: replay-тест проверяет provider/model,
  reasoning/maxTokens, system/user messages, два SDK prompt и notifications,
  но не API key/header или сеть. Остальные тесты тоже используют in-memory
  transport, replayed model и tempdir.
- Синхронная JS microtask/setImmediate очерёдность представлена owned Rust
  futures и yield перед flush; проверяется наблюдаемый порядок фреймов,
  disposal и exit, но не сам V8 scheduler.
- В полном SDK выводе модели рабочая папка попадает в system prompt.
  Текущая `hoh-working-directory` ещё имеет частный Agent/SessionProjections
  adapter на границе system-prompt; RPC get/set и добавляемое сообщение
  проверяются, но не интеграция двух чужих private Agent типов.

## Вопросы ведущему

- `SHIM(hoh-subagent)`: `root_of`, `wait_for_tree_work`, `drain_descendants` и lifecycle event;
  заменить типами owner crate. До этого глубина дерева и parked
  inbox проверяются через узкий owner fixture, а не копией runtime.
- `SHIM(hoh-schedule)`: `ScheduleResolveAgent`; заменить owner event.
- `SHIM(hoh-protocol)`: `SdkPromptContentBlock` остаётся закрытым enum и не может
  представить declaration-merged plugin block на сырой `session/query` границе.
  До появления у владельца open tagged input `query_content` удерживает `Value`;
  framing и все существующие protocol-типы уже используются напрямую.
- `hoh-working-directory` содержит временные private Agent/SessionProjections
  contracts. Сервер адаптирует реальный Agent точечно; после интеграции
  owner types убрать `directory_agent` и test-only двойную регистрацию projection.

## Проверки 19.11

Все cargo-команды: `CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true`,
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w4` (tmpfs `/dev/shm/hoh/target-w4`).

- `cargo check -p hoh-sdk-jsonrpc-server --lib`: exit 0; `Finished dev profile ... in 4.63s`.
- `cargo clippy -p hoh-sdk-jsonrpc-server --lib -- -D warnings`: exit 0;
  `Finished dev profile ... in 0.54s`.
- `cargo fmt -p hoh-sdk-jsonrpc-server -- --check`: exit 0, без вывода.
- После снятия внешнего blocker-а `cargo check -p hoh-sdk-jsonrpc-server --tests`:
  exit 0; `Finished dev profile ... in 21.64s`. Чужие crates не изменялись.
- `timeout 300s cargo test -p hoh-sdk-jsonrpc-server --quiet`: exit 0;
  176 passed, 0 failed. В test composition добавлен пустой executor: реальный
  оконный профиль монтирует native executor вместе с `ToolRuntime`, а server
  tests не определяют и не вызывают tools, но prompt assembly всё равно требует
  выбранный executor.
- `timeout 300s cargo clippy -p hoh-sdk-jsonrpc-server --all-targets -- -D warnings`:
  exit 0; `Finished dev profile ... in 0.67s`.
- Финальный `cargo fmt -p hoh-sdk-jsonrpc-server -- --check`: exit 0, без вывода.
- Восемь locale-файлов побайтно совпадают с образцом. В production нет
  `todo!`, `unimplemented!`, `unsafe`, stdout/stderr-печати или `process::exit`.

## Исправления ревью 19.11

- `handles_prefilled_initialize_only_after_installing_the_request_handler`:
  reader запускается после регистрации handler; до исправления получал `-32601`.
- `keeps_the_process_alive_after_a_failed_shutdown_response`: exit/disposal
  планируется только после успешного ответа `shutdown`; до исправления root завершался.
- `checks_prompt_initialization_before_validating_its_wire_shape`: prompt до
  initialize всегда возвращает `SDK server is not initialized`, затем применяются
  дополнительные Rust wire-проверки.
- `reproduces_the_node_path_resolve_error_for_a_missing_cwd` и
  `reproduces_the_attachment_error_for_an_unsupported_prompt_image_type` фиксируют
  исходные тексты, а второй также `AttachmentError/UNSUPPORTED_IMAGE_TYPE`.
- `replays_the_configured_route_and_multiple_sdk_prompts_without_a_live_provider`
  закрывает переносимую часть `server.spec.ts:280`; HTTP/header/API key остаются
  отложенными до `hoh-llm-openai`, как описано выше.

Финал: `timeout 300s cargo test -p hoh-sdk-jsonrpc-server --quiet` — exit 0,
182 passed, 0 failed; `timeout 300s cargo clippy -p hoh-sdk-jsonrpc-server
--all-targets --no-deps -- -D warnings` — exit 0 (`0.51s`); `cargo fmt -p
hoh-sdk-jsonrpc-server -- --check` — exit 0.

## Снятие shim-ов 19.11

- `hoh-commands`, `hoh-goal` и `hoh-llm-deepseek-api-key` подключены как path
  dependencies; их локальные service/event/data shim-типы удалены. Командные и
  goal-activity тесты используют owner service keys и owner event TypeId.
- `hoh-protocol` оставлен только на описанной выше открытой input-границе: owner
  пока не экспортирует тип для declaration-merged content block.
- С `CARGO_BUILD_JOBS=2`, offline и target-w3: `timeout 600s cargo test -p
  hoh-sdk-jsonrpc-server --quiet` — 182 passed, 0 failed; `timeout 600s cargo
  clippy -p hoh-sdk-jsonrpc-server --all-targets --no-deps -- -D warnings` —
  exit 0; финальный `cargo fmt -p hoh-sdk-jsonrpc-server -- --check` — exit 0.

## Протокол окна (часть 2b, sg2, 19.11)

Контракт: `docs/window.md` §4, эталон поведения — `hoh-window-stub`. Новых зависимостей нет.

- `initialize`: `capabilities.approvals:"host"` + `hoh:{protocolVersion:1, approvalKey}`.
  Ключ проверяется `hoh_protocol::approval::key_from_hex` до изменения состояния (64 hex,
  не нулевой), затем отдаётся сервису `hoh_user_approval::Approval::install_host_mode`
  вместе с адаптером транспорта (`TransportApprovalPeer`); в состоянии сервера, логах и
  ответах ключа нет (`SecretString`, Debug скрыт). Установка — последний шаг: неудачный
  `initialize` не оставляет ключ. Ответ в host-режиме: `hoh:{protocolVersion:1,
  approvals:"host"}`; без host — прежний ответ SDK. Без смонтированного `Approval` host
  `initialize` — ошибка (fail-closed). Версия `hoh.protocolVersion` не 1 — ошибка.
- Повторный `initialize` (и параллельный) — ошибка `SDK server is already initialized`,
  в том числе без host: прежний SDK-код допускал повтор, окну это запрещено §4.
- `session/list {employeeId?}` → `{sessions:[{id,title,employeeId,updatedAt}]}`: хранилище
  (`SessionPersistenceService.list`) плюс живые сессии; subagent-сессии скрыты. Название —
  последний `session/title`, иначе `title` в заголовке, иначе первая строка первого
  человеческого сообщения (80 символов). `updatedAt` — время последнего события,
  `employeeId` — `employeeId` заголовка, иначе `"default"` (сотрудников пока нет).
  Нечитаемый журнал одной сессии не скрывает остальные (предупреждение в лог).
- `session/events {sessionId, afterSeq?, limit}` → `{events,nextSeq}`: `limit` 1..1000,
  `afterSeq >= -1`; события те же, что в `session.event`. Открытая сессия читается из памяти,
  иная — из хранилища только на чтение; неизвестная — ошибка. `nextSeq` — seq последнего
  события или `afterSeq` (как заглушка).
- `session/cancel {sessionId}` → `{}`: `Agent::cancel(User)` у открытой сессии; ход
  завершается `turn/end` `aborted/user`. Неизвестная сессия — ошибка (заглушка отвечала `{}`).
- Таблица `hoh/*` (`HohRpcMethods`, `HohRpcMethodRegistry::register(ctx, method, handler)`):
  принимает только объявленные в `hoh_protocol::REQUEST_METHODS` методы с префиксом `hoh/`,
  дубликат — ошибка, `Disposer` снимает регистрацию вместе с владельцем. Диспетчер требует
  `initialize`; не зарегистрированный метод — `unknown HoH method`.
- Привилегированные методы (`PRIVILEGED_METHODS`): вне host-режима `hoh/*` отвергается
  (mac проверить нечем), прежний `session/working-directory/set` работает как в SDK. В host-
  режиме перед обработчиком вызывается `HohPrivilegedApprover` — **GLUE-SHIM(4)**: сервис, который
  смонтирует часть 4 (человекочитаемый текст, `approval/request`, mac); пока его нет —
  отказ. Обработчики привилегированных методов второй раз пользователя не спрашивают.
- `shutdown` и stdio — без изменений (флаш, затем dispose корня, затем код 0).

Тесты: `tests/window.rs` (14) и сценарий `runs_the_window_scenario_over_stdio_…` в
`tests/plugin_apply.rs`.

## Исправления по ревью (sg2-fix1, 19.11)

- Транспорт (`transport.rs`): запрос больше не создаёт эффект ядра (`ctx.spawn` на каждый
  запрос копил `Effect` и `JoinHandle` до выгрузки плагина; `tests/request_leak.rs`:
  резидентная память после 60 000 ответов, раньше +100 МБ). Запросы идут обычными задачами с
  счётчиком «в работе». Писатель — эффект `jsonrpc.write` без отмены токеном выгрузки: его undo
  ждёт ответа на работающие запросы (не дольше 5 с, `DRAIN_TIMEOUT`), затем останавливает
  писателя после уже поставленных в очередь кадров. Как в TS, `close()` только перестаёт читать;
  ответ на идущий `session/query` при выгрузке пишется (`plugin_apply.rs`). Публичный API
  не менялся.
- Гонка query/shutdown: проверка `shutting_down` и вставка в `command_tasks` теперь под одним
  замком (как синхронный участок `server.ts:297-311`). Как воспроизводимая ошибка пункт не
  подтверждён: `CommandRuntime::execute` отвергает уже прерванный signal до вызова обработчика
  (`hoh-commands/src/lib.rs`), а `shutdown()` прерывает lifetime до снимка `command_tasks`, поэтому
  обработчик над освобождаемыми агентами не запускается и до исправления
  (`tests/shutdown_race.rs` проходит и на старом коде). Замок оставлен как соответствие образцу.
- `initialize`: провайдер не строка / отсутствует → `no adapter registered for provider
  "<String(provider)>"` (`undefined`, `5`, `null`, `a,b`, `[object Object]`), как `server.ts:233-234`.
- `QueryActivity::accept` вернул ошибку → activity закрывается, как `catch` в `server.ts:320-322`.
  Тестом не покрыто: ошибку даёт только `ctx.spawn` на выгруженном контексте.
- Осознанное отличие: `lib.rs` закрывает транспорт и после неудачного `shutdown` (TS при ошибке
  `shutdown()` пропускает `transport.close()`): иначе читатель stdin остался бы жить.
- Перенесены тесты образца: approval `disconnect` и кадр запоздалого grant на проводе
  (`tests/approval.rs`), порядок «ответ → flush → root-disposed → exit», один root-disposed,
  ошибка flush, тишина после exit, точный текст неизвестного метода (`tests/plugin_apply.rs`),
  `session/wait` без `sessionId` / неизвестная сессия / после shutdown / дочерний ход во время
  ожидания дерева и ветка `seq != idleSeq` (`tests/wait.rs`; мутация проверки `seq` ловится).

## Исправления по ревью (sg2-fix4, 19.11)

- Выгрузка без `shutdown` (HMR, EOF, сигнал): ядро запускает undo всех эффектов одновременно, и
  писатель мог получить `Stop` раньше кадров, которые выдаёт undo `jsonrpc.serve`
  (`user-question.cancelled` из `server.shutdown()`), — те отбрасывались. Теперь плагин вызывает
  `transport.hold_writer_until_closed()`: undo писателя ждёт `close()` (его делает `jsonrpc.serve`
  после `server.shutdown()`) и окончания запросов в работе, оба ожидания под одним лимитом
  `DRAIN_TIMEOUT` (5 с). Транспорт без такого владельца (`JsonRpcLineTransport::start` в тестах)
  по-прежнему останавливает писателя сразу после запросов в работе. Тест
  `tells_the_host_a_question_is_cancelled_when_the_plugin_is_unloaded` (`plugin_apply.rs`): вопрос
  открыт, `fiber.dispose()`, кадр `user-question.cancelled` с тем же `requestId`, `exit` не вызван.
- Запись, пришедшая после того, как писателю велено остановиться, — ошибка `JSON-RPC transport
  closed` (раньше `send` проходил, а кадр молча терялся; `after_response` срабатывал для
  незаписанного ответа, а `notify` не давал предупреждения). Признак и отправка в канал — под одним
  замком (`write` и `finish`), поэтому кадр не может оказаться в очереди за `Stop`. Тест
  `tests/transport_stop.rs`: писатель заблокирован на кадре, `Stop` в очереди, `notify` → `Err`.
  Точная формулировка: запись — ошибка с момента, когда писателю велено остановиться (а не после
  выхода его задачи); `flush()` после этого тоже ошибка.
- Отступление от PORTING §10 (`tokio::time::pause`): тесты `request_leak.rs` (опрос `sleep(1ms)`
  до ответа на пакет запросов), `shutdown_race.rs` (multi_thread, `spawn_blocking` и сдвиг по
  `black_box`) и ожидание тишины после exit в `plugin_apply.rs` (`sleep(20ms)`) идут в реальном
  времени: на multi_thread-рантайме `pause` недоступен, а передать «ничего не пришло» иначе, чем
  ожиданием, нельзя. Утверждения от времени не зависят (они о наличии/отсутствии кадра и росте
  памяти с большим запасом); `request_leak.rs` дополнительно зависит от резидентной памяти
  процесса (аллокатор), порог 8 МБ против десятков МБ при утечке.
