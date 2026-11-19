# hoh-llm-mock-server ← packages/test-support/llm-mock-server @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | 24 поведения, валидация, seeded random, telemetry, HTTP/SSE и lifecycle сервера перенесены |
| `src/cli.ts` | `src/cli.rs` | vocabulary, defaults, cross-option validation и usage перенесены |
| `src/bin.ts` | `src/bin.rs` | unavailable/ready JSONL, request events, SIGINT/SIGTERM и коды 130/143 перенесены |
| `tests/server.spec.ts` | `tests/server.rs` | все исходные классы wire cases покрыты: prefix/observer, forced resets, client close, IPv6, ошибки, exhaustion/repeat, random и validation; добавлены chunked-body, JS JSON, close-active и keep-alive регрессии |
| `tests/cli.spec.ts` | `tests/cli.rs` | все классы parser/default/error cases покрыты |
| — | `tests/bin.rs` | дополнительная process-level проверка unavailable → ready → request/result → SIGTERM/143 |
| `README.md`, `package.json`, build config | `Cargo.toml`, эти заметки | прочитаны; отдельной лицензии в пакете нет |

## Отличия от образца

1. Вместо `node:http` используется небольшой safe-Rust HTTP/1.1 listener поверх Tokio. Он понимает
   `Content-Length` и `Transfer-Encoding: chunked`, сохраняет соединение для последовательных запросов и
   выдаёт SSE chunked responses. Это решение заранее зафиксировано в `docs/porting/DEPS.md`; новый HTTP-server
   dependency не добавлен.
2. `server.requests` как синхронный live JavaScript array представлен `MockLlmServer::requests().await`,
   возвращающим согласованный snapshot: обработчики работают конкурентно, поэтому доступ защищён async mutex.
3. Числовые поля options имеют `Option<f64>`, чтобы публичная Rust-граница могла воспроизвести исходную проверку
   `Number.isInteger`/`Number.isFinite`, включая дроби и бесконечности. После проверки они сужаются до native
   integer типов.
4. Заголовки захватываются в lowercase `IndexMap<String, String>`. Node допускает array-valued повторные
   заголовки; raw listener объединённое представление повторов не экспонирует. Ни production consumers, ни
   исходные тесты повторные заголовки не используют.
5. Разрез `toolArguments` вычисляется по UTF-16 units, как JavaScript `String.length`, но не создаёт невалидную
   lone-surrogate Rust-строку, если midpoint попадает внутрь surrogate pair: граница сдвигается к следующему
   Unicode scalar. Для обычного JSON/ASCII wire полностью совпадает.
6. Последовательный HTTP/1.1 keep-alive сохранён. Настоящий HTTP pipelining за незавершённым первым response
   (например, второй request после `stall` без ожидания первого response) raw listener не диспетчеризует до
   завершения первого. Node создаёт оба `ServerResponse` сразу и буферизует второй. Ни исходные тесты, ни
   известные consumers pipelining не используют; точное воспроизведение потребовало бы отдельного ordered
   response multiplexer. Обычные pooled-клиенты, включая fetch/reqwest, совпадают с образцом.
7. Review-round исправил eager fallback при выдаче scripted behavior: Rust `Option::unwrap_or` вычислял ветку
   `script_exhausted` даже для существующего элемента последовательности. Явный `match` теперь повторяет
   `resolved.sequence[cursor] ?? fallback` из `src/index.ts`; бессокетный unit-тест фиксирует первый элемент и
   последующее exhaustion независимо от доступности loopback.

## Не перенесено

- npm package exports/shebang и TypeScript declaration-only aliases как механизмы языка. Им соответствуют
  Rust module, `[[bin]]` и публичные enums/structs.
- `NodeJS.IncomingHttpHeaders` с array-valued дублями — см. принятое отличие №4.
- Concurrent HTTP pipelining за незавершённым response — см. принятое отличие №6.

## Не проверено

- Windows signal glue компилируется через `ctrl_c`, но process-level тест SIGTERM выполняется только на Unix.
- IPv6 loopback зависит от конфигурации хоста; форматирование `[::1]` реализовано, но тест не требует доступного
  IPv6 listener на CI.
- TCP-интеграционные тесты выполняются полностью там, где разрешён loopback. В sandbox писателя сам syscall
  `bind(127.0.0.1:0)` возвращает `EPERM`; только при этом конкретном `PermissionDenied` тесты завершаются до
  сетевого сценария. Остальные ошибки bind остаются падением, а parser/validation/PRNG/serialization выполняются
  безусловно.

## Вопросы ведущему

- Нет. Изменения чужих API и новые внешние зависимости не потребовались.
