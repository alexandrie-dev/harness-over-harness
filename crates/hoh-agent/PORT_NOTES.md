# hoh-agent ← packages/core/agent @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/types.ts` | `src/types.rs` | `InboxTarget`, `InboxState`, `InboxWireState`, `TurnBoundaryProjection`, `ModelSelection`, `ModelSelectionProjection`; интерфейс `Agent` см. ниже |
| `src/runtime-types.ts` | `src/runtime_types.rs` | трейт `Agent` (сводит два объявления образца), `Model`, `Inbox`, `AgentOptions`, `CancelOptions`, `AgentStatus`, `PreStepDecision`, `RequestErrorAction`, `SessionStartSource`, `AssistantStreamFrame`; 12 событий `agent/*` структурами с `event!` |
| `src/index.ts` | `src/lib.rs` | `AgentRegistry` (сервис `agents`, ключ `Agents`), плагин `AgentRegistryPlugin`, `AgentFactory`, `AgentHandle`, `CreateAgentOptions`, `ResumeAgentOptions`, `AgentSetup` |
| `src/dispatch.ts` | `src/dispatch.rs` | `agent_carrier`, `agent_events` (`AgentEvents::{emit,serial,waterfall}`), `emit_agent_event`, `assemble_context_for` |
| `src/model-selection.ts` | `src/model_selection.rs` | `AgentModel`, проекция `modelSelection`, `install_model_selection` |
| `src/projection.ts` | `src/projection.rs` | проекция `turnBoundary` |
| `src/consumed-work.ts` | `src/consumed_work.rs` | `fold_consumed_work` |
| `src/archive-admission.ts` | — | не перенесено, ждёт `hoh-workspace` (см. ниже) |
| — | — | `src/shim.rs` удалён: проекции берутся из настоящего `hoh-session-projection` |
| `locale/*.json` | `locale/*.json` | скопированы без изменений |
| `README*.md`, `package.json`, `tsconfig.json`, `tsdown.config.ts` | эти заметки | прочитаны |

LICENSE и отдельно лицензированных фрагментов у пакета нет.

### Тесты

