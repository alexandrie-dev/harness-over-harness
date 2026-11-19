# hoh-session-telemetry ← packages/session/session-telemetry @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs, src/redaction.rs | record/backend vocabulary, service key `sessionTelemetry`, sharing status и синхронная redaction-цепочка |
| src/coordinator.ts | src/coordinator.rs | live/on-demand capture, adoption, lifecycle ops, handoff cursor, severity и containment |
| tests/telemetry.spec.ts, tests/redact.spec.ts | tests/telemetry.rs | все 35 объявлений и все строки `it.each`: 41 исходный сценарий плюс 2 review-регрессии, итого 43 Rust-теста |
| README.md, README.zh.md | — | прочитаны; собственного locale и LICENSE у пакета нет |

Сохранены ordered ledger/ops records, отделение envelope от body, порядок identity attributes, append-time и on-demand redaction, fail-closed отдельной записи, suffix capture через handoff cursor, включение истории только по опции, flush hint, shutdown до backend drain и изоляция ошибок от session/agent loop.

## Отличия от образца

- TypeScript `SessionTelemetryBackend` разделён на объектобезопасный `SessionTelemetryBackendApi` и kernel service key `SessionTelemetry = "sessionTelemetry"`; `SessionTelemetrySink::{emit,flush}` возвращают `Result`, а `shutdown` — `BoxFuture`, то есть JS throw/rejection выражены типизированно.
- `SessionTelemetryCaptureOptions` — полный Rust value с `Default` (`Live`, `include_history = false`) вместо объекта с optional полями. `SessionTelemetryChannel`, severity, sharing и string/number attribute представлены enum-типами; `IndexMap` сохраняет insertion order.
- `SHIM(hoh-kernel)`: текущий kernel `Waterfall` только асинхронный, а исходный `session-telemetry/record` обязан завершиться синхронно внутри `session/event`. Поэтому публичные `SessionTelemetryRedactions::register` и `SessionTelemetryCoordinator::new_with_redactions` дают lifecycle-owned синхронную цепочку с точным outermost-first/skip-next/fail-closed поведением. Обычный `new` соответствует отсутствию правил. Имя `session-telemetry/record` сохранено как label эффекта. После появления sync-waterfall shim нужно заменить на kernel event.
- Module-scope handoff `WeakMap<Session, seq>` представлен process-wide списком `Weak<Value>` к стабильному `Arc` заголовка exact Session. Он переживает замену coordinator, не удерживает Session и не переносит cursor на свежий объект с тем же id.
- Произвольное JS thrown value в Rust backend/rule не существует: граница принимает `BoxError`. Не-Harness ошибки нормализуются как `{ name: "Error", message }`; `HarnessError` и `SessionError` сохраняют имя.
- Как и `String(new Error(''))` в образце, пустая ошибка форматируется как `Error` без завершающего `: `.

## Исправления по ревью

- `session/disposed` больше не удаляет handoff cursor. Это соответствует module-scope
  `WeakMap` образца: cursor принадлежит объекту Session, а не одному coordinator, и
  исчезает только после уничтожения самого объекта. Регрессия с live-coordinator A,
  on-demand coordinator B и disposal общей Session доказывает отсутствие повторной
  отправки уже принятого seq.
- Тесты fork и restore теперь строят реальные `SessionStore::fork` / `Session::from_restore`
  объекты и сверяют точные seq, типы, body, constructor boundary, повторный capture и
  handoff после reload. Прежние alias-вызовы удалены.
- Контракты envelope/body cloning, severity, assistant stream order, prefix idempotency,
  late redaction, on-demand isolation, shutdown ops и warning text теперь проверяются
  полными утверждениями образца.

## Решения по заметкам ревью

- `Debug` оставлен производным: backend получает запись уже после redaction, а
  deployment-rule по определению является доверенной частью export pipeline. Сам крейт
  не пишет record в лог; скрывающий поля `Debug` создал бы отдельное от TS публичное
  представление, не устраняя доступ sink/rule к тем же данным.
- Линейный weak-list cursor остаётся частью записанного `SHIM(hoh-kernel)`. Записи
  очищаются по умершим `Weak` при каждом доступе, mutex не удерживается через `await`,
  а замена структуры до появления общего sync-waterfall усложнила бы pointer-identity и
  риск повторного использования адреса без изменения наблюдаемого контракта.
- Подключение listeners до обнаружения отсутствующего `Sessions` сохранено: это точный
  порядок конструктора TS. Отдельная rollback-семантика здесь была бы расхождением.
- Rust `flush` синхронен и возвращает `Result`, поэтому coordinator физически не может
  ожидать backend async-work; тест фиксирует немедленный одинарный hint. Это типизированный
  эквивалент JS fire-and-forget контракта.

## Не перенесено

Нет исключённых production-экспортов или тестовых сценариев. TypeScript module augmentation заменена типизированными Rust service/record API. Публичные `SessionTelemetryRedactions` и `RedactionNext` — записанный выше временный API для отсутствующего sync-waterfall.

## Не проверено

Платформенных веток, сети и файлового I/O нет. Реальная очередь/transport не входят в этот пакет; fake backend проверяет non-awaiting flush, shutdown ordering и независимые отказы записей.

## Вопросы ведущему

- Нужен sync `Waterfall` в `hoh-kernel`, чтобы будущий `hoh-session-telemetry-otel` мог монтировать deployment rules через общий Context без явной передачи `SessionTelemetryRedactions`. Чужие файлы не менялись; до такого API следует совместно передавать один registry в redaction plugin и coordinator.
- Новых внешних зависимостей нет.

## Проверка

- `cargo test -p hoh-session-telemetry --offline`: 43 passed, 0 failed.
- `cargo clippy -p hoh-session-telemetry --all-targets --no-deps --offline -- -D warnings`: чисто.
- `cargo fmt -p hoh-session-telemetry -- --check`: чисто.

Все команды выполнялись с `CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x5`, offline и под внешним `timeout`.
