# hoh-tools ← packages/core/tools @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | `ToolsService`, `ToolRuntime` (плагин), реестр по слоям областей, `restrict`, `guard`, `resolve`, `execute`, провайдер tools в SystemPrompt |
| `src/contracts.ts` | `src/contracts.rs` | `ToolDefinition`, `ToolExecution`, результаты, решения pre/post, активация, ошибки |
| `src/error-codes.ts` | `src/error_codes.rs` | коды `APPROVAL_*`, `TOOL_CONTEXT_UNAVAILABLE` |
| `src/events.ts` | `src/events.rs` | `tools/pre-execute`, `tools/execute`, `tools/post-execute` (Waterfall), `tools/result` (Contained), `tools/change` (Emit) |
| `src/executor.ts` | `src/executor.rs` | трейт `ToolExecutor`, вход/результат пакета, `ToolExecutorUnavailableError` |
| `src/json-schema.ts` | `src/json_schema.rs` | поддерживаемое подмножество, тексты отказов и порядок нарушений без изменений |
| `src/pipeline.ts` | `src/pipeline.rs` | `ToolExecutionPipeline`: prepare / dispatch / complete, ask → approval, guard, отмена |
| `src/batch.ts` | `src/batch.rs` | планировщик пакета, журнал `tool/call` и `tool/result` |
| `src/presentation.ts` | `src/presentation.rs` | сериализуемые виды вызова и результата |
| `src/schema.ts` | `src/schema.rs` | DSL параметров, `validate_args`, `define_tool`, `project_tool_schema` |
| `locale/*.json` | `locale/*.json` | все 8 файлов без изменений |
| — | `src/errors.rs`, `src/shim.rs` | внутренние ошибки пакета; заглушки (см. ниже) |

### Тесты

Случай — один вызов `it` / `it.each` образца; строки таблиц проверяются циклом.

