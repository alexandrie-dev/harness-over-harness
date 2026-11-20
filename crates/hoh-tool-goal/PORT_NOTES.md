# hoh-tool-goal ← packages/goal/tool-goal @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | plugin, Config, три model-facing tool, schemas, presenters и prompt policy |
| `src/authority.ts` | `src/authority.rs` | live-agent, root-human и exact goal-round authority |
| `src/wrapup.ts` | `src/wrapup.rs` | дословные terminal-round инструкции complete/blocked |
| `tests/tool-goal.spec.ts` | `tests/tool_goal.rs` | 31 интеграционный тест регистрации, presentation, authority, fork/reload, порядка ошибок, переходов, CAS, threshold и wrap-up |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | те же пути | скопированы без изменений |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json` | эти заметки и `Cargo.toml` | прочитаны целиком; npm/build metadata не дублируется |

Инструменты возвращают один compact JSON-контракт: `{goal:null}` либо goal с
`id`, `revision`, `objective`, `phase`, `roundsStarted`, `maxGoalRounds`,
опциональным `blockedReason` и live-наблюдением `activation`. Мутации делегированы
настоящему `hoh-goal`, поэтому в одном exclusive batch сохраняются CAS, журнал
`goal/change` и доменные коды ошибок.

## Отличия от TypeScript

- `Config` хранит threshold как `u64`; JSON Schema явно фиксирует исходные
  `minimum: 1`, safe-integer maximum и default. JSON-десериализация Rust отвергает
  дробное/отрицательное значение до `apply`, а `apply` сохраняет исходную
  проверку нуля и превышения safe integer. Успешные значения и default `3`
  совпадают с образцом.
- Cordis TypeScript сохраняет `ctx.agents.currentInitiator()` внутри tool body.
  Rust `hoh-tools` вызывает body через kernel waterfall, где task-local initiator
  внешнего Agent driver уже недоступен. До расширения чужого `ToolExecution`
  аттестацией initiator Rust-порт проверяет доступную наблюдаемую границу: exact
  live Agent, статус `running`, открытый turn и последний `step/start` внутри
  него. После `step/end`, для stale/idle Agent и вне turn вызов отклоняется как
  `GOAL_TOOL_DRIVER_REQUIRED`; вручную подставленный `exec.agent` во время
  production step отличить от настоящего вызова пока нельзя.
- Branded TypeScript `GoalId` представлен доменным Rust newtype `GoalId`; wire
  имена и JSON остаются исходными snake/camel-case именами инструмента.
- Rust function plugin представлен публичной `apply(...)` и loader-типом
  `ToolGoal`; `Deps` соответствует исходному `inject`.

## Круг исправлений review 024042

- Восстановлен исходный порядок доменных ошибок для одновременно невалидных
  полей: create проверяет objective до cap; edit сначала проверяет существование
  и exact revision, затем `GOAL_INVALID_EDIT`, objective и только потом cap.
  Поскольку typed `hoh-goal` принимает cap как `u64`, невалидное JSON-число
  передаётся ему как sentinel `Some(0)`; literal edit `0` остаётся `None`.
  Поэтому create сохраняет доменный порядок objective→cap, а все
  state-dependent проверки edit и его мутация выполняются атомарно настоящим
  `GoalService` под mutation gate. Регрессии `create_invalid_objective…`,
  `edit_invalid_objective…`, `edit_stale_revision…` на старом коде получали
  `GOAL_INVALID_MAX_ROUNDS`.
- Закреплены оставшиеся assertions исходной спеки: все presenter fallbacks,
  exclusive mode, direct apply default, поздний paused-resume, пять действий с
  лишними полями и одним `goal/change`, вся цепочка empty fillers, whitespace
  blocker, terminal contexts/concludesTurn, точный wrap-up запрет новых tools и
  поля успешного goal-round результата.

## Не перенесено

- Проверки npm namespace (`default` export, `Loader.unwrapExports`) не имеют
  runtime-аналога; закреплены `Plugin::NAME`, `NAME`, регистрации и dispose.
- Проверка дробного прямого `Config` непредставима после typed Rust boundary
  (`u64`); direct `apply` отдельно проверен на нуле до любой регистрации, а
  дробное/отрицательное loader-значение отвергает serde/schema до `apply`.

## Не проверено

- Полная agent-loop модельная генерация и восстановление с диска. Tool pipeline,
  Native executor, Session/Goal projections и все три tool body проверены без
  сети, процессов, bind и GPU.

## Вопросы ведущему

- Для полного совпадения нужен driver-attestation в чужом
  `hoh_tools::ToolExecution`, захваченный до входа в kernel waterfall. Чужие
  крейты в этой сдаче не менялись.

## Проверки

Команды выполняются offline, jobs=2, target
`/mnt/toshiba/alexandrie/hoh/target-x7`; тесты запускаются только под `timeout`.

- `timeout 240s cargo test --offline -p hoh-tool-goal --no-fail-fast`:
  31 passed, 0 failed; unit/doc-tests 0/0.
- `cargo clippy --offline -p hoh-tool-goal --all-targets --no-deps -- -D warnings`:
  exit 0.
- `cargo fmt -p hoh-tool-goal -- --check`: exit 0.
- `diff -ru <source>/locale crates/hoh-tool-goal/locale`: exit 0.

