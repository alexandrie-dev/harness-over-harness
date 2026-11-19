# hoh-tool-bash ← packages/shell/tool-bash @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs`, `src/execute.rs` | Плагин `ToolBash` (NAME `tool-bash`), описание и схема параметров инструмента, `output_schema`, `present_bash_call/result`, тело `execute` (политика → проверка → одобрение → cwd → `shellEnv.collect` → запрос → фон/продвижение/foreground). |
| `src/background.ts` | `src/background.rs` | `process_outcome`, `process_sources`, `ring_delta`, `process_job`. |
| `src/render.ts` | `src/render.rs` | `render_result`, `render_job_read`, `render_promoted`, `runner_failed_notice`. |
| (сервисы `jobs`, `sandboxPolicy` соседних пакетов) | — | `src/shim.rs` удалён: настоящие `hoh-jobs` и `hoh-sandbox-policy`. |
| `tests/tools.spec.ts` | `tests/tool.rs`, `tests/render.rs`, `tests/background.rs`, unit-тесты в `src/lib.rs` | Часть случаев, см. «Не перенесено». |
| `tests/background.spec.ts` | `tests/render.rs` (renderPromoted), `tests/background.rs` (processSources, ringDelta) | Только чистые случаи. |
| `tests/background-start.spec.ts` | `tests/background.rs` (`process_job_cases`) | Все 7 случаев. |
| `tests/integration.spec.ts` | — | Не перенесено. |
| `locale/*.json` | `locale/*.json` | Скопированы без изменений. |
| `README.md` | эти примечания и rustdoc | Прочитан полностью. |

В образце нет `LICENSE`. Экспорты `index.ts`, нужные другим пакетам, — `Config`, плагин, `render`/`background` как публичные модули.

Описание инструмента и описания параметров сверены с `index.ts` построчно, тексты секции `tool:bash` и маркеры результата закреплены тестами; `$DSH_*` собирается из `DSH_ENV_PREFIX`.

## Отличия от образца
- `present_bash_result`: в образце `isPromoted = result.value?.kind === 'promoted'`. Условие не мёртвое: pipeline образца возвращает успешный `ToolExecutionSuccess` вместе с `value` (`core/tools/src/pipeline.ts:626`, `deepFreeze({ ...detached, value: result.value })`), а обёртка presenter передаёт объект без удаления полей (`core/tools/src/schema.ts:623`); поэтому для успешного вызова с `value.kind === 'promoted'` оно истинно и показ — generic, без пилюли кода выхода. Сохранённый в журнале `ToolResult` поля `value` не имеет, для него условие ложно, как в образце. В Rust `hoh_tools::ToolResult` (`contracts.rs:211`: `content`, `is_error`, `meta`) `value` не несёт, а `meta` bash-инструмента — это `{cwd}` (его менять нельзя: проекция `presentationMeta`), поэтому признак остаётся константой `false`. Точное условие переноса: presenter должен получать успешное значение инструмента (`value`); для этого `hoh-tools` обязан передавать его в `present_result` (запрос владельцу `hoh-tools`), после чего `is_promoted = value.kind == "promoted"`.
- Порядок ключей результата — `cwd, exitCode, signal, timedOut, aborted, timeoutMs, stdout, stderr, sandbox`, затем `stopped`, как у `canonicalBashResult`; числа выводятся в JS-виде (`js_number_string`).
- `tokio::spawn` используется в `process_job` и в ожидании задачи: функция образца асинхронна, исполнение не привязано к `ctx`; владельцы работы — `cancel`/`done` реестра. Тело стартёра выполняется синхронно до первого `await`, как async-функция образца: `process_job` один раз опрашивает его future с пустым waker'ом до `spawn`, поэтому `shell.execute` вызван ещё внутри `registry.start` (тесты `runs_the_starter_up_to_its_first_await_before_returning`, `background_call_…`).
- `ShellExecutor::resolve` в `hoh-shell` не возвращает `Result`, а образец бросает исключение при непригодном бюджете: `resolve_spec` перехватывает панику `LocalBashExecutor` через `catch_unwind` и возвращает исходный текст ошибки (SHIM(hoh-shell)).
- Паника в `kill()` процесса превращается в `failed`-исход после ожидания `done` (аналог `try/finally` образца).
- `DSH_NODE_EXECUTABLE` не выставляется (см. PORT_NOTES `hoh-shell-env`); тест образца с ним заменён проверкой отсутствия.
- Ошибка `AbortError` (`TOOL_ABORTED`) — `HarnessError` с именем `AbortError` (класс-наследник по §7).

## SHIM
- SHIM(hoh-jobs) и SHIM(hoh-sandbox-policy) сняты (sg4, 19.11): `src/shim.rs` удалён, инструмент использует
  настоящие `hoh_jobs::{Jobs, JobSpec, JobHandle, JobHooks, JobOutputSource, …}` и
  `hoh_sandbox_policy::{SandboxPolicy, SandboxPolicyRequest}`. `JobSpec::run` получает `JobHandle` и возвращает
  `Result<JobHooks>`; `output_limit_bytes: None` (образец не задаёт); `output: Some(источники)`. Ошибки по TS:
  чтение источника — `Ok` (стрим процесса не бросает); `cancel` — `Err`, если завершение процесса бросило
  (паника `kill`), `done` — всегда `Ok` (образец не отклоняет промис; сбой задачи = исход `failed`).
  Без `Jobs` инструмент регистрируется только как foreground; при появлении сервиса переключается на вариант с реестром и обратно.
