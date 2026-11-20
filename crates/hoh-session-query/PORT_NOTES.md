# hoh-session-query ← packages/session-query/session-query @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/config.ts`, `src/cursor.ts`, `src/types.ts` | `src/error.rs`, `src/types.rs`, `src/lib.rs` | закрытые коды ошибок, константы, cursor и публичные exact/search records перенесены |
| `src/extraction.ts`, `src/documents.ts`, `src/filters.ts`, `src/sources.ts` | `src/helpers.rs` | semantic text, surface classification, literal filters и проверка совместимости источников перенесены |
| `src/tracing.ts` | `src/helpers.rs` | lineage и event relationship traces, включая итеративное дерево глубины 3000, перенесены |
| `src/cold-read.ts`, `src/corpus.ts` | `src/corpus.rs` | close-on-all-path cold read, live precedence, dynamic optional persistence и bounded title batch перенесены |
| `src/observation.ts` | `src/observation.rs` | live/prepared cuts, revision+identity cache, retain/dispose leases, projection hydration и pin-safe LRU перенесены |
| `src/index.ts` | `src/engine.rs` | единый exact/search service, exact reads и Rust-композиция search backend перенесены |
| `tests/search-helpers.spec.ts` | `tests/search_helpers.rs` | semantic extraction, фильтры, владение обоими видами фильтров и Unicode parity перенесены |
| `tests/tracing.spec.ts` | `tests/tracing.rs` | lineage и event tracing, включая всю таблицу malformed surface, перенесены |
| `tests/observation.spec.ts` | unit-тест `src/observation.rs`, `tests/engine.rs`, `tests/persistence.rs` | live/prepared/cache/persistence, projection failure/hydration, attach-races, error taxonomy и inspect lease lifecycle перенесены |
| `tests/session-query.spec.ts`, `tests/test-service.ts` | `tests/engine.rs`, `tests/persistence.rs` | exact API, in-flight cancellation с ожиданием cleanup, bounded title batch и service lifecycle перенесены |
| `README*`, `package.json`, `tsconfig.json` | `PORT_NOTES.md`, `Cargo.toml` | прочитаны; package metadata перенесены в Cargo manifest |

Итого: все 84 исходные декларации `it` аудированы и представлены 91 Rust test case;
один malformed-title вариант внутри исходной декларации заблокирован поведением зависимости и указан ниже.
После первого ревью race/cancellation и observation-варианты больше не считаются перенесёнными только за счёт
сгруппированного smoke-теста.

## Отличия от образца

1. TypeScript abstract `SessionQueryEngine`, который backend наследует и сам публикует под одним ключом,
   представлен concrete `SessionQueryEngine` плюс object-safe `SessionSearchProvider` и
   `mount_session_query`. Это публичный Rust-адаптер для будущего `hoh-session-query-sqlite`: exact и search
   остаются одним значением `SessionQuery = "sessionQuery"`; duplicate provide fail-loud, disposer принадлежит
   owner context. Отдельного параллельного search service нет.
2. `SessionObservation` — RAII-lease с явными `retain()`/`dispose()` и `Drop`. Каждый `retain` создаёт независимый
   lease; retain после dispose отклоняется. `Arc` удерживает заменённую prepared revision до освобождения всех
   leases, `AtomicUsize` реализует исходный `refs`.
3. TypeScript runtime-malformed union values нельзя создать через публичные Rust enums. Numeric range и empty
   text по-прежнему проверяются с `SESSION_QUERY_INVALID_FILTER`; неизвестные shape/discriminant отклоняет serde
   до вызова API.
4. Все `String.trim` и `/\s+/u` из extraction/filter перенесены через
   `hoh_util_values::{js_trim,is_js_white_space,JS_WHITE_SPACE_SET}`. Дополнительные тесты фиксируют U+FEFF как
   ECMAScript whitespace и U+0085/U+001C как не-whitespace.
5. Оба места `localeCompare` (корпус при равном `createdAt` и siblings lineage) используют `feruca` 0.13:
   CLDR-root, non-ignorable, stable sort, без byte-tiebreak при collation equality. Это разрешённое workspace
   приближение Node ICU; конкретная пользовательская locale не поддерживается, как и в других Rust-портах.
