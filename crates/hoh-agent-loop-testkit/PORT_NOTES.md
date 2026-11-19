# hoh-agent-loop-testkit ← packages/test-support/agent-loop-testkit @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | native prerequisite spine, production `AgentLoop` driver, create/claim |
| `src/inbox.ts` | `src/inbox.rs` | mutable process-local Inbox и fail-fast Inbox |
| `src/model.ts` | `src/model.rs` | fail-fast Model |
| `src/working-directory-fixture.ts` | `src/working_directory_fixture.rs` | точный production service key через реальный plugin |
| `tests/agent-loop-testkit.spec.ts` | `tests/agent_loop_testkit.rs` | 7 из 8 случаев образца и дополнительная проверка directory fixture |
| `README*.md`, `README.i18n.yaml`, `package.json`, `tsconfig.json` | rustdoc и эти заметки | прочитаны; build/i18n metadata не копируется |

В пакете нет `locale/`, собственного `LICENSE` или отдельно лицензированных фрагментов.
Зависимости — только существующие path-crates и `serde_json`/`tokio` из workspace; новых внешних
пакетов нет. Чужие crates не менялись.

## Отличия от образца

- TypeScript возвращает структурные объекты `Inbox`/`Model`; Rust возвращает `Arc<dyn Inbox>` и
  `Arc<dyn Model>`. Mutating methods выражают throw через `BoxError`; единственный безошибочный
  getter `Model::current` паникует с исходным текстом при неожиданном обращении.
- `AgentLoopTestHarness::create_with` соответствует трём аргументам source `create`, а `create`
  даёт Rust-вариант его optional defaults. `claim` воспроизводит production policy публичными
  Inbox splice и `AgentInboxClaimed`: next-step целиком, затем один next-turn.
- Порядок native prerequisites сохранён: LLM, sessions, projections, system prompt, optional
  filesystem/working directory, tools, executor, agents. `AgentLoop` монтируется отдельно.
- `provide_working_directory_fixture` публикует настоящий `WorkingDirectoryKey`, но владелец
  сервиса не экспортирует fixture-конструктор. Поэтому helper монтирует production service над
  `FsLocal`; `get` совпадает с образцом, а `ensure` дополнительно проверяет существование пути и
  может записать recovery event. Это важнее фиктивного одноимённого key другого `TypeId`, который
  production-потребители не смогли бы inject.

## Fixed-interface shims

Нет private type shim. Недоступный PTC режим отклоняется на явной границе после уже смонтированных
предшественников. Нулевой `maxParallelSubCalls` всё равно отклоняется с именем исходного поля.

## Не перенесено

- Случай «mounts the PTC executor with its runtime»: `packages/core/tool-executor-ptc` и
  `packages/ptc-runtime/ptc-runtime` имеют в `MAP.tsv` категорию `redesign` и не имеют Rust crates.
  Копировать их выполнение или публиковать фиктивный `run_code` из testkit запрещено границами
  задания. Публичный `ToolMode::Ptc` сохранён и возвращает точную диагностическую ошибку о gap.

Остальные семь исходных случаев перенесены. Durable Inbox тест проверяет production log,
projection validation, live inserted/claimed notifications, duplicate admission и claim order.

## Не проверено

- PTC runtime/executor до решения redesign.
- Fixture working directory на несуществующем `Session.header.cwd`: production service выполнит
  восстановление, тогда как структурный объект образца вернул бы строку без I/O.

## Вопросы ведущему

1. После появления Rust PTC runtime/executor подключить их в ветку `ToolMode::Ptc` и перенести
   оставшийся исходный тест с `run_code`.
2. Если structural directory fixture должен оставаться без I/O, владельцу
   `hoh-working-directory` нужен публичный fixture constructor либо trait-backed service value;
   сейчас concrete `WorkingDirectoryService::start` закрыт.

## Проверки

Все команды выполнялись с `CARGO_BUILD_JOBS=2`, offline и
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x6`; тесты — под `timeout` и с
`RUST_TEST_THREADS=2`.

- `cargo test --offline -p hoh-agent-loop-testkit` — 8 passed, 0 failed; unit/doc 0/0.
- `cargo clippy --offline -p hoh-agent-loop-testkit --all-targets --no-deps -- -D warnings` — exit 0.
- `cargo fmt -p hoh-agent-loop-testkit -- --check` — exit 0.
