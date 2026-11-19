# hoh-session ← packages/core/session @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs`, `src/session.rs`, `src/store.rs`, `src/events.rs`, `src/validation.rs` | Session, SessionStore, публикация, владение, restore/adopt/snapshot, plugin records |
| `src/types.ts` | `src/types.rs` | бренды позиций, метаданные, открытые события и причины, surface intent |
| `src/surface.ts` | `src/surface.rs` | полный и оконный fold, замены, исторические ссылки, чистые проекции |
| `src/request-header.ts` | `src/request_header.rs` | canonicalHeader, headerEquals, foldRequestHeader |
| `src/tool-history.ts` | `src/tool_history.rs` | исторические определения, серии, отложенные объявления |
| `src/repair.ts` | `src/repair.rs` | ToolCallRecovery, оба вида closers, исходные тексты и идентификаторы |
| `src/fork.ts` | `src/fork.rs` | точный включительный префикс, inherited marker до closers |
| `src/seq-ranges.ts` | `src/seq_ranges.rs` | прибыльные диапазоны и проверяемое развёртывание |
| `src/preparation.ts` | `src/preparation.rs` | точная Session и однократный release |
| `src/known-event-types.ts` | `src/known_event_types.rs` | оба исходных build-static списка, без генератора TypeScript |
| `locale/*.json` | `locale/*.json` | все 8 файлов без изменений; diff не показывает различий |
| `README.md`, `package.json` | эти заметки, Cargo manifest | прочитаны; root, types, fork и surface доступны |

Отдельного LICENSE и отдельно лицензированного фрагмента у пакета нет.
Изменения ограничены `crates/hoh-session`. Источник, корневой manifest и чужие crates не правились.

### Тестовые файлы

Случай здесь — один исходный вызов `it`/`it.each`, без умножения на строки параметризации.
Строки переносимых таблиц проверяются циклами. Всего в 21 `*.spec.ts` — 341 такой случай.

