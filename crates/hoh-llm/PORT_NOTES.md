# hoh-llm ← packages/llm/llm @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs, src/runtime.rs, src/error.rs | реестр, подготовка вызова, discovery, адаптерная граница, сервис и события |
| src/types.ts | src/types.rs, src/wire.rs | публичная модель данных, serde-имена, lossless ordered snapshots |
| src/brand.ts | src/brand.rs | пять newtype; общий SessionId реэкспортирован из hoh-brand::ids |
| src/message.ts | src/message.rs | роли, происхождение, конструкторы, identity, bounded summary |
| src/content.ts | src/content.rs | файлы, изображения, бюджеты, история инструментов |
| src/assembler.ts | src/assembler.rs | сборка, закрытие блока, interruption, pruning replay metadata |
| src/assistant-stream.ts | src/assistant_stream.rs | упаковка, строгая валидация, развёртывание и читатели |
| src/call-config.ts | src/call_config.rs | controls, сравнение, метка loop-built request |
| src/retry-policy.ts | src/retry_policy.rs | normal/always, исходные defaults, bounds, порядок отказов |
| src/error.ts | src/error.rs | все коды и retry choices, классификаторы, цепочки причин |
| src/adapter-failure.ts | src/adapter_failure.rs | нормализация HarnessError, переносимые facts, UNKNOWN для чужих ошибок |
| src/api-key.ts | src/api_key.rs | точный JS trim, printable ASCII, безопасная диагностика |
| src/attribution.ts | src/attribution.rs | User-Agent, launch suffix, исходная версия 0.2.2-rc.3 из Cargo metadata |
| tests/adapter-failure.spec.ts | tests/adapter_failure.rs | 3 исходных случая; 2 исключения ниже |
| tests/api-key.spec.ts | tests/api_key.rs | все 10 именованных случаев; it.each сохранены циклами |
| tests/assembler.spec.ts | tests/assembler.rs | 20 исходных случаев; 2 исключения ниже |
| tests/assistant-stream.spec.ts | tests/assistant_stream.rs | все 17 случаев, таблица ошибочных записей, early exit |
| tests/attribution.spec.ts | tests/attribution.rs | все 10 случаев; env-input проверяется чистым helper |
| tests/call-config.spec.ts | tests/call_config.rs | 3 исходных случая; 3 исключения deepFreeze ниже |
| tests/content.spec.ts | tests/content.rs | 34 исходных случая; 1 исключение ниже |
| tests/message.spec.ts | tests/message.rs | все 7 исходных случаев |
| tests/properties.spec.ts | tests/properties.rs | все 4 свойства; 512 детерминированных потоков по 30 чанков |
| tests/retry-policy.spec.ts | tests/retry_policy.rs | все 8 случаев; таблицы ошибок сохранены |
| tests/service.spec.ts | tests/service.rs | все 71 случай; типово невозможные строки таблиц проверяют serde-отказ |
| tests/topology.spec.ts | tests/topology.rs | все 18 случаев, включая contained sync/async failures |
| review round | tests/review_fixes.rs | 13 регрессий: unfiltered dispatch/logger scope, ordered JSON, projection, order, cleanup, schema |
| locale/*.json | locale/*.json | все 8 файлов побайтно; проверено diff -rq |
| README.md, package.json | PORT_NOTES.md, Cargo.toml | прочитаны; дополнительные entry points соответствуют публичным Rust-модулям |

Отдельного LICENSE у пакета и заимствованных фрагментов, требующих копирования лицензии, нет.
В src всего 13 файлов. До реализации и при завершении просмотрены импорты из root, /types, /message,
/assistant-stream и /src/* в переносимых src/ других packages, без client/api/typert/host/experimental.
Сгенерированный api-catalog не считается потребителем символа.

## Отличия от образца

- Typert-транспорт, host/client stubs и declaration merging как механизм не переносятся (§8).
  Методы остались public; у list_providers, list_configurable_providers и remote_discover_models
  стоят точные строки `/// Remote: llm.listProviders`, `llm.listConfigurableProviders`,
  `llm.discoverModels`. Последний возвращает обычный HarnessError с name=RemoteError,
  code=llm/model-discovery-rejected, typed detail ModelDiscoveryRejectedDetails (settingsNs, baseURL)
  и исходной причиной. Обёртки над HarnessError нет.
- LlmError — пространство конструктора с NAME; new/with_options/from_failure возвращают
  Result<HarnessError, ValueError>: невалидный конструктор образца сам бросает обычную Error.
  LlmFailure хранится через HarnessError::with_detail. Произвольные provider-коды проходят from_failure;
  BuiltinLlmFailureCode задаёт встроенные коды и HTTP_<status>. offloadImages конструктор не проверяет,
  как в образце; нормализатор отвергает некорректный carried snapshot целиком.
  FailureCodes/FailureCodeInfo — Rust-оболочка справочника LLM_FAILURE_CODES с iter/get;
  сериализация сохраняет исходный объект code -> {} либо {retryChoice:true} и порядок кодов.
- LlmAdapter — object-safe trait. LlmFuture, LlmDispatch и ModelDiscovery задают boxed Send futures
  и callbacks. Методы, создающие effects или использующие caller scope, принимают &Context
  (docs/kernel.md §7). Llm/LlmPlugin предоставляют сервис llm; дополнительные типы событий —
  AdaptersUpdated, ModelsUpdated, LlmStreamEvent. Исходные имена событий не изменены.
- AdaptersUpdated использует именно Contained: ctx.root().dispatch(...).invoke(report), sync и async
  listener errors получают два исходных warn. Commit закончен до уведомления, mutex освобождён,
  поздние слушатели не теряются из-за раннего отказа. Receiver filter registrant не применяется.
  Оба warn принадлежат context сервиса: LlmPlugin вызывает LlmRuntime::with_context(&ctx).
  Для standalone LlmRuntime::new()/default() используется root registrant. with_context —
  дополнительный Rust-конструктор для явного service owner. Обычный emit не эмулирует containment.
- LlmStream::Item = Result<StreamChunk, BoxError>. Ошибки подготовки, синхронного dispatch
  и polling адаптера становятся терминальным finish; ошибки middleware и потребителя остаются Err.
  Явный `stream.close().await` ждёт iterator.return и возвращает cleanup error потребителю.
  Drop незавершённого и незакрытого runtime stream передаёт cleanup в ctx.spawn владельца
  вызова; отмена/выгрузка этого fiber отменяет задачу, а её ошибка попадает в kernel logger.
  Drop не ждёт задачу. После полного исчерпания и adapter iteration Err cleanup не запускается;
  close не дублируется Drop. Owner передаётся и уже опрошенным deferred streams из middleware.
  Standalone LlmStream без runtime не знает owner: такой потребитель вызывает close().await.
  Middleware, строящее произвольные StreamExt::chain и подобные обёртки, отвечает за
  передачу awaited cleanup через with_cleanup; синхронное владение само передаёт Drop.
- PreparedLlmCall привязывает metadata, policy и dispatch к одной adapter generation, имеет
  mutex для one-shot и отдельный captured config. Публичные snapshots можно менять как собственные
  Rust-значения, но изменение config не меняет скрытый captured config и приводит к
  INVALID_PREPARED_CALL. Для listener-чтения есть LlmStreamEvent::options(); update_options
  отвергает loop-built request. Обычные owned clones не переносят process-local loop marker;
  перемещение исходного объекта и начальная передача в waterfall сохраняют его.
- Immutable snapshots реализованы владением/Clone и Arc<Message>. Исходные специализированные
  конструкторы возвращают UserMessage/DeveloperMessage/AssistantMessage/SystemMessage/ToolResultMessage;
  From переводит их в закрытый Message и RequestMessage. freeze_message — generic Clone.
  user/developer записывают content,source,role,id, как role-specific helpers образца;
  system/assistant/tool сохраняют свой исходный порядок с role впереди.
  create_message принимает Message и заменяет его id; для role-specific входов без id служат
  публичные NewUserMessage/NewDeveloperMessage/NewAssistantMessage и ToolResultMessageInput.
- Message — закрытый enum над payloads, каждый из которых проверяет и сериализует свой role.
  ContentBlock, MessageSource и FinishReason — открытые тег + ordered raw JSON (§4);
  чужие варианты не перечисляются общим enum. ContentBlockMap/FinishReasonMap/ModelModalityMap/
  MessageSourceMap/MessageRoleMap/LlmRequestPurposeMap как TS-таблицы не существуют в Rust.
  Typed payload structs конвертируются в ContentBlock. ContextFormed — закрытый enum,
  отсутствие form выражается отсутствием соответствующего поля в MessageSource.data.
- Публичные вспомогательные типы без отдельного имени в TS: ModelSourceKind, ToolSourceKind,
  SystemPromptSourceKind, RequestUserRole, ImageRepresentation, ToolHistoryUpdate. Доступ к объединениям
  и raw payload предоставляют role/id/content, kind/failure, text_value/image/file, constructors,
  From и GenerateOptions::{call_config,with_call_config}. Это обычные Rust-аксессоры вместо
  JavaScript property access. SessionId — дополнительный re-export одного общего бренда.
- Projection helpers принимают &[RequestMessage]; durable messages передаются через Into<RequestMessage>.
  Cow и Arc сохраняют отсутствие копирования для нетронутых сообщений. project_tool_updates возвращает
  Result, поскольку исходная функция может бросить при неконсистентной истории.
  RequestUserInput не получает id/source при любой проекции.
- AssistantStreamRun — alias AssistantStreamRecord, поскольку packed payloads совместно используются
  читателями; передача raw Chunk возвращает None в run readers. RawStreamChunkType — закрытый enum.
  Raw record хранит Value, потому что исходный expand принимает любой lossless JSON object, даже с
  будущим type. expand_assistant_stream возвращает typed StreamChunk и требует известный protocol;
  expand_assistant_stream_value сохраняет также будущие raw chunk objects. validate_record,
  push_value и expand_assistant_stream_value — дополнительные входы проверки нетипизированных границ.
- u64/i64 исключают дробные, отрицательные индексы/счётчики и NaN на типизированной границе;
  safe-integer ограничения и диагностические строки проверяются дополнительно. Строки неверных
  primitive-типов в adapter metadata тестах отвергаются serde до вызова typed adapter. Для сырых
  assistant records и retry config есть Value-входы с ручными исходными сообщениями.
  Сообщения JSON для модели/журнала пишутся hoh_util_values::js_json_stringify после to_value.
  В тестах тот же codec уравнивает JS-числа 500 и 500.0, не меняя строк и полей.
- bound_context_summary считает UTF-16. Если предел разрезает surrogate pair, Rust String сохраняет
  только целый scalar перед многоточием; одиночный surrogate не представим. Конкретная
  граница: notice summary содержит в JS lone surrogate на UTF-16 index 119;
  Rust сохраняет только целый scalar перед ним. Это учтённое отличие, код не менялся.
  Другие строки не меняются.
  Классификаторы provider failures учитывают UTF-16 длины, ASCII case folding, JS whitespace и четыре
  JS line terminators. error_chain обходит cause/aggregate итеративно, различает cycle и diamond,
  сдерживает панику чужого Display; AggregateError и error_chain_value — явные Rust-входы вместо
  JavaScript AggregateError и произвольного thrown value.
- AdapterFailure — дополнительный явный bridge для own code/failure SDK errors; Rust Error
  не имеет рефлексивных JS properties. normalize_llm_failure_value передаёт произвольный JSON throw
  с JS String coercion. Hostile Display содержит panic и даёт исходный fallback.
- RetryPolicyConfig имеет serde enum и JsonSchema с исходными bounds/defaults и built-in choices
  плюс arbitrary non-empty string. RetryPolicySchema::{schema,validate}, retry_policy_schema и
  resolve_retry_policy_value заменяют вызываемый объект schemastery. validate возвращает Value:
  исходную configuration с defaults и вложенным backoff, сохраняет unknown keys, допускает null
  union и nullable array members, не проверяет resolver-only duplicates/cross-field bounds.
  Resolver отдельно возвращает плоскую ResolvedRetryPolicy и отвергает неизвестные ключи.
  normal/always и Resolved* aliases относятся к соответствующим вариантам enum.
  DEFAULT_RETRYABLE_CODES дополнительно открыт для inspection/test.
- Имя z в retry-policy.ts — импорт @deepseek-ai/schemastery, а не библиотеки zod.
  По §9/DEPS runtime схемы заменён serde и ручной проверкой. retry_policy.rs вручную
  воспроизводит возвращаемый config и полный текст отказа union из vendor/schemastery:
  `expected { mode: "normal", ... } | { mode: "always", ... } but got <JSON>`.
  Текст включает все поля и built-in choices; вложенные branch errors поглощены union.
  Отсутствующий/null input union возвращает null (для undefined Rust-вход — Value::Null).
  Ручные resolver diagnostics и остальные сообщения сохранены в retry_policy.rs,
  error.rs, runtime.rs, assistant_stream.rs. Прямых zod parse/safeParse в исходном src нет.
- APP_IDENTITY — immutable static; User-Agent читает DSH_USER_AGENT_SUFFIX при каждом default-use,
  включая явный &APP_IDENTITY. launch_suffix_value открыт для чистой проверки без изменения
  глобального окружения тестового процесса. AppIdentity использует заимствованные строки;
  dynamic suffix — функция при user_agent, а не JS property getter.
  Версия собственного Cargo package установлена 0.2.2-rc.3, как package.json образца, чтобы
  env!(CARGO_PKG_VERSION) сохранил исходный provider-visible User-Agent.
- ApiKeyCheck сериализуется как {ok:true,value} либо {ok:false,reason}; serde bool discriminator
  написан вручную. ApiKeyRejection сохраняет empty/illegalCharacters. Новых настроек нет.

### Replay adapter: подключение в conformance

Публичная точка замены — LlmAdapter::stream(GenerateOptions) -> Result<LlmStream, BoxError>,
а публичный serde StreamChunk сохраняет исходные семь вариантов и camelCase-поля.
Плагин прогонщика получает ctx.get::<Llm>(), создаёт Arc<ReplayAdapter> и вызывает
`llm.register_adapter(&ctx, providers, adapter)?` в своей активации. Настоящие адаптеры этих routes
должны быть отключены патчем; попытка двойной регистрации даёт DUPLICATE_ADAPTER.
Эффект принадлежит ctx плагина и автоматически освобождает все routes.

Replay JSON разбирается через `serde_json::from_value::<StreamChunk>(value)?`: wire snapshot
сохраняет provider extensions и их позиции; для новых literals wire = Default::default().
ReplayAdapter читает скрипт, выбирает/закрепляет cursor по options.session_id (общий SessionId;
отсутствие id — отдельная anonymous-сессия), purpose и порядку вызовов, затем возвращает
`Ok(LlmStream::chunks(chunks))` с исходными границами deltas, usage и terminal finish.
Таймированный поток можно построить через LlmStream::new и context-owned sleep; отложенные
работы должны принадлежать ctx. Данные assistant stream разбираются validate_record /
expand_assistant_stream[_value]; не склеивать дельты до передачи в runtime.

Каталог replay должен реализовать provider_info, list_models, resolve_model, provider_retry_policy,
image_request_pricing/image_request_target, если они заданы fixture: context, defaultMaxTokens,
reasoning efforts/default, modalities, systemPromptUpdate, toolUpdate, badge влияют на заголовки,
проектирование запроса и compaction. Для generation-specific поведения переопределяется
prepare_call(Arc<Self>, ...), возвращающий PreparedAdapterCall {model,dispatch}.
Для единственного сценария без каталога допустим
`ctx.on_waterfall::<LlmStreamEvent,_,_>(|event,next| async move { ... })`:
вернуть собственный LlmStream вместо next.run().await; event.runtime заменяет bound this,
event.options() даёт запрос. Это тот же слой, что §1.4/6 README conformance, без HTTP/SSE.
Сам сценарный replay cursor/fixtures принадлежат hoh-conformance, не hoh-llm.

### Экспорты без прямых импортов в переносимых пакетах

Полный поиск прямых root/subpath imports (выше исключённые пакеты и тесты не считаются):
`AlwaysRetryPolicyConfig`, `ApiKeyCheck`, `ApiKeyRejection`, `AppIdentity`, `AssistantProviderMetadata`, `AssistantStreamRun`, `BackoffConfig`, `CONTEXT_SUMMARY_MAX_CHARS`, `ConfigureCall`, `ContentBlockMap`, `ContentBlockType`, `ContextForm`, `FinishReasonMap`, `LLM_FAILURE_CODES`, `LlmCallControls`, `LlmErrorOptions`, `LlmImageRequestBudget`, `LlmModelContext`, `LlmModelDiscoveryRequest`, `LlmModelReasoningInfo`, `LlmNativeAuth`, `LlmReasoningEffortInfo`, `LlmRequestPurpose`, `MessageRoleMap`, `ModelModalityMap`, `NormalRetryPolicyConfig`, `ProjectedToolUpdates`, `RETRY_CODE_CHOICES`, `RawStreamChunkType`, `ReasoningBlock`, `RequestUserInput`, `ResolvedAlwaysRetryPolicy`, `ResolvedNormalRetryPolicy`, `ResolvedRetryBackoff`, `SystemPromptMessageSource`, `TextBlock`, `TimedStreamChunk`, `ToolAdditionBlock`, `ToolMessageSource`, `ToolRemovalBlock`, `ToolResultMessageInput`, `assistantStreamChunks`, `assistantStreamHasVisibleContent`, `assistantStreamHasVisibleText`, `chunkHasVisibleText`, `contentHasFile`, `createMessage`, `fileHandleText`, `isHarnessError`, `isTokenDelta`, `isVisibleChunk`, `normalizeLlmFailure`, `projectFilesToText`, `projectImagesForTextModel`, `projectToolUpdates`, `runFirstTokenTime`, `runFirstVisibleTime`, `userAgent`.

MessageSourceMap и LlmRequestPurposeMap используются через module augmentation, хотя не импортируются.
Остальные перечисленные символы сохранены как данные, aliases, helpers либо учтённые выше
TS-only map-механизмы. Непрямое использование типа через поле другого типа/метод сервиса не
считается отсутствием этого поля или метода.

### JSON после review

StreamChunk (все семь вариантов), TokenUsage, ReplayEnvelope, LlmFailure, FinishReason и
ContentBlock сохраняют исходные неизвестные поля и порядок всех ключей при serde round-trip.
У них дополнительный публичный wire: wire::JsonSnapshot; для новых struct/variant literals
передаётся Default::default(). Он хранит original и typed baseline; изменение известного поля
обновляет его на исходной позиции, удаление известного optional field убирает его, новые поля
добавляются в конец. Явный null без изменения сохраняется. PartialEq игнорирует порядок
и сравнивает unknown values вместе с typed fields. JsonSnapshot/wire — дополнительные Rust API.
TimedStreamChunk и AssistantStreamRecord::Chunk несут этот JSON дальше; raw record payload —
Value, вложенные attachment/response/provider JSON остаются Value. Упаковка delta сохраняет
только исходные поля compact protocol, как в TS. При prune envelope с удалённым tool-call
исходник строит новый {response,blocks}, и extensions там намеренно исчезают.
normalize_llm_failure и LlmError::from_failure строят новый объект только из known facts, в исходном
порядке message,code,status,providerRetryAfterMs,requestId,offloadImages. Просто полученная
failure JSON сохраняется losslessly. ToolSchema пишет name,description,parameters,deferLoading;
LlmResolvedModelInfo пишет reasoning после systemPromptUpdate/toolUpdate.
Projection images/files выбирает блок по type, а не успешности typed serde attachment.

## Не перенесено

Из 213 именованных it/it.each definitions перенесено 205. it.each считается одной именованной
группой; варианты таблиц выполняются циклами, а не теряются. Дополнительно есть 8 прежних Rust regressions и 13 случаев review_fixes.rs.
Все 12 test files имеют пару. Восемь исключений механизмов, неприменимых в Rust (§8/10):

1. adapter-failure: contains_hostile_non_error_coercion — Symbol.toPrimitive/hostile JS coercion
   отсутствуют у serde_json::Value; обычное JSON coercion сохранено.
2. adapter-failure: contains_hostile_error_property_reflection — Proxy/getOwnPropertyDescriptor
   не существуют. Typed own-failure bridge и hostile Display проверяются отдельно.
3. assembler: must_get_throws_when_an_index_is_missing_from_the_partials_map — исходный тест
   через any портит private order; здесь один IndexMap, независимого order и такой инвариантной
   дыры нет. Свойство distinct-index size проверяется.
4. assembler: throws_with_diagnostics_when_a_value_escapes_a_closed_union_at_runtime — это
   внешний assertNever из hoh-util-values; closed Rust enums дают исчерпывающий match.
   Соседний тест неизвестного сырого StreamChunk сохранён через push_value.
5–7. call-config: freezes_nested_structure_in_place_and_returns_the_same_reference,
   passes_primitives_through_and_terminates_on_cycles,
   freezes_nesting_deeper_than_the_javascript_call_stack — внешний deepFreeze;
   Rust ownership/Arc и отсутствие циклов у Value заменяют runtime freeze.
8. content: rejects_a_media_type_that_escaped_the_closed_union_at_runtime — source casts
   image/tiff в закрытый MIME union; ImageMediaType enum не позволяет этот typed вызов.
   Обычные четыре MIME/расширения проверены.

Из типизированных тестов убраны JS accessor/Proxy детали, включая getter done/value, getter
message/failure/code, read counters на Proxy массивов, number-as-string и фиктивные non-JSON
functions/undefined. Их доступное поведение проверяется Err из fallible poll, Value-валидацией,
hostile Display, малформированным хвостом после early exit и serde-отказом. Живое сетевое API,
npm/HMR transport и generated Typert stubs не запускались и не переносились.

## Не проверено

Платформенных веток нет. Сеть и настоящие provider adapters принадлежат другим crates.
Нельзя выразить одинокие UTF-16 surrogates, JS descriptors/accessors и cross-realm prototype
identity. Генератор fast-check заменён воспроизводимыми потоками без новой зависимости;
runtime invariants сохранены, shrinking отсутствует. Не выполнялся полный corpus conformance —
данная работа предоставляет слой, в который прогонщик вставляет replay.

## Вопросы ведущему

В исходнике fileReadPath обращается к structural `ctx.get('fs').processPathFromHostPath`.
В workspace пока нет owning fs crate/общего ServiceKey, а hoh-kernel::get требует типизированный ключ.
Чужой сервис в hoh-llm не объявлен. До появления owning API composition передаёт
`ctx.with_tag(HostPathMapping(Arc::new(map_host_path)))`; callback вызывается заново на каждый запрос,
attachments запрашиваются через существующий Attachments. Без mapping выбирается исходный
no-readable-path placeholder. Нужна интеграция этого callback с будущим fs provider либо общий
структурный read-only face у владельца fs. Других недостающих зависимостей нет.
fileReadPath проверен отдельно: Cordis createTraceable подставляет use-site ctx в this.ctx;
поэтому Rust file_request_text(&Context, ...) и adapter_stream используют caller context, а не
registrant или root. Это сохраняет attachment/filesystem scope. Сценарии fromRequest, читающие
`saved at "…"`, требуют настоящего пути от filesystem provider; placeholder не заменяет его.
Все известные различия Rust API описаны выше.

## Проверки

Все финальные Cargo-команды выполнены с
`CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w7`.

- `cargo test -p hoh-llm`: exit 0, **226 passed; 0 failed; 0 ignored**,
  сумма 13 integration binaries; unit/doc-tests — 0/0.
- `cargo clippy -p hoh-llm --all-targets -- -D warnings`: exit 0,
  чисто, предупреждений нет.
- `cargo fmt -p hoh-llm -- --check`: exit 0.
- `diff -rq <source>/locale crates/hoh-llm/locale`: различий нет.
- Поиск `todo!()` / `unimplemented!()` / TODO / stub в src и tests: совпадений нет.
- `git diff --check -- crates/hoh-llm`: exit 0.

Точные итоговые строки test binaries (без переменной длительности):

| binary | итог |
| --- | --- |
| tests/adapter_failure.rs | `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/api_key.rs | `test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/assembler.rs | `test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/assistant_stream.rs | `test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/attribution.rs | `test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/call_config.rs | `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/content.rs | `test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/message.rs | `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/properties.rs | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/retry_policy.rs | `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/review_fixes.rs | `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/service.rs | `test result: ok. 74 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| tests/topology.rs | `test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |

В review round первые 6 regression tests упали до правки production-кода. Дополнительные
регрессии сначала упали на rebuilt failure/pruned replay, исчерпанном deferred stream,
уже опрошенном middleware stream и from_failure; затем исправлены. Исходные weakened
assertions восстановлены: raw empty identities, reader/assemble/silent fixtures, обе таблицы
service failures, отсутствие cleanup после iteration failure и события при fiber disposal.
Clippy потребовал компактного хранения snapshots: immutable Arc вместо двух inline maps.

Ранние проверки выявили и исправили различие 500/500.0 в тестовом сравнении JS JSON
и предупреждения Clippy type_complexity/await_holding_lock/collapsible_if. Проверки не
ослаблены: внешние JSON-строки используют общий JS codec; guards в тестах ограничены
синхронным чтением snapshot. В чужих crates ошибок не было, правок чужого кода нет.
Все обязательные проверки завершены; заглушек и незаконченного кода в crate нет.
Интеграция будущего owning fs API и правила awaited close / fiber-owned Drop оговорены выше.
