# hoh-tool-schedule ← packages/schedule/tool-schedule @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | четыре инструмента, схемы, проекции результатов, представления вызовов и секция системного промпта перенесены |
| `tests/harness.ts` | `tests/tool_schedule.rs` | собран настоящий kernel-стенд с JSON storage, persistence, Schedule, Tools и scoped mount |
| `tests/tool-schedule.spec.ts` | `tests/tool_schedule.rs`, unit-тесты в `src/lib.rs` | основные случаи образца сгруппированы в 23 Rust-теста; исключения перечислены ниже |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json`, `tsconfig.json` | эти заметки и `Cargo.toml` | прочитаны, видимый контракт и состав пакета учтены |

LICENSE в пакете образца отсутствует.

## Отличия от образца

- `hoh-tools::Agent` отдаёт `Session` и `ScopeKey`, но не `agent.ctx`. Поэтому композиция пресета
  запрашивается в контексте, в котором смонтирован `ToolSchedule`. Если сервис `AgentPresets` там есть,
  его результат используется буквально, включая `None`; сохранённый `session.header.agentPreset`
  применяется только для глобального/безреестрового монтажа, где сервиса нет. Это отдельно проверено
  регрессией случая «сервис есть, composed preset отсутствует».
- `hoh-tools::Agent` не отдаёт исходный `options.subagentDepth`. Канонический Rust `Session` уже запрещает
  отрицательный и дробный `delegationDepth` при создании/восстановлении
  (`hoh-session/src/validation.rs`), поэтому невалидного вызывающего из случая образца нельзя собрать через
  публичный API. Защитная проверка инструмента всё равно возвращает точные
  `SubagentError/INVALID_DEPTH` и текст образца, если инвариант Session когда-либо будет ослаблен; это
  проверено напрямую на `-1` и `1.5`. Полное совпадение именно с runtime options потребует добавить это
  поле в чужой trait `hoh-tools::Agent`.
- В тесте уже отменённого batch общий Rust pipeline останавливает вызов раньше тела инструмента и возвращает
  канонический `AbortError/ABORTED_BEFORE_DISPATCH`; образец вызывает `definition.execute` напрямую и видит
  скрытый `internal_error`. Само тело Rust также скрывает ошибку Schedule после начала dispatch, но внешний
  конструктор `ToolExecution` намеренно закрыт. Проверено, что отменённые create/edit не меняют Schedule.
- Динамический `ctx.inject(['schedule'], ...)` образца перенесён kernel-инъекцией: без Schedule инструменты
  и guidance отсутствуют, при появлении сервиса восстанавливаются, а disposal монтажной области удаляет их.
- Нативный вывод использует `js_json_stringify`; поля служебного учёта и `lastDelivery.messageId` скрываются
  перед моделью, как в образце.

## Не перенесено

- PTC-вариант общей инструкции и копируемого RRULE-примера: `packages/core/tool-executor-ptc` помечен как
  `redesign` и не имеет Rust-крейта (`docs/porting/MAP.tsv`, строка 303). Native-схемы отдельно проверяют
  RRULE-only текст, отсутствие authorization и точную строку примера.
- End-to-end `lastDelivery` через настоящий `Schedule::run`: ему нужен работающий `AgentLoopKey` и модельный
  testkit; `hoh-agent-loop-testkit` ещё не готов и сам зависит от PTC redesign. Проекция настоящего
  `ScheduleCatalogEntry` с `ScheduleDeliveryReceipt` проверена напрямую, включая удаление `messageId`.
- Инъекция произвольного отказа `list`/`view` и следующей записи storage: Rust сервис `Schedule` конкретный,
  kernel не позволяет подменить его mock-объектом, а JSON backend не имеет `failNextWrites`. Общая ветка
  `run_for_caller` скрывает все ошибки, кроме явно разрешённых `ScheduleInputErrorCode`; разрешённые коды и
  их точные тексты проверяются через настоящий сервис.
- README и npm/TypeScript build-проверки намеренно не переносятся.

## Не проверено

- Живая доставка через модель и создание новой Session для `new`/`new_once`: нет готового
  `hoh-agent-loop-testkit`. Точное построение `ScheduleTargetRequest`, различие `new`/`new_once`, перенос
  `cwd`, отсутствие лишних creation values и правила fallback пресета проверены unit-тестами.
- Отказ файловой системы во время записи schedule storage и платформы кроме Linux.

## Вопросы ведущему

- Для оставшихся end-to-end случаев нужны публичный runtime `subagentDepth` в `hoh-tools::Agent`, готовый
  `hoh-agent-loop-testkit`/PTC executor и тестовый failure hook для `Schedule` либо storage. Изменять эти чужие
  API в рамках этого крейта запрещено.