| образец → tests/*.rs | случаев образца перенесено / исключено | пояснение |
| --- | --- | --- |
| append → append.rs | 11 / 3 | исключены sparse/Proxy/getter |
| canonical-envelopes → canonical_envelopes.rs | 17 / 2 | undefined и TypeScript-only case |
| derived-cache → derived_cache.rs | 6 / 0 | scratch oracle и неизменяемая identity |
| developer-header → developer_header.rs | 9 / 0 | все исторические binding checks |
| fork → fork.rs | 22 / 0 | 21 integration; private corruption case — unit в session.rs |
| gen-persistence-catalog | 0 / 27 | генератор TypeScript, §10 PORTING |
| json → json.rs | 6 / 11 | оставшиеся состояния JS либо глубина вне §4 PORTING |
| message-projections → message_projections.rs | 6 / 0 | атомарность, windows, удаление интерпретатора |
| plugin-records → plugin_records.rs | 8 / 0 | namespace, freeze/snapshot, replay/fork |
| preparation → preparation.rs | 2 / 0 | release и Drop |
| properties → properties.rs | 6 / 0 | 256 воспроизводимых логов на каждое свойство |
| repair → repair.rs | 16 / 0 | обе причины, все четыре точных текста |
| request-header → request_header.rs | 10 / 0 | folding и equality |
| scoped → scoped.rs | 12 / 0 | рассылка root без receiver filter, по указанию задания |
| seq-ranges → seq_ranges.rs | 4 / 0 | включая 1 000 001 source sequence |
| sequence-types → sequence_types.rs | 11 / 0 | runtime-части смешанных TS/type cases |
| session → session.rs | 67 / 10 | исключения перечислены ниже |
| store-membership → store_membership.rs | 6 / 3 | три getter-only сценария |
| surface → surface.rs | 56 / 0 | оба исходных markerless guard cases имеют отдельные тесты |
| tool-history → tool_history.rs | 9 / 0 | включая hoh-llm project_tool_updates |
| typert | 0 / 1 | транспорт исключён §8 PORTING |

Итого: **284 перенесённых исходных случая, 57 исключённых**. Дополнительно `serialization.rs`
содержит 10 собственных регрессий: байтовая запись, сохранение extensions, dispatch snapshots,
тождество immutable messages, порядок ошибок constructor/restore и однократная admission проекции.
Это собственные регрессии, не исходные случаи.
`append.type-test.ts` и `plugin-records.type-test.ts` не входят в число 341 и не переносятся (§10).

## Отличия от образца

### Публичный API для следующих писателей

Один plugin: `SessionPlugin`, исходное имя **SessionStore**, сервисный ключ `Sessions`, имя сервиса
**sessions**. Config — `hoh_kernel::NoConfig`, обязательных inject нет. Typert lookup не создаётся.
Методы с владением эффектами получают `ctx: &Context` явно (§7 kernel.md):

```rust,ignore
let sessions = ctx.get::<hoh_session::Sessions>().unwrap();
let session = sessions.create(&ctx, Some("id".into()), Default::default())?;
let event = session.append("user/message", serde_json::to_value(message)?,
    Some(hoh_session::SurfaceIntent::append()))?;
let messages = session.derive_messages()?;
sessions.flush(&session).await?;
```

`Session::create(id, seed, header, inherited_event_count, projections)` создаёт detached instance;
`seed: None` отличается от `Some(&[])`. `from_restore` принимает Arc событий и сохраняет их
тождество; обычный create копирует события. `SessionPreparation` вызывает release при явном dispose
или Drop ровно один раз. `SessionDetach::detach()` синхронен и идемпотентен; `into_undo()` передаёт
его упорядоченному составному эффекту. Для advanced lifecycle: `prepare` → `enter(&ctx, session)` →
adopt detach undo → `announce(&session)`. До announce и после detach запись запрещена, включая пустой batch.

`SessionEvent` — открытый тег и ordered raw JSON, не общий enum с событиями других пакетов.
Методы `event_type`, `seq`, `time`, `data`, `as_value`, `surface_op`, `source_event_seqs`, `message`
заменяют JS properties. `from_value` принимает сырой storage envelope; Session валидирует его до приёма.
`llm_message()` при необходимости переводит сообщение в импортированный `hoh_llm::Message`;
для lossless replay хранится исходный Value. Сообщения, request/context, EpochHeader, ToolHistory
возвращаются как `Arc<Value>`, чтобы не терять неизвестные поля и их позиции. `ToolHistoryProjection::llm_snapshot`
даёт типизированный вид для LLM. Source-типы сообщений реэкспортированы из `hoh-llm`, не объявлены повторно.

Открытые карты SessionEventMap, PluginRecordMap, TurnEndReasonMap как механизм declaration merging
не существуют в Rust (§4/8): события и причины остаются raw JSON, а владельцы расширений задают свои payloads.
`SessionId` реэкспортирован из `hoh_brand::ids`; SessionSeq и SessionLogOffset — разные private-field newtypes
с проверяющими `new(u64)`, `from_number(f64)` и serde, не голые числа. SessionSeqCursor — закрытая пара
BeforeFirst/Seq с JSON -1/номер; OptionalSessionSeq — Option с JSON null/номер.
Ошибки числовых конструкторов и диапазонов — SessionError::TypeError (`name() == "TypeError"`),
остальные простые Error — SessionError::Message. SessionForkError остаётся собственной plain Error,
с отдельным enum кодов, а не HarnessError. Произвольные fallible listeners сохраняют BoxError без обёртки.

Перегрузки append выражены двумя методами: `append(type, Value, Option<SurfaceIntent>)` и
`append_batch(&[SessionEventInput])`. Input::with_options(Value) — raw граница для runtime metadata checks;
читаются только два исходных surface-поля. Требования surface intent и запрет assistant sources
проверяются при приёме, включая raw seeds. TypeScript conditional types не воспроизводятся через
большой закрытый enum чужих событий. `SurfaceOp` — закрытый serde enum; `AppendOp`, `ReplaceOp`, `ReplaceTag`
— Rust-имена его вспомогательных форм. Optional-поля отсутствуют, а явный null в сыром JSON сохраняется.

`Session::surface()` возвращает read-only живой `SessionSurface`; его `nodes()`, `replace_generation()`
и `content_generation()` читают актуальное committed состояние. Сохранённый view продолжает видеть изменения;
выданный Vec номеров — собственный снимок вызывающего. `SurfaceSnapshot` — данные оконного SurfaceManager.
Менеджер владеет своим окном, поэтому новые события принимает `commit_batch`; `prepare_batch` возвращает
PreparedSurfaceBatch с `commit(&mut manager)` вместо JS callback, вызываемого после внешнего log.push.
`SurfaceManager::new` сразу валидирует и сворачивает переданное окно через Result; исходный standalone
manager делает этот первый fold лениво при чтении. Для Session порядок приёма seed/header сохранён отдельно.
`validate_next` кеширует решение кандидата; не принятый кандидат не удерживает интерпретатор как использованный.
`MessageProjections` — разделяемый список определений; `sessions.message_projections()` возвращает именно
живой список для detached replay. Реализация SessionMessageProjection — чистый trait с context events/nodes/base_seq/messages.

Immutable events и messages доступны по Arc; внутренний JSON не выдаётся изменяемой ссылкой.
Copy-on-write клонирование собственного Value разрешено и не меняет журнал. Object.freeze не нужен;
getters, prototypes, sparse arrays, Proxy, циклы и произвольные JS throw values в Value не представимы.
Стабильные derived-message Arc кешируются также на событии. Снимок event array кешируется до append;
одиночный append планирует delta через временные Arc-снимки committed log/fold без копирования истории;
изолированная копия fold нужна лишь сложному batch с replace/projection, как в source prepareBatch.
План применяется после всех dispatch validation. Короткие mutex-секции
не проходят через await или пользовательские callbacks. Глобальных mutable реестров нет.

### События и containment

Типы событий: SessionCreated, SessionDisposed, SessionEventEmitted, SessionFlush; исходные имена
`session/created`, `session/disposed`, `session/event`, `session/flush`. Все используют kernel Contained
и **ctx.root().dispatch**, как исходный `ctx.events.dispatch('emit', ...)` без receiver.
Owner context используется для effect ownership и logger, но его filter не применяется.
Это явное указание задания: ожидания scope-фильтрации старых scoped.spec.ts заменены проверками
фактической unfiltered рассылки, включая другого scoped listener.

Для синхронного creation listener используется `SessionStore::on_created(&ctx, callback)`:
Err немедленно ставит veto, последующие адаптированные listener bodies не вызываются.
`on_created_async` запускает prefix через kernel и логирует отказ future без rollback.
`SessionEventEmitted` и `SessionDisposed` слушаются обычными `ctx.on_contained` /
`ctx.on_contained_async`; сбои каждого observer содержатся отдельно после commit.
Для awaitable flush применяется **SessionStore::on_flush(&ctx, callback)** или синхронный
on_contained<SessionFlush> с `event.wait_for(future)`. wait_for опрашивает async prefix inline,
затем flush ждёт все futures и возвращает первую ошибку в registration order с исходным типом/identity.
Регистрировать flush как fire-and-forget on_contained_async неправильно: его future принадлежит
kernel observer, а не durability checkpoint. Результат bool сообщает наличие listeners.

`Context::on_dispatch` не возвращает Result, а panic содержит ядро. Ожидаемый source throw в
инструментации выражается `event.validation.reject(error)`, проверяемым **до** log push; для disposal
отказ только логируется. Это явный Rust error channel вместо JS исключения. На direct panic действует
политика ядра; копии шины событий или изменение kernel не понадобились. Direct raw sync on_contained
creation listeners не дают first-veto skip для других raw listeners; для source-контракта нужен on_created.
Весь listener snapshot batch разрешается до первого push; все события и финальная surface видны
первому observer. current_batch_publication ставится только на время выдачи batch.

### JSON, тексты и дополнительные символы

Порядок нового append: type,seq,time,data,sourceEventSeqs?,surfaceOp?; plugin record заканчивается ignorable.
У recovery порядок другой, как в TS: type,seq,time,data,surfaceOp,sourceEventSeqs?. Его message:
id,role,toolCallId,isError,source,content; data: turn,step,message,error. Header store:
version,id,createdAt,cwd?,parentSession?,isSeeded,origin?,delegationDepth?,agentPreset?.
Исходный порядок raw seeds и вложенных extensions не пересобирается. JSON-текст пишет
`hoh_util_values::js_json_stringify`; сами Session API не создают JSONL-файлов. Date.now представлен
Unix epoch milliseconds; ISO-строк в этом пакете нет. isAbsolute берётся из общего node_path helper.
Тексты обеих причин repair и четыре сообщения риска сохранены дословно. Locale не переводился.

Дополнительные публичные Rust-символы: Result, SessionError, SessionPlugin, Sessions, SessionDetach,
DispatchValidation, четыре payload-типа событий, MessageProjections, SurfaceSnapshot, PreparedSurfaceBatch,
MAX_SAFE_INTEGER, helper-типы SurfaceOp и их описанные конструкторы/аксессоры. Это представление
исключений, сервисного ключа, implicit JS ownership и callable-disposer API. Новых настроек нет.

Проверены прямые imports в переносимых packages/**/src, без client/api/typert/host/experimental.
Не имеют прямых imports (но сохранены как runtime/API либо учтены выше): CreateSessionOptions,
RestoredSessionOptions, PrepareSessionOptions, TurnEndCancelCause, RequestHeaderReason, SessionEventInput,
PluginRecordType/Map/Record, TurnEndReasonMap, SessionSurface, SurfaceFoldReplacement/Result,
SessionMessageProjectionContext, SessionPreparationOptions, SessionForkSource/ErrorCode/Error,
appendPluginRecord, pluginRecordOf, buildForkSeed, foldRequestHeader, encodeSeqRanges, DeveloperMessage.
Механизм TS-карт сам по себе не считается runtime-экспортом; его данные не потеряны.

