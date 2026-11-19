# hoh-goal ← packages/goal/goal @ 39163dd30f70

## Файлы

| TypeScript | Rust | Примечание |
|---|---|---|
| `src/types.ts`, `src/domain.ts` | `src/types.rs` | Branded id, CAS-ref, snapshot/view/projection, change и activation payloads |
| `src/fold.ts` | `src/fold.rs` | Строгий decoder, lifecycle replay и учёт admitted goal rounds |
| `src/index.ts` (projection) | `src/projection.rs` | `goal` projection version 6, sticky replay failure и wire view |
| `src/index.ts` (service) | `src/service.rs` | Durable mutations, process-local activation, scoped notification и plugin |
| `src/runtime.ts` | `src/service.rs`, `src/types.rs` | `GoalError`, версия change и branded `GoalId` |
| `src/client.ts` | re-export из `src/lib.rs` | Rust не разделяет host/client namespace; чистые типы остаются самостоятельными |
| `tests/goal.spec.ts`, `tests/projection.spec.ts` | `tests/service.rs`, `tests/fold.rs`, `tests/projection.rs` | Lifecycle, replay, projection и wire contracts |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | одноимённые файлы в `locale/` | Перенесены дословно |
| README/package metadata | этот файл и `Cargo.toml` | Прочитаны; README не копируется |

## Сохранённые контракты

- Сервис и plugin называются `goals` и `GoalService`; durable событие — `goal/change`, projection key — `goal`, state version — 6.
- Все non-clear mutations пишут полный snapshot, `clear` пишет revisioned tombstone. CAS требует точные `id` и `revision`, каждая мутация повышает revision ровно на один, goal id нельзя повторно использовать в одной Session.
- Objective и blocker message нормализуются на domain boundary; blocker code проверяется как lower-kebab-case. Конфигурационный и request round cap обязаны быть положительными safe integers.
- Activation не сохраняется в Session. Create/resume вооружают goal, pause/block/complete/clear и любой внешний `goal/change` разоружают; session-start edge также разоружает без durable revision.
- Перед синхронным `Session::append` сохраняется pending activation с ожидаемым offset. Поэтому reentrant `session/event` observer видит уже согласованную durable projection и требуемую activation.
- Durable mutations одной Session сериализуются reentrant per-session gate на всём синхронном участке prepare/CAS/append/notify. Разные потоки не могут записать одну revision дважды; callback того же потока не блокируется и достигает штатной защиты `Session::append` от reentry.
- `goal/changed` отправляется через agent scope только после commit; ошибки observers контейнируются. `goal/activation-changed` — обычное contained session notification и публикуется только при реальном изменении activation.
- Строгий fold принимает только canonical payload shape, проверяет безопасные counters/timestamps, монотонное время, допустимые phase transitions и последовательные goal-round sources.
- Domain normalization и replay используют JavaScript `trim` через `hoh-util-values::js_trim`: U+FEFF считается пробелом, U+0085 — нет. Тексты safe-integer/version ошибок и порядок `maxGoalRounds` до `blockedReason` совпадают с source.
- Projection не бросает replay failure в registry event drive: сохраняет первую ошибку, оставляет последнее валидное client view и делает host `get`/mutation ошибкой `GOAL_STATE_CORRUPT`. Lifecycle activation edge при таком состоянии остаётся успешным и не публикует неконсистентный view.
- Checkpoint schema требует все три outer keys (`current`, `seenGoalIds`, `failure`), непустые ids/objective, положительные safe `revision`/`maxGoalRounds`, уникальные непустые seen ids и прежние cross-field invariants. Wire view проходит ту же проверку goal-полей.

## Отличия от образца

- Source class одновременно Cordis plugin, service и Typert remote owner. Rust разделяет его на `GoalService`, `GoalServicePlugin` и service key `Goals`; typed public methods находятся на `GoalService`.
- `GoalError::new` возвращает общий `hoh_error::HarnessError` с именем `GoalError`, как остальные error owners workspace.
- TypeScript `WeakMap<Session, GoalRuntimeState>` представлен identity-списками runtime и mutation gates по pointer-equality `Session`. Записи удерживают Session до выгрузки service scope; состояние не переживает plugin. Reentrant gate использует owner thread только внутри без-await участка, без `unsafe` и без изменения `hoh-session` API.
- Rust request types используют `u64`, поэтому отрицательный и дробный `max_goal_rounds` в typed create/edit API непредставим. JSON plugin config оставлен `f64`, чтобы `-1` и `1.5`, как в source test, доходили до domain validation и возвращали `GOAL_INVALID_MAX_ROUNDS`.
- Typed `CreateGoalRequest.objective: String` и `remote_export_create` не представляют non-string JSON objective: wire-deserialization отклоняет его до domain method. Это Rust-аналог Typert/Zod boundary; строковые пустые/JS-whitespace случаи возвращают исходный `GOAL_INVALID_OBJECTIVE`.
- `edit` намеренно разрешает `maxGoalRounds < roundsStarted`, как source. Получившийся live state доступен, но строгая checkpoint schema его отвергает; порт не исправляет upstream policy без решения владельца контракта. Отдельный regression фиксирует это намеренное совпадение.
- JavaScript object spread возвращает плоский `GoalView`; Rust view содержит `goal: GoalSnapshot`, но `serde(flatten)` сохраняет исходную wire-форму. `as_ref()` даёт тот же CAS identity.
- Typert-generated `remote`/`typert` entry points не генерируются по PORTING §8. `remote_export_create` сохранён как typed wire-safe adapter.
- Projection registry владеет `Arc` states: same-reference contract для unrelated/sticky-failure событий проверяется через `Arc::ptr_eq`, а wire boundary сериализует значение.

## Не перенесено

- `tests/goal.e2e.ts` монтирует пакет через Loader и проверяет Loader-owned snapshot. Он относится к следующему пакету списка `packages/test-support/loader-smoke → hoh-loader-smoke`; без этого owner-крейта нельзя честно воспроизвести composition boundary. Service-side invariant «create не начинает goal round» уже проверен: единственная запись после create — `goal/change`, `rounds_started == 0`.
- Typert-generated entry points — по PORTING §8.

## Не проверено

Нет платформенных веток. Loader composition проверяется владельцем `hoh-loader-smoke`, а не временным локальным shim.

## Вопросы ведущему

Нет. Чужие API и крейты не менялись.

## Тесты источника

В источнике 44 именованные unit-группы (`goal.spec.ts` и `projection.spec.ts`) и одна Loader e2e-группа. Все представимые domain/service/projection ветви, включая concurrency CAS, reentrant observer, seed/fork, plugin unload, activation triples, sticky failure wire view, timestamp monotonicity, все malformed snapshots/tombstones и replay exclusions, покрыты **67 Rust-тестами**. Табличные source cases объединены; отдельные regression cases фиксируют каждый дефект ревью. Loader e2e отнесён к owner-пакету, как описано выше.

## Проверка

- `cargo test -p hoh-goal --offline --no-fail-fast`: 67 passed, 0 failed; doc-tests 0/0.
- `cargo clippy -p hoh-goal --all-targets --no-deps --offline -- -D warnings`: чисто.
- `cargo fmt -p hoh-goal -- --check`: чисто.
- Все восемь locale JSON побайтно совпадают с источником.

Новых внешних зависимостей нет: все внешние зависимости уже присутствовали в workspace/Cargo.lock; остальные зависимости — path-крейты рабочего пространства.