| образец → tests/*.rs | случаев образца перенесено / не перенесено |
|---|---|
| `agent.spec.ts` → `agent.rs` | 13 / 3 |
| `agent-initiator.spec.ts` → `agent_initiator.rs` | 9 / 3 |
| `consumed-work.spec.ts` → `consumed_work.rs` | 11 / 0 |
| `model-selection.spec.ts` → `model_selection.rs` | 8 / 0 |
| `archive-admission.spec.ts` | 0 / 3 |
| `verify-export-jsdoc.spec.ts` | 0 / 41 |

## Отличия от образца

1. **`Agent` — трейт `Agent`** вместо двух интерфейсов с declaration merging. Живые аксессоры
   `readonly options/model/inbox/status/ctx` — методы. Добавлен `scope_key() -> ScopeKey`: в образце сам
   агент служит ключом области (`createScope(ctx, agent)`, `scopeTarget(agent, agent)`), в `hoh-scope`
   ключ — отдельное значение. Методы, которые бросают, возвращают `Result<_, BoxError>`.
   `runMaintenance<T>` обобщён, а трейт должен быть объектно-безопасным: метод трейта
   `run_maintenance_erased(MaintenanceTask)`, типизированная форма — функция `run_maintenance(agent, task)`.
2. **Причина отмены.** `cancel(cause, options)` поднимает сигнал с причиной `CancelCauseError(AgentCancelCause)`
   (тип ошибки, потому что причина `hoh_timeout::AbortSignal` — `SharedError`). Читать:
   `signal.reason().and_then(|r| r.downcast_ref::<CancelCauseError>())`. Текст `Display` свой (в образце
   причина — простой объект).
3. **События.** Одноимённые строки образца (`agent/created`, …). Режимы: `agent/created` и
   `agent/turn-stopping` — `Serial<()>` (слушатель возвращает `Ok(None)`, чтобы цепочка продолжалась);
   `agent/pre-step`, `agent/request`, `agent/request-error` — `Waterfall<_>`; остальные — `Contained`
   (в образце это `emit` с изоляцией каждого слушателя, `agentEvents.emit`). Полезная нагрузка содержит
   `agent: Arc<dyn Agent>` и неизменна; значения, которые образец мутировал бы в нагрузке, возвращаются
   из `waterfall`. Построитель нагрузки получает агента диспетчера, но может положить в неё чужого:
   как `{ ...payload, agent }` в образце, диспетчер после построителя перезаписывает `agent` своим
   (`AgentSubjectEvent::set_agent`, реализован для всех двенадцати событий крейта; у типа вне крейта
   метод по умолчанию ничего не делает, такой тип обязан его переопределить — сейчас это
   `ApprovalRequestEvent` из `hoh-user-approval`). Тексты предупреждений о слушателях —
   `agent event "<имя>" listener threw|rejected: <String(error)>` и
   `agent "<id>": agent/disposed listener threw|rejected: <String(error)>`; `String(error)` образца —
   `<класс>: <сообщение>`: `HarnessError` даёт `<name>: <message>`, любая другая ошибка — `Error: <Display>`
   (так закреплены тесты `agent.rs`).
4. **Фильтр области.** `impl ScopeFilter for dyn Agent` пропускает всех (у образца у агента нет `Context.filter`);
   маршрутизирует только ключ.
5. **`AgentOptions`.** Поля образца могли быть живыми аксессорами; тип `Live<T>` (`Value` или `Accessor`).
6. **Методы реестра принимают `ctx: &Context`** (`register`, `enter`, `announce`, `set_factory`, `create`,
   `resume`): в образце `this.ctx` — контекст вызывающего (Proxy), kernel.md §7. `enter` и `announce`
   запоминают его для `agent/disposed`. `set_factory` возвращает `Disposer`, ошибка «второй фабрики» —
   `BoxError`.
7. **Инициатор.** `AsyncLocalStorage` → `tokio::task_local!` с цепочкой кадров, идентифицируемых
   номером реестра (несколько реестров в процессе не видят друг друга). Границы: `with_initiator` /
   `without_initiator` принимают **future** и возвращают future (граница живёт, пока future не завершён или
   не уронен; `Err` синхронно, если область закрывается или закрыта); синхронные формы —
   `with_initiator_sync`, `without_initiator_sync`. **Задача, запущенная `tokio::spawn`, инициатора не
   наследует** (в Node контекст наследуют все продолжения). Исключение собственной линии из дренажа
   (`releaseReentrantInitiatorRuns`): задачи выгрузки запускает ядро, метка задачи вызывающего до них не
   доходит, поэтому цепочку вызвавшего `Fiber::dispose()` снимает `capture_on_dispose` ядра и отдаёт
   выгрузке (вопрос ведущему 2); `release_reentrant_initiator_runs()` остаётся для границы, начинающей
   выгрузку не через `Fiber::dispose()`.
   **Правило для `tokio::spawn`.** Задача, запущенная обычным `tokio::spawn`, начинает новую цепочку
   (`current_initiator() == None`); продолжение цепочки, как у любого продолжения под `AsyncLocalStorage`,
   даёт `hoh_agent::spawn_inherited(future)`: он переносит кадр инициатора в задачу (вне границы — просто
   `tokio::spawn`). Внутри крейта `tokio::spawn` не используется; потребители крейта, запускающие задачу из
   границы и ждущие от неё атрибуции, обязаны брать `spawn_inherited`. Задача не входит в дренаж границы и
   после её конца видит того же инициатора (как отложенное продолжение в образце); `spawn_blocking`
   обёртки нет — понадобится, добавить рядом. Тест: `preserves_exact_return_values_across_await`.
8. **Закрытие при выгрузке предка.** Образец закрывает инициаторы, когда начинает выгружаться любой
   предок fiber (`internal/status` + обход `fiber.parent`). У `Fiber` нет `parent()`, поэтому цепочка
   предков читается один раз при загрузке плагина из `ctx.reflect().store()` (родитель fiber не меняется)
   и сравнивается с `event.fiber.uid()` в обработчике `internal/status`; пустой снимок закрытого ядра
   даёт цепочку из одного себя. Тест: `rejects_new_boundaries_once_an_ancestor_fiber_starts_unloading`.
9. **Проекция.** `projections.register(definition)` и `ProjectionHandle::state_of` — настоящие
   `hoh-session-projection`. Состояние читается из внешнего источника при восстановлении по checkpoint,
   поэтому схема `modelSelection` перенесена вручную как в образце (`model-selection.ts:63,76`): `lastUsed` и
   `pending` обязательны (ключ может быть `null`, но не отсутствовать), `provider`/`model` — непустые строки,
   `reasoningEffort` — отсутствует или непустая строка (не `null`); так же `viewSchema` для `lastUsed`/`next`.
   Схема `turnBoundary` (`turnBoundaryProjectionSchema`) перенесена так же: все четыре ключа обязательны
   (nullable-ключ может быть `null`, не отсутствовать), остальное проверяет `serde`; тесты
   `restore_rejects_a_turn_boundary_state_without_its_required_keys`, `restore_accepts_…`.
   `wire.view` модели — функция `model_selection_view`.
10. **Публичные символы сверх образца:** `Live`, `CancelCauseError`, `MaintenanceTask`, `MaintenanceFuture`,
    `run_maintenance`, `AgentDetach` (замыкание `enter` стало значением с `detach()` и `into_undo()`),
    `AgentSessionMeta`, `Agents`, `AgentRegistryPlugin`, `AgentRegistryDeps`, `AgentSubjectEvent`,
    `AssembleAgent`, `AssembleModelSelection` (расширения `AssembleContext` из `hoh-system-prompt`: поля
    `agent` и `modelSelection` образца), `StepBoundary`, `StepBoundaryKind`, `agent_events_with`,
    `model_selection_view`, `NO_FACTORY_MESSAGE`.
11. **Плагин** — класс-плагин образца без `export const name`: `NAME = "AgentRegistry"`, `Config = NoConfig`,
    `Deps = { sessionProjections }`. Строка загрузчика `@deepseek-ai/dsh-agent` → этот плагин; запись в таблице
    имён — за ведущим (`hoh-official-bundles`).
12. **Typert** (`typert.lookups.register('agent', …)`, `TypertLookupMap`) не переносится (PORTING §8).
13. **Гнездо внешнего исполнителя (HOH-18, сверх образца).** Трейт `SessionExecutor: AgentFactory`
    с методом `claims(ctx, id)` и методы реестра `set_executor(ctx, name, executor) -> Disposer`
    (эффект `agents.setExecutor(name)`, имя уникально), `executor(name)`, `executor_names()`.
    `create`: если маршрут провайдера (`AgentOptions::provider()`) совпадает с именем
    зарегистрированного исполнителя, сессию создаёт он, иначе — фабрика цикла, как прежде.
    `resume`: по порядку регистрации спрашивается `claims`, первый согласившийся продолжает
    сессию. Если никто не согласился (или исполнителей нет), реестр читает начало сохранённого
    журнала (`stored_executor_binding` через `sessionPersistence`, страницами по 16 событий до
    привязки или до первого события хода — `turn/*`, сообщения, `tool/result`; ревью 221315 №5:
    записи слушателей `session/created` перед привязкой могут быть в любом числе): сессия
    с записью привязки `plugin:external-harness/session` получает отказ
    «session belongs to external harness <имя>» — цикл не продолжает журнал, который писал внешний
    исполнитель, даже когда пакет исполнителя не загружен. Иначе — фабрика цикла, как прежде.
    Отличие от прежнего пути: перед `resume` цикла теперь всегда есть чтение начала журнала
    (сессия не сохранена или нет службы хранения — чтение пропускается). Ошибка хранилища
    (`stat`, `open`, `read`, в том числе испорченный журнал) — отказ `resume` с ошибкой хранилища
    как есть (её же вернул бы `resume` цикла), а не «не привязана»: журнал, который не удалось
    проверить, циклу не отдаётся (ревью 232500 №3, fail closed; `stored_executor_binding` возвращает
    `Result<Option<String>, BoxError>`, `claims` гнезда передаёт ошибку дальше). Тест
    `a_log_that_cannot_be_read_is_not_handed_to_the_loop` (файлы хранилища без прав на чтение);
    на прежнем коде падает. Для этого добавлена зависимость `hoh-session-persistence`
    (цикла зависимостей нет); в тестах — `hoh-session-persistence-jsonl`.
    Имя записи, число событий для поиска и разбор (`EXECUTOR_BINDING_RECORD`,
    `EXECUTOR_BINDING_SCAN`, `bound_executor`) объявлены здесь, `hoh-external-harness` берёт их
    отсюда. Новых полей в `CreateAgentOptions` / `ResumeAgentOptions` нет (их строят литералами
    в других крейтах). Реализация — `hoh-external-harness`; тесты — `tests/executor.rs`.
14. **Исполнитель новых чатов (HOH-18, решение основателя 20.11, сверх образца).**
    `select_executor(Option<ExecutorSelection {executor, model}>)` и `selected_executor()`:
    выбор человека в окне (`hoh/executor/select`, привилегированная команда; зовёт только
    `hoh-external-harness`). Пока выбор есть, `create` новой сессии верхнего уровня (без
    `parent_session`, без `origin`, без seed) отдаёт её выбранному исполнителю, подставив
    `provider` = имя исполнителя и `model` = выбранная модель. Если исполнитель не загружен,
    `create` отказывает («the executor "…" chosen for new chats is not loaded»): тихой подмены
    моделью движка нет. Подагенты, форки и без выбора — как прежде. Тест
    `the_selected_executor_takes_new_chats_and_leaves_subagents_to_the_loop`.

## Не перенесено

- `src/archive-admission.ts` и `tests/archive-admission.spec.ts` (3 случая): используют события
  `workspace/session-activity`, `workspace/session-stop` пакета `hoh-workspace`, которого нет среди перенесённых
  (в образце это необязательный peer). Функция `installTurnArchiveAdmission(ctx, lookup)` тривиальна и
  добавляется вместе с `hoh-workspace`; зависимости на тип `SessionActivity` у нас пока нет.
- `tests/verify-export-jsdoc.spec.ts` (41): проверка комментариев документации исходников — генератор/линтер
  документации (PORTING §10).
- `agent.spec.ts`: «contributes Agent lookup and scoped Context providers while Typert is live» (Typert),
  «canonicalizes an already traced Service before tracing it for the caller» (Proxy `Service`),
  «exposes the closed typed cancellation cause at the Agent seam» (проверка типов).
  «injects the fused subject even when the payload carries a conflicting agent field» перенесён
  (`agent_events_injects_the_fused_subject_even_when_…`, плюс вариант для `serial`).
- `agent-initiator.spec.ts`: «tracks a branded Promise without calling its overridable then property»,
  «preserves a settled branded Promise when its species blocks observer construction», «drains cross-realm
  Promise boundaries before disposal» — особенности `Promise` JavaScript.
- Экспорты, которыми не пользуется ни один переносимый пакет: `emitAgentEvent`/`agent_events_with` (используются
  только в самом пакете и в `hoh-agent-loop`), `AssistantStreamFrame` используется `hoh-agent-loop`; пакеты
  `client`, `api`, `host` не переносятся.

## Не проверено

Ветвей платформ в пакете нет. Не покрыто тестом: `AgentRegistry::resume`/`create` с настоящей фабрикой
(тесты работают с заглушкой; настоящая — `hoh-agent-loop`), закрытие инициаторов при выгрузке предка (п. 8).
Параллельность `with_initiator` между потоками runtime проверена только однопоточным `#[tokio::test]`.
Гонка `announce`/`detach`: окно между поиском записи и пометкой флагов закрыто общей блокировкой реестра.
Проверка детерминированная, без планировщика: `src/announce_window_tests.rs` (в окне блокировка реестра занята),
`tests/agent.rs` — три порядка (detach первым → `not live`; announce первым → `created`, `disposed`; detach из
другого потока во время диспетчеризации `created` → удаление и `disposed` после неё).

## Вопросы ведущему

1. ~~SHIM(hoh-session-projection)~~ — снят: `src/shim.rs` удалён, зависимость настоящая.
2. ~~Выгрузка из initiator-границы (P1 ревью rv-agent, п. 1)~~ — закрыто через ядро (s16, 19.11).
   Причина: `Fiber::dispose` лишь кладёт команду актору, а выгрузку и очистку эффектов запускают задачи
   ядра, куда `tokio::task_local!` вызывающего не переходит; реестр не мог отличить границу, ждущую
   выгрузки, от границы, ждущей внешнего события (её дренаж обязан дождаться). Теперь реестр при загрузке
   регистрирует `ctx.capture_on_dispose(|| INITIATOR…)`: `Fiber::dispose()` снимает цепочку инициатора
   в задаче вызывающего, а `dispose_initiators` читает её через `hoh_kernel::dispose_captures::<Frame>()`
   и сам освобождает эти границы (`release_chain`); пока дренаж идёт, он ждёт и
   `ctx.next_dispose_captures`, поэтому второй `Fiber::dispose()` / `Kernel::shutdown()` из другой границы,
   пришедший позже, тоже исключается; `fiber.disposer().dispose()` передаёт цепочку в cleanup-задачу,
   где идёт `child.dispose()`. Описание механизма и обоснование реестра функций
   снятия (поле ядра, не процессный) — PORT_NOTES `hoh-kernel`, раздел «Добавлено s16». Ручной
   `release_reentrant_initiator_runs()` остаётся для границы, начинающей выгрузку иным путём
   (например, путь, не проходящий через `Fiber::dispose()`, `Kernel::shutdown()` и `Disposer::dispose()`). Тесты: три «does not self-deadlock …»,
   «excludes an asynchronous teardown initiator …» и `does_not_self_deadlock_without_the_manual_release`
   идут без ручного вызова.
3. ~~`Fiber::parent()`~~ — не нужен: цепочка предков читается из `ctx.reflect().store()` (п. 8 отличий).
4. Для `hoh-tools`/`hoh-agent-loop`: `Agent` — трейт с `scope_key()`; расширения `AssembleAgent`,
   `AssembleModelSelection` лежат в `hoh-agent::dispatch`.
