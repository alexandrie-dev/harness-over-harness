# hoh-tool-executor-native ← packages/core/tool-executor-native @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | `NativeToolExecutor` (`describe`, `execute`), плагин `native-tool-executor` с inject `tools`. |
| `locale/*.json` | `locale/*.json` | Все 8 файлов скопированы без изменений (проверено побайтово `cmp`). |
| `README.md`, `README.zh.md` | rustdoc и эти примечания | Прочитаны; конфигурации и собственных событий нет. |

В пакете нет `tests/*.spec.ts`, собственного `LICENSE` и указаний на заимствованный код. Типы и функции
`ToolExecutor`, `ToolDefinition`, `ToolModelInput`, `ToolBatchInput`, `ToolBatchResult`,
`ToolExecutionPipeline`, `executeToolBatch`, `projectToolSchema` берутся из `hoh-tools`; самих по себе
сервис и реестр инструментов принадлежат `hoh-tools`. Временного shim в этом crate нет.

## Отличия от образца

1. Класс исполнителя реализует объектно-безопасный `ToolExecutor` из `hoh-tools`; async-результат —
   `BoxFuture<Result<_, BoxError>>`. Регистрация — `ToolsService::set_executor(&ctx, executor)` с явным
   `Context`; возвращённый `Disposer` не удерживается: эффект регистрации принадлежит контексту плагина
   и снимается вместе с ним.
2. Из-за отсутствия перегрузок конструктор с умолчанием — `new(ctx)`, а вариант с callback —
   `with_max_parallel(ctx, callback)`. Callback — `Fn() -> u64 + Send + Sync`; умолчание остаётся `10`
   и читается при каждом вызове batch-функции. Отсутствие сервиса `tools` в конструкторе возвращает
   `HarnessError` (`ToolExecutorUnavailableError` / `TOOL_EXECUTOR_UNAVAILABLE`) вместо JS throw —
   так делает `ToolExecutionPipeline::new` из `hoh-tools`.
3. `describe` сохраняет порядок definitions и проецирует схемы через `project_tool_schema(definition, false)`;
   `knownNames` отсутствует (как в образце), его подставляет `hoh-tools`. Ошибка проекции возвращается
   как `Err` вместо JS-исключения.
4. `maxParallelToolCalls` у `hoh-tools` — `Option<u64>`, callback возвращает `u64`: дробные JS-значения
   на этой границе не представимы (решение `hoh-tools`).

## Не перенесено

Ничего: исходный пакет состоит из одного файла `src/index.ts`, и он перенесён целиком.
Тестов в исходном пакете нет, поэтому не добавлялось и перенесённых.

## Не проверено

Сквозные `describe`/`execute` и регистрация на запущенном ядре с настоящим `hoh-tools` в этом crate
автоматическими тестами не покрыты: в образце их нет, поведение целиком лежит в `hoh-tools`
(`ToolExecutionPipeline`, `execute_tool_batch`). Проверена компиляция, clippy и побайтовое копирование locale.

## Вопросы ведущему

Нет. `hoh-tools` на момент переноса имеет статус `todo` в `MAP.tsv` и собственный shim `shim.rs`
(`Agent`, `ModelSelection`, `BoxFuture`); после его интеграции импорт `hoh_tools::shim::BoxFuture`
в `src/lib.rs` заменить на финальное место типа.
