# hoh-agent-loop ← packages/core/agent-loop @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/constants.ts` | `src/constants.rs` | `DEFAULT_MAX_PARALLEL_TOOL_CALLS = 10` |
| `src/inbox.ts` | `src/inbox.rs` | проекция `inbox` (`inbox_projection_definition`), `ReactLoopInbox` (трейт `hoh_agent::Inbox`) |
| `src/runtime-context.ts` | `src/runtime_context.rs` | `SystemPromptProjection`, `RuntimeContextProjection` |
| `src/assistant-stream.ts` | `src/assistant_stream.rs` | `AssistantStreamAttempt` |
| `src/agent.ts` | `src/agent.rs` | `ReactLoopAgent` (драйвер: фазы Idle/Maintenance/Running, turn/step, pre-step, запрос, поток, пакет инструментов) |
| `src/index.ts` | `src/factory.rs` + `src/lib.rs` | `AgentLoop` (фабрика, владение жизненным циклом: `create`, `create_agent`, `resume_agent`, `resume_with`), плагин `AgentLoopPlugin`, `Config`, события `ConfigStartFailed`, идентичности лаунчера |
| `locale/*.json` | `locale/*.json` | скопированы без изменений |
| `README*.md`, `package.json`, `tsconfig.json`, `tsdown.config.ts` | эти заметки | прочитаны |

LICENSE и отдельно лицензированных фрагментов нет.

### Тесты

Каркас — `tests/common/mod.rs` (мок-адаптер по сценарию: `Chunks`/`Hang`/`HangSlow`/`HangAfter`; `multi_call`,
`Gated` — инструмент, блокирующий вызовы до освобождения; `content_tool`; `try_harness` с лимитом исполнителя и
JSONL-хранилищем во временном корне; `until`, `events_of`, `result_codes`). Нативный исполнитель — настоящий
`hoh-tool-executor-native`. Проекции — настоящий `hoh-session-projection`.

