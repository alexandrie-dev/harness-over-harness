# hoh-tool-call-timeout-policy ← packages/guard/timeout-policy

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | policy plugin, `TOOL_TIMEOUT`, cooperative deadline wrapper |
| `tests/timeout-policy.spec.ts` | `tests/timeout_policy.rs` | все 12 исходных случаев и одна точностная регрессия |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | те же пути | побайтно перенесены |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json`, `tsconfig.json` | эти заметки, rustdoc, Cargo manifest | прочитаны целиком; npm/build metadata не копируется |

Использованы только path-crates рабочего пространства и уже зафиксированные workspace
dependencies. Новых внешних зависимостей нет, чужие crates не менялись.

## Соответствие поведения

- `ToolCallTimeoutPolicy` имеет `Plugin::NAME = "timeout-policy"`, `NoConfig` и типизированную
  зависимость `Service<Tools>`. Listener регистрируется как настоящий `ToolsExecute` waterfall и
  удаляется вместе с plugin fiber.
- Бюджет берётся с фактически видимого `ToolDefinition`: lookup учитывает scope вызывающего Agent
  и step-time `ResolvedToolSet`. Инструмент без `timeout_ms` делегируется без создания deadline.
- Для ограниченного инструмента `hoh_timeout::deadline` объединяет caller signal с таймером.
  `exec.set_signal` ставит производный сигнал только на время downstream dispatch; локальный RAII
  guard возвращает upstream signal при обычном результате, ошибке и отмене самого wrapper future.
- Policy ждёт завершения `next.run()` и потому остаётся строго cooperative: проигнорировавший
  отмену tool продолжает удерживать caller. После quiescence результат заменяется только если
  `timeout_of(..., Some("TOOL_TIMEOUT"))` распознал таймер именно этого policy. Более ранняя отмена
  caller или внешний deadline с другим code сохраняют штатный `ABORTED`.
- Timeout формирует точный model-facing текст `Error: tool call timed out after <ms>ms` и
  структурированный `ToolTimeoutError` / `TOOL_TIMEOUT`; downstream provider error при собственном
  срабатывании policy также заменяется этим результатом. Числовая вставка использует канонический
  `js_number_string`, то есть повторяет ECMAScript `Number::toString` и для дробных/экспоненциальных
  значений.

## Отличия от TypeScript

- JavaScript namespace-проверки (`default` export и `Loader.unwrapExports`) не имеют runtime-аналога
  в Rust. Вместо них тест проверяет `Plugin::NAME`, owned code и загрузку реального plugin через
  `Kernel` вместе с `SystemPrompt`, `ToolRuntime` и `NativeToolExecutorPlugin`.
- У `AbortSignal` Rust-порта намеренно нет публичного identity comparison. Тесты проверяют
  наблюдаемую семантику производного сигнала (его `TimeoutReason`), восстановление сигнала для
  `tools/post-execute` и последующую propagation отмены caller.
- Добавлена отдельная регрессия на внешний deadline с иным code: она закрепляет описанное в исходнике
  scoped-classification поведение, не меняя публичный контракт.
- После независимого ревью добавлена регрессия для `timeout_ms = 1e-7`: обычный Rust formatter
  выдавал `0.0000001`, тогда как template interpolation образца требует `1e-7`.

## Не перенесено

Пропущенных поведенческих test declarations нет. README и npm metadata не дублируются в Rust-крейте;
их контракт отражён в rustdoc и этом файле. Конфигурации и model-visible schema у policy нет.

## Вопросы ведущему

Нет. Актуальные публичные API `hoh-tools` и `hoh-timeout` достаточны; правки чужих crates не нужны.

## Проверки

Все Cargo-команды выполнены с `CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true` и
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x7`; тесты запускались только под `timeout`.

- `cargo test --offline -p hoh-tool-call-timeout-policy` — 13 passed, 0 failed; unit/doc 0/0.
- `cargo clippy --offline -p hoh-tool-call-timeout-policy --all-targets --no-deps -- -D warnings` — exit 0.
- `cargo fmt -p hoh-tool-call-timeout-policy -- --check` — exit 0.
- `cargo check --offline --workspace --all-targets` — exit 0 после завершения параллельной работы
  владельца `hoh-mcp-manager`; чужие предупреждения не относятся к этому крейту.
- `diff -ru <source>/locale crates/hoh-tool-call-timeout-policy/locale` — exit 0.
