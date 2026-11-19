# hoh-mcp-client ← packages/mcp/mcp-client @ 39163dd3

## Файлы

| образец | перенос |
|---|---|
| `src/index.ts`, `connection.ts`, `generation.ts` | `src/lib.rs`, `runtime.rs`, `connection.rs`, `generation.rs` |
| `src/credentials.ts`, `templates.ts`, `diagnostic.ts`, `deadline.ts` | одноимённые Rust-модули |
| `src/tools.ts`, `transport.ts`, `probe.ts`, `server-context.ts`, `status.ts`, `types.ts` | одноимённые snake_case-модули |
| `tests/*.spec.ts` | `tests/contracts.rs` и unit-тест `generation.rs` |
| `locale/*.json` | `locale/*.json`, без изменений |

## Сохранённое поведение

- Plugin `McpClient` требует `tools`, резервирует `serverName` внутри scope, блокирует activation до первой попытки и при `failOnStartupError` откатывает instance. Supervisor владеет поколениями, применяет ограниченный exponential backoff, stability window, статусы и bounded close.
- `rmcp 3.10.0` выполняет настоящее MCP negotiation (включая 2026-07-28), stdio child transport и Streamable HTTP. Stdio получает scrubbed ambient environment из `hoh-subprocess`; explicit env перекрывает его. HTTP headers валидируются типами `http`.
- `${credential:REF}` проверяется fail-loud, каждое имя разрешается один раз за pass. Конфигурация остаётся сырой; все когда-либо раскрытые plain и `encodeURIComponent`-формы секретов редактируются.
- Tool names детерминированно нормализуются до `[A-Za-z0-9_-]`/64 символов с 12 hex SHA-256 identity suffix. Discovery завершается до регистрации; при конфликте частичная регистрация откатывается.
- MCP `isError`, malformed result, output-schema violation, timeout, disconnect и unsupported task имеют раздельные machine codes. Canonical content/structuredContent не теряется. Text/audio/resource/link проецируются стабильными строками.
- Изображения декодируются только из canonical RFC 4648 base64 и четырёх поддержанных MIME. Batch валидируется до записи; admission требует точного текущего model route с image modality и durable attachment store. Отказ меняет только model content, raw MCP value сохраняется.
- Resource provider выбирает текущее живое поколение. Инструкции публикуются literal (`interpolate=false`), атрибутируются `### MCP server: …` и ограничиваются UTF-8 byte budget.
- Probe одноразово разрешает credentials, ограничивает общий budget/pages/names, сообщает stage и отдельно возвращает cleanup result.

## Адаптация Rust

- Promise/AbortController представлены async future, `AbortSignal` и `CancellationToken`; `Disposer` обеспечивает ordered teardown.
- Status хранится по uid Fiber на время жизни instance; финальный unload отсоединяет observers и удаляет запись. Observer panic содержится и передаётся в logger. В Rust callback синхронный: async observer выражается вызывающим через spawn.
- `rmcp::model::Tool` 3.10 не сохраняет неизвестное top-level поле `execution.taskSupport` при десериализации. Дополнительной feature для него нет; прямой constructor API всё равно поддерживает `task_required`, а wire-discovery не может увидеть этот флаг без изменения upstream rmcp API.
- Browser/e2e `mcp-client.e2e.ts` (23 кейса) не переносился согласно `PORTING.md` §10; protocol path проверяется in-memory duplex server без сети.

## Исправления по ревью 19.11