## Не перенесено

Только следующие исходные тесты / языковые механизмы:

- Все 27 gen-persistence-catalog и один typert case: §8/10 PORTING. Генератор, registry lookup transport
  и их TypeScript AST fixtures не являются Rust runtime этого пакета. Обе type-test.ts также исключены.
- append: sparse input arrays, Proxy append wrapper, getter, дописывающий событие во время snapshot.
- store-membership: чтение getter при disabled writes; detach внутри getter; adoption внутри getter.
- canonical-envelopes: own header.system со значением undefined (такого Value нет); conditional TS type assertions.
- session: reads_each_seed_array_entry_once; reads_a_nested_seed_data_getter_once;
  rejects_exotic_seed_metadata; rejects_an_exotic_seed_event_shell; accepts_a_null_prototype_seed_event_shell;
  reads_a_nested_seed_metadata_getter_once; adds_seed_context_when_validation_throws (spy Object.hasOwn);
  reads_a_nested_append_data_getter_once; rejects_exotic_surface_metadata;
  reads_a_nested_append_metadata_getter_once. Остальные JSON/scalar подслучаи выполнены через Value;
  -0 остаётся представимым и проверяется, NaN/Infinity/functions/BigInt/cycles/prototypes — нет.
- json: foreign realm containers; WebKit constructor formatting; once-only getters; captured getter keys;
  deleted array slot; Proxy index schedule; fractional Proxy length; exotic/sparse/cyclic containers;
  throwing getter; отдельный isJsonValue exotic/sparse/cycle case. Все исключены как JS-only (§8).
  Одиннадцатый — 5000 уровней nesting: §4 PORTING запрещает строить Value глубже 4096;
  это проверка общего util-values, не отдельная реализация JSON snapshot здесь.

