# hoh-command-goal ← packages/goal/command-goal @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | plugin, `/goal` grammar, rendering, domain adapter и attachment followup |
| `tests/command-goal.spec.ts` | `tests/command_goal.rs` | все 14 runtime-групп регистрации, lifecycle, состояний и attachments |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | те же пути | скопированы без изменений |
| `README*.md`, `package.json` | эти заметки и `Cargo.toml` | прочитаны; npm/build metadata не дублируется |

Команда зарегистрирована под exact definition id
`@deepseek-ai/dsh-command-goal`, принимает attachments и делегирует все durable
переходы настоящему `hoh-goal`. Результат команды не раскрывает goal id/revision;
`command/run`/`command/done` принадлежат `hoh-commands`, а домен пишет только
`goal/change`.

## Отличия от TypeScript

- Закрытые TypeScript union представлены Rust enum/match; недостижимые ветки
  `assertNever` не имеют отдельного runtime-пути.
- `GoalError` распознаётся через общий `HarnessError` с именем
  `GoalError::NAME`. Все такие ошибки кроме `GOAL_STATE_CORRUPT` превращаются в
  стабильный command error; corruption и другие `BoxError` пробрасываются.
- JS `trim`, whitespace после `edit` и objective normalization используют
  `hoh-util-values`, включая ECMAScript whitespace.
- Проверка неожиданной ошибки вместо mock `goals.get` использует отказ
  `Agent::followup` после успешной attachment admission: ошибка не поглощается и
  не превращается в ожидаемый command result. Corrupt projection проверяется
  настоящим malformed `goal/change`.

## Не перенесено

- Только JS Loader namespace assertions (`default` export и
  `Loader.unwrapExports`). Их Rust-аналоги закреплены `Plugin::NAME`, `NAME`,
  `INJECT`, descriptor registration и disposal.

## Не проверено

- Платформенных ветвей, сети, процессов, bind и GPU у пакета нет.

## Вопросы ведущему

- Нет. Чужие API и крейты не менялись; новых внешних зависимостей нет.

## Проверки

Команды выполняются offline, jobs=2, target
`/mnt/toshiba/alexandrie/hoh/target-x7`; тесты только под `timeout`.

- `timeout 240s cargo test --offline -p hoh-command-goal --no-fail-fast`:
  14 passed, 0 failed; unit/doc-tests 0/0.
- `cargo clippy --offline -p hoh-command-goal --all-targets --no-deps -- -D warnings`:
  exit 0.
- `cargo fmt -p hoh-command-goal -- --check`: exit 0.
- locale diff: exit 0.
