# hoh-tool-bash ← packages/shell/tool-bash @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs`, `src/execute.rs` | Плагин `ToolBash` (NAME `tool-bash`), описание и схема параметров инструмента, `output_schema`, `present_bash_call/result`, тело `execute` (политика → проверка → одобрение → cwd → `shellEnv.collect` → запрос → фон/продвижение/foreground). |
| `src/background.ts` | `src/background.rs` | `process_outcome`, `process_sources`, `ring_delta`, `process_job`. |
| `src/render.ts` | `src/render.rs` | `render_result`, `render_job_read`, `render_promoted`, `runner_failed_notice`. |
| (сервисы `jobs`, `sandboxPolicy` соседних пакетов) | `src/shim.rs` | SHIM, см. ниже. |
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
- `tokio::spawn` используется в `process_job` и в ожидании задачи: функция образца асинхронна, исполнение не привязано к `ctx`; владельцы работы — `cancel`/`done` реестра. Стартёр вызывается синхронно до первого `await`, как в образце.
- `ShellExecutor::resolve` в `hoh-shell` не возвращает `Result`, а образец бросает исключение при непригодном бюджете: `resolve_spec` перехватывает панику `LocalBashExecutor` через `catch_unwind` и возвращает исходный текст ошибки (SHIM(hoh-shell)).
- Паника в `kill()` процесса превращается в `failed`-исход после ожидания `done` (аналог `try/finally` образца).
- `DSH_NODE_EXECUTABLE` не выставляется (см. PORT_NOTES `hoh-shell-env`); тест образца с ним заменён проверкой отсутствия.
- Ошибка `AbortError` (`TOOL_ABORTED`) — `HarnessError` с именем `AbortError` (класс-наследник по §7).

## SHIM
- SHIM(hoh-jobs): `JobId`, `JobStatus`, `JobSpec`, `JobHooks`, `JobOutputSource`, `JobRegistryApi`, сервис `Jobs` (`service!(Jobs = "jobs")`). Без `Jobs` инструмент регистрируется только как foreground; при появлении сервиса переключается на вариант с реестром и обратно.
- SHIM(hoh-sandbox-policy): `SandboxPolicyApi::resolve(Option<&Session>)`, ключ `SandboxPolicyKey`. Загрузка не удаётся, если исполнитель ограничивает песочницей, а сервиса политики нет.
- SHIM(hoh-user-approval): `ApprovalAdapter` над shim `Approval` из `hoh-tools`.
- SHIM(hoh-agent): `WorkingDirectoryAgent` — адаптер `Agent` из `hoh-tools` к `AgentApi` из `hoh-working-directory`; `inject` уходит в сервис `agentNotices` (`shim::AgentNoticesKey`/`AgentNoticesApi`): `Agent` из `hoh-tools` не несёт `inject`. Сервис предоставляет композиция, владеющая настоящими агентами (должна положить сообщение в очередь следующего шага); без сервиса уведомление теряется с предупреждением working-directory, как раньше. Тест `tells_the_agent_when_the_selected_working_directory_is_unavailable`.

## Не перенесено
- `tools.spec.ts`, блоки, которым нужны ещё не существующие пакеты (`jobs-local`, `tool-jobs`, `user-approval`, `sandbox-policy`, `session-projection`, `agent`/цикл агента): «the background surface follows the job registry», «background execution through the job runtime», «sandbox escalation through the generic task producer»; в «bash tool» — случаи с `setupWithJobs` (`run_in_background` не булево, схема job-backed, `job_kill`/`job_list`/`job_output`, встроенное значение фона); в «launch directory metadata» и «named args only» — строки `background=true`.
- Случаи с реальными процессами оболочки (`echo hello`, `pwd`, усечение через `seq`, SIGTERM-ловушка): `hoh-subprocess-local` ещё пишется. Их предмет — отрисовка результата и построение запроса, он перенесён на записывающем исполнителе (`tests/tool.rs`); реальный запуск покрывает `hoh-bash-local`.
- «rejects a non-JSON numeric argument» — `serde_json::Value` не хранит NaN.
- «tools depend on the executor: no registration without ctx.shell» и «applies the built-in background default when apply() receives a bare config» — зависят от порядка монтирования в ядре и реестра заданий; не переносились.
- `background.spec.ts`: блоки «foreground commands as jobs», «background bash output», «owned background output» (реальный реестр заданий и процессы).
- `integration.spec.ts` (5 случаев): цикл агента + реальный реестр.
- Из «keeps this call's cancellation» перенесены обе половины (отказ запуска тем же поводом отмены; ошибка запуска после отмены).

## Не проверено
- Фоновый путь `execute`, продвижение по таймауту, эскалация песочницы и одобрение как целое: покрыты только чистые части (`process_job`, `process_outcome`, рендер), без реального реестра.
- Переключение foreground ↔ job-backed регистрации при появлении/исчезновении `Jobs`.
- Только Linux.

## Вопросы ведущему
- `hoh-shell`: сделать `ShellExecutor::resolve` возвращающим `Result<_, BoxError>` — тогда `catch_unwind` здесь и паника в `hoh-bash-local` исчезают.
- Нужны crates `hoh-jobs`, `hoh-user-approval`, `hoh-sandbox-policy` и единый тип `Agent` (сейчас три: shim `hoh-tools`, shim `hoh-working-directory`, будущий `hoh-agent`); после них убрать `src/shim.rs` и адаптеры и доделать фоновые тесты.
- `hoh-tools` не даёт публичного конструктора `ToolExecution` для тестов: тесты идут через `ToolExecutionPipeline`.
- `hoh-tools`: передавать в `present_result` успешное значение вызова (`value`), как pipeline образца; тогда `present_bash_result` читает `value.kind == "promoted"` и константа `false` удаляется (см. «Отличия»).
