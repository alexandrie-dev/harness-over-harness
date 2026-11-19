# hoh-session-checkpoint-policy ← packages/session/session-checkpoint-policy @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | plugin, четыре injected services, три fail-closed waterfall listener |
| `tests/session-checkpoint-policy.spec.ts` | `tests/policy.rs`, `tests/support/mod.rs` | все 11 исходных случаев |
| `tests/crash-recovery.e2e.ts` | `tests/crash_recovery.rs` | оба hard-crash случая через отдельный убиваемый test process |
| `tests/fixtures/crash-child.ts` | ignored `crash_child_fixture` в `tests/crash_recovery.rs` | child запускается родительскими тестами и убивается в failpoint |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | те же пути | побайтно скопированы |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json`, `tsconfig.json` | эти заметки, rustdoc, Cargo manifest | прочитаны; npm/build metadata не копируется |

Использованы только path-crates образца и общие workspace dependencies. Новых внешних
зависимостей нет, чужие crates не менялись.

## Отличия от образца

- Namespace plugin `name`/`inject`/`apply` представлен `SessionCheckpointPolicy`:
  `Plugin::NAME = "session-checkpoint-policy"`, `NoConfig`, а `SessionCheckpointPolicyDeps`
  типизирует обязательные сервисы `llm`, `sessionPersistence`, `sessions`, `tools`. Отдельного
  default export в Rust нет.
- `llm/stream` использует настоящий `LlmStreamEvent` waterfall. Await flush находится внутри
  deferred stream: downstream adapter stream не конструируется до успешного checkpoint.
  Запрос без `sessionId` и id уже detached session делегируется без flush.
- `tools/execute` использует настоящий `ToolExecutionPipeline`. Только вызов с Agent и без
  `parent` проходит checkpoint. После flush сигнал проверяется повторно; отмена возвращает
  точные content/message/name/code канонического `ABORTED_BEFORE_DISPATCH`. Ошибка flush
  отклоняет wrapper и штатно нормализуется pipeline в tool error, не запуская body.
- `agent/pre-step` проходит через scoped `agent_events` и сохраняет исходное решение `next`.
  Ошибка flush прерывает waterfall до следующего model request.
- Listener disposers принадлежат fiber policy; отдельный lifecycle-тест проверяет, что после
  dispose model request больше не вызывает checkpoint.
- Rust crash child — ignored test того же integration binary, а не TypeScript-файл. Родитель
  запускает его с `current_exe`, ждёт fsynced marker, затем принудительно убивает. Flush пишет
  фактический snapshot live Session в fsynced JSONL; после kill родитель читает raw events и
  вызывает настоящий `interrupted_turn_closers`. Родитель проверяет именно `SIGKILL`, полный
  порядок сохранённых и восстановленных событий (`step/end`, `turn/end` включительно) и весь
  payload неизвестного tool outcome (`name` и `code`), а не только отдельные признаки.

## Исправления по ревью 115752

- Crash-recovery тест request checkpoint теперь сравнивает полный Rust-вариант списка TS:
  `turn/start`, `step/start`, `request/header`, `request/context`, `step/end`, `turn/end`.
- Tool recovery проверяет точный объект ошибки
  `ToolOutcomeUnknownError`/`TOOL_OUTCOME_UNKNOWN`.
- Failpoint fixture подтверждает завершение дочернего процесса сигналом 9; обычный panic или
  другой ненулевой exit больше не принимается за ожидаемый hard crash.

## Не перенесено

Пропущенных source test declarations нет: 13 из 13 представлены. Проверка JavaScript
namespace identity (`'default' in module`, `Loader.unwrapExports`) не имеет runtime-аналога по
PORTING §8/§10; вместо неё проверены Plugin NAME и все четыре точных ServiceKey NAME.

Crash child не поднимает целый `AgentLoop`: его source testkit — следующий пакет этого
назначения и ещё отсутствует. Он воспроизводит ровно принадлежащую этому пакету границу на
настоящих SessionStore/LlmRuntime/ToolExecutionPipeline: записывает request prefix или
assistant+tool intent, flush через policy, adapter/tool failpoint, SIGKILL, чтение durable
snapshot и production recovery. Полный loop event choreography остаётся интеграционной
проверкой владельца `hoh-agent-loop-testkit`, но durability ordering policy здесь проверен
реальным process crash, не mock-флагом.

## Не проверено

Windows hard-kill ветка исключена, как исходный `describe.skipIf(process.platform === 'win32')`.
Пакет не выполняет сеть и не добавляет prompt/schema. Exactly-once внешних эффектов и активные
Assistant stream frames сознательно не гарантируются исходным контрактом.

## Вопросы ведущему

Нет. Все runtime API зависимостей доступны; private shim и правки чужих crates не понадобились.

## Проверки

Все Cargo-команды: `CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true`,
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x6`; tests запускались под
`timeout 900s` и с `RUST_TEST_THREADS=2`.

- `cargo test --offline -p hoh-session-checkpoint-policy` — 13 passed, 0 failed,
  один ignored child fixture (оба parent crash-теста запускают его явно); unit/doc 0/0.
- `cargo clippy --offline -p hoh-session-checkpoint-policy --all-targets -- -D warnings` — exit 0.
- `cargo fmt -p hoh-session-checkpoint-policy -- --check` — exit 0.
- `diff -ru <source>/locale crates/hoh-session-checkpoint-policy/locale` — exit 0.
