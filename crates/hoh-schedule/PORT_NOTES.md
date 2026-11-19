# hoh-schedule ← packages/schedule/schedule @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/types.ts` | `src/types.rs` | перенесён |
| `src/domain.ts` | `src/domain.rs` | перенесён |
| `src/delivery-history.ts` | `src/delivery_history.rs` | перенесён |
| `src/storage.ts` | `src/storage.rs` | перенесён |
| `src/update.ts` | `src/update.rs` | перенесён |
| `src/runtime.ts`, `src/index.ts` | `src/service.rs` | перенесены |
| `src/client.ts` | — | Typert, исключён по §8 |
| доменные spec | `tests/domain.rs` | перенесены, родственные табличные случаи сгруппированы |
| storage/history spec | `tests/storage_history.rs` | перенесены основные доменные контракты |
| service/runtime spec | `tests/service.rs`, `src/service.rs` | перенесены основные контракты и регрессии ревью |

## Отличия от образца

- Внутренние методы сохраняют типизированные `ScheduleInputError`, а
  `remoteCreate`, `remoteList`, `remoteHistory` и `remoteEdit`, как TS,
  преобразуют только их в `RemoteError` с кодом `schedule/input-invalid` и
  detail `{code}`. Неожиданные ошибки проходят без преобразования.
- Числовой `delivery_limits()` оставлен внутренним accessor для принятого
  `hoh-tool-schedule`; публичный контракт `schedule.deliveryLimits` представлен
  `remote_delivery_limits()` и сериализуется объектом
  `{newSessionIntervalSeconds}`.
- `scheduleDomain` строится как `Arc<DomainSpec>`, так как его
  создание в Rust может вернуть ошибку.
- Вместо Web Session controller используется `hoh-agent::Agents`.
  Как и в `runtime.ts:465-468` и
  `api/session-controller/agent.ts:313-317`, schedule оставляет `Agent`, но не
  копит `AgentHandle` и не вызывает disposer после idle: жизненным
  циклом владеет `Context` (`hoh-agent:572-590`).
- Наличие Rust-замены `sessionController` проверяется по опубликованному
  имени сервиса `agentLoop` через `Context::has`, как исходный код проверяет
  строковый ключ `ctx.get('sessionController')`; конкретный тип сервиса
  расписанию не нужен.
- JSON-декодер `weekly.weekdays` совпадает с TS. Типизированный
  Rust API принимает `Vec<u8>`, поэтому отрицательное/дробное
  число отклоняет serde до доменной функции.
- `edit` читает часы отдельно для правила и `updatedAt`; наблюдаемый
  контракт совпадает.
- `NewOnce` в `resolve_target`, как `index.ts:634-636`, проверяет только
  наличие фабрики Session и не вызывает `stat` для только что созданного
  случайного id. Полная проверка цели выполняется позднее внутри
  сериализованного `create`/`edit`.
- Свежая reservation повторяемой цели `new` сразу передаётся `createAgent`;
  `stat`/resume выполняются только при восстановлении уже сохранённой
  reservation. Ошибка recovery после удержания execution-slot всегда снимает
  slot, включая отказ `Session.flush`, поэтому повторный recovery возможен.
- Явно переданная в `edit` цель повторно проверяется под operation-lock, даже
  когда равна сохранённой: архивирование/удаление между внешней проверкой и
  записью не проходит гонку.
- Wire-методы `remoteList` и `remoteHistory` принимают отдельные DTO с `f64`,
  чтобы отрицательные и дробные JSON-числа доходили до проверки
  `Number.isSafeInteger`-эквивалента и возвращали штатный
  `schedule/input-invalid`, а внутренние методы сохраняли `u64`.
- Между due-группами и перед расчётом следующего таймера `drive_once` повторяет
  проверку `stopping`. Наблюдатель исполнения после остановки только освобождает
  execution-slot. Ошибка форматирования локального времени новой Session
  оставляет её без имени и пишет то же предупреждение, что TS.
- Тестовый переключатель отказа batch-delete существует только под
  `cfg(test)` и позволяет запускать публичные ветки `delete_inactive` и
  `stop_session_tasks`, не меняя production API. Наличие фабрики Session
  тестируется настоящей регистрацией сервиса с именем `agentLoop`.