| образец → tests/*.rs | перенесено | пояснение |
| --- | --- | --- |
| json-schema → json_schema.rs | 23 из 25 | глубина 4000 на потоке с 64 МБ; JS-графы (Proxy, getter, цикл) исключены |
| schema → schema.rs | 5 из 10 | пять случаев — выведение типов TypeScript |
| properties → properties.rs | 6 из 6 | детерминированный SplitMix64 вместо fast-check, 300 случаев на свойство |
| scoped → scoped.rs | 21 из 27 | исключены 6, см. ниже |
| activation → activation.rs | 10 из 10 | |
| execution-mode → execution_mode.rs | 8 из 10 | исключены «truthy не-boolean» и `expectTypeOf` |
| tools → tools.rs, tools_more.rs | 121 тест на 148 случаев | часть случаев объединена, 14 исключены, см. ниже |
| круг исправлений по ревью → review_round.rs | см. ниже | «freezes dispatch outcomes» и «register() … composite yield» перенесены сюда; остальное — регрессии по пунктам ревью |
| execution-signal-types, gen-tool-catalog, exports.built.e2e | 0 | типы TypeScript, генератор, сборка JS (§10 PORTING) |

Итог прогона: `cargo test -p hoh-tools` — 194 прошло, 0 упало (activation 10, execution_mode 8, json_schema 23,
properties 6, schema 5, scoped 21, tools 80, tools_more 41).

## Отличия от образца

- **Заглушки.** `hoh-agent` пуст, `hoh-user-approval` ещё нет. `src/shim.rs`: `SHIM(hoh-agent)` — трейт `Agent`
  (`scope_key()`, `session()`) и `ModelSelection`; `SHIM(hoh-user-approval)` — `ApprovalRequest`,
  `ApprovalOutcome`, трейт `ApprovalApi`, сервис `Approval`. Когда появится настоящий сервис с тем же именем,
  но другим типом, `ctx.get` вернёт `None` и `ask` деградирует в отказ — заглушку нужно убрать в тот же момент.
  `BoxFuture` из `shim` используют другие крейты (`hoh-tool-executor-native`).
- **`ToolExecutionPipeline::new(ctx)`** не падает без сервиса `tools` (как конструктор образца, который только читает
  `ctx.tools`): сервис хранится как `Option`, а операции, которым нужен реестр (поиск определения, guards),
  отвечают `TOOL_EXECUTOR_UNAVAILABLE` результатом вызова. `Result` в сигнатуре сохранён ради вызывающих крейтов.
- **Паника в колбэке** (`execute`, `finalize_content`, `project_content`, `render`, `presentation_meta`,
  `is_concurrency_safe`, guard) превращается в ошибочный результат, как брошенное исключение.
- **Метка «канонический результат»** (WeakMap и `deepFreeze` в образце) — приватное поле результата с замороженной
  копией. `Clone` метку сбрасывает, как spread в JS: копия проходит нормализацию заново. Правка поля результата
  на месте (поля публичны) в образце невозможна — объект заморожен: поэтому результат с меткой своей отправки,
  отличный от замороженной копии, заменяется этой копией, а изменение не действует.
- **Реестр исполнителей** по областям держит `ScopeKey` сильной ссылкой (слабого ключа в `hoh-scope` нет): запись
  исполнителя области живёт до выгрузки сервиса. Исполнитель корня после выгрузки плагина остаётся записью
  «недоступен». Проверка «один исполнитель в области» и занятие области — один шаг под одной блокировкой; при
  сбое настройки область возвращается в прежнее состояние.
- **`ToolGuard`** возвращает `Result<Option<ToolGuardDenial>, BoxError>` (в образце — значение или исключение).
- **Аргументы и значения — `serde_json::Value`.** Неразобранный JSON попадает в инструмент как `Value::String`.
  Нестандартные значения JS (`undefined`, `NaN`, функции, Map) невыразимы; «не JSON без потерь» проверяется на `-0.0`.
- **Схемы** — сырой JSON; DSL параметров принимает JSON-описание. `ToolsConfig` принимает посторонние поля, как
  объектная схема zod, а `mode` и `maxParallelSubCalls` (`z.never()`) описаны явно: схема `false` и десериализация,
  которая отвергает любое присутствие поля. `validate_json_schema_value` с фальшивым `type` паникует текстом
  `unreachable variant in JsonSchemaType: …` (бросок `assertNever` образца); вложенный узел с допустимым `type`
  превращает такой отказ в общий диагноз по своему пути, как в образце.
- **`define_tool`** возвращает `Result<_, BoxError>`: неверный `timeoutMs` — простая `ToolsError::Error`
  (в образце — голый `Error`, без `error.info`), ошибки схемы — `JsonSchemaError`. `project_tool_schema` — `ToolsError`.
- **Публичные предикаты** `is_plain_json_record`, `is_json_schema_record`, `is_plain_json_array` над `Value`:
  у `Value` нет цепочки прототипов, поэтому они сводятся к проверке вида значения.
- **`GenericCallView::raw_input`**: `Some(Value::Null)` — явный `null`, `None` — отсутствие поля.
- **`maxParallelToolCalls`** — `Option<u64>`; ноль даёт `RangeError` с текстом образца.
- **`ToolExecution` и `ToolRunContext`** — один тип; сигнал меняется через `set_signal`, состояние вызова приватно.
- Расширение `AssembleContext.toolSet` реализовано через `with_extension::<ResolvedToolSet>`.

## Не перенесено

- Правило §10: типовые тесты (`expectTypeOf`, `InferArgs`, литералы enum/const), генератор каталога, e2e сборки.
- tools.spec, 12 случаев без аналога в Rust: обязательное объявление `output` (проверяет тип), getter / Proxy
  в теле, значении, ошибке и снимках (3 случая о брошенных не-Error значениях, 2 о снимках проекторов,
  «hostile thrown value»), «throwing body snapshot», «required key present as undefined»,
  «rogue outcome» подменён проверкой отказа сервиса одобрения (перечисление закрыто).
- scoped.spec, 6 случаев: тип `tools/result`, алиасинг массива фильтра, getter `parent`, единый снимок
  входа с getter, getter `arguments`, вложенный getter аргументов. Map и экземпляр класса сведены к проверке `-0.0`.
- Экспорты образца без отдельной пары: типы-обёртки TypeScript (`InferArgs`, `ParameterSchemaSpec` как тип) —
  в Rust это `serde_json::Value`.

## Не проверено

- Работа с реальным `hoh-agent` и `hoh-user-approval`: оба заменены заглушками, тесты используют стенд `Answerer`.
- Поведение планировщика при сотнях параллельных вызовов, кроме случаев из spec.
- Сильная ссылка на `ScopeKey` в таблице исполнителей не измерялась на утечку при массовом создании областей.

## Вопросы ведущему

1. Заглушки Agent / Approval: кто владеет трейтом `Agent` — `hoh-agent`? После его появления `hoh-tools` должен
   перейти на него и удалить `shim.rs`.
2. Нужна ли слабая таблица исполнителей по областям (например `Weak<ScopeKeyData>` в `hoh-scope`)?
