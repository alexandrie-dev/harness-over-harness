# hoh-session-projection ← packages/session/session-projection @ 39163dd3

## Файлы

| Образец | Перенос | Состояние |
|---|---|---|
| src/index.ts | src/lib.rs, src/registry.rs, src/registration.rs | Registry/plugin/service, eager drive, lazy cells, retained readers, ref-counted registration, snapshots, checkpoint/restore/hydrate, change feeds и warning vocabulary |
| src/types.ts | src/types.rs | Open client table и summary vocabulary; type-only TS marks не имеют runtime-сущности |
| JSON state/JS thread semantics | src/state_json.rs, src/owner.rs | Undefined/null, primitive/object identity, один synchronous owner с re-entry и фиксированным Session cut |
| Недостающий weak Session API | src/shim.rs | Приватный отмеченный SHIM(hoh-session), описан ниже |
| tests/registry.spec.ts | tests/registry.rs | 44 исходные группы, включая все 960 state/view/listener сценариев; 3 дополнительные Rust-регрессии |
| tests/registration-reader.spec.ts | tests/registration_reader.rs | Все 9 групп / 11 параметризованных случаев |
| tests/summary.spec.ts | tests/summary.rs | Все 8 случаев |
| tests/compatibility.spec.ts | tests/compatibility.rs | 14 из 16 групп / 16 из 18 случаев; два JS-only случая исключены ниже |
| Test helpers | tests/support.rs | Изолированный Kernel, Context owners, source-equivalent unit definitions и точные cursors |
| tests/registration.type-test.ts | Не переносится | Проверка типов TypeScript, PORTING §10 |
| tests/loader-compatibility.e2e.ts, tests/fixtures/* | Не переносится | Source/lib npm Loader + TypeScript plugin smoke; граница другого runtime, PORTING §8/10 |
| locale/{de,en,es,fr,ja,pt,ru,zh}.json | locale/ | Все восемь файлов скопированы без изменений; diff -rq exit 0 |
| README.md, package.json | PORT_NOTES.md, Cargo.toml | Прочитаны, ./types outlet сохранён как public types module |

В четырёх исходных spec — 77 групп / 81 развёрнутый runtime-случай. Представлены
75 групп / 79 случаев; параметры двух reader it.each и трёх legacy effect styles
сохранены в циклах. С тремя дополнительными регрессиями это 78 Rust test-функций.
Все исходные src, tests, fixtures, README и package exports прочитаны.

## Отличия от образца

- SessionProjectionRegistry одновременно native Plugin и cloneable instance service;
  SessionProjections — kernel key с исходным именем `sessionProjections`.
  NAME плагина — SessionProjectionRegistry, Config — kernel NoConfig, Deps — ().
  new(ctx) ставит listeners; публикацию сервиса выполняет Plugin::apply. Методы,
  создающие эффекты или пишущие caller warning, явно принимают &Context — вместо
  Cordis service Proxy. События SessionCreated/SessionEventEmitted импортированы
  из hoh-session, не объявляются повторно. Creation veto использует on_created adapter.
- ProjectionDefinition<S> содержит synchronous Result callbacks. Host state — Arc<S>,
  S: Serialize + Send + Sync; возвращение прежнего Arc сохраняет object identity.
  Для разных Arc primitive identity проверяется по JSON виду (включая знак нуля),
  а новый объект/массив остаётся изменением при равных полях. Raw wire view —
  Option<Arc<Value>>; для object-valued view нужно переиспользовать Arc. Проверенный
  результат viewSchema передаётся key/event listeners как одна общая immutable Arc.
  State schema вызывается только там, где исходник её вызывает; init/apply не
  получили дополнительного schema parse. Схемы — функции разбора по DEPS.md zod:
  владелец unit переносит strip/default/refine/transform и видимые тексты отказов.
  Registry не изобретает общую схему и не оборачивает ошибки unit в HarnessError.
- Option::None на корне state означает JavaScript undefined; явный JSON null —
  Value::Null (либо unit/null вариант собственного serde enum). state_json
  сохраняет различие при checkpoint: val=None отсутствует при сериализации,
  val=Some(Null) остаётся null. Custom deserializer различает отсутствие val и
  явно записанный null. Root serialization не изменяет вложенную JSON структуру.
  Nullable state не следует кодировать Option::None, если требуется именно null.
- stateVersion и checkpoint ver — u64; источник отказывает non-safe integers.
  Общий checker сохраняет текст и верхнюю safe-integer границу. Исходные -1/1.5
  не представимы публичным полем Rust: тест проверяет их через тот же admission
  checker, а публичный register — через MAX_SAFE_INTEGER+1. Sequence/offset/cursor
  используют исходные newtypes из hoh-session, watermark -1 не смешивается с offset 0.
- ProjectionRegistration<S> — native Future с dispose() и into_undo() вместо
  callable JS thenable. Установка синхронна; handoff асинхронен и проверяет owner
  activation. Явный dispose до await безопасен, повторная выдача через &mut Future
  сохраняет уже выданный результат. into_undo передаёт оригинальный kernel effect,
  поэтому предыдущий awaited cleanup сохраняет регистрацию до своего завершения.
  Наблюдение rejection — poll Future; ignored rejection сообщает owned ctx.spawn
  после runtime yield, только для Loading/Active owner. Это native counterpart
  Node setImmediate/microtask handoff, без реализации JS Promise runtime. Успешный
  handoff фиксируется только при опросе Future после обязательного первого yield,
  а не конкурирующей spawn-задачей: `register(); owner.dispose()` всегда отказывает,
  тогда как завершённый handoff остаётся успешным после выгрузки owner. `OnceLock`
  хранит уже принятый вердикт; фоновой остаётся только запись ненаблюдаемой
  синхронной ошибки установки. Если успешная установка не была опрошена до выгрузки
  owner и Future затем вообще не опрашивают, Rust не создаёт и не логирует отказ
  handoff: native Future ещё не исполнялся. В исходнике уже созданный Promise после
  microtask отвергается и среда сообщает unhandled rejection. Это намеренное отличие
  native Future от eager JS Promise; отдельная задача ради неопрошенного успеха снова
  внесла бы устранённую межпоточную гонку.
- Заголовок Session читается проекцией через `typed_header`: целые числа в форме
  float (`createdAt: 1.0`, `delegationDepth: 2.0`) hoh-session принимает как JS
  `Number.isSafeInteger`, поэтому перед разбором в `SessionHeader` они приводятся
  к целым (index.ts:842 передаёт заголовок в `init` без разбора).
- PORTING §1 (решение ведущего): ключ проекции `"__proto__"` не становится own
  property ни в одном plain-object результате. В образце присваивания
  `rows["__proto__"]`/`values["__proto__"]` вызывают сеттер прототипа, и JSON даёт
  `{}` (index.ts:525/553/612/680/722/750/751/791). Общий helper применяется в
  snapshot/cached snapshot, checkpoint, viewCheckpoint, restore и hydrate; wire
  view и schema при этом вычисляются, но значение не вставляется. Внутреннее typed
  state всё равно сохраняется. Это проверяют тесты checkpoint/restore и
  `proto_wire_values_are_omitted_from_every_plain_object_projection_face` для
  примитивного и объектного view.
- Последний release только выключает live participation. Registry хранит released
  predecessor и его последнюю cell; retained handle продолжает ту же cell, пока
  новая регистрация не заменит key в таблице. Старый handle и после этого читает
  свою исходную definition. reader_for лишь связывает точный Session, без init,
  storage, новых subscriptions и отдельного кеша. Explicit disposal идемпотентен.
- State tables/listeners защищены короткими mutex-секциями; synchronous operations
  имеют одного reentrant owner между потоками. Глобальных mutable registries нет,
  блокировки не проходят через await. Pure unit transitions синхронны; перед
  внешними view/listener callbacks table/cell mutex освобождён. Один read захватывает
  immutable Session log cut, чтобы последовательные unit values и asOfSeq совпадали
  и при native concurrent append. Listener table сохраняет JS Set identity и live
  insertion/deletion iteration; registrations — IndexMap с исходным порядком.
- Checkpoint values — detached JSON copies; restore/hydrate используют внутренние
  typed states без повторного schema parse. Restore floor остаётся one-below anchor,
  suffix replay проверяет missing seq, неверная/overreaching row при baseSeq>0
  требует re-read from seq 0. Hydrate не откатывает cell, уже прошедшую observation cut.
  Snapshot selection не трогает unselected/host-only units; cachedSnapshot не fold-ит.
  Summary set — стабильная Arc<IndexSet>, invalidation при изменении key table.
  Event-level feed выполняется как finally, даже при ошибке позднего unit/key listener.
- Дополнительные Rust-символы: Result/ProjectionError (plain Error), SessionProjections,
  ProjectionWire и callback aliases, ProjectionReader, ProjectionRegistration,
  ProjectionChange, ProjectionRestoreResult. Это имена anonymous source shapes,
  error channels, service key и native effect/Future representation, не новые настройки.
  Open map — ordered JSON, TS summary marks остаются aliases; runtime решает только
  wire.summary (ProjectionWire::new даёт исходный default true). Native wire outputs
  — JSON values; JS prototypes, getters и Promise objects не являются Value.

Проверены src-потребители, включая agent/agent-loop, goal, session-title,
working-directory, session-stats/turn-outline, session-reference, projection-cache,
todo/context contributors. Все runtime exports сохраняются. Типовые marks/table
exports нужны клиентским агрегатам как словарь; TS declaration merging не
воспроизводится runtime-механизмом (PORTING §4/8), закрытого общего enum чужих unit нет.
Механическая проверка imports в 46 внешних переносимых src-файлах не нашла direct
named imports ProjectionChangeListener, ProjectionEventChangeListener,
ProjectionCheckpointRow, SessionProjectionSummaryExcludedKey, SessionProjectionSummaryValues,
SessionProjectionViewValue и SessionProjectionViewValues. Их контракты сохранены;
inline callbacks и client-only aggregates не считаются отдельными runtime consumers.

## Не перенесено

- compatibility: rejects_an_async_plugin_that_returns_the_legacy_registration_as_a_disposer
  и leaves_an_async_generator_yield_owned_by_the_plugin_instead_of_the_composed_effect.
  Они проверяют JS await-ассимиляцию callable thenable в недопустимый Cordis effect.
  В Rust apply обязан вернуть Result<(),Error>, а composition принимает Undo;
  неправильный return/yield не компилируется. Native retained cleanup и transfer
  проверены оставшимися случаями, включая задержку более раннего cleanup.
- Promise instanceof/function/toStringTag и callable syntax assertions заменены
  Future/explicit dispose/into_undo semantics; TS type-test исключён по §10.
- Один registry case с Promise-valued view адаптирован к валидному JSON неверной
  формы: parser всё так же отказывает до выдачи view. Async view/apply не проходят
  Rust callback signature. Никакой async unit не добавлялся.
- Loader source/lib npm smoke и TypeScript fixtures не исполняются и не копируются
  в runtime crate; kernel ownership сценарий покрыт native compatibility tests.

## Не проверено

Настоящий Node/TS Loader, JavaScript microtask timing и полный dsh conformance corpus
не запускались. Реальный persisted-cache backend, minimal profile и replayed model
с read/write/bash собираются соседними пакетами; этот registry не запускает агент.
Платформенных process.platform веток в src нет. Windows/macOS здесь не запускались.
Сложные invalid custom Serialize implementations и JS executable/prototype objects
выходят за source plain-JSON synchronous-unit contract.

## Вопросы ведущему

- SHIM(hoh-session): текущий Session даёт exact equality и stable header Arc, но
  не публичную WeakSession/lifecycle identity. Приватная функция session_identity
  берёт weak header Arc как identity; разные prepared lifecycles с одинаковыми
  id и metadata не сливаются (отдельная регрессия). Cells не удерживают Session.
  Если отдельный header Arc удержан после удаления Session, cell может жить дольше;
  weak pruning выполняется лениво. После принятия hoh-session заменить shim на
  публичную weak lifecycle identity. Код зависимости не копировался и не менялся.

## Второй круг ревью

- Handoff больше не может выиграть гонку на другом worker до следующего шага
  вызывающего кода. Многопоточный
  `fix2_unpolled_registration_cannot_handoff_on_another_worker` сначала воспроизвёл
  преждевременный успех; теперь порядок `register; dispose` даёт отказ, а явно
  завершённая передача переживает последующую выгрузку.
- `restore()` исключает `__proto__` и из wire snapshot, и из обновлённого checkpoint,
  сохраняя только внутреннее состояние unit. Host-only счётчик версии 1, состояние
  0 и пустой лог теперь совпадают с исходным `{}`.
- Все остальные плоские `values` используют тот же общий plain-object helper.
  Регрессия с wire view `5` и объектом проверяет snapshot, cached snapshot,
  viewCheckpoint и оба пути hydrate; до исправления первый snapshot возвращал
  `{"__proto__":5}`.

## Проверки

Все Cargo-команды: CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w3. Во время написания только
cargo check --tests; финальные проверки выполняются после завершения кода по
plans/briefs/interface-first.md.

- `cargo check -p hoh-session-persistence -p hoh-session-projection --tests`:
  exit 0, Finished dev profile, 9m 51s; текущие API зависимостей приняты без правок.
- `RUST_TEST_THREADS=2 cargo test -p hoh-session-persistence -p hoh-session-projection`:
  exit 0, projection `78 passed; 0 failed; 0 ignored`, включая 960 сценариев matrix;
  doc target без случаев также успешен. Совместная test-сборка заняла 20m 21s,
  выполнение projection suite — 0.30s.
- `cargo fmt -p hoh-session-persistence -p hoh-session-projection -- --check`:
  exit 0, без вывода.

- Первый финальный Clippy обнаружил один `collapsible_if` в `same_view`:
  вложенная проверка Arc identity объединена в let-chain, условие и результат
  прежние. Fmt --check после этой style-only правки снова exit 0; runtime suite
  повторно не запускалась (interface-first: tests один раз в конце).
- `cargo clippy -p hoh-session-persistence -p hoh-session-projection --all-targets -- -D warnings`:
  финальный exit 0, Finished dev profile, 6m 47s, предупреждений нет.

Проверка второго круга теми же переменными окружения:

- `RUST_TEST_THREADS=2 cargo test --offline -p hoh-session-persistence -p hoh-session-projection --no-fail-fast`:
  exit 0; persistence `20 passed; 0 failed`, projection `83 passed; 0 failed`,
  doc targets без случаев успешны; всего 103/0.
- `cargo clippy --offline -p hoh-session-persistence -p hoh-session-projection --all-targets -- -D warnings`:
  exit 0, Finished dev profile за 13.85s, предупреждений нет.
- `cargo fmt -p hoh-session-persistence -p hoh-session-projection -- --check`:
  exit 0, без вывода.

Дополнение по ревью fix2persist4:

- Новый `proto_wire_values_are_omitted_from_every_plain_object_projection_face`
  до исправления получил `{"__proto__":5}` и завершился 0/1; после общего helper
  focused-прогон завершился 1/0. Сценарий проверяет primitive/object wire view в
  snapshot, cached snapshot, viewCheckpoint и hydrate.
- `cargo test --offline -p hoh-session-projection -p hoh-session-persistence`:
  exit 0; projection `84 passed; 0 failed`, persistence `20 passed; 0 failed`,
  doc targets без случаев успешны; всего 104/0.
- `cargo clippy --offline -p hoh-session-projection --all-targets --no-deps -- -D warnings`:
  exit 0, Finished dev profile за 9.72s, предупреждений нет.
- `cargo fmt -p hoh-session-projection -- --check`: exit 0, без вывода. Полный
  `cargo fmt --check` также exit 0 после появления временно отсутствовавшего тестового
  модуля чужого `hoh-sandbox-policy`; чужие файлы не менялись.
