# hoh-session-persistence ← packages/session/session-persistence @ 39163dd3

## Файлы

| Образец | Перенос | Состояние |
|---|---|---|
| src/index.ts | src/lib.rs | Service seam, identity, snapshot/options/storage metadata/inspection, все re-exports |
| src/handle.ts | src/handle.rs | read/write access, immutable event graphs, read/append/flush/close |
| src/errors.ts | src/errors.rs | Все восемь plain Error subclasses и direction-aware format refusal |
| src/revision.ts | src/revision.rs | Domain-owned branded string SessionPersistenceRevision |
| src/storage-contract.ts | src/storage_contract.rs | Все шесть shared helpers, fail-closed vocabulary pre-scan до adoption |
| tests/storage-contract.spec.ts | tests/storage_contract.rs | 16 из 17 исходных случаев, плюс 2 проверки header/identity/wire shape |
| tests/contract.ts, tests/live-write-contract.ts | Учтены как provider test suites | Прочитаны полностью; функции объявляют тесты только при вызове factory из backend-пакета, в данном пакете не вызываются |
| README.md, package.json | PORT_NOTES.md, Cargo.toml | Прочитаны, дополнительных runtime exports и locale нет |

## Отличия от образца

- Abstract Service выражен object-safe trait SessionPersistence и kernel key
  SessionPersistenceService (`sessionPersistence`). Сам seam не создаёт backend,
  не ставит live listeners и не владеет файлами — это исходная граница пакета.
  Все async методы возвращают BoxFuture; backend возвращает Arc<dyn SessionHandle>.
- SessionPersistenceIdentity — дополнительный Rust-тип вместо Symbol. Клоны
  сравниваются по одному Arc, новая identity всегда отличается. Новый backend
  хранит один token и возвращает его clone; context service views его не заменяют.
- SessionFormatStatus — Rust enum для current/migration-required. SessionHeader,
  SessionId, SessionLogOffset, SessionEvent и SessionSeedEventState берутся из
  hoh-session; бренд id не объявляется повторно. Вызов revision-конструктора — From.
- Events в handle read/append — Arc<SessionEvent>, outer Vec принадлежит вызывающему.
  SessionEvent не предоставляет mutable graph; Object.freeze заменён ownership.
  validate_stored_events возвращает тот же mutable slice; clone перед adoption
  не меняет identity элемента/массива и сохраняет вход при отказе. Пропуск required
  unknown и retired fallback проверяется перед validation любого record.
- Ошибки наследуют обычный Error в исходнике: здесь отдельные thiserror structs,
  не HarnessError. name() сохраняет имена; session_id и artifact location доступны,
  corruption хранит исходную BoxError cause. Arc<SessionLocation> сохраняет identity.
  Неподдерживаемый формат не превращается в corruption. Storage helper plain
  Error/TypeError выражены существующим hoh_session::SessionError.
- Header — текущий typed SessionHeader; созданный typed header и event snapshots
  копируются без потери extra JSON. Проверка lossless JSON пользуется общим
  snapshot_json_value. createdAt проверяется как non-negative safe integer на сериализованном
  снимке (после слияния `extra`, как `snapshot.createdAt` в storage-contract.ts:117),
  а не на типизированном поле: `extra["createdAt"]` перекрывает его при записи.
  `materialize_create_header` возвращает именно этот слитый снимок, как
  storage-contract.ts:120, поэтому перекрытое значение видно и вызывающему.
  Read offsets/lengths — u64; отрицательные и дробные значения в typed API не
  представимы, backend всё ещё обязан проверять safe-integer верхнюю границу.
- Snapshot wire fields идут header, formatStatus?, revision, eventCount?, sizeBytes?,
  location?; отсутствующие поля не сериализуются. AsyncDispose — явный close().
  Никакой async работы в Drop и никаких глобальных mutable registries нет.