## Не проверено

Полный dsh conformance corpus не запускался: этот crate даёт session boundary, а не агент/модель/JSONL backend.
Платформенных process.platform branches в src нет; проверка cwd использует POSIX node_path helper.
Windows-native Node path.isAbsolute (drive/UNC) отдельно не проверен и не реализуется собственным кодом Session.
JS-only состояния, перечисленные выше, не объявляются выполненными Rust runtime-проверками.
Property generators воспроизводимы, но не имеют fast-check shrinking; отдельный JS runtime не запускался.

## Вопросы ведущему

По итогам повторного ревью остаётся зависимость от ядра; вопрос стека закрыт решением ведущего:

1. `hoh-kernel/src/events.rs:138–234`: поля Callbacks private, единственный invoke всегда продолжает
   loop после sync Err/panic; callback report возвращает (). Для source `index.ts:1378–1390`
   нужен kernel API, прекращающий snapshot на первом синхронном отказе и возвращающий исходный BoxError,
   но сохраняющий prefix/ownership/logging асинхронных observers. Raw listener не проходит через
   session adapter, поэтому `on_created` не закрывает этот контракт. Шину ядра не копировали и не меняли.
2. Закрыто решением ведущего по PORTING §4: рекурсивные Value::clone/eq/drop разрешены
   на стеке приложения 64 МиБ; отдельные stack-safe helpers не нужны. Тест depth_4096
   снят с ignore и выполняет append, seed, fork, derive, сравнение и уничтожение всех
   глубоких значений внутри std::thread::Builder::stack_size(64 * 1024 * 1024).
   На стандартный test stack глубокие значения не передаются.
   Проверен apps/hoh/src/main.rs: run запускается на отдельном потоке со stack_size
   64 МиБ; Tokio new_multi_thread задаёт thread_stack_size 64 МиБ. В Tokio 1.54.1
   эта настройка применяется также к blocking pool. Правки l1 не требуются;
   приложение и зависимости w7 не менял.

