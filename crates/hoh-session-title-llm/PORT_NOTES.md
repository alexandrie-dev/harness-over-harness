# hoh-session-title-llm ← packages/session/session-title-llm @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | перенесён |
| `tests/llm.spec.ts` | `tests/session_title_llm.rs` | 16 runtime-сценариев перенесены в 10 сгруппированных Rust-тестов |

## Отличия от образца

- `SessionTitleLlmConfigSchema` и `SessionTitleLlmConfigFields` представлены
  `JsonSchema` на `SessionTitleLlmConfig`: required-поля, `additionalProperties:
  false`, `minimum`, `maximum` и `multipleOf` сохраняют схемный контракт.
- Отдельный `resolve_session_title_llm_config_value` сохраняет точные
  ошибки JavaScript direct-construction для нетипизированного JSON; обычный
  Rust-вызов использует типизированную функцию.
- Закрытый TypeScript union успешных finish-причин представлен
  `SessionTitleLlmFinish`; исходный `FinishReason` целиком сохраняется
  внутри варианта. `SessionTitleLlmFinishError` явно хранит динамическое
  JS-поле `error.code`.
- Declaration merging для purpose, message source и event map в Rust не
  требуется: владельцы используют открытые string-имена, а payload
  типизирован публичной структурой.
- После точных JS-проверок целые execution limits хранятся как
  `u64`, потому что API `hoh-llm` использует `u64` для `max_tokens`;
  непредставимые в Rust целые выше этого диапазона отклоняются.

## Круг исправлений review 201159

- Нетипизированный config повторяет `Object.keys` для JSON-массивов: пустой
  массив доходит до ошибки отсутствующего `maxInputBytes`, непустой отклоняется
  как неизвестный ключ `"0"`. JavaScript `undefined` не представим в
  `serde_json::Value`; `null` и строковый non-object покрывают ту же boundary.
- Ошибка элемента stream теперь разбирается до проверки уже сработавшего
  deadline, как отклонённый `next()` в `for await`: исходные message/code не
  подменяются `SESSION_TITLE_TIMEOUT`. Ошибки самого adapter workspace-runtime
  нормализует в `FinishReason::failed`; этот путь отдельно сохраняет code.
- Восстановлены оракулы исходных сценариев: typed/untyped config matrix,
  ceiling и unset reasoning selectors, оба conversation `maxTokens`, exact
  explicit-route event и синхронизированная caller cancellation.

## Круг исправлений review 203202

- `timeoutMs` валидируется как JS integer без промежуточного ограничения
  `u64`; поэтому любое целое выше `MAX_TIMER_DELAY_MS`, включая `1e20`, даёт
  точную ошибку `must not exceed 2147483647`. Ограничение `u64` остаётся только
  у byte/token caps, которые передаются в соответствующие Rust API.
- Ошибка `LlmStream::close` при abrupt completion теперь подавляется, как
  неудачный iterator `return()` при исходном throw в JS: caller abort и ошибка
  элемента stream сохраняют собственные message/code. Обе ветки закреплены
  stream cleanup, который намеренно возвращает `close failed`.
- Typed/untyped config, unset selector при непустом effort catalog,
  отсутствие `reasoningEffort` в событии и exact max-token blocks закреплены
  расширенными матрицами. JSON-массивы уже покрыты предыдущим кругом.
- Прямой serde boundary для `SessionTitleLlmConfig` трактует явные
  `provider: null`/`model: null` как отсутствие. Loader и публичная value-функция
  отклоняют `null`, как TS; различие остаётся только у типизированного
  Rust-deserialize, который не является JS direct-construction API.

## Круг исправлений review 203903

- Ревью читало предшествующий снимок: оба runtime-исправления уже находятся в
  `src/lib.rs`, а регрессии `1e20` и падающего cleanup покрывают обе ветки.
  Дополнительно закреплена точная f64-граница `2^64`
  (`1.8446744073709552e19`): для `timeoutMs` она получает max-delay error, а
  не ошибку positive integer. На коде до review 203202 этот случай падал.

## Не перенесено

- ESM `default` export и npm Loader reflection не имеют runtime-аналога в Rust.

## Не проверено

- Нет. `cargo test --offline -p hoh-session-title-llm` — 10/0;
  `cargo clippy --offline -p hoh-session-title-llm --all-targets --no-deps -- -D warnings`
  и `cargo fmt --check --package hoh-session-title-llm` чисты.

## Вопросы ведущему

- Нет; shim и правки API чужих крейтов не требуются.
