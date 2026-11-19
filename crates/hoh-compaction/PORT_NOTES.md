# hoh-compaction ← packages/compaction/compaction @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/brand.ts` | `src/brand.rs` | `CompactionId` — отдельный branded newtype |
| `src/checkpoint.ts` | `src/checkpoint.rs` | constructor, predicate и чистый checkpoint-source тип |
| `src/types.ts` | `src/types.rs` | result и payload-типы четырёх `compaction/*` событий |
| `src/tool-pairing.ts` | `src/tool_pairing.rs`, `src/shim.rs` | баланс границ, corrupt-surface проверки, исходные тексты ошибок и отмеченный fallback недостающего cache API |
| `src/index.ts` | `src/engine.rs`, `src/lib.rs` | service key, object-safe backend seam, agent contexts, manual error и summary-error waterfall |
| `tests/compaction.spec.ts` | `tests/compaction.rs` | все 5 исходных случаев; плюс JSON key-order regression |
| `tests/tool-pairing.spec.ts` | `tests/tool_pairing.rs` | 9 из 11 исходных случаев; один cache case адаптирован, два fake-session cases исключены ниже |
| `README.md`, `package.json` | этот файл и `Cargo.toml` | прочитаны; `./types` и `./checkpoint` сохранены публичными модулями |

Отдельного `LICENSE` и каталога `locale/` в пакете нет.

## Отличия от образца

- Абстрактный Cordis `Service` представлен object-safe trait `CompactionEngine`, ключом `Compaction`
  с точным именем `compaction` и `provide_compaction`. Асинхронные методы возвращают `BoxFuture`; backend
  получает ссылки на owned Rust contexts. Это native-представление наследования и регистрации класса.
- `ManualCompactAgentContext` хранит базовый `CompactionAgentContext` и type-erased runner, а `Deref`/`AsRef`
  сохраняют исходное `extends`; generic `run_maintenance<T>` восстанавливает тип
  результата. Дополнительные `MaintenanceFuture`, `MaintenanceTask`, `MaintenanceRunner`,
  `CompactionAgentOptions` и `as_compaction_context` — Rust-представления анонимных source shapes,
  наследования интерфейсов и generic callback.
- Открытая `SessionEventMap` остаётся raw JSON в `hoh-session`; владельцу принадлежат типизированные
  `CompactionStartEvent`, `CompactionSummaryEvent`, `CompactionEndEvent`, `CompactionPruneEvent` и
  `ShadowedRange`, каждый с исходным event type. `CompactionSummaryOutput` не позволяет записать
  `llmStreamCall: false` или `true` без `rawOutput`. `True` — служебный marker этого закрытого инварианта.
- `CompactionCheckpointSource` — типизированный source; `From` переводит его в открытый
  `hoh_llm::MessageSource` с порядком `kind, compactionId, sourceCommandId?`. Predicate, как образец,
  проверяет только `kind` и не требует остальных полей. `CompactCheckpointKind` — Rust-тип фиксированного
  строкового discriminant вместо TypeScript literal type.
- `ManualCompactionError` остаётся plain Error, не `HarnessError`; `name`, шесть кодов, message и optional
  cause сохранены. Произвольное JavaScript throw-value в summary-error представлено `SharedError`.
- **SHIM(hoh-session):** у текущего `Session` нет weak lifecycle identity и plugin-owned extension slot.
  Поэтому два boundary helper не могут законно хранить модульный `WeakMap` образца без запрещённого
  глобального mutable registry. Они пересчитывают fold текущей поверхности при каждом вызове. Результаты,
  corrupt-surface отказы и тексты совпадают; отличается только число `event_at` чтений. После появления
  weak identity/slot перенести `BalanceCache` туда, не меняя публичные helper signatures.

## Не перенесено

- `tool-pairing.spec.ts`: exact assertion «does no event reads for unchanged or log-only growth…» адаптирован
  к тем же append/log-only/replace результатам, но счётчик чтений не проверяется из-за SHIM выше.
- Случай «rebuilds defensively when a same-generation surface entry count regresses» и часть случая
  «current surface seq has no matching event or indexes the wrong event» строят duck-typed fake `Session`,
  внутренне противоречащий публичным Rust-инвариантам `hoh-session`. Настоящий `Session` не позволяет
  уменьшить nodes без replace generation и всегда адресует `event_at(seq)` по тому же принятому seq.
  Проверка отсутствующего seq через настоящий пустой Session перенесена в соседний исходный случай.

Итого: 14 из 16 исходных `it` перенесены отдельными Rust-тестами; один из них адаптирован без performance
assertion. Два fake-object сценария не представимы публичным Rust API. Дополнительно один тест проверяет
байтово важные optional/key-order свойства payload.

## Не проверено

Платформенных веток нет. Cache-hit/read-count поведение ждёт weak lifecycle API `hoh-session`.
Полная интеграция с `compaction-basic` проверяется следующим пакетом.

## Вопросы ведущему

- Нужен публичный weak lifecycle identity или extension slot в `hoh-session` для точного per-session
  `BalanceCache`; чужой crate не менялся, глобальный registry не добавлялся.
- `hoh-compaction-image-offload/src/shim.rs` теперь может быть заменён реэкспортом
  `hoh_compaction::CompactionSummaryError`; это правка владельца чужого crate и здесь не выполнялась.
- Проверены прямые переносимые consumers: `compaction-basic` использует весь engine/result/checkpoint/tool
  contract; `command-compact` — `ManualCompactionError`; `session-reference` — predicate; token-meter,
  image-offload и pruner — event payloads. Из runtime exports образца без прямого внешнего named import
  остаются `CompactionTrigger` и `CompactionCheckpointSource`, но они входят в публичные method/data contracts.