- `ApprovalAdapter` использует настоящий сервис и типы `hoh-user-approval`; `ToolExecution.agent` — настоящий
  `hoh_agent::Agent`, который напрямую передаётся working-directory вместе с его `inject`.

## Не перенесено
- `tools.spec.ts`, случаи, которым нужен настоящий реестр заданий (`hoh-jobs-local`, `tool-jobs` ещё нет): «the background surface follows the job registry», «background execution through the job runtime»; в «bash tool» — случаи с `setupWithJobs` (`run_in_background` не булево, схема job-backed, `job_kill`/`job_list`/`job_output`, встроенное значение фона). Строки `background=true` из «launch directory metadata» и «named args only» перенесены на сценарный `FakeJobs` (`records_the_executor_directory_for_a_background_call`, `injects_the_same_trusted_variables_into_a_background_request_…`, `a_background_bash_call_likewise_carries_no_trusted_only_fields`); в «launch directory metadata» не воспроизведён лишь хвост `job_output {wait: true}` (инструмент `job_output` — пакет `tool-jobs`). Путь с реестром проверен на сценарном фейке `JobRegistryApi`.
- «sandbox escalation through the generic task producer» перенесён, кроме трёх случаев (четвёртый — «omits sandbox facts…» — перенесён: `omits_sandbox_facts_the_executor_did_not_acquire_from_the_canonical_result`; фейковый исполнитель отдаёт факты песочницы как `RecordingSandboxExecutor`): «does not publish detached work when cancellation follows the escalation grant» (нужен хук на запись `approval/decided` в сессию; в Rust нет подписки на события сессии — отмена внутри ответчика приводит к исходу `cancelled`, а не к «отмене после выдачи»), «keeps the exhaustiveness backstop for a rogue approval implementation» (перечисление исходов закрыто, значение `rogue` не выразимо) и поддельное событие `sandbox/mode` с неизвестным режимом (режим в Rust — закрытое перечисление). Остальное — в `tests/tool.rs`, блок «Cases that need no job registry».
- Случаи с реальными процессами (`echo hello`, усечение через `seq`, SIGTERM-ловушка) перенесены на `hoh-subprocess-local` + `hoh-bash-local` (`real_process_*` в `tests/tool.rs`). `pwd`/`workdir` и спавн-ошибки остаются на записывающем исполнителе: их предмет — построение запроса.
- «rejects a non-JSON numeric argument» — `serde_json::Value` не хранит NaN.
- «tools depend on the executor: no registration without ctx.shell» и «applies the built-in background default when apply() receives a bare config» — зависят от порядка монтирования в ядре и реестра заданий; не переносились.
- `background.spec.ts`: блоки «foreground commands as jobs», «background bash output», «owned background output» (реальный реестр заданий и процессы).
- `integration.spec.ts` (5 случаев): цикл агента + реальный реестр.
- Из «keeps this call's cancellation» перенесены обе половины (отказ запуска тем же поводом отмены; ошибка запуска после отмены).

## Расхождение в крайнем случае
- Нестроковый `justification` (например, число) при исполнителе без песочницы: образец в `rejectUnadvertisedArgs` вызывает `justification.trim()` и бросает `TypeError`; в Rust аргументы разбираются типизированно, и тот же вход отклоняется раньше как `ToolArgsError` (`INVALID_ARGS`). Исход для модели — ошибка вызова без запуска команды в обоих случаях; различается только класс ошибки. Закреплено тестом `a_non_string_justification_without_a_sandbox_is_a_tool_args_error`.

## Не проверено
- Фоновый путь `execute` и продвижение по таймауту: проверены на фейке реестра, не на `hoh-jobs-local` (его нет). Эскалация песочницы с настоящими `hoh-sandbox-policy` и `hoh-user-approval` перенесена и проверена.
- Переключение foreground ↔ job-backed регистрации при появлении/исчезновении `Jobs`.
- Только Linux.

## Вопросы ведущему
- `hoh-shell`: сделать `ShellExecutor::resolve` возвращающим `Result<_, BoxError>` — тогда `catch_unwind` здесь и паника в `hoh-bash-local` исчезают.
- Нет реализации реестра (`hoh-jobs-local`): фоновые и promote-пути проверены на сценарном фейке `JobRegistryApi` в `tests/tool.rs` (3 теста), не на настоящем реестре; `job_kill`/`job_list`/`job_output` — пакет `tool-jobs`.
- `hoh-tools` не даёт публичного конструктора `ToolExecution` для тестов: тесты идут через `ToolExecutionPipeline`.
- `hoh-tools`: передавать в `present_result` успешное значение вызова (`value`), как pipeline образца; тогда `present_bash_result` читает `value.kind == "promoted"` и константа `false` удаляется (см. «Отличия»).