## Не перенесено

- `src/client.ts`: transport Typert по §8; remote-методы остались
  публичными методами сервиса.
- `delete-inactive.perf.ts` не является `*.spec.ts`.
- Послучайные названия TS-тестов не повторяются один к одному там, где
  несколько `it.each` перенесены одной Rust-матрицей. В частности,
  `daily_source_matrix_*`, `weekly_source_matrix_*`,
  `cron_source_matrix_*`, `rrule_source_matrix_*` и
  `update_source_matrix_*` проверяют нормализацию, строгую будущность,
  дробные секунды, устойчивое декодирование и все 49 преобразований видов
  расписания.
- Не перенесены проверки, требующие полноценного model-loop исполнения:
  `execution.spec.ts` (ожидание idle и падение выполнения),
  `serial-delivery.spec.ts` (ворота исполнения) и model-loop части
  `recovery.spec.ts`. В текущем графе крейтов такой запуск требует рабочего
  `hoh-agent-loop`; доменная логика `run`, recovery и сериализации покрыта
  управляемыми Agent/persistence doubles без модели.
- Не повторены как отдельные restart-сценарии все варианты из
  `domain-restart.spec.ts`, `target-restart.spec.ts`,
  `target-recovery.spec.ts` и `update-restart.spec.ts`: те же wire-формы и
  проверки восстановления покрыты прямым reopen в `storage_history.rs` и
  декодированием сохранённых записей в `domain.rs`.

### Точная инвентаризация оставшихся service/runtime сценариев

Ниже приведены точные названия отсутствующих самостоятельных TS-тестов из
файлов, отмеченных ревью. Матрица `it.each` указана вместе со всеми вариантами.
Сценарии, закрытые регрессиями этого круга, сюда не входят:
`creates no Session for a new-target task that a clock rollback made future
before its target resolution` →
`clock_rollback_filters_future_tasks_before_target_resolution` и
`stops between committed tasks without arming a new timer (1/2 due tasks)` →
`close_between_due_groups_does_not_reserve_the_next_new_session`.

- `targets.spec.ts`, доставка/пол исполнения: `delivers two tasks of one
  Session in one message again after a manual run held one of them`, `limits a
  frequent recurring new-Session task to one creation per configured interval`,
  `launches each occurrence of a new-Session task while the earlier launches
  still run`, `admits a manual run of a new-Session task while its earlier
  launch still runs`, `wakes the idle earlier Session of a new-Session task and
  still launches the due occurrence`. Отложены до связки с рабочим model-loop:
  им нужны одновременно running/idle Agent и несколько незавершённых запусков.
- `targets.spec.ts`, архивные цели: `refuses a manual execution whose session
  target Session is archived`, `... new_once ...`, `... new ...`, `keeps a due
  session task while its target Session is archived and delivers after
  unarchiving`, `... new_once ...`, `... new ...`, `runs a per-occurrence task
  in a fresh Session while its earlier reminder waits in an archived Session`.
  Отложены как полные сценарии Web/workspace-архива; чистая ошибка и её remote
  detail покрыты `archived_target_failure_keeps_remote_code_and_session_detail`.
- `targets.spec.ts`, Workspace/имя: `joins a created Session to the Workspace
  at its directory and names it after the task`, `names each per-occurrence
  Session with its launch minute in the task time zone`, `leaves a created
  Session outside every Workspace when no Workspace owns its directory`,
  `delivers when the Workspace join and the naming of a created Session fail`.
  Отложены до единого интеграционного fixture Workspace+SessionTitle; ветка
  ошибки формата имени отдельно покрыта
  `invalid_new_session_time_zone_skips_the_title_and_warns`.
- `targets.spec.ts`, переход цели: оба варианта `creates a new-once Session when
  Web/persistence reports that its reserved id is absent` (в том числе
  `new_once` → `session`) отложены до Web activation provider; сохранение
  reservation/recovery покрыто управляемыми doubles.
- `runtime.spec.ts`: `segments long waits and cancels the timer on disposal`,
  `coalesces management wakes while a delivery is waiting for Session
  resolution`, оба варианта `keeps one timer after wakes during
  resolution/flush and none after disposal`, оба варианта `preserves the
  previous daily target, status, and receipt when flush/commit fails`.
  Отложены: первые четыре проверяют fake-timer topology JS runtime, последние
  два требуют инъекции отказа между Session durability и storage commit.