| образец → tests/*.rs | перенесено (тестов Rust) / случаев образца |
|---|---|
| `tool-calls.spec.ts` → `tool_calls.rs` | 19 / 24 (отложено 5 — ниже) |
| `cancel.spec.ts` → `cancel.rs` | 31 / 37 (отложено 7 — ниже) + 2 регрессионных ревью |
| `resume.spec.ts` → `resume.rs` | 13 / 47 + 1 регрессионный ревью |
| `scope-lifecycle.spec.ts` → `scope_lifecycle.rs` | 15 / 39 |
| `loop.spec.ts` → `loop.rs` + `loop_tools.rs` | 11 + 8 / 66 (в `loop.rs` ещё 4 моих регрессионных теста) |
| `inbox.spec.ts` → `inbox.rs` | 6 / 6 |
| `runtime-context.spec.ts` → `runtime_context.rs` | 8 / 8 |
| `system-prompt-projection.spec.ts` → `system_prompt_projection.rs` | 9 / 9 |
| `config-session-id.spec.ts`, `settings.spec.ts` → `config.rs` | 5 (правила проверки и умолчания) |
| `tool-order`, `tool-updates`, `tool-executors`, `request-error`, `input-lifecycle`, `shutdown-drain`, `model`, `resource-lifecycle` | s17 (не мои файлы) |
| `interception.spec.ts` | s12 (не мой файл) |
| остальные (`agent`, `agent-initiator`, `contract-regressions`, `coverage-edges`, `properties`, `request-cache.e2e`, `request-freeze`, `request-reconstruction`, `serial-listener-review`, `system-prompt-admission`, `config-session-id` часть с persistence) | не перенесены |

## Отличия от образца

1. **Драйвер — `Arc<ReactLoopAgent>` под `Arc::new_cyclic`** (слабая ссылка на себя вместо `this`).
   `Agent` — трейт (см. PORT_NOTES `hoh-agent`); `scope.rawDispose` образца → `scope.dispose()` в разборе жизненного цикла.
2. **Фаза — `enum Phase` под `Mutex`.** `status` меняется с событием `agent/status` вне замка. Завершение
   работы агента (`whenIdle`) — `tokio::sync::watch<bool>` на каждую активность; `when_idle()` сразу
   возвращается, если активности нет. **`send`/`followup` синхронно открывают активность**, поэтому
   `agent.followup(m); agent.when_idle().await` — корректное ожидание конца хода (в образце тесты ждут события
   `agent/status`).
3. **Драйвер запускается `tokio::spawn`** внутри границы инициатора `AgentRegistry::with_initiator`, как
   `AsyncLocalStorage.run` образца. Остаток: спавн внутри хода инициатора не наследует (см. `hoh-agent` п. 7).
4. **Отмена.** `AbortController::abort_with(CancelCauseError(cause))`; причину читает
   `aborted_cancel_cause(signal)`. `turn/end.reason` строится по ней (`aborted`) либо по ошибке
   (`LlmError` → `failure`, `ToolActivationError` → `{message, code}`, остальное → `{message: errorChain, code: "UNKNOWN"}`).
5. **`agent/error`** несёт `SharedError` (клонируемую обёртку): событие и возвращаемая ошибка драйвера делят одну ошибку.
6. **Ошибка проекции не подавляется** (п. 3 ревью rv-agent; TS `index.ts:570`). Читатели проекций `inbox` и
   `modelSelection` возвращают `Result`: `ReactLoopInbox::has_pending/claim/locate/mutate` и предшаг (`model_check`)
   передают ошибку наверх как `invalid persisted inbox splice at session seq N: <причина>`. Трейты `hoh_agent::Inbox::next_turn/
   next_step` и `Model::current` без `Result` (крейт `hoh-agent` не мой): геттер при сломанной проекции **паникует** с
   тем же сообщением (аналог броска из геттера образца); безопасная форма — `ReactLoopInbox::try_next_turn/try_next_step`.
   Тест: `reading_the_inbox_reports_an_invalid_persisted_splice`.
7. **Фабрика.** `FactoryOwnership` с атомарными счётчиками и `Tasks` (счётчик + `Notify`) вместо набора
   `Promise`; запись жизненного цикла `Lifecycle` с общим `Shared`-будущим сворачивания (идемпотентно для всех
   владельцев). Эффект на `ownerCtx` (`agentLoop.lifecycle(<id>)`) вызывает то же сворачивание; усыновление scope
   в эффект цикла (`loopCtx.effect(rawDispose, teardown)`) заменено тем, что scope — потомок контекста цикла
   (`create_scope(loop_ctx)`), а фабричные `FactoryOwnership::dispose` дожидаются каждого сворачивания.
8. **`SessionPreparation` не используется:** `SessionStore::prepare` возвращает `Session` без освобождаемого
   ресурса; подготовленная, но не введённая сессия просто отбрасывается.
9. **`waitForDrainingConfiguredIdentity`** — опрос раз в 5 мс реестров агентов/сессий (ограничен сигналом фабрики),
   вместо подписки на `agent/disposed` и `session/disposed`.
10. **Конфигурация.** `Config { max_parallel_tool_calls: Volatile<u64> (по умолчанию 10, минимум 1), agents }`
    (serde `camelCase`, схема `schemars`). `ConfiguredAgent` — плоская запись: `provider`, `model`, `reasoningEffort`
    (минимум 1 символ), `maxTokens` (1..2^53-1) — как в схеме образца; `AgentOptions` строится из них значениями
    `Live::Value`. Идентичности лаунчера — сервис `ConfiguredAgentIdentitiesKey` (`configuredAgentIdentities`):
    `ctx.provide` образца → `ctx.provide::<ConfiguredAgentIdentitiesKey>`.
11. **Событие `agent-loop/config-start-failed`** — `ConfigStartFailed` режима `Contained` (образец: `emit` с
    изоляцией слушателей); предупреждения слушателей — `agent "<id>": config-start-failed listener threw/rejected: …`.
12. **Переменные системного промпта `provider`/`model`** читают расширения `AssembleModelSelection` и `AssembleAgent`
    контекста сборки.
13. **Плагин** — `AgentLoopPlugin`: `NAME = "AgentLoop"`, `Deps = { agents, sessions, llm, tools, systemPrompt,
    sessionProjections }`, ключ сервиса `AgentLoopKey` (`agentLoop`). Конфигурация `agents[*]` с `resumeSessionId`
    ждёт `sessionPersistence` через `ctx.inject` (как образец).
14. **Размер ожидания.** Запуск драйвера из `send` — синхронный (`wake_driver`): проверка `Phase::Idle`, создание
    нового `watch`, `tokio::spawn`; ошибка запуска возвращается из `send`.

15. **Запрос к адаптеру: `request_message`** оборачивает `Message` в `RequestMessage::Durable`. Корень дефекта
    «запрос только с system» (`#[serde(untagged)]` у `Message`: user/assistant/tool читались как `Developer`, `LlmRuntime`
    вырезал их) исправлен в `hoh-llm`: `Message` десериализуется по `role` (тесты `hoh-llm/tests/message_role.rs`); обход
    по `role` в agent-loop удалён, остался строгий `from_value::<Message>`. Регрессии здесь:
    `the_adapter_request_carries_the_user_message_and_the_history`, `the_adapter_receives_user_messages_with_the_user_role`.
16. **Маркер `agent_loop_request`** переносится явно (`dispatch_copy`): `GenerateOptions::clone` сбрасывает его намеренно.
17. **Драйвер стартует синхронно до первого `await`** (`wake_driver` опрашивает будущее один раз, затем `tokio::spawn`):
    как асинхронная функция образца, `send` уже открыл ход и забрал inbox к возврату (`cancel` сразу после `send`).
18. **Порядок разбора при выгрузке цикла**: эффект цикла `agentLoop.scope(<id>)` принимает `scope.raw_dispose()` и
    затем `teardown` (обратный порядок: сначала teardown, потом scope), как `loopCtx.effect(function*(){ yield rawDispose; yield teardown })`.
    Ядро запускает эффекты одного узла одновременно, пока не усыновлено; усыновление даёт порядок.

19. **Замки `std::sync::Mutex` не держатся через `await`/обратные вызовы** (ревью rv-agentloop, блокер 1). Образец однопоточный,
    в Rust защитник замка во временном значении `if let`/`match`/`for` живёт до конца тела. `Lifecycle::dispose` и
    хвост `teardown` сначала вынимают `Disposer`/`detach`/`untrack` в `let`, затем вызывают. Проверка всего крейта:
    `grep -E "(if let|while let|match|for .* in) .*lock\("` — остались три `match` без `await` и обратных вызовов в теле.
    Тест: `cancel.rs::two_concurrent_handle_disposals_both_complete_on_a_current_thread_runtime`.
20. **`run_maintenance_erased` запускает работу `tokio::spawn`-задачей** (блокер 2; TS `agent.ts:204` — `async` функция, исполняется
    независимо от того, ждёт ли её вызывающий). Возврат агента в `Idle`, повтор отложенного пробуждения и
    `activity.send_replace(true)` — внутри задачи; возвращаемое будущее лишь ждёт `JoinHandle`. Падение задачи
    (`catch_unwind`) превращается в ошибку. Тест: `loop.rs::a_dropped_maintenance_future_still_returns_the_agent_to_idle`.
21. **Переходы фаз драйвера — одна критическая секция под одним замком `state`** (ревью rv-agentloop и rc-agentloop, fix2).
    - `wake_driver`: проверка фазы, запись latch и резервирование `Running` — одна секция (два параллельных пробуждения
      не запустят два драйвера); ошибка запуска откатывает фазу в `Idle`. `run_maintenance_erased`: проверка `Idle` и
      переход в `Maintenance` — одна секция.
    - `settle_idle`: чтение/сброс latch и переход `Running|Maintenance → Idle` — одна секция. Запись latch из `wake_driver`
      либо видна в ней, либо происходит после неё и застаёт `Idle` (тогда пробуждение запускает свой драйвер).
    - `cancel`: окно «очистка очереди → abort» помечено счётчиком `State::cancelling` (открывается вместе со сбросом latch,
      до `inbox.clear()`); `wake_driver` в `Running` защёлкивает пробуждение, если счётчик > 0 или сигнал уже отменён.
      Пробуждение в окне — «после отмены», как в синхронном `cancel` образца (`agent.ts:192–201`). `inject` не вызывает
      `wake_driver`, поэтому не защёлкивается и не порождает повтор (`agent.ts:171–190`). Причина `Disposed` окна не открывает.
    - Потеря пробуждения при завершении хода закрыта структурно (fix5), без меток по местам выхода: счётчик
      `State::unseen_wakes` под тем же замком `state`. `wake_driver` в `Running` под замком делает `+= 1`; драйвер сбрасывает
      его в нуль, когда «смотрит» на очередь: сразу после возврата `turn_steps` (успех, ошибка, отмена, отклонение шага — один
      момент для всех исходов; сторож `SeesQueueOnExit` делает то же на ранних выходах до шагов) и при новом резервировании;
      `settle_idle` в той же секции читает остаток как latch (`unseen_wakes > 0` → повтор, если очередь не пуста). Всё, что
      пришло после последнего «взгляда», на любом пути выхода, даёт ещё круг; работа, поставленная до «взгляда» (отказ или
      отклонение шага, `keep_inbox`), считается учтённой и не повторяется (`interception.rs` s12 зелёный). Метки
      `closing`/`ClosingOnExit` удалены. Окно «после `turn_steps`, до формирования причины и `turn/end`» входит в «после
      взгляда» (`Window::AfterTurnStepsOutcome`). Согласуется с образцом: пришедшее ДО исхода шага в TS остаётся в очереди
      (живой драйвер его «обещал»), пришедшее после исхода в TS застаёт `idle` и запускает драйвер.
    - Запоздавший wake (fix6): вставка сообщения и `wake_driver` в Rust не атомарны (отправитель может застрять в
      синхронном слушателе `AgentInboxInserted`), а в образце это один синхронный шаг. `send_impl` запоминает
      `State::settled_epoch` (число драйверов, ушедших в `Idle`) и id сообщения; `wake_driver`, найдя `Idle`, под тем же
      замком не резервирует `Running`, если между вставкой и wake драйвер уже ушёл в `Idle` И сообщения уже нет в очереди
      (его обработали) — иначе пустой лишний ход. Без драйвера между ними, или пока сообщение стоит в очереди, ход
      открывается как раньше (в т.ч. «удалённое слушателем сообщение всё равно открывает границу хода»). Работа, оставленная
      `keep_inbox`, не влияет: проверяется именно id сообщения отправителя. Тест
      `loop.rs::a_wake_delayed_after_its_message_was_handled_opens_no_empty_turn` (multi-thread, шлюз на первом `AgentPreStep`,
      блокирующий `AgentInboxInserted`; без проверки получается 3 `turn/start`).
    - То же правило в `Running` (fix7): `wake_driver` увеличивает `unseen_wakes`, только если сообщение отправителя ещё в очереди
      (`ReactLoopInbox::is_queued` под замком `state`); запоздавший отправитель, чьё сообщение драйвер уже взял, ничего не
      доставляет и не даёт лишнего круга, который съел бы последующую работу (`inject`, оставленный слушателем `agent/error`).
      Тест `loop.rs::a_wake_delayed_past_its_handled_message_leaves_later_work_queued_while_running` (без проверки — 3 хода).
    - Известное: событие `agent/status` издаётся после выхода из секции (слушатели могут вызывать `send`), порядок событий двух
      переходов из разных потоков не гарантируется (сессия сама запрещает параллельные `append`, так что обычное использование
      однопоточно по сессии).
    Тесты: `src/window_tests.rs` (15, в т.ч. «сито» `a_followup_at_any_window_of_the_driver_is_never_lost`: followup в каждой пробе, на успешном и падающем ходе; `match` по `Window` исчерпывающий, новая проба без сценария не компилируется; детерминированные: проба `Window::{CancelAfterClear, AfterReserve, WakeCheckedIdle, SettleLatchRead, AfterTurnStepsOutcome, BeforeSettleIdle, AfterTurnOutcome}` только
    под `cfg(test)`; проверено мутациями — без `cancelling` три теста падают) и стресс `cancel.rs::a_followup_racing…`.
22. **Откат resume ждёт закрытия дескриптора записи** (п. 4; TS `index.ts:813/828`). `HandleGuard::drop` (запуск закрытия
    без ожидания) удалён: `load_for_resume`/`resume_with` при ошибке выполняют `handle.close().await` до возврата ошибки.
    Тесты: `resume.rs::a_resume_failing_in_the_read_waits_for_the_deferred_close_before_the_error_surfaces` (обёртка над
    JSONL: `read` отказывает, `close` отпускает владение через 100 мс; сразу после ошибки повторный resume получает сессию;
    мутация «закрытие в `tokio::spawn`» тест роняет) и `a_resume_failing_after_the_load_…` (отказ setup).
23. **Ошибка `step/end` перекрывает ошибку шага** (п. 5; `finally` в `agent.ts:380/392`): `ended?` проверяется до `failure`.
    Тест `loop.rs::a_failure_to_close_the_step_supersedes_the_steps_own_error`: шаг падает (пустой сценарий), а проекция
    сообщений `StepEndTrap` отвергает добавление `step/end`; в `agent/error` — отказ закрытия (мутация порядка роняет тест).

## Заменено SHIM

- `SHIM(hoh-session-projection)` **заменён** настоящим крейтом: зависимость добавлена, `hoh_agent::shim` удалён
  (для `hoh-agent` это сделал я до передачи ревью; дальнейшие правки `hoh-agent` — у s16). `fold_definition` в `hoh-agent`
  строит `ProjectionDefinition` из функции свёртки.
- `hoh-tools`: используется `hoh_tools::Agent` (в крейте — свой минимальный трейт `scope_key`/`session`);
  адаптер `ToolsAgent` в `agent.rs` оборачивает `Arc<dyn hoh_agent::Agent>` (оставлен по решению ведущего).

## Не перенесено

- `tool_calls.rs` (5): «failure quiescence» ×3 (`execution-mode`/`prepare`/`complete`), «preserves the scheduler
  error when recording recovery results also fails», «isolates pending requests from child agents…», «stops new
  dispatches and drains started bodies…» — подмена приватных методов `ToolExecutionPipeline` (`prepare`/`complete`/`dispatch`);
  в Rust их нельзя заменить. Проверка недопустимого `maxParallelToolCalls` у `Tools::execute` (2) — предмет тестов `hoh-tools`;
  тест «applies the configured cap to every factory-created agent» покрыт `harness_limits` (все агенты берут лимит из плагина);
  PTC-отказ нативного инструмента (`hoh-ptc-runtime` отсутствует).
- `cancel.rs` (7): мутация объекта причины (`assignTransportStack`, `captureStackTrace`) — у JS-объекта; вместо этого
  проверена запись каждой типизированной причины; «cancel during reasoning-only streaming», «retains terminal replay
  state when cancellation races the final stream chunk», «cancel during error recovery», «retry retains the failed
  attempt», «retains a partial attempt when stream middleware rejects» — живой поток с reasoning/ретраем (нужен
  `agent/stream`-посредник и сценарий ошибок адаптера; частично покрыто `loop.rs`); «disposal from a synchronous step/start
  listener» — вариант «during scope cleanup» требует эффекта в области агента с ожиданием (`before` проверен).
- `resume.rs` (34): подмена `SessionPersistence`/`SessionHandle` заглушками (отказ закрытия, отказ создания, зависшее
  открытие), `vi.spyOn`, разбор очерёдности `session/created`/setup при resume с отказами, конфигурируемый запуск
  `agents[*].resumeSessionId` с отказами — нужны управляемые отказы бэкенда; перенесены сценарии на реальном JSONL.
- `scope_lifecycle.rs` (24): случаи, лезущие во внутренности cordis (`fiber._disposables`, метки эффектов, `symbols.effect`),
  вложенный teardown из слушателей `session/created`/`agent/created`, программный `agentEvents` для кастомных драйверов.
- `loop.spec.ts` (47): `vi.spyOn(session.append / llm.prepareCall)`, сжатие и материализация контекста
  (`hoh-compaction*` не подключён), конфигурация `reasoningEffort` по описанию модели, повтор с replay-state.
- Сжатие (compaction): отложено (подмена плагином-фикстурой не сделана: сжатие требует `hoh-compaction`, а подмена приватных `prepare`/`complete`/`dispatch` у «failure quiescence» невозможна без шпиона) — зависимости `hoh-compaction`/`agent-loop-testkit` для образца отсутствуют в этом крейте.
- Typert, Proxy-слежение за `Service` (PORTING §8).

## Не проверено

- Расхождение числа тестов образца и Rust: `it.each` считается одним случаем образца, строки — отдельными проверками.
- `cargo clippy --all-targets` по всему крейту падает на `tests/interception.rs:284` (файл s12: `MutexGuard` через `await`);
  мои файлы и `src/**` чисты (`clippy --lib --test …`).
- `hoh-agent` `tests/agent.rs::concurrent_announce_and_detach_never_leave_an_unpaired_or_misordered_lifecycle` упал
  при моём прогоне (agent.rs:725, `0/4000`) — крейт s16; здесь не исследовался.
- Паника геттеров `Inbox::next_turn/next_step` и `Model::current` при сломанной проекции (пункт 6 выше) покрыта тестом
  для `next_turn`; `Model::current` — не покрыта.

## Вопросы ведущему

1. (закрыто) `hoh_llm::Message`: десериализация по `role` сделана в `hoh-llm`.
2. Сменить `hoh_agent::Inbox::next_turn/next_step` и `Model::current` на `Result`, чтобы убрать панику геттера (п. 6)?
3. Ядро запускает эффекты узла одновременно (`unload`: `begin` всех, затем `wait`); порядок даёт только усыновление.
   Нужно ли это документировать в `hoh-kernel`?
