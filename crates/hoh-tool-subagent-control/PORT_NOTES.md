# hoh-tool-subagent-control ← packages/subagent/tool-subagent-control @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | `send_message`, `interrupt_agent`, конфигурация и обязательный identity-context |
| `src/list-agents.ts` | `src/list_agents.rs` | полный root-tree projection, статусы, фильтрация one-shot/external и diagnostics |
| `src/wait-agent.ts` | `src/wait_agent.rs` | exact-live caller, ожидание inbox, дедупликация в пределах root tool call и cleanup |
| `tests/*.spec.ts` | `tests/{control,list_agents,wait_agent}.rs`, unit tests | 26 offline-тестов публичных контрактов, projection и lifecycle-граней |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `README.md`, `README.zh.md`, `README.i18n.yaml` | rustdoc и этот файл | прочитаны, пользовательский контракт учтён |

## Сохранённое поведение

- Все четыре инструмента зарегистрированы глобально с исходными именами, описаниями,
  параметрами, output-schema и render-текстами; `interrupt_agent` по умолчанию включён и
  единственный зависит от `config.interrupt`.
- `send_message` и `interrupt_agent` передают разрешение имени, exact-live авторизацию,
  cold-resume и lifecycle-маршрутизацию в общий `hoh-subagent`, не добавляя собственной власти.
- `list_agents` всегда начинает с `lead`, обходит дерево в порядке сервиса, скрывает
  one-shot/external строки, но сохраняет их проходимость, и выводит unreadable branch как
  `corrupt`/`unsupported`/`unavailable`.
- `wait_agent` не потребляет inbox. Один root model-call не может крутиться на уже сообщённых
  message id, а другой root call видит те же pending messages. Все исходы снимают оба listener;
  caller cancellation, agent disposal и plugin unload сохраняют типизированную причину. Timeout
  измеряется одним абсолютным `Sleep` на весь вызов и не продлевается ложными inbox notifications;
  уже сработавшая отмена проверяется до pending input и не меняет reported-set.
- Identity-context обязателен, не интерполируется и показывает guidance только когда в данном
  scope/tool-set видна та же самая зарегистрированная definition, а не одноимённая shadow-копия.

## Отличия от образца

- JavaScript `WeakMap<Agent, ...>` представлен таблицей по `SessionId` с `Weak<dyn Agent>` и
  обязательной `Arc::ptr_eq`: одинаковый id нового Agent не наследует reported-set старого.
- Cordis-функции отписки представлены effect-owned kernel disposer-ами. Два listener снимаются
  после любого settlement; lifetime `AbortController` принадлежит fiber плагина.
- JavaScript safe-integer check выполнен до преобразования timeout в `u64`; JSON schema также
  отклоняет строки и дробные значения на общей tool-pipeline границе.
- Интеграционные сценарии cold-resume, deep-ancestor interrupt и lineage authorization не
  дублируют внутреннюю реализацию: инструмент вызывает production `hoh-subagent` seam, где эти
  сценарии уже проверяются. Здесь закреплены параметры вызова, коды ошибок, no-op отсутствующей
  цели, exact caller и model-facing результат.

После ревью добавлены regressions на два порядка эффектов `wait_agent`: ложное уведомление через
6 секунд больше не перезапускает 10-секундный timeout (готовность проверяется без автопродвижения
paused-времени), а отмена на входе в body побеждает уже pending
message и не записывает его id в reported-set. Второй тест вызывает owning `wait_for_input` напрямую:
общий tool pipeline намеренно отсекает pre-aborted execution раньше с кодом `ABORTED_BEFORE_DISPATCH`.
Каждый wait теперь, как source Promise, один раз фиксирует начальный `seen`: поэтому два уже ожидающих
вызова одного `rootCallId` оба видят следующее сообщение, а последовательный вызов его не повторяет.
Отдельные regressions эмитят production `AgentDisposed`: событие exact calling agent завершает wait с
`WAIT_UNAVAILABLE`, а disposal другого агента не прерывает ожидание.

## Не перенесено

- TypeScript-only проверка отсутствия `default export` заменена проверкой Rust plugin type,
  `NAME`, `INJECT` и default config.
- Сетевых, GPU-, shell- или key-gated тестов у исходного пакета нет.

## Не проверено

- Реальные внешние subagent executions не запускались: пакет сам не создаёт процессы и сеть,
  а external lifecycle принадлежит `hoh-subagent`.
- Полная source-матрица снятия двух wait-listeners и timer после input/timeout/cancel/dispose/unload
  не наблюдаема через публичный Kernel API (listener counts не экспортируются). Все settlement-пути
  сходятся в одном cleanup после `result.await`; inactive-context rollback и plugin unload проверяются
  косвенно unregister/dispose regressions. Clean reload остаётся integration обязанностью loader lifecycle.
- Subscribe-before-read сохраняется структурно: оба disposer создаются до первого `has_new_input`.
- Расширенная control-матрица (две независимые identity guidance, shadow/restrict, `names_of` error,
  JSON-escaping, success render, HMR config/reload) отложена как composition с `hoh-tools`/`hoh-subagent`.
  Здесь проверены exact schemas, identity precedence, interrupt on/off, отсутствие caller, self/absent target,
  error codes и production seam; cold resume/authorization/ancestor routing тестируются owning-крейтом.
- Расширенная `list_agents` матрица (forwarded caller/signal, каждый registry status, unreadable sibling,
  root/branch snapshots) отложена: service traversal принадлежит `hoh-subagent`; локально проверены порядок,
  deep leaf, one-shot filtering, diagnostics, exact root projection и tool lifecycle.
- Некоторые пункты ревью уже были покрыты до круга: другой `rootCallId` в
  `pending_input_is_reported_once_per_root_model_call`. Concurrent regression усилен после независимого ревью:
  два вызова с одинаковым `rootCallId` завершаются одним input, затем после `inbox.clear()` следующий wait
  истекает. `timeout_ms: null` отклоняется общей schema-границей как `INVALID_ARGS`, как optional integer TS
  schema; executor-level `??` для schema-invalid значения ненаблюдаем.

## Вопросы ведущему

- Нет. SHIM, новые внешние зависимости и правки чужих API не понадобились.

## Проверка

- `cargo test -p hoh-tool-subagent-control --offline`: 26 passed, 0 failed.
- `cargo clippy -p hoh-tool-subagent-control --all-targets --no-deps --offline -- -D warnings`.
- `cargo fmt -p hoh-tool-subagent-control -- --check`.

Все команды выполнялись с `CARGO_BUILD_JOBS=2`, заданным `CARGO_TARGET_DIR`, offline и под
внешним `timeout`.

### Redrun 022137

Приёмочный `test=101` не дошёл до запуска тестов: rustc для `hoh-agent-loop-testkit` и
`hoh-agent-loop` завершился с `No space left on device (os error 28)` при записи в
`/dev/shm/hoh/pl-022137-hoh-tool-subagent-control`. `clippy=101` имеет ту же причину:
не удалось записать `invoked.timestamp` для `libc` и `serde_core`. В логах нет ошибки сокета,
Rust diagnostic или расхождения с TS; поэтому код и тесты не менялись. Повторный полный прогон
в выделенном `target-x5` с доступным местом завершился 26/0, включая обе зависимости, на которых
оборвалась приёмка.
