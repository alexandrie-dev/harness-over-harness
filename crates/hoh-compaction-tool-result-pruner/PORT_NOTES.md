# hoh-compaction-tool-result-pruner ← packages/compaction/compaction-tool-result-pruner @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` (класс `ToolResultPruner`, `inject = ['tokenMeter']`, `Config`) | `src/lib.rs`: сервис `ToolResultPruner` (`"toolResultPruner"`), значение `ToolResultPrunerRuntime`, плагин `ToolResultPrunerPlugin` (`NAME = "ToolResultPruner"`, как класс-сервис) | перенесён |
| `src/config.ts` (`PRUNE_MARKER`, `DEFAULTS`, `codePointLength`, `resolveConfig`) | `src/config.rs` | перенесён |
| `src/types.ts` | `src/types.rs` | перенесён |
| `locale/*.json`, `README*.md`, `README.i18n.yaml` | те же пути | скопированы, `diff`/`cmp` без различий |
| `tests/tool-result-pruner.spec.ts` (10 `it`) | `tests/tool_result_pruner.rs` (10) | перенесён полностью |
| `tests/loader-composition.spec.ts` (2 `it`) | `tests/loader_composition.rs` (2) | перенесён полностью |

Все экспорты образца перенесены; использующих их пакетов среди перенесённых пока нет (по `MAP.tsv`).

## Отличия от образца
- **Класс-сервис** (`new ToolResultPruner(ctx, config)`) — плагин, который предоставляет сервис. Конструктор
  значения: `ToolResultPrunerRuntime::new(Service<TokenMeter>, &ToolResultPruneConfig)`.
- **Конфигурация** — прозрачный `IndexMap<String, Value>` (как `TokenMeterConfig`): неизвестный ключ доходит
  до `resolve_config` и отвергается текстом образца. Схема — готовый JSON (`number`, `multipleOf: 1`, `minimum`,
  `default`), как `z.number().step(1).min(..).default(..)`. Тексты отказа schemastery при нецелом числе в
  плагине не воспроизводятся: нецелое отвергает `resolve_config` текстом образца.
- **`ResolvedConfig`** хранит `u64`; числа проверяются как `f64` до сведения (текст ошибки — `js_number_string`).
  `deepFreeze`/`structuredClone` не нужны: структура `Copy`.
- **`pruneContent`/`pruneSession`** возвращают `Result` вместо брошенных `Error` (`ToolResultPruneError` —
  простой `Error`, только сообщение). Сообщение и событие данных работают над JSON: ключ `content`/`message`
  заменяется на месте (`{ ...x, key }`), порядок ключей сохранён. `freezeMessage` — пустая операция.
- Блок `text` без строки `text` считается пустым (в образце `Array.from(undefined)` бросил бы `TypeError`);
  недостижимо для допущенных сессией сообщений.
- Типовые импорты `dsh-compaction`/`dsh-token-meter` (`import type {}`) выражены зависимостями crate:
  `CompactionPruneEvent` даёт полезную нагрузку `compaction/prune`.

## Не перенесено
Нет. `v8 ignore`-ветки (недостижимые защитные ошибки) перенесены, прямого теста у них нет.

## Не проверено
- Недостижимые ветки `failed to locate the removed text span` и `replacement must be smaller…` (в образце помечены
  `v8 ignore`).
- Платформенных веток нет.

## Вопросы ведущему
- `/dev/shm` был заполнен целиком другими писателями (0 байт свободно), а `target-s3` — ссылка в него. Сборка
  и тесты выполнены с `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-s3-disk` и
  `TMPDIR=/mnt/toshiba/alexandrie/hoh/tmp-s3`; свой старый кэш в `/dev/shm/hoh/target-s3/debug` я удалил.
- Cargo сам обновил `Cargo.lock` (новый крейт в workspace); вручную не правился.
