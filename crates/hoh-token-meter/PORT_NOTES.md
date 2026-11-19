# hoh-token-meter ← packages/llm/token-meter @ 39163dd3

## Файлы

| Образец | Перенос | Состояние |
|---|---|---|
| src/index.ts, src/types.ts | src/service.rs, src/types.rs, src/lib.rs | `TokenMeter` service/plugin, конфигурация без полей, replay-cache и публичные типы |
| src/estimate.ts | src/estimate.rs, публичный `estimate` | UTF-16/4 heuristic, role/block overhead, system/tools и JS JSON |
| src/surface-fold.ts, src/route-pricing.ts | src/surface_fold.rs, src/route_pricing.rs | Транзакционный positional fold, image/file occurrence pricing |
| src/surface-projection.ts | src/surface_projection.rs | Ограниченный shadow-price fold с точными диапазонами |
| src/measurement-projection.ts | src/measurement_projection.rs | Host-only v2 state, исходная ошибка при повторном replay |
| src/usage-projection.ts | src/usage_projection.rs | `tokenUsage` v2 и `contextPressure` v5 |
| src/breakdown-projection.ts | src/breakdown_projection.rs | `contextBreakdown` v6, system position и wire identity |
| src/turn-usage.ts, src/client.ts, src/projection.ts | src/turn_usage.rs, `client` в src/lib.rs | Browser-safe типы и полный attempt lifecycle fold |
| tests/*.spec.ts | 8 файлов tests/*.rs и unit-test service.rs | 98 именованных групп / 137 развёрнутых source-case представлены 75 Rust test-функциями; матрицы объединены в циклы |
| locale/*.json | locale/*.json | 8 файлов скопированы без изменений (`diff -rq`, exit 0) |
| README.md, README.zh.md, package.json | Cargo.toml, этот файл | Прочитаны; package entrypoints root/client/estimate сохранены |

Все 137 развёрнутых source-сценариев перенесены, 0 исключено. Однотипные
invalid-lifecycle, unsafe-count, config-key, header-reference и shadow-endpoint
варианты выполняются таблицами внутри Rust-тестов; дополнительно проверены scoped
caller context, освобождение dead-session cache, JS-number wire spelling и отсутствие
вызова image pricer для поверхности без изображений. После ревью отдельно закреплены
порядок двойной ошибки range/lifecycle, транзакционность локального replay,
несогласованный usage-checkpoint без unsigned panic, eager catch-up/reload и
`image/offload` через настоящий session fold с двумя одинаковыми occurrences.

## Отличия от образца

- Методы с исключением возвращают `Result<_, BoxError>`. Тип исходной ошибки —
  обычный `Error`, поэтому порт использует собственный `TokenMeterError`, не
  `HarnessError`. Тексты ошибок, включая malformed replay, range и config, сохранены.
- По docs/kernel.md §7 service proxy заменён явным `caller: &Context` у `measure`:
  scoped `Llm`, image pricing и file handle path выбираются по контексту вызывающего,
  а не владельца provider. `context_pressure` и чистый `estimate_message` контекст не
  принимают, поскольку не читают scoped services.
- JS `WeakMap<Session, ReplayState>` — `HashMap` с `Weak` identity. Перед каждой
  новой синхронизацией умершие ключи удаляются; поэтому повторное создание/удаление
  сессий не накапливает surfaces. Последний мёртвый хвост может жить до следующего
  чтения или unload, что не наблюдается API и заменяет недоступный native weak-key map.
- `deepFreeze(structuredClone(...))` заменён owned Rust snapshot без mutable alias.
  `Arc<Value>` сохраняет source `Object.is` identity для неизменившегося breakdown
  wire-view. Public measurement числа остаются `f64`, но integral значения
  сериализуются как JS `0`, а не `0.0`.
- Signed intermediate `contextPressure.surfaceTokens` хранится как `i64`: как в JS,
  отрицательный heuristic delta сохраняется внутри live fold, а только wire
  `projectedTokens` зажимается в ноль. Persisted schema, как source Zod, отрицательный
  state не принимает.
- Source WeakMap/eager listener и projection checkpoint разделены так же: cold cache
  может перечитать prefix, host v2 row хранит только header refs, step и usage anchor;
  mismatch любого reference сбрасывает rebuildable cache и повторяет расчёт с начала.
- Конфигурация — закрытый `TokenMeterConfig`; schema оставляет входные ключи до apply,
  чтобы выдать точный source-текст для первого неизвестного ключа. Первый ключ
  выбирается в порядке `Object.keys` (array-index имена сначала), а в сообщение он,
  как в template literal образца, вставляется без JSON escaping. `TokenMeterRuntime::new`
  представляет синхронное construction-time чтение с пустой конфигурацией.
- Локальный replay, как projection apply образца, запоминает `invalidSeq`, но перед
  повторным lifecycle throw успевает проверить surface plan. Поэтому при двух
  одновременных дефектах range-ошибка имеет исходный приоритет, а неуспешный plan или
  lifecycle не меняют surface/cursor. Source-тесты создают такой журнал приватным
  `appendUnchecked`; публичный Rust `Session` отвергает его раньше, поэтому те же
  переходы проверяются unit-тестами внутреннего replay без ослабления Session API.
- Replacement usage buckets вычисляются в знаковом `i128` и проверенно переводятся
  обратно в `u64`: несогласованный, но schema-valid checkpoint возвращает ошибку,
  а не паникует в debug и не переполняется в release.
- Все модули `src/*` доступны как Rust-модули; дополнительно сохранены публичные
  package entrypoints `client` и `estimate`. Среди портируемых потребителей найдены
  root service/types, `client::{TokenUsageProjection, ContextPressureProjection,
  derive_turn_token_usage}` и `estimate_content`; неиспользуемых package exports нет.

## Не перенесено

Только test-механика JavaScript: `Object.isFrozen`, mutation throws, Vitest spies и
WeakMap introspection заменены невозможностью mutable borrow, `Arc::ptr_eq`, реальными
projection snapshots и проверкой pruning. Runtime-сценариев, event names, projection
keys, model-visible текстов или protocol shapes не исключено.

## Не проверено

Node/Vitest и полная dsh composition не запускались. Платформенных веток у пакета нет.
Сетевых, home-directory и постоянных файловых операций нет. Locale проверены побайтно.

## Вопросы ведущему

Нет. API hoh-kernel, hoh-session, hoh-session-projection, hoh-llm и hoh-attachment
достаточны; shim нет, чужие крейты не менялись.

## Проверки

Все команды: `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w5`,
`CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true`, `--offline`; тесты — под `timeout`.
Во время написания использовался `cargo check --tests`; затем выполнены focused и
полный прогоны, включая исправление найденных ими ошибок тестовой оснастки.

- `timeout 300s cargo test --offline -p hoh-token-meter --no-fail-fast`: exit 0;
  75 passed, 0 failed (doc-tests: 0/0).
- `cargo clippy --offline -p hoh-token-meter --all-targets --no-deps -- -D warnings`:
  exit 0, `Finished dev profile`.
- `cargo fmt -p hoh-token-meter -- --check`: exit 0.