Потребители src проверены в session-persistence-jsonl, agent-loop, session-query,
session-log-export, schedule, message-feedback и других переносимых пакетах.
Seam сохраняет весь публичный словарь, включая редко используемые ошибки владения;
read/header metadata types используются также backend test harnesses. Type-only
options — Rust structs, default export — trait, src/* outlets — public modules.
Механическая проверка named imports в 18 внешних переносимых src-файлах не нашла
прямых imports assertVersion, SessionOwnershipLostError и re-export SessionHeader
именно из этого seam; они всё равно сохранены (Header обычно берут у hoh-session).

## Не перенесено

- Единственный исходный spec-case passes_an_adoption_side_format_refusal_through_unwrapped
  бросает ошибку из JS getter event.data. Accessors, arbitrary throws и monkey-patch
  adoption не представимы в immutable SessionEvent (PORTING §8); сам passthrough
  branch сохранён. Остальные 16 случаев представлены.
- Семь JS-only badValues (BigInt, undefined в объекте, Infinity, function, Symbol,
  Map, cycle) из rejects_every_non_json_value не представимы в Value. Случай
  адаптирован к отрицательному нулю — остающемуся non-lossless значению общего helper.
- Factory suites contract.ts/live-write-contract.ts не являются запущенными здесь
  spec-файлами: их provider-specific instantiations и storage/live routing проверки
  относятся к backend-пакету. Новый in-memory или mock storage runtime ради этих
  suites в production seam не добавлялся. Долговечность seam сам не реализует.

## Не проверено

Реальные файлы, leases, torn-tail repair, live buffers и provider-wide AggregateError
проверяются поставщиком, не abstract seam. Платформенных веток и locale здесь нет.
Общий conformance/minimal-profile milestone требует ещё JSONL backend и agent loop;
этот crate предоставляет их контракт, а не запускает состав самостоятельно.

## Вопросы ведущему

Недостающих API зависимостей пока не найдено; shims не понадобились.
Provider должен хранить stable SessionPersistenceIdentity и регистрироваться через
SessionPersistenceService. Чужие crates не редактируются.

## Второй круг ревью

- Исправлён результат `materialize_create_header`: прежний код проверял слитый
  JSON-снимок, но возвращал исходный typed header. Регрессия
  `fix2_create_header_returns_the_merged_snapshot` с `created_at=-1` и
  `extra.createdAt` в формах `7`/`7.0` сначала получила `-1`, теперь получает `7`;
  входной header остаётся неизменным.

## Проверки

Окружение всех Cargo-команд: CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w3. Во время написания — только
cargo check --tests; финальные проверки выполняются после завершения кода по
plans/briefs/interface-first.md.

- `cargo check -p hoh-session-persistence -p hoh-session-projection --tests`:
  exit 0, Finished dev profile, 9m 51s; текущие API зависимостей приняты без правок.
- `RUST_TEST_THREADS=2 cargo test -p hoh-session-persistence -p hoh-session-projection`:
  exit 0, persistence `18 passed; 0 failed; 0 ignored`; unit/doc targets без случаев
  также успешны. Совместная test-сборка заняла 20m 21s.
- `cargo fmt -p hoh-session-persistence -p hoh-session-projection -- --check`:
  exit 0, без вывода.
- `cargo clippy -p hoh-session-persistence -p hoh-session-projection --all-targets -- -D warnings`:
  финальный exit 0, Finished dev profile, 6m 47s, предупреждений нет. Первый общий
  запуск отказал только из-за одного style lint в projection; persistence правок
  после test не потребовал.

Проверка второго круга теми же переменными окружения:

- `RUST_TEST_THREADS=2 cargo test --offline -p hoh-session-persistence -p hoh-session-projection --no-fail-fast`:
  exit 0; persistence `20 passed; 0 failed`, projection `83 passed; 0 failed`,
  doc targets без случаев успешны; всего 103/0.
- `cargo clippy --offline -p hoh-session-persistence -p hoh-session-projection --all-targets -- -D warnings`:
  exit 0, Finished dev profile за 13.85s, предупреждений нет.
- `cargo fmt -p hoh-session-persistence -p hoh-session-projection -- --check`:
  exit 0, без вывода.
