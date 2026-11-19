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
- TypeScript runtime hooks `input/output/exit` не сериализуются в Config: Rust хост
  предоставляет `JsonRpcRuntimeHooks`. Библиотека не зовёт `process::exit`:
  публикует `SdkProcessExit`, а app завершает процесс после disposal. Тестовый
  callback сохранён. Стандартный stdin/stdout остался default.
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
- DeepSeek provider из `llm-deepseek-api-key` по прямому решению ведущего не
  переносится. Заранее зарегистрированный adapter всегда побеждает;
  fallback — приватный host hook до подключения `hoh-llm-openai` в профиль.
- `compact/permission/plan/goal-parity.spec.ts` зависят от полных native
  command owners, которые задание явно не разрешает переносить сюда. Вместо
  копии их логики tests/commands.rs проверяет dispatch, aliases, аргументы,
  image shape, lifecycle и точную передачу результата через узкий fixture owner.
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

- `SHIM(hoh-commands)`: list/execute, descriptor и единственный image attachment;
  заменить на native commands service, когда его crate будет готов.
- `SHIM(hoh-subagent)`: `root_of`, `wait_for_tree_work`, `drain_descendants` и lifecycle event;
  заменить типами owner crate. До этого глубина дерева и parked
  inbox проверяются через узкий owner fixture, а не копией runtime.
- `SHIM(hoh-goal)`: active lookup и activation event; заменить owner types. Тесты
  query-activity подают durable goal events и тот же availability/lifecycle сигнал.
- `SHIM(hoh-schedule)`: `ScheduleResolveAgent`; заменить owner event.
- `SHIM(hoh-llm-deepseek-api-key)`: только mount hook. По решению ведущего
  конечный профиль должен зарегистрировать `hoh-llm-openai`.
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