- Validation advertised `outputSchema` отделена от ограниченного schema renderer. До provider call проверяются форма схемы, локальные `$ref` и regex. Runtime validator исполняет boolean schemas, type unions, object/property/pattern/dependency constraints, array/tuple/contains/unique constraints, numeric/string limits, `enum`/`const`, combinators и `if/then/else`; ошибки используют Ajv-подобный путь `data/...`. Renderer по-прежнему безопасно откатывается к `{}` для vocabulary вне `hoh-tools` subset.
- Это намеренно не заявляется полной реализацией Ajv/Draft 7: крейта `jsonschema` нет в workspace/Cargo.lock, а добавлять новую внешнюю зависимость запрещено. Проверенные исходные boundary-кейсы (`patternProperties`, invalid regex before call, integer diagnostic) перенесены буквально; `regex` уже был workspace dependency и в lock.
- `isError` сначала целиком переводится в model-visible text и затем редактируется, поэтому secret в `resource_link` не утекает. Все `MCP_INVALID_RESULT` diagnostics также проходят через redactor.
- Неподтверждённый close после любой startup/setup/registration ошибки имеет приоритет и терминально классифицируется как `unclosed-generation`; reconnect не запускается. `close()` больше не держит session mutex во время bounded await.
- Credential notification сначала повторно разрешает config и сравнивает его с текущим: одинаковое значение не закрывает generation, не сбрасывает retry budget и не оживляет `gave-up`; ошибки refresh логируются. Stability window теперь применяется и к resync loss.
- Optional `McpResources` и `SystemPrompt` регистрируются двумя реактивными `ctx.inject`, поэтому порядок монтирования не влияет на публикацию provider/section. На unload status listeners отсоединяются и статическая запись очищается.
- Уточнены контракты: незакрытый `${` остаётся литералом; unknown reconnect keys отвергаются; runtime проверяет границы startup/close/max-instruction; `io::NotConnected` — `MCP_SERVER_UNAVAILABLE`; probe abort всегда `probe aborted`; disconnected diagnostics называют `serverName`.

## Исправления по ревью 20.11

- `pattern` и `patternProperties` предкомпилируются один раз, включая targets произвольных локальных `$ref`; cycle guard сохраняет рекурсивные schemas, а validation не содержит regex-cache panic path. В дополнение к workspace `regex` поддерживаются ведущие JS-assertions `^(?=...)` и `^(?!...)`; поэтому распространённый Ajv-шаблон `^(?!tmp)` принимает `abc` и отвергает `tmp-*`. Полная ECMAScript RegExp-совместимость без нового regex-движка недостижима: lookbehind, внутренние lookaround и backreferences остаются compile-error и явно считаются границей порта.
- Любая abandoned startup-ветка теперь пропускает close через `prefer_close_failure`. Supervisor-тесты с oversized instructions и с generation, закрывшимся после setup, доказывают terminal `Failed(unclosed-generation)` и ровно одну попытку.
- Credential refresh рассматривается как `Option<Config>`: отсутствующий reference — реальное изменение к `None`, а не логируемая ошибка. Одинаковое значение сохраняет generation и `connectedAt`; удаление закрывает generation, снимает tools и переводит supervisor в `WaitingCredentials`, не запуская новый transport. Та же семантика применяется во время backoff и после `gave-up`/`reconnect-disabled`.
- Probe duplex-тесты покрывают переход по cursor, `maxPages`, `maxToolNames`, общий list-timeout и обязательный close. Transport-тест подаёт собственный parent snapshot и проверяет одновременно scrub sensitive/`DSH_*`, сохранение safe env и explicit override.
- `format`, `prefixItems` и `unevaluated*` по-прежнему не исполняются ограниченным валидатором и остаются документированной Ajv-границей. `hoh-kernel` при restart сохраняет uid Fiber; текущая status-запись удаляется cleanup-эффектом и создаётся заново при apply. Поэтому guard предыдущего incomplete cleanup гарантирован для нового apply до unload, но не переносится через kernel restart того же Fiber; менять lifecycle без отдельного согласованного API чужого крейта в этом круге не стали.

## Зависимости и чужой API

Новых внешних версий не добавлено: direct `regex.workspace = true` использует уже зафиксированный workspace dependency. Дополнительные feature `rmcp` не нужны. Корневые файлы и чужие крейты этим кругом не менялись.

## Проверка

Команды выполняются offline с `CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x8`; тесты — только под внешним `timeout`.

После review-fix в крейте 66 Rust-тестов (35 unit + 31 contracts; doc-tests 0). Они включают настоящие in-memory MCP negotiation/tool execution, supervisor lifecycle, credential refresh с проверкой снятия tools, local-ref schema compilation, close-timeout classification, probe pagination/limits/timeout и transport env policy. `SOURCE_TEST_CASES.md` — матрица контрактов, а не заявление о построчном соответствии 161 не-e2e объявлениям TS: несколько параметризованных и смежных TS-кейсов сведены в один Rust boundary-тест.
