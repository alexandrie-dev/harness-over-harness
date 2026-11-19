# hoh-sdk-protocol ← packages/sdk/protocol @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | все публичные типы и transport экспортируются из корня crate |
| src/types.ts | src/types.rs, src/shim.rs | все request/result/notification payloads, литералы и имена методов |
| src/transport.ts | src/transport.rs | newline JSON-RPC, correlation, handlers, close/failure, cancellation и flush |
| tests/transport.spec.ts | unit-тесты src/transport.rs | 19 из 19 случаев перенесены |
| README.md, package.json | Cargo.toml, эти заметки | прочитаны; собственного LICENSE в пакете нет |

## Отличия от образца

- TypeScript interface maps представлены точными константами имён методов и
  именованными Rust DTO. Для анонимных source-shapes добавлены Rust-имена
  `ServerInfo`, `InitializeCapabilities`, `SessionQueryCompletedNotification`,
  `SdkUserQuestionCancelledResult`, `EmptyResult`; для строковых/boolean литералов —
  `EncodedImageTag`, `SdkImageMediaType`, `LiteralTrue`, а для union — Rust enum.
- `pending_len` — скрытый из документации диагностический метод только для точного
  переноса source-тестов, которые напрямую проверяют private `pending.size`.
- `JsonRpcLineTransport::new` и `start` возвращают `Result` и требуют активный
  Tokio runtime: у Rust нет неявного Node event loop. `notify` возвращает ошибку
  закрытой очереди вместо синхронного JS `void`/throw.
- `AsyncWrite` не имеет синхронного `Writable.write` throw. Каждый фрейм проходит
  `write_all` и `flush`; request ждёт ack обеих операций и получает их I/O-ошибку,
  но close/EOF/input-error одновременно будят его через pending-канал, даже если
  запись стоит на backpressure. I/O-ошибка записи не становится постоянной failure
  транспорта. Ошибки notification-записи, как асинхронные ошибки Node stream, не
  возвращаются вызывающему.
- Rust handler и `AbortSignal` несут только ошибки. Брошенные JS string/undefined
  представлены ошибкой с тем же `Display`; shared abort reason возвращается без
  замены и проверяется через `SharedError::ptr_eq`. Для panic owner-handler сделан
  аналог catch любого JS throw: peer получает `-32603` с текстом panic как при
  синхронном вызове closure, так и при poll возвращённого future.
- Отсутствующий `result` ответа становится `Value::Null`, поскольку у JSON/Rust нет
  значения `undefined`. JSON на проводе от этого не меняется.
- `SdkPromptContentBlock` пробует строгий inline image раньше открытого
  `ContentBlock`, иначе lossless owner поглотил бы обе формы. Inline-looking блок
  с неизвестным полем уходит в открытый owner и сохраняется без потерь; обычный
  durable image и неизвестные plugin blocks также остаются lossless.

## Исправления после ревью

- `SessionQueryResult::Prompt` использует `messageId` в обе стороны wire.
- Owner request-handler вызывается в reader синхронно до создания задачи ожидания,
  поэтому старт двух request предшествует следующей notification, как до первого
  `await` в JavaScript.
- Abort сразу снимает даже ещё не опрошенный request из `pending`; сторожевая задача
  завершается при abort или drop/completion самого request. Уже принятый response
  остаётся результатом request, если abort произошёл после его settlement.
- Прямые регрессии покрывают buffered-output flush, handler panic, заблокированную
  запись при close/EOF/input-error и отсутствие `params` у `notify(method, None)`.

## Не перенесено

Нет. Все runtime-экспорты и 19 source test cases перенесены. Проверки non-`Error`
throw используют нативную Rust ошибку с тем же наблюдаемым текстом: произвольное
брошенное JS-значение не существует в Rust.

## Не проверено

- Настоящие stdin/stdout и системный backpressure не запускаются; transport тесты
  используют только `tokio::io::duplex` и детерминированные `AsyncRead/AsyncWrite`.
- Сценарий construction/start вне Tokio runtime документирован возвращаемым
  `Result`, но намеренно не создаёт отдельный OS thread/runtime в тесте.

## Вопросы ведущему

- `SHIM(hoh-subagent)`: `SubagentStopReason` пока открытая `String`, что точно
  сохраняет declaration-merge-extensible source union. Заменить re-export типом
  владельца после появления `hoh-subagent`.
- Принятый ранее `hoh-sdk-jsonrpc-server` пока использует исторические SDK DTO и
  framing из `hoh-protocol`. Интеграцию на этот owner crate следует делать
  отдельным разрешённым кругом, не в рамках текущей границы.