- `management.spec.ts`: `rejects stale human edits without overwriting a newer
  revision`, `rejects a stale delete and removes the task once the current
  revision is named`, `advances optimistic revisions even when two writes share
  one clock millisecond`, `keeps a pending per-run Session identity across
  descriptive edits and drops it on a rule or target change`. Отложены до
  отдельного сценарного fixture управления; соответствующие ветви
  `expectedUpdatedAt`, монотонного `updatedAt` и `pendingDelivery` перенесены,
  но пока проверяются только доменно/косвенно.
- `subagent-ownership.spec.ts`: `refuses to create a reminder for a live
  subagent Session and stores nothing`; `refuses to create for a live child
  with an explicit subagent origin`; `refuses to update a stored reminder bound
  to a delegated child Session`; `refuses a delegated child recorded only by
  its delegation depth`; `still creates a reminder for a top-level Agent`;
  `still lists and deletes a stored reminder bound to a delegated child
  Session`; `refuses a task targeting a live subagent Session without reading
  its saved header`; `refuses a task targeting a stored Session whose saved
  header records a delegated child`; `refuses to retarget a task to a stored
  delegated child Session and keeps the stored target`; `still creates a task
  targeting a stored top-level Session without activating it`. Все десять
  отложены до fixture с реальным metadata/header ownership; production-проверки
  перенесены, но не считаются тестовым покрытием.
- `change-notice.spec.ts`: `adds notices queued while the Agent is idle to the
  first step of its next turn`; `adds one step to a turn whose last step was
  running when a notice arrived`. Оба отложены: требуют рабочего model-loop с
  границами turn/step; FIFO самой очереди покрыт
  `notification_chain_is_fifo_even_when_only_the_tail_is_polled`.

Чтобы инвентаризация не зависела от толкования «основного контракта», ниже
перечислены и все остальные самостоятельные service/runtime сценарии этих пяти
spec-файлов, не имеющие очевидной полной пары в Rust. Они считаются
**неперенесёнными как сценарные тесты**, даже если отдельная ветвь уже покрыта
узкой регрессией.

- Остаток `targets.spec.ts`: `delivers to a designated Session and retains an
  ended receipt`; оба варианта `keeps a stored new_once/new task and reports
  failure when its Host cannot create Sessions`; `persists a pending identity
  before creating a new Session for an occurrence`; `runs per-occurrence tasks
  immediately in fresh Sessions without moving their timer`; `delivers the
  latest missed occurrence once and keeps the next occurrence on its time`;
  `keeps minutely occurrences on time when each Session flush takes time`;
  `holds only the occurrence that falls inside the interval of a manual run`;
  `falls through from an unavailable restart receipt to a fresh execution`;
  `refuses a manual execution whose target Session resolves after shutdown
  starts`; `reuses a pending per-run identity and forwards both optional
  creation fields`; `reuses a pending new-Session identity for a later manual
  retry`; `retries a failed receipt write in the Session that the first attempt
  created`; оба варианта `creates no Session when the reserved identity has a
  Web/persistence ownership error`; `retries in the created Session whose Agent
  did not resolve after creation`; `keeps the timer of a failed member that
  restoration moved back into the future`; `delivers the latest occurrence
  after a target resolution that spans several periods and advances past it`;
  `delivers to the Session that an earlier delivery attempt created and keeps
  its title`. Причина: это многоточечные сценарии activation/create,
  durability, recovery и времени; имеющиеся doubles проверяют отдельные
  инварианты, но не полный порядок эффектов каждого сценария.
