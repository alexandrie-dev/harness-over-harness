# hoh-session-projection-cache ← packages/session/session-projection-cache @ 39163dd3

## Файлы

| Образец | Перенос | Состояние |
|---|---|---|
| src/index.ts | src/lib.rs, src/service.rs | Service/plugin, identity-guarded reads, hydrate/cold paths, mandatory/count/interval write-behind, fail-soft warnings |
| src/spec.ts | src/spec.rs | Типизированные identity/record/row decoder-ы и domain spec `session_projcache` v7 |
| tests/cache.spec.ts | tests/cache.rs | Политика записи, listing/summary, lineage, hydrate/cold, failures и teardown |
| tests/fixtures.spec.ts | tests/cache.rs, tests/fixtures/ | Старые v3/v4/v5 документы, opaque v7, backup-and-skip |
| locale/*.json | locale/*.json | 8 файлов скопированы без изменений (`diff -rq` exit 0) |
| README.md, README.zh.md, package.json | Cargo.toml, этот файл | Прочитаны; runtime exports и storage layout сохранены |

В исходнике 39 runtime-групп / 48 развёрнутых случаев. Rust-тесты дополнены
review-регрессиями;
табличные варианты schema, identity, predecessor, archived versions и malformed
documents выполняются циклами. Проверяются настоящие Storage/JSON/Domain, Session и
SessionProjection плагины, а не отдельная имитация таблицы. Все временные данные —
только `tempfile::TempDir`. Пять fixture JSON скопированы byte-for-byte (`diff -rq`
exit 0).

## Отличия от образца

- Source class разделён на native `SessionProjectionCachePlugin`, injectable key
  `SessionProjectionCache` с тем же именем `sessionProjectionCache` и cloneable
  `SessionProjectionCacheService`. Методы с ошибкой возвращают `Result`; source
  исключения остаются теми же каналами ошибки, а `write` остаётся async.
- Zod schemas перенесены owner decoder-ами. Typed Rust record не может содержать
  JS function/symbol/bigint/cycle/array hole; представимые lossless-нарушения
  (negative zero, non-integral/negative counters, отсутствующий `val`) отвергаются.
  Неизвестные поля object schema strip-ятся; непрозрачные ключи и массивы в `val`
  сохраняются в исходном порядке.
- JS `Map<Session, DirtyState>` — коротко запираемый список по identity `Session`
  (`Session::eq` — Arc identity). Блокировка никогда не проходит через await.
  Отдельный synchronous owner не даёт native threads вклиниться между checkpoint
  и markClean. Timer — context-owned task с generation token и oneshot-cancel;
  `markClean` реально прекращает sleep, а teardown очищает dirty table до domain.
  Как в образце, отработавший interval остаётся в timer-слоте, если checkpoint
  упал до `markClean`: следующий event не пере-взводит его и не дублирует warning.
- Вызов `void flushSoft()` в JS исполняет checkpoint/markClean до первого await.
  Rust listener делает тот же синхронный префикс, затем регистрирует захват
  `tokio::Mutex` одним poll под synchronous owner. Сам `Sessions::flush`/domain
  `put` под `std::Mutex` не опрашивается: reentrant listener не самоблокирует runtime.
  FIFO permit сохраняет порядок подготовленных cut при задержанном flush, поэтому
  старый cut не затирает новый. Уже начатая запись не проверяет cancel token:
  teardown дожидается её, затем закрывает domain, как исходный reverse disposer.
- `coldSnapshot` выполняет synchronous restore полного хвоста под общим preparation
  lock. Это сохраняет атомарность restore/checkpoint относительно event path, но
  длинный cold restore одной сессии задерживает подготовку записей других.
- Public Rust `write()` — `async fn`: checkpoint/markClean начинаются при первом
  poll, не при создании неопросенного future. Все внутренние source-`void` пути
  делают подготовку сразу в listener; различие остаётся только у прямого Rust API.
- `coldSnapshot` write-back и write-behind представлены owned Context tasks. Ошибка
  содержится внутри задачи и пишет ровно одну строку с исходным текстом и JS-подобным
  `ErrorName: message`; event/read path её не получает. Kernel journal — диагностический
  stream, поэтому цепочка cause остаётся одним formatted record по docs/kernel.md §7.
- Source `ProjectionKeySelection` остаётся borrowed `List`/`Set`; ordered maps —
  `IndexMap`, watermark `-1` — `SessionSeqCursor::BeforeFirst`, ids/offsets — types
  из hoh-session. Header-only listing намеренно не сравнивает inherited cut; fold
  faces сравнивают его точно. Отсутствующая lineage трактуется как unseeded/cut 0.
- Допустимый Session header с integral-float `createdAt`/`delegationDepth`
  нормализуется на typed границе так же, как в hoh-session-projection; JavaScript
  не различает `1` и `1.0`. Для `setTimeout` значение выше 2^31−1 превращается в
  1ms, как Node; сам sleep идёт через `Context::sleep` и завершается при teardown.
  Config принимает integral JSON floats и натуральные `u64 >= 1` без прежнего
  выдуманного ограничения `MAX_SAFE_INTEGER`: source `z.natural().min(1)` тоже не
  задаёт maximum. Значения выше представимого `u64::MAX` остаются различием typed
  Rust-границы; для обоих runtime throttle это за пределом практического диапазона.
- Public domain spec — функция, возвращающая новый immutable `Arc<DomainSpec>`, а
  не JS module singleton. Значение декларации, version/layout/compat policy и типы
  таблицы одинаковы; identity объекта spec нигде в исходнике не наблюдается.

## Не перенесено

- Vitest spy call-shape (`toHaveBeenCalledExactlyOnceWith`) заменён наблюдаемыми
  durable rows и journal records. Timer-тесты используют paused Tokio time;
  отдельно проверено, что disposal отменяет armed timer. Это тестовый механизм,
  runtime timeout и порядок остаются теми же.
- Восемь JS-only верхнеуровневых значений из `it.each` (function, symbol, bigint,
  Date, Map и т. п.) не имеют типа `serde_json::Value`. Их boundary покрыт тем,
  что API принимает только JSON Value; negative zero и вложенные lossy shapes,
  которые Rust может представить, проходят runtime rejection.

## Не проверено

Node/Vitest и настоящий dsh composition не запускались. Windows/macOS не
проверялись; пакет не имеет platform branches. Сетевых и home-directory операций
нет.

## Вопросы ведущему

Нет. Публичные API hoh-session, hoh-session-projection и hoh-storage-domain покрыли
исходный контракт без shim и без правок чужих крейтов.

## Проверки

Все команды: `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w5`,
`CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true`, `--offline`; тесты запускаются под
`timeout`. Во время написания выполнялся только `cargo check --tests` (последний:
exit 0, Finished dev profile).

- `timeout 300s cargo test --offline -p hoh-session-projection-cache --no-fail-fast`:
  exit 0; 31 passed, 0 failed (unit/doc targets: 0/0).
- `timeout 300s cargo clippy --offline -p hoh-session-projection-cache --all-targets --no-deps -- -D warnings`:
  exit 0.
- `cargo fmt -p hoh-session-projection-cache -- --check`: exit 0.
