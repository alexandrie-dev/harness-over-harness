# hoh-subagent ← packages/subagent/subagent @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/types.ts`, `src/error.ts`, `src/client.ts` | `src/types.rs`, `src/error.rs`, `src/lib.rs` | provider/run/activation/result, lifecycle и client-safe re-exports |
| `src/activation.ts`, `src/manager.ts` | `src/activation.rs`, `src/manager.rs` | capacity, child serialization, local/external ownership, cold resume, delivery, interrupt и drain |
| `src/catalog.ts`, `src/descriptor.ts` | `src/catalog.rs`, `src/descriptor.rs` | строгие durable форматы v3 и projection v6 |
| `src/projection-types.ts`, `src/projection.ts` | `src/projection_types.rs`, `src/projection.rs` | identity v2 и timing v3 |
| `src/child-agent.ts`, `src/depth.ts` | `src/child_agent.rs`, `src/depth.rs` | composition, policies, route inheritance и safe-integer depth |
| `src/assistant-output.ts`, `src/continuation-messages.ts` | `src/assistant_output.rs`, `src/continuation_messages.rs` | final-output fold, typed source shape и model-facing тексты |
| `src/structured.ts` | `src/structured.rs` | tool/prompt/guard и two-phase commit по execution token |
| `src/list-children.ts`, `src/control-types.ts`, `src/control.ts` | `src/list_children.rs`, `src/control_types.rs`, `src/control.rs`, `src/runtime.rs` | recursive discovery, prompt/interrupt DTO и RemoteError mapping |
| `src/lifecycle.ts`, `src/archive-admission.ts` | `src/lifecycle.rs`, `src/archive_admission.rs` | contained lifecycle и workspace archive admission |
| `src/out-of-process.ts` | `src/out_of_process.rs` | cwd через effective `access(X_OK)`, timing/diagnostic settlement и cancellation-safe общий result/teardown |
| `src/index.ts` | `src/runtime.rs` | plugin, volatile config readers, provider registry и public service |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |

## Отличия от образца

- `SubagentRun.result()` и `dispose()` возвращают Rust future над общим settlement;
  `subprocess_run_handle` запускает result сразу, как eager Promise. Оба settlement выполняются
  в независимых Tokio task и публикуются через `watch`, поэтому отмена отдельного ожидателя не
  забирает `JoinHandle` и не обрывает teardown; повторный вызов ждёт тот же результат.
- `SubagentStopReason::Other(String)` сохраняет declaration-merging расширяемость.
- `SubagentDepthError` представлен constructor namespace, возвращающим именованный
  `HarnessError` с `SubagentDepthErrorDetail { attempted_depth, max_depth }`.
  Однако `resolve_child_depth` пока возвращает `SubagentError`: это закреплено
  уже принятым `hoh-subagent-spawn-in-process` и сохраняет его совместимость;
  имя runtime-ошибки отличается от TS `SubagentDepthError`.
- Вместо `Map<ToolExecution, ...>` structured capture использует уникальный
  `ToolExecutionToken`, выдаваемый тем же owner.
- Zod 4 issue для видимой ошибки empty `childSessionId` воспроизведён
  вручную по zod 4; сам zod в Rust-тестах не запускается.
- Typert в MAP помечен `skip`: control API — обычные async-методы,
  а `RemoteError` представлен через общий `HarnessError`; transport principal поэтому
  не записывается в browser prompt source.
- `AbortSignal` не имеет JS-listener identity: Rust settlement/drop освобождает
  futures структурно, без отдельной проверки `removeEventListener`.

## Не перенесено

- TypeScript-only declaration merging (`Context`, `Events`, projection maps, Remote details)
  заменено Rust service/event/type ключами.
- `AgentOptions.subagentDepth` нет в `hoh-agent`; depth поэтому берётся из
  durable `SessionHeader.delegationDepth`, который остаётся монотонным источником.
- `AgentOptions` не различает отсутствующее поле и явно переданный
  `reasoningEffort: undefined`. Поэтому Rust не может воспроизвести единственный
  JS-вариант, который очищает effort при неизменном route; смена route и явное
  значение effort воспроизводятся. Нужен presence-preserving API в `hoh-agent`.

## Не проверено

- Windows-ветка search permission для cwd исключена по заданию; Unix-путь проверяется настоящим
  `rustix::fs::access(..., X_OK)`, как `accessSync(..., constants.X_OK)` в Node, с учётом effective
  uid/gid, ACL и root semantics. Одни лишь чужие execute-биты больше не дают ложного допуска.
- Динамическая смена volatile config после plugin apply читается на каждой
  активации; отдельного Rust integration test для Cfg hot-update нет.

## Тесты

- Пакетный набор частично перенесён как модульные тесты рядом с реализацией и внешние
  contract-тесты в `tests/public_contract.rs` и `tests/runtime_registry.rs`.
  Они фиксируют wire vocabulary, имена и depth, zod-compatible control errors,
  model-visible settlement messages, JS JSON stringify, UTF-8 diagnostic limit,
  provider registry и config/max-depth resolution.
- Тяжёлые сценарии полного service lifecycle дополнительно проверяются принятыми
  `hoh-subagent-spawn-in-process` и `hoh-subagent-fork-in-process`, которые
  проверяются вместе с этим крейтом на совместимость публичного API; это не подменяет буквальный
  перенос исходных случаев.
- Полная проверяемая опись находится в `TEST_MATRIX.md`: 386 объявлений `it`/`test`, которые после
  разворачивания `it.each` дают 398 случаев по подсчёту ревью. Для каждого указан точный Rust-тест
  либо конкретная причина отсрочки. Неперенесённые сценарии явно остаются долгом parity и не
  объявляются исключениями §10.
- Круг ревью `024844` добавил регрессии на отмену первого ожидателя `result()` и `dispose()`,
  локальный `requestCancel` без abort сигнала и Unix-каталоги с mode `001`/`010`.

## Вопросы ведущему

- Добавить `subagent_depth` в `hoh_agent::AgentOptions` и после этого убрать
  `SHIM(hoh-agent)` в `depth.rs`; чужой crate не изменялся.
- Добавить presence-preserving override для `AgentOptions.reasoning_effort`,
  чтобы отличать omission от явного `undefined` без изменения чужого crate.
- Согласовать миграцию `hoh-subagent-spawn-in-process` с runtime-именем
  `SubagentDepthError`; сейчас его принятый тест требует `SubagentError`, поэтому
  core не может переключить имя без правки чужого крейта.
- Если будет портирован typert invocation/principal, добавить не-owner
  `principal` в `MessageSource` для `SubagentRuntime::prompt`.

## Проверки

Все команды выполнялись offline, jobs=2, с target
`/mnt/toshiba/alexandrie/hoh/target-x5`; тесты — только под `timeout`.

- `cargo test -p hoh-subagent --offline`: 70 passed, 0 failed; doc-tests 0/0.
- `cargo clippy -p hoh-subagent --all-targets --no-deps --offline -- -D warnings`: exit 0.
- `cargo fmt -p hoh-subagent -- --check`: exit 0.
- Совместимость: `cargo test -p hoh-subagent-spawn-in-process -p hoh-subagent-fork-in-process --offline`:
  43 passed, 0 failed; doc-tests 0/0.