С ignore и прежней причиной остаётся только raw_creation_veto_stops_the_next_raw_listener.
Ожидающий вопрос ведущему: kernel stop-on-sync-error API. Контракт глубины/владения JSON закрыт.
Следующим писателям важно соблюдать on_created/on_flush/validation.reject adapters выше:
они выражают source exceptions и awaitable checkpoint поверх существующего kernel Contained.

## Проверки

Все Cargo-команды выполнены offline, только для своего crate, с четырьмя build jobs:

```sh
CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w8 cargo test -p hoh-session
CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w8 cargo clippy -p hoh-session --all-targets -- -D warnings
CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w8 cargo fmt -p hoh-session -- --check
```

Первоначальный этап до повторного ревью: полный `cargo test -p hoh-session` завершился с exit 0 после пересборки hoh-llm:
**294 passed; 0 failed; 0 ignored** суммарно по всем бинарям. Unit: 1; integration: 293
в 20 файлах (включая 56 surface и 10 serialization); doc-tests: 0. Это 284 исходных случая
и 10 дополнительных регрессий. Все строки `test result` — `ok`, с нулём failed/ignored/filtered out.

Финальный `cargo clippy -p hoh-session --all-targets -- -D warnings` после test-прогона
завершился с exit 0, без предупреждений. `cargo fmt -p hoh-session -- --check` завершился с exit 0.
`git diff --check -- crates/hoh-session` не находит whitespace errors;
`diff -ru` исходного и перенесённого locale не находит различий. В src нет todo!, unimplemented!
или debug-print macros.

Итог повторного ревью и повторных проверок записывается ниже; прежние 294 tests не являются
результатом того fix round. На том этапе открытыми оставались raw creation veto и глубина на 2 MiB;
последующее решение ведущего о стеке 64 МиБ записано выше и в разделе depth ниже.

## Повторное ревью w8