6. `SessionLineageCompletion` и fulfilled title observation boxed только для уменьшения размера Rust enum;
   JSON/смысловой контракт не меняется.
7. `SessionQueryError` — marker, чей constructor возвращает единый `HarnessError` с name
   `SessionQueryError`, как требует правило для TypeScript subclasses. Это намеренно не downcastable wrapper;
   общая маршрутизация использует `HarnessError::of`, `is_named` и `code`.
8. Рекурсивный lineage node имеет итеративный `Drop`, поэтому обычное освобождение дерева глубины 3000 не
   расходует Rust call stack. Построение также итеративно.
9. `SessionEventTraceObservation` хранит `trace: SessionEventTrace`, тогда как TypeScript разворачивает поля trace
   рядом с `session`. Тип пока не имеет serde wire-контракта; потребители обращаются к типизированным полям Rust.

## Не перенесено

- TypeScript declaration merging и возможность передать malformed `as never`: это compile-time/runtime
  особенность JavaScript. Wire discriminants и закрытые Rust enums сохранены.
- JavaScript promise может reject любым non-`Error` значением; Rust persistence seam возвращает
  `BoxError`, поэтому TS-вариант `statFailure = "offline"` невыразим. Taxonomy для всех выразимых
  persistence errors проверяется.
- npm package-export/loader smoke отсутствует в пакете образца как отдельный e2e; Rust service registration
  проверяется напрямую через kernel.

## Не проверено

- Интеграция с принятым `hoh-session-query-sqlite` проверяется его собственными package gates; этот крейт
  проверяет backend seam через тестовый `SessionSearchProvider`.
- Node ICU не запускался; `feruca`-приближение и его ограничение зафиксированы выше.
- Платформы кроме Linux; платформенного кода в крейте нет.
- Drop future посреди `SessionHandle::read` отдельно не проверяется: публичная отмена кооперативна, дожидается
  backend future и закрывает handle; обязательного async-close в `Drop` seam зависимости не предоставляет.
- У `regex::RegexBuilder` остаётся стандартный предел размера автомата. Очень длинный допустимый JS literal
  может получить `SESSION_QUERY_INVALID_FILTER`; повышать предел без общего resource-limit решения не стали.

## Вопросы ведущему

- `hoh-session-title::fold_session_title` превращает malformed `session/title` в `None`, тогда как TS `foldSessionTitle`
  может бросить, а `projectMany` изолирует этот throw как rejected по id. Проверку из `session-query.spec.ts:795`
  нельзя воспроизвести без исправления принятого зависимого crate; `hoh-session-query` чужой crate не меняет.
- Остальные API зависимостей достаточны; `hoh-session-query-sqlite` реализует текущий `SessionSearchProvider`.

## Круг исправлений по ревью 024142

- Text-фильтр материализуется без компиляции; literal regex строится только после `SessionCorpus::load` и surface
  projection. Поэтому missing session, persistence/corruption и invalid surface имеют тот же приоритет, что в TS.
- `SessionCorpus::load` клонирует detached events без преждевременной admission-валидации, как TS
  `structuredClone`; поэтому `readSession` валидирует seed, а surface API оборачивают fold-ошибку в `InvalidSurface`.
- `inspect_session` после acquire вызывает `AbortSignal::throw_if_aborted` напрямую: возвращается исходная reason
  с сохранением identity, getter не вызывается, RAII-lease освобождается при abort, return и panic.
- Добавлены явные проверки in-flight cancellation/cleanup для пяти exact/list операций, bounded title workers,
  fold/release до следующего queued read, signal-aware stalled read/listing, drain без запуска очереди,
  late live attach во время read и после listing miss, prepare/live collision, stat→open deletion, projection
  registry/cache hydration, restore/error taxonomy, exact malformed reads и optional-persistence lifecycle.

## Проверка

Все Cargo-команды выполняются offline с `CARGO_BUILD_JOBS=2` и
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x9`; тесты — только под `timeout`.
