# hoh-goal-round-driver ← packages/goal/goal-round-driver @ 39163dd30f70

## Файлы

| TypeScript | Rust | Примечание |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | plugin, per-agent scheduler, reservation/admission fences, durability и teardown |
| `src/prompt.ts` | `src/prompt.rs` | точный retained `<goal_round>` prompt |
| optional `dsh-subagent` service | `hoh-subagent::{Subagents, SubagentRuntime}` | настоящий optional workspace service; локальных shim нет |
| `tests/goal-round-driver.spec.ts` | `tests/goal_round_driver.rs` | реальные agent loop/session/goal интеграционные сценарии |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | одноимённые `locale/*.json` | перенесены дословно |
| README/package metadata | rustdoc, этот файл, `Cargo.toml` | прочитаны; npm/tsconfig metadata не копируется |

Зависимости только path/workspace, новых внешних пакетов нет. Чужие крейты не менялись.
`futures-util` является обычной dependency, а не только dev-dependency: production scheduler
использует `FutureExt::catch_unwind` для containment паники driver task. Red-run ведущего от
02:20 был снят более новым снимком manifest/source (02:21–02:23); регрессией служат panic-path
и forged-source тесты плюс обязательный `clippy --all-targets --no-deps`, который на старом
manifest не компилировал library target.

## Сохранённые контракты

- Plugin называется `goal-round-driver`, конфигурации не имеет и требует `agents`, `goals`,
  `sessions`; `subagents` остаётся опциональным.
- Один process-local attempt хранит точные `{goalId, revision, round}`, id и полный content
  сообщения, фазы queued/claimed/admitted, cancellation/stale flags. Round расходуется только
  после durable `user/message` с тем же id.
- Перед резервированием проверяются exact live Agent, idle, armed active Goal, отсутствие
  competing input и durability checkpoint. Descendant join прерывается выгрузкой plugin.
- Pre-step проверяет полную reservation до и после downstream waterfall. Stale round отвергается,
  а чужой claimed context возвращается без round-zero goal context и без дублей уже queued input.
- Human input имеет приоритет; edit инвалидирует старую revision; downstream pause/cancel/veto не
  превращаются в ложный blocker. Stable blocker codes/texts сохранены: `round-limit`,
  `prompt-rejected`, `queue-failed`.
- Max tokens, terminal/post-turn error, durability failure, unrelated cancellation и teardown
  fail closed. Host pause отменяет running turn; model-originated pause завершает свой turn.
- Выгрузка сначала закрывает admission и descendant waits, разоружает goals, затем ждёт driver
  runs и running attempts, пока listeners ещё установлены. Disposal Agent/owner не записывает
  ложную pause в durable goal history.
- Prompt JSON-кавычит objective через JS-совместимый stringify и сохраняет исходные инструкции.

## Отличия от образца

- `Map<Agent, State>` представлен identity-списком `Arc<DriverState>` с pointer equality;
  mutable поля защищены `Mutex`. Записи удаляются на exact `agent/disposed` и при teardown.
- Eager JavaScript Promise scheduler представлен вручную отслеживаемым `tokio::spawn` с coalesced
  `requested`, явным completion latch и ожиданием всех run при teardown. Task создаётся только
  внутри plugin-owned lifecycle и не переживает его закрытие. `catch_unwind` воспроизводит reject
  handler исходного Promise: паника разоружает goal, завершает latch и не блокирует teardown.
- При teardown ошибка `agent.cancel(Parent)` логируется; для этого агента `when_idle` не ожидается,
  потому что после неуспешной отмены он может никогда не наступить. Rust продолжает разоружать и
  ждать остальные agent states/runs, затем очищает driver states. В TypeScript синхронный throw
  прерывает cleanup до `Promise.allSettled`/`states.clear`; Rust намеренно не даёт отказу одного
  агента навсегда заблокировать выгрузку и очистку остальных goal-драйверов.
- Open TypeScript `MessageSourceMap` представлен `MessageSource { kind, data }`. Любой source
  `kind == "goal"` с положительным числовым `round` попадает под pre-step fence, как в source;
  полная reservation identity по-прежнему требует строковый `goalId` и целые `revision`/`round`.
  Неполный или дробный forged source поэтому отвергается, а round zero остаётся ordinary context.
- `agent/status` может выполняться параллельно scheduler-задаче, тогда как TypeScript между await
  однопоточен. Idle-settlement очищает attempt только при совпадении наблюдённого `messageId`, чтобы
  не удалить более новую reservation другого потока.
- В regression terminal-failure deferred input добавляется напрямую в `Inbox::append`, затем
  будится через `steer`: Rust `Agent::followup` сам немедленно будит loop, в отличие от source
  microtask fixture. Проверяемый observable contract тот же — driver не перевооружает goal и не
  присваивает чужому input reservation.

## Не перенесено

В source 59 именованных групп и 62 развёрнутых case. Перенесены 54 source-case и добавлены три
Rust regression case: panic settlement, неполный forged goal source и защита новой reservation от
устаревшего idle-settlement. Восемь source case требуют Vitest monkey-patching конкретных
runtime-объектов, для которого у
Rust API нет безопасной точки подмены:

- `goals.block` бросает внутри scheduler; `agents.withoutInitiator` синхронно бросает либо
  возвращает rejected Promise;
- `goals.get`/первый `goals.disarm` бросают на initial pre-step; `goals.get` бросает после hook;
- `goals.pause` бросает при settlement cancelled reservation;
- две ветви root disposal order (`ownerUnloadsFirst = true/false`) зависят от динамически
  подменяемого Cordis dependency graph.

Обе ветви отказа `followup` теперь проверяются локальным structural Agent без изменения чужого API:
точный `queue-failed` blocker и сохранение более нового disarm. Остальные production ветви
fail-closed перенесены и частично проверяются ближайшими
наблюдаемыми сценариями: throwing downstream hook, checkpoint/turn-end rejection, agent/owner
disposal и queued/claimed/admitted teardown. Публичные test-only fault knobs не добавлялись.

## Зависимость в работе

`hoh-subagent` на момент takeover всё ещё помечен в `MAP.tsv` как `writing:x5`. Driver использует
его опубликованные `Subagents`/`SubagentRuntime` и точную ссылочную identity-проверку root Agent.
Интеграционные тесты поднимают настоящий runtime, регистрируют внешний provider/run и проверяют
ожидание descendants, отсутствие ожидания без armed goal, pause, human-input priority, restart и
выгрузку driver. Чужой крейт не менялся.

## Проверки

Команды выполняются offline, с `CARGO_BUILD_JOBS=2`, общим tmpfs target и тестами под `timeout`.

- `cargo test --offline -p hoh-goal-round-driver --no-fail-fast` — 57 passed, 0 failed;
  doc-tests 0/0.
- `cargo clippy --offline -p hoh-goal-round-driver --all-targets --no-deps -- -D warnings` — чисто.
- `cargo fmt -p hoh-goal-round-driver -- --check` — чисто.
- `diff -ru <source>/locale crates/hoh-goal-round-driver/locale` — чисто.