| пункт | сверка с source | исправление / состояние |
| --- | --- | --- |
| 1 | surface.ts:520–536, 643–652: validateNext без кандидата; prepareBatch с candidates | Session планирует одиночный delta по committed log; только batch projection получает log+candidates |
| 2 | index.ts:1378–1390: sync throw обрывает loop | подтверждено; блокируется Callbacks API, вопрос выше |
| 3 | index.ts:882–886: все snapshots до createAppendEvent | payload validation перенесена после snapshot всех members; member 1 с -0 выигрывает у invalid member 0 |
| 4 | repair.ts:116–117, 157–194; fork.ts:26–30 | raw seq/time читаются без admitted-only expect; только новые seq проходят SessionSeq; исходный timestamp копируется, отсутствующий не превращается в null |
| 5 | index.ts:972–990, surface.ts:550–577 | Arc planning views + validated SurfacePlan, commit без полного log/fold clone; простые batches тоже delta-only |
| 6 | known-event-types.ts:23–95 | source имеет **63**, не 64, sorted names; image/offload есть в KNOWN и является единственным элементом projection set; оба списка pinned |
| 6 depth | user: 4096 data, default 2 MiB | воспроизведён SIGABRT stack overflow на append; дальнейшие seed/fork/derive/Drop/equality в этой цепочке не объявляются проверенными; общий JSON gap выше |
| 7 | fork.spec.ts:181–254, 285–417, 464–471; append.spec.ts:109–131; repair.spec.ts:210–233 | восстановлены exact error texts, nonempty last seq, live parents, полные marker/turn-end JSON bytes, reentrant warning и полный closed-step closer |

Red evidence перед исправлениями:

- review_fixes: `test result: FAILED. 0 passed; 4 failed` — пункты 1/2/3/4.
- 100000 flat events: `FAILED. 0 passed; 1 failed`, 15.03 s, только 41443 appends.
- fork: `FAILED. 18 passed; 3 failed` после добавления source assertion, запрещающего seed marker в live parent.
- depth_4096: signal 6 SIGABRT, `fatal runtime error: stack overflow, aborting`.

Новый performance test использует 100000 событий (50000 surface appends и 50000 log-only),
bound **15 секунд** в debug build. Предположение: хотя бы одно доступное x86-64 ядро >=2 GHz,
без длительного descheduling; рабочие данные помещаются в RAM. Это проверка runtime, без дискового I/O.
Отказавшая baseline была даже легче (только log-only events). Выигрыш не получен ослаблением нагрузки.

Замечание review №5 про `version: 4.0` дополнено по rv-catalog и указанию ведущего:
публичный Session принимает и отдельно сконструированный `Value::from(4.0)`, как
TS `record.version === SESSION_FORMAT_VERSION` (core/session/src/index.ts:106).
Проверка сравнивает числовое значение без coercion: строка "4" и число 4.5
по-прежнему отвергаются с прежними текстами. Регрессия
`validates_header_version_as_a_javascript_number` покрывает integer/float 4 и
оба отказа; прежний parser regression сохранён. Другие проверки этим исправлением не менялись.

Focused fix-round command (с тем же offline/JOBS=4/TARGET_DIR окружением выше):

```sh
cargo test -p hoh-session --test review_fixes --test known_event_types --test fork --test append --test repair
```

Exit 0: append `11 passed; 0 failed`; fork `21 passed; 0 failed`; known_event_types `1 passed; 0 failed`;
repair `16 passed; 0 failed`; review_fixes **`6 passed; 0 failed; 2 ignored`**, 1.55 s для всей последней
suite, включая 100000 mixed appends. Суммарно **55 passed; 0 failed; 2 ignored**. Игнорируются ровно
два подтверждённо failing reproducer зависимостей, не исходные cases и не объявленные исправленными баги.
Новый rollback/reentry regression также прошёл: failed append освобождает planning views до disposal callbacks.

Повторный `cargo clippy -p hoh-session --all-targets -- -D warnings` завершился с exit 0,
без warnings. Повторный fmt --check прошёл.

Финальный полный fix-round command:

```sh
CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w8 cargo test -p hoh-session
```

Exit **0**. Суммарно по 24 result lines (включая doc-tests): **302 passed; 0 failed; 2 ignored**.
Unit: **2 passed**; integration: **300 passed, 2 ignored** в 22 файлах; doc-tests: **0**.
Все 284 перенесённых source cases остались passing. Новый review_fixes suite:
`test result: ok. 6 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out`, **1.83 s**,
включая 100000 mixed appends. Surface 56, session 67, projections 6, serialization 10 — все passed.

