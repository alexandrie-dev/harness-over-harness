# hoh-session-query-sqlite ← packages/session-query/session-query-sqlite @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs`, `src/provider.rs` | конфигурация, единый `sessionQuery`, live-preferred reconciliation, поколения, курсоры и оба search API перенесены |
| `src/query.ts` | `src/query.rs` | нормализация, параметризованные predicates, лимиты SQLite, fingerprint и Unicode-snippet перенесены |
| `src/schema.ts` | `src/schema.rs` | application id, schema version, FTS5 `unicode61`, TEMP overlay, reset и защита чужих БД перенесены |
| `locale/*.json` | `locale/*.json` | восемь файлов скопированы без изменений |
| `tests/query.spec.ts` | `tests/query.rs` | контракты 10 исходных случаев перенесены в параметризованные проверки нормализации, владения значениями, predicates, лимитов, fingerprint и Unicode-snippet |
| `tests/sqlite.spec.ts` | `tests/sqlite.rs`, `tests/schema.rs`, unit-тесты `provider.rs` | 57 исходных случаев перенесены группами: schema/permissions, open policy, real FTS5/surfaces, pagination/cursors, live-preferred reconciliation, topology/revision races, cancellation/close и реальный JSONL/reopen |
| `README*`, `package.json`, `tsconfig.json` | `Cargo.toml`, этот файл | прочитаны; метаданные перенесены |

## Отличия от образца

1. Наследование TypeScript `SqliteSessionQueryEngine extends SessionQueryEngine` представлено реализацией
   `SessionSearchProvider`, переданной в `hoh_session_query::mount_session_query`. Exact-read API и search API
   остаются одним сервисом `sessionQuery`, как предписывает публичный API зависимого крейта.
2. `node:sqlite` заменён на одобренный ведущим `rusqlite 0.43` с `bundled`; схема и SQL сохранены. UUID экземпляра
   курсора заменён уникальным в процессе значением `pid + AtomicU64`: курсоры остаются непрозрачными и не
   принимаются другим экземпляром.
3. `localeCompare` при канонизации filters приближён сортировкой Rust по Unicode scalar values. Для ASCII
   discriminants, путей и идентификаторов из тестов результат совпадает; различия ICU для не-ASCII возможны.
4. `rusqlite` синхронный. Все операции одного provider сериализованы `tokio::sync::Mutex`, а открытие,
   reconciliation, запросы и закрытие выполняются через `spawn_blocking`; блокировка состояния не удерживается
   через `.await`. Это сохраняет исходную очередь, не блокируя async runtime.
5. Единственный экспорт TS `requestFingerprint` представлен двумя типобезопасными Rust-функциями
   `request_fingerprint_session` и `request_fingerprint_event`, поскольку Rust не перегружает функции по
   размеченному объединению аргумента.

## Исправления ревью 003710

- Смена `sessionPersistence` принудительно заменяет загруженные строки даже при совпавшей opaque revision;
  это проверено поиском нового текста и исчезновением старого.
- Ошибки `open`/`read`/`close` холодного журнала получают точные `SESSION_QUERY_PERSISTENCE_FAILED` либо
  `SESSION_QUERY_ABORTED` с исходной причиной.
- Отмена после второго `list`, включая игнорирующий сигнал успешный ответ, проверяется до reconciliation;
  дубликаты второго snapshot-list дают typed failure, и оба случая не оставляют записей в индексе.
- Рекурсивное создание всех новых уровней каталога использует `0700`; файлы БД и sidecar — `0600`.
- Нормализация обоих search scope валидирует filters до query, как TypeScript-образец.
- SQLite reconciliation errors сохраняют точный префикс образца без внутренних operation-labels.

## Исправление redrun 012459

- `tests/query.rs` использует импортированные aliases `Range`, `Filter` и `EventFilter` в
  `reports_invalid_filters_before_an_invalid_query_in_both_scopes`. До исправления добавленная
  регрессия ссылалась на полные имена без соответствующих импортов: обычный тестовый прогон ведущего
  завершился раньше этой правки, а последующий `clippy --all-targets` увидел промежуточный файл и
  закономерно отказал с E0422/E0433. Актуальный test-target компилируется и проверяет приоритет
  `SESSION_QUERY_INVALID_FILTER` над пустым query для обоих search scope.

## Не перенесено

- `tests/load-path.e2e.ts`: npm/package loader в Rust отсутствует. Его наблюдаемая часть — keyless-композиция
  реальных JSONL persistence + SQLite search, повторное открытие и смена store — перенесена в `tests/sqlite.rs`.
- Runtime-unknown discriminants из `query.spec.ts` не создаются безопасным публичным Rust API: закрытые enum
  `SessionResultFilter`/`SessionEventMetadataFilter` отсекают их при десериализации. Остальные наблюдаемые
  проверки выполняются через публичные границы, без открытия приватного состояния provider.

## Не проверено

- Файловые режимы проверены на Unix; ветка Windows создаёт те же каталоги/файлы без POSIX mode bits.
- Внутренние TypeScript-поля `_db`, `_ready` и `_optionalPersistenceFiber` недоступны из публичной Rust-границы;
  соответствующие наблюдаемые контракты проверяются через plugin settlement/disposal, файловую БД и provider-unit.

## Вопросы ведущему

- Нет; новых внешних зависимостей и правок чужих API не требуется.

## Проверка

Все команды выполняются offline с `CARGO_BUILD_JOBS=2`, тесты — под `timeout`, target — `target-x4`.