- Остаток `runtime.spec.ts`: `appends after stored history in arrival order and
  keeps the change time monotonic through wall-clock rollback`; `keeps
  reminders unacknowledged when no Session activation provider is mounted`;
  `uses the embedding activation handler instead of the Web controller`;
  `restores the original Session and retains an ended one-shot with its flushed
  message receipt`; `delivers one latest recurring occurrence after downtime
  and retains the fixed-rate next target`; оба варианта `rearms a future target
  after clock rollback and returned/rejected restoration failure`; `keeps an
  advanced batch member armed while an unpersisted member waits for each next
  drive`; `keeps the daily task identity and rule across two deliveries with
  distinct message receipts`; `keeps a failed enqueue without a retry timer`;
  `keeps the task when Session durability fails after enqueue`; `publishes a
  daily advance only after the Session flush completes`; `drains an in-flight
  resolution on disposal without enqueueing after stop`; `batches same-Session
  recurring tasks once while keeping other Session obligations separate`;
  `batches daily and fixed-rate occurrences in one Session after downtime`;
  оба варианта `rearms a after/at task when the clock rolls back during Session
  resolution`; `delivers only still-due members of a mixed batch and rearms its
  future member after clock rollback`; `rearms a mixed batch without enqueue or
  commit when every member becomes future during resolution`; `keeps a task
  when the Session controller returns a declared resolution error`; `keeps an
  enqueued task when the Session reports no durability acknowledgment`;
  `retires a recurring task after the final representable occurrence`; `retains
  the final daily occurrence as the ended target without rearming`; `logs a
  rejected serialization operation and honors a wake requested before
  retirement`; `contains recomputation admission refusal between ancestor
  unloading and runtime cleanup`; `drains a rejected serialization promise
  during disposal`. Причина: полные тесты требуют управляемого runtime clock,
  activation controller и точечных отказов enqueue/flush/commit/serialization;
  Rust сейчас имеет только узкие doubles для соответствующих ветвей.
- Остаток `management.spec.ts`: `keeps committed mutations successful when a
  change observer throws`; `updates title, prompt, rule, target and lifecycle
  atomically, then deletes globally`; `filters and paginates active and inactive
  tasks in operational order`; `rejects a missing designated Session before
  storing the task`; оба варианта `rejects an archived target
  without activation, live Agent: false/true`; оба варианта `refuses to re-arm
  an archived session/new_once target but permits pausing and descriptive
  edits`; оба варианта `checks target availability inside the create/edit FIFO
  slot`; оба варианта `re-arms a reserved target without a creation provider
  only when its Session exists: false/true`; оба варианта `refuses a queued
  new_once/new destination when its creation provider unloads`; `preserves an
  unexpected list failure across the Remote transport`; `orders historical
  ended tasks by creation time or the saved occurrence when timestamps are
  absent`; `lists all target kinds in operational order with stable tie
  breakers`; `describes the rule that owns dispatch of each Session-bound task
  and lists those rules for their Session`; `retargets any timing kind without
  converting its authoritative rule`; `returns not found for unknown task
  updates and deletions`; `tells live involved Sessions about committed changes
  without activating stored ones`; `tells other live involved Sessions about
  Session-bound updates and deletions without activating stored ones`; `stops
  the Host tasks of an archived Session and tells the other live involved
  Sessions`; `commits runtime delivery through the service and exposes its
  receipt`; `queues one manual execution until whole-agent idle and consolidates
  repeated requests`; `resumes a stored RFC-valid RRULE without reapplying the
  stricter new-rule authoring check`. Причина: узкие management/domain тесты
  покрывают операции и wire-формы, но не эти полные сочетания lifecycle,
  уведомлений, Agent idle и конкурентного unload.

Прямые пары из тех же файлов, поэтому не отнесённые к пропускам:
`reports the effective new-Session delivery interval ...` →
`remote_delivery_limits_are_a_camel_case_object_for_supported_configurations`;
`manages Host tasks globally with Host-generated ids` →
`management_create_list_view_and_delete_are_durable_operations`; `rejects
invalid rules ... pagination ...` →
`remote_management_methods_wrap_only_schedule_input_failures` и доменные
матрицы; `rejects durable key/record identity mismatches and mutations after
shutdown` → `start_preserves_a_key_mismatch_when_closing_the_bad_domain_also_fails`
и `plugin_teardown_closes_the_service_before_future_mutations`.

## Не проверено

- Нет. Отказ `Domain::close` после mismatch и отказ `delete_many` теперь
  проверяются инъекцией backend/табличного отказа через настоящие ветки
  `start`, `delete_inactive` и `stop_session_tasks`. Отмена во время
  persistence `stat`, фильтрация будущей задачи до резерва Session и пол
  интервала после ошибки доставки также проверяются на уровне сервиса.

## Вопросы ведущему

- Нет; правки API чужих крейтов не требуются.