Пункты 1/3/4/5/7 исправлены и проверены. Данные обоих catalog sets уже соответствовали source;
добавлена полная фиксация их содержимого. **Не завершены пункт 2 и глубина из пункта 6**:
их failing reproducers сохранены с ignore, причины и требуемые dependency API указаны выше.
Никакая замена стека на 64 MiB в этих тестах не выполнялась. Чужие crates и source не редактировались.

## Исправление числовой версии по rv-catalog (w3)

По отдельному указанию ведущего изменено только сравнение version в
src/validation.rs; добавлен validates_header_version_as_a_javascript_number в
tests/review_fixes.rs. Принимаются 4 и отдельно сконструированный Value::from(4.0),
как TS core/session/src/index.ts:106. Для 4.5 и строки "4" проверены прежние
полные сообщения отказа. В catalog добавлен прямой вызов обоих current-валидаторов
с float 4.0 и пустым artifact. Другую логику Session это исправление не меняет.

Все команды: CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w3.
`cargo check -p hoh-session -p hoh-session-format-catalog --tests`: exit 0, 5.93s.
`RUST_TEST_THREADS=2 cargo test -p hoh-session -p hoh-session-format-catalog`:
exit 0, Finished test profile, 64m 22s. Session — 304 passed, 0 failed, 1 ignored;
catalog — 32 passed, 0 failed, 0 ignored. Новый Session regression прошёл в
review_fixes: `test result: ok. 8 passed; 0 failed; 1 ignored`; новый catalog
regression — в current: `test result: ok. 3 passed; 0 failed; 0 ignored`.
Суммы — сумма строк Cargo, а не отдельная строка его вывода.
`cargo clippy -p hoh-session -p hoh-session-format-catalog --all-targets -- -D warnings`:
exit 0, без диагностик; финальный повтор по текущему дереву — 29m 32s.
`cargo fmt -p hoh-session -p hoh-session-format-catalog -- --check`: exit 0.

Параллельная правка depth-теста на application stack была внесена другим
исполнителем во время ожидания прогона и мной не менялась. Единственный ignored
текущего прогона — прежний kernel raw-creation-veto reproducer. Проверки этого
отдельного исправления version полностью завершены; исторические результаты
предыдущих кругов выше не переписывались.

## Depth: решение ведущего по PORTING §4

Последующее решение закрывает прежний блокер глубины: рекурсивные операции над Value
разрешены при стеке приложения 64 МиБ, stack-safe clone/eq не добавляются.
`depth_4096_append_seed_fork_derive_and_drop_on_the_application_stack` выполняется без ignore:
весь прежний сценарий, включая создание и Drop глубоких значений, перенесён внутрь
`std::thread::Builder::stack_size(64 * 1024 * 1024)`. Основной тестовый поток получает только ().
Ignore и тело `raw_creation_veto_stops_the_next_raw_listener` не изменены.

Read-only проверка apps/hoh/src/main.rs: `main` запускает `run` на потоке со стеком
64 МиБ, а Tokio `new_multi_thread` задаёт `thread_stack_size(64 * 1024 * 1024)`
для worker threads и blocking pool. Требуемая настройка уже есть, действий l1 не нужно.
apps/hoh не редактировался и не запускался.

Проверки этого круга выполняются с CARGO_NET_OFFLINE=true, CARGO_BUILD_JOBS=2,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w7 и явным `cargo --offline`.

`cargo --offline test -p hoh-session -- --quiet --test-threads=2`: exit 0,
суммарно 24 result lines: **304 passed; 0 failed; 1 ignored**. Depth 4096 выполнен,
единственный ignored — прежний raw listener veto, ожидающий kernel API.
Все 284 ранее перенесённых source cases сохранены; добавленное здесь покрытие —
тот же depth regression, теперь на согласованном стеке, без ослабления тела теста.
`cargo --offline clippy -p hoh-session --all-targets -- -D warnings`: exit 0,
`Finished dev profile [unoptimized + debuginfo] target(s) in 19m 34s`.
`cargo --offline fmt -p hoh-session -- --check`: exit 0, без вывода.
Логи: /tmp/w7-depth-test.log и /tmp/w7-depth-clippy.log.
