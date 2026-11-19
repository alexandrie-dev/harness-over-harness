# hoh ← apps/cli @ 39163dd3: stage-0 launcher

## Перенесено

### Профиль окна

`window` — встроенный самостоятельный профиль нативного движка окна (`@hoh/window`, файл
`assets/bundle/window/cordis.patch.yml`): stdio JSON-RPC (`hoh-sdk-app` + `hoh-sdk-jsonrpc-server`),
llm/session/agent-loop, `hoh-model-host` (модель и ключ через `hoh-credentials-local`),
`hoh-user-approval` (host-режим), bash/read/write, working-directory, todo, ask-user и
`session-persistence-jsonl`. Профиль создаётся только при отсутствии его manifest; существующий
`profiles/window` пользователя не перезаписывается.

Песочница, как в базовом профиле dsh: `sandbox` (`hoh-sandbox-local`: bwrap → Landlock),
`sandbox-policy` (`workspace-write`, корень — рабочая папка сессии), `bash-sandbox`
(`@deepseek-ai/dsh-bash-sandbox`: `bash -c <команда>` оборачивается в `sandbox.confine`; сломанный раннер даёт
`SANDBOX_UNAVAILABLE`, а не запуск без ограничений) и `filesystem` = `@deepseek-ai/dsh-fs-sandbox`. Оба исполнителя
сообщают режим по умолчанию, поэтому `tool-bash` и `tool-fs` предлагают модели `sandbox_permissions` и сами
спрашивают пользователя (`approval/request` с mac) до выполнения. `workspace-write` всегда оставляет `/tmp`
записываемым — тесты песочницы поэтому создают папки в `CARGO_TARGET_TMPDIR`.

Склейка (`src/window_glue.rs`), которую не может сделать ни один крейт:
- `provide_stdio` — один читатель stdin: отдельный `std::thread` (чтение tokio-пула нельзя прервать, и
  закрытие рантайма ждало бы его после `shutdown`, пока клиент держит stdin) пересылает байты серверу через
  канал в памяти. Конец ввода объявляется, когда читатель сервера принял все байты (как `end` в Node), и
  его узнаёт `hoh-sdk-app` (закрыл stdin → выход 0). После `shutdown` сервер просит `request_exit`.
  Вызывается из `run_profile` только для профиля `window`, до загрузки плагинов.
- `WindowPrivilegedApprover` (`hoh:window-privileged-approver`) — подключает службу `HohPrivilegedApprover`,
  которую сервер спрашивает перед привилегированным `hoh/*` вызовом, к `Approval::approve_privileged`.
  Цикла зависимостей нет (сервер уже зависит от `hoh-user-approval`): сервер мог бы вызывать `approve_privileged`
  сам, тогда строка не нужна. Адрес модели с логином/паролем/ключом (`user:KEY@`, `?api_key=`) отклоняется
  до вопроса пользователю (`hoh/settings/set`, `hoh/model/key/set`, `hoh/model/probe`) и при восстановлении
  из файла.
- `WindowModelSettings` (`hoh:window-model-settings`) — адрес и имя модели переживают перезапуск:
  при старте читает `<профиль>/model-settings.json` поверх конфига `model-host`, затем ждёт
  `ModelHost::subscribe()` (`watch`) через `select!` по `token.cancelled()` и `changed()` и пишет файл
  атомарно (0600, с `fsync` файла и каталога) из `borrow_and_update()`. Пишется только та часть, что
  отличается от уже сохранённой. Ключ в файл не попадает. Ответ на `hoh/settings/set` приходит чуть раньше
  записи; при отмене (`shutdown`) выполняется последняя запись, так что к выходу процесса файл уже на диске.
  Ошибка записи уходит в журнал и повторяется через 1 с (`SAVE_RETRY`), пока не получится.
  Тем же приёмом правила первого запуска (`hoh/settings/rules/set`: интернет и разрешённая папка)
  хранятся в `<профиль>/window-rules.json` (атомарно, 0600): при старте `ModelHost::set_rules(saved)`,
  затем запись при изменении `PersistedSettings.rules`.
- `shutdown` во время ещё идущей загрузки дерева плагинов. Сам `request_exit` контекст профиля не
  освобождает. Освобождает его сервер: после ответа на `shutdown` и сброса очереди он вызывает
  `ctx.root().fiber().dispose()` (остановка всего ядра) и только потом `request_exit`. Загрузка, которая
  в этот момент ещё доделывает аудит, падает с `InactiveContext` (или, если ядро закрылось до аудита,
  со `StartupError`, где все причины — `Closed`). Чтобы это не стало кодом 1, сервер перед `dispose`
  вызывает необязательный хук `JsonRpcRuntimeHooks::with_exiting`; `provide_stdio` подключает его к
  `ProfileHost::mark_exiting` (поднимает флаг выхода без запуска остановки). `run_profile`
  (`stopped_by_exit_request`) считает отказ загрузки запрошенным выходом только при поднятом флаге и
  только для `InactiveContext` либо `StartupError` из одних `Closed`; любая другая причина и отказ без флага
  остаются ошибкой. Проглоченная ошибка не теряется: её текст пишется в журнал уровнем `warn`
  (`plugin tree load ended by the requested exit: …`). Код выхода по-прежнему даёт `request_exit(0)` сервера.
  Известное ограничение: если настоящий сбой обязательной строки совпал с `shutdown` между первым и
  аудитным `loader.settled()`, загрузчик перезаписывает причину на `Closed`, и сбой выглядит как запрошенный
  выход (виден только в этой строке журнала); исправить можно лишь в загрузчике, его не трогали. Строка,
  которую ядро успело освободить, но не убрать, может дать `StartupError` без причины — это остаётся кодом 1.
- Правка соседнего крейта `hoh-sdk-jsonrpc-server` (по разрешению ведущего): поле `exiting` и
  `JsonRpcRuntimeHooks::with_exiting`; `new` не менялся, тест порядка (flush → exiting → dispose корня → exit)
  — `answers_racing_shutdowns_before_disposal_and_exits_zero_once`.

Имя модели в запросах: окно объявляет в `initialize` заглушку (`local`) и ждёт, что движок возьмёт имя из
настроек; `HostedOpenAiAdapter::prepare_call` в `hoh-model-host` подставляет сохранённое имя в вызов
(метаданные маршрута остаются прежними).

Тесты `tests/window_profile.rs` (все через собранный `hoh` по stdio): состав профиля; initialize → prompt на
mock (`model`/`stream` в запросе, порядок и `seq` событий, текст ответа) → shutdown при открытом stdin, код
0, stderr пуст; выход при закрытии stdin; 3000 запросов с концом ввода сразу за ними — все отвечены;
привилегированные вызовы в host-режиме (`approval/request`, mac, отказ ничего не меняет и не пишет файл,
права 0600, перезапуск); адрес с секретом отклонён без вопроса (`hoh/settings/set`, `hoh/model/key/set`, `hoh/model/probe`); ключа нет ни в кадрах, ни в stderr, ни
в файлах кроме `.credentials.yaml`; опасная команда bash: `approval/request` до запуска, `rejected` — файл на
месте, `allowed-once` — выполнена; без подтверждения bash пишет внутри папки и не может писать вне неё;
привилегированный вызов без host-режима отклонён; запись вне папки (`write`, `edit` и `apply_patch` вне рабочей папки):
без эскалации не пишет/не правит, с эскалацией — `approval/request`, `rejected` — файл не создан/не изменён, `allowed-once` —
запись или правка прошла; ошибка записи настроек повторяется и в итоге файл содержит последнее значение (проверяется
содержимое); после смены настроек и `shutdown` при открытом stdin процесс выходит с кодом 0 быстрее 500 мс
(столько ядро Tauri ждёт до SIGKILL), файл настроек уже записан; смена модели через `hoh/settings/set` —
следующий запрос в mock несёт новое имя.
`tests/window_stdio.rs`: настоящий профиль окна в этом процессе поверх своих потоков
(`ProfileHost::with_window_stdio`, `WindowStdio`): `initialize` и `shutdown` отвечены, `request_exit(0)`, и флаг
выхода хоста уже поднят в момент освобождения корня (падает, если вместо `mark_exiting` передать `|| {}`
или убрать `with_exiting`).
`tests/exit_during_boot.rs` (без процесса, `run_profile` на тестовых плагинах): корень, закрытый после
поднятого флага выхода — запрошенный выход (изнутри плагина и снаружи во время монтирования); без флага и при
настоящем сбое плагина — ошибка; `StartupError` из одних `Closed` — выход только с флагом.

Требование к запускающему: `DSH_HOME` должен быть закрыт для других (0700), иначе
`hoh-credentials-local` отказывается хранить ключ.

Native binary `hoh`; source args, bin, profile-boot, dump-config,
dump-config-schema, startup-diagnostics, process-shutdown map to same-named
snake_case modules. Command parsing uses accepted hoh-commander, no new parser.
Five default profiles use hoh-official-bundles; six bundle patches, four Web
presets and their manifests are embedded without executing the source.
Dumps never activate plugins or evaluate expressions. The registry contains
only actual compiled plugins; unresolved packages produce incomplete schemas.
The minimal compiled table includes loader carriers/editor, LLM and Session stores,
Session projections, local FS, persona, system prompt, skills, storage/JSON/domain,
attachments, DeepSeek API extensions, Web registry/HTTP fetch and file references.
Core Agent currently uses its private SessionProjectionRegistry shim; it is not
silently wired to the incompatible native projection service. Product agent-loop,
headless/SDK/ACP startup modules and transports still need their own ports.

cmdline stays in hoh-loader as MAP row3 requires: Commander/kernel dependencies
do not point back to loader. Native cwd/home/environment/streams/exit/readiness/
stdin are explicit host inputs. Only main reads ambient facts, on a 64MiB stack;
tokio worker stacks are also 64MiB. Tests use temporary homes and private maps.
Profile creation stages, replaces with two renames, rolls back, and retains the
old copy on rollback failure. Root comments and empty list match the source.
CLI overlays are frozen per invocation, including subsequent ConfigEditor edits.
SIGINT/SIGTERM handlers precede boot and remain until completion; shutdown is
coalesced, bounded at five seconds, and escalates on repeated signals.

## Отличия от образца / решения

- O6: native PluginRegistry replaces npm/module resolution, runtime installation,
  compatibility and version policy. plugin.ts and installed-bin.ts management
  dispatch await MAP226's package-directory installer redesign (no crate assigned).
  Its command grammar is retained; execution returns an unavailable-build error.
- Source Node global proxy dispatcher has no native global counterpart.
  Profile boot resolves hoh-http-proxy from the immutable launch snapshot, with an
  explicitly supplied inherited map (no ambient environment read). ProxyTransport
  is an owned context tag exposing the installation; the HTTP provider receives
  its route_client_builder factory. A root effect clears the installation after
  child disposal, on success and failed startup. Diagnostic writer errors retain
  their original type/identity. hoh-http-proxy has nonfallible Drop; kernel shutdown
  also returns (). JS cleanup rejection aggregates cannot arise from those APIs.
  Source-module HMR is MAP225 **drop**, not a future hoh-hmr promise. This launcher
  does not claim filesystem watching; loader recomposition remains available.
- Native YAML dumps intentionally differ from the source's run-grouped headers:
  the Rust writer emits one header per row, uses symbolic `profile`/`home` labels
  rather than their file paths, and quotes strings according to its own scalar
  writer. This is a known format difference retained because `Composition::origins`
  stores loader layer identities, not the source's display-path runs. The binding's
  L2/L18 rules (`docs/kernel.md:930,1025`) specify the writer and per-row provenance,
  but do not prescribe those label/path and quoting choices.
- Launcher file diagnostics use the source's `dsh: failed to read/parse
  patches|overlay <path>: ...` forms in boot, YAML dump, and schema dump; structural
  list/mapping failures use the source's direct `dsh: <label> ... must be ...` text.
  Boot freezes overlays before reading the optional home patch, while YAML/schema
  dumps read home first, exactly as `profile-boot.ts:234` and
  `dump-config.ts:62-71` respectively. Both compose profile, home, then argv
  overlays (`profile-context.ts:74-80`). Proxy policy is installed before boot reads.
- Help, version, and argument rejection finish before resolving HOME/DSH_HOME,
  cwd, or the executable path, matching `apps/cli/src/bin.ts:24-27`. An appExit
  calls the bounded shutdown controller immediately (`profile-boot.ts:349`). To
  reproduce the immediate `fiber.dispose()` at `profile-boot.ts:298` without losing
  a row error racing that disposal, Rust immediately disposes active non-container
  rows, then disposes the profile owner once the current mount reports its result.
  Thus a stuck later mount cannot retain earlier rows and still reaches the bounded
  force deadline with the appExit code; a reported row error remains exit code 1.
- Schema-dump diagnostics deliberately retain `dsh: warning:`: the source writes
  `${NAME}: ${diagnostic.level}: ...` in `apps/cli/src/dump-config-schema.ts:43-45`
  and pins that exact warning in `dump-config-schema.expected.e2e.ts:346`.
- tests/expected/dump-config-schema.json is the unchanged source fixture. Its other
  bytes are compared after exactly four native regions: $defs.config0/config1
  (schemars typed Config vs Schemastery); loaderExpression.additionalProperties
  false (binding L1 reserved marker); entryMetadata.properties.inject (binding L4
  list/null). The actual application schema reports unresolved packages as errors,
  emits available native schemas and exits1 for incomplete headless configuration.
  Closed-grammar throwing JSON.parse expressions replace source test IIFEs.
  The source fixture's import-time stderr write is excluded by O6 static linking.
  Ajv is not a workspace dependency: the complete document is pinned byte for byte
  outside those four regions; valid/invalid native Config values are additionally
  checked through kernel's closed validator with the referenced Config definitions.
  Full if/then document execution by Ajv is not claimed.
- Native reports retain Error Debug/source/aggregate members, full long values,
  cycle-safe cause traversal, entries and composition. runtime hoh/<version>
  replaces nodeVersion; JS symbols/property descriptors/getters/custom-inspect
  have no Rust equivalents. Files create_new0600/directories0700, write+flush;
  terminal writes are awaited, their errors propagated unchanged.
- FailLoudProcess is an owned native host-event adapter, not global JS
  unhandled-promise listeners. Kernel contains plugin task failures; launch panics
  are caught and routed through the guarded cleanup. Tests inject native process
  events, microtasks, report facts and terminal acknowledgements.
- Linux SIGTERM0/SIGINT130 handlers and a subprocess regression are implemented;
  execution of the signal regression is blocked by the sandbox's EPERM on Unix
  self-pipe writes (see verification). Non-Unix uses ctrl_c; Windows signal/drive-relative
  cwd behavior has not been executed on Windows.
- The source's execa capture strips exactly one trailing newline. Help snapshots
  reproduce that capture step; process output still contains Commander's original
  two trailing newlines (commander/lib/command.js:2670–2687).
- Profile suffix is injected into the owned EvalEnv before config interpolation;
  no unsafe process environment mutation is introduced. Future model-provider
  crates (hoh-llm-deepseek-api-key/hoh-llm-pi-ai) must pass the profile identity to
  hoh-llm attribution helpers explicitly; assembled network requests are not a
  stage0 claim.

## Таблица исходных test files

All 71 source test files, including nested suites, are listed. A numbered group
means a source it/it.each declaration; every parameter row is kept when ported.
Fixture/helper files are data, not separately claimed tests.

| Source under apps/cli/tests | Fate / implementation or missing owner |
|---|---|
| `agent-team-headless.e2e.ts` | Не применим: hoh-headless (stage2), hoh-agent/agent-team and task orchestration. |
| `args.spec.ts` | Перенесено: all 18 groups and parameter tables; args.rs, complete launcher-help.txt. |
| `built-bin.e2e.ts` | Частично: native binary groups :360,:646,:676,:813,:1207–1306; installed symlink/help added. Details and remaining groups below. |
| `desktop-host.e2e.ts` | Не применим: browser/Web host skip MAP228/384; desktop separate workspace. |
| `dsh-badge.expected.e2e.ts` | Не применим: experimental badge integration/UI missing, no assigned Rust crate in MAP. |
| `dump-config-schema.expected.e2e.ts` | Частично: #1–4,#7–10 native equivalents; #4 raw malformed rows tested collector (L4 CLI strictness); local npm copy O6. #5 invalid native annotation/#6 Schemastery lazy/transform/import formats not applicable (schemars + static registry). Four explicit golden differences below. |
| `github-webhook-real.e2e.ts` | Не применим: real GitHub service (§10); webhook integration owner not assigned. |
| `headless-shutdown.e2e.ts` | Частично: escalation semantics in process_shutdown.rs and native signal in built_bin.rs; assembled app awaits hoh-headless (stage2). |
| `lazy-search-startup.compat.spec.ts` | Не применим: hoh-agent, hoh-tools, hoh-mcp-manager and source Node import timing are outside stage0. |
| `memory-mcp-configs.spec.ts` | Частично: #1 all 3 unchanged examples parsed with package pins/transport/secret checks. #2 waits hoh-mcp-client and hoh-tools fixture-tool discovery. |
| `optional-bundle-transitions.e2e.ts` | Не применим: experimental auto/badge/live-share/AI decision integrations absent; owners not assigned in MAP; later product decision. |
| `plugin-compatibility.expected.e2e.ts` | Не применим: npm/version exemptions excluded O6; package-directory installer redesign, MAP row226, crate not assigned. |
| `plugin.spec.ts` | Не применим: all 10 groups operate npm/pnpm or Desktop package manager; O6 / MAP226 redesign. |
| `process-shutdown.spec.ts` | Перенесено: 9 groups; process_shutdown.rs; extra concurrent-signal regression. |
| `profile-hmr.spec.ts` | Перенесено: all 4 pure command-line flag groups, profile_hmr.rs. This does not implement HMR (MAP225 drop). |
| `profile-initialization.spec.ts` | Перенесено: all 11 groups (8 integration + 3 fault-injection units), all templates and synchronized processes. |
| `profile-mcp.spec.ts` | Перенесено: both pure metadata/composition groups; profile_mcp.rs. Runtime MCP activation not claimed. |
| `profiles/acp/tests/acp.e2e.ts` | Не применим stage0: hoh-acp-app (stage4), hoh-sdk-app, hoh-agent/session. |
| `profiles/acp/tests/cleanup.e2e.ts` | Не применим stage0: hoh-acp-app (stage4), hoh-sdk-app, hoh-agent/session. |
| `profiles/acp/tests/control-surface.e2e.ts` | Не применим stage0: hoh-acp-app (stage4), hoh-sdk-app, hoh-agent/session. |
| `profiles/acp/tests/escalation.e2e.ts` | Не применим stage0: hoh-acp-app (stage4), hoh-sdk-app, hoh-agent/session. |
| `profiles/acp/tests/goal.expected.e2e.ts` | Не применим stage0: hoh-acp-app (stage4), hoh-sdk-app, hoh-agent/session. |
| `profiles/acp/tests/hooks.e2e.ts` | Не применим stage0: hoh-acp-app (stage4), hoh-sdk-app, hoh-agent/session. |
| `profiles/acp/tests/image-offload.expected.e2e.ts` | Не применим stage0: hoh-acp-app (stage4), hoh-sdk-app, hoh-agent/session. |
| `profiles/acp/tests/stdout.expected.e2e.ts` | Не применим stage0: hoh-acp-app (stage4), hoh-sdk-app, hoh-agent/session. |
| `profiles/cua-mcp/cua-mcp.expected.e2e.ts` | Не применим: CUA/browser integration and hoh-mcp-client unavailable; GUI checks §10. |
| `profiles/headless/tests/coding-task.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/compaction.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/credentials.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/full-loop.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/headless.expected.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/keyless-smoke.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/mcp-pagination.expected.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/ptc.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/real-model.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/resume.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/semantic-checkpoint.expected.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/session-format-guard.expected.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/source-tool.built.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/subagent-diagnostic.expected.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/subagent-inheritance.expected.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/todo-write.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/headless/tests/workspace-context-resume.expected.e2e.ts` | Не применим stage0: hoh-headless (stage2), hoh-agent/session/tools persistence; real-model/credential network checks §10. |
| `profiles/official-installer-packed.e2e.ts` | Не применим: npm packed installer O6/MAP226 redesign; webhook integration not assigned. |
| `profiles/sdk/checkpoints.expected.e2e.ts` | Не применим stage0: hoh-sdk-app/hoh-sdk-jsonrpc-server (stage1), hoh-agent/session/tools; npm plugin manager O6 where used. |
| `profiles/sdk/desktop-tools.expected.e2e.ts` | Не применим stage0: hoh-sdk-app/hoh-sdk-jsonrpc-server (stage1), hoh-agent/session/tools; npm plugin manager O6 where used. |
| `profiles/sdk/dynamic-tool-cache.e2e.ts` | Не применим stage0: hoh-sdk-app/hoh-sdk-jsonrpc-server (stage1), hoh-agent/session/tools; npm plugin manager O6 where used. |
| `profiles/sdk/keyless-smoke.e2e.ts` | Не применим stage0: hoh-sdk-app/hoh-sdk-jsonrpc-server (stage1), hoh-agent/session/tools; npm plugin manager O6 where used. |
| `profiles/sdk/office-cli.e2e.ts` | Не применим stage0: hoh-sdk-app/hoh-sdk-jsonrpc-server (stage1), hoh-agent/session/tools; npm plugin manager O6 where used. |
| `profiles/sdk/plugin-manager.expected.e2e.ts` | Не применим stage0: hoh-sdk-app/hoh-sdk-jsonrpc-server (stage1), hoh-agent/session/tools; npm plugin manager O6 where used. |
| `profiles/sdk/schedule-resume.e2e.ts` | Не применим stage0: hoh-sdk-app/hoh-sdk-jsonrpc-server (stage1), hoh-agent/session/tools; npm plugin manager O6 where used. |
| `profiles/sdk/tool-parity.spec.ts` | Перенесено: все 3 pure helper groups, tests/tool_parity.rs + support/tool_parity.rs; SDK runtime не требуется. |
| `profiles/sdk/user-questions.e2e.ts` | Не применим stage0: hoh-sdk-app/hoh-sdk-jsonrpc-server (stage1), hoh-agent/session/tools; npm plugin manager O6 where used. |
| `profiles/web/tests/creator-plugin-manager.expected.e2e.ts` | Не применим: Web transport skipped MAP228/384, hoh-agent/session/SDK absent; HMR source replacement MAP225 drop. |
| `profiles/web/tests/office.expected.e2e.ts` | Не применим: Web transport skipped MAP228/384, hoh-agent/session/SDK absent; HMR source replacement MAP225 drop. |
| `profiles/web/tests/public-url.expected.e2e.ts` | Не применим: Web transport skipped MAP228/384, hoh-agent/session/SDK absent; HMR source replacement MAP225 drop. |
| `profiles/web/tests/runtime-roster-isolation.spec.ts` | Не применим: O6 npm alias/module-resolution evidence and browser runtime roster (MAP228/384 skip); experimental package-policy script has no native stage0 owner. |
| `profiles/web/tests/schedule-timing.expected.e2e.ts` | Не применим: Web transport skipped MAP228/384, hoh-agent/session/SDK absent; HMR source replacement MAP225 drop. |
| `profiles/web/tests/web-best-effort-startup.expected.e2e.ts` | Не применим: Web transport skipped MAP228/384, hoh-agent/session/SDK absent; HMR source replacement MAP225 drop. |
| `profiles/web/tests/web-default-isolation.expected.e2e.ts` | Не применим: Web transport skipped MAP228/384, hoh-agent/session/SDK absent; HMR source replacement MAP225 drop. |
| `profiles/web/tests/web-failure-matrix.expected.e2e.ts` | Не применим: Web transport skipped MAP228/384, hoh-agent/session/SDK absent; HMR source replacement MAP225 drop. |
| `profiles/webhook-packed.e2e.ts` | Не применим: npm packed installer O6/MAP226 redesign; webhook integration not assigned. |
| `resolved-profile-boot.spec.ts` | Частично: #1 preparation/boot cleanup; #2 native shared layers/services/readiness/disposal; #3 all five URI encodings; #4 suffix before config/apply. npm runtime links O6, HMR MAP225 drop; proxy/tree disposal are nonfallible native contracts, not JS rejection mocks. |
| `source-launch.compat.spec.ts` | Не применим: source TS execution/lib layout excluded O6; Rust installed binary is tested in built_bin.rs. |
| `startup-diagnostics.spec.ts` | Частично: 6/6 native equivalents; #4 native Error Debug/source/aggregate/cycle instead of JS symbols/property descriptors/custom inspect. |
| `telemetry-switch.spec.ts` | Перенесено: all 3 pure groups, telemetry_switch.rs. |
| `web-agent-presets.e2e.ts` | Не применим: hoh-agent, hoh-session, missing Web app (MAP228 skip); pure shipped presets parse in loader shipped_yaml. |
| `web-agent-shutdown.e2e.ts` | Не применим: Web app skip MAP228; hoh-agent and Web session lifecycle absent. |
| `web-auth.e2e.ts` | Не применим: Web host/browser authentication skipped MAP228/384. |
| `web-browser-open.expected.e2e.ts` | Не применим: browser launch/Web transport explicitly skipped MAP228/384; desktop is a separate workspace. |
| `windows-shell.spec.ts` | Частично: all 4 declaration/platform groups; #1 Node cold-import success excluded O6; stage5 hoh-pwsh-sandbox/tool-pwsh absent. |

## Built-bin: case-level exclusions

- :377 app help/usage routing covered by native cmdline tests; assembled Web/SDK
  awaits hoh-sdk-app and skipped Web app. :434 SDK import/EOF and :458 protocol
  await hoh-sdk-app/hoh-sdk-jsonrpc-server; :534 ACP awaits hoh-acp-app.
- :618 task positional awaits hoh-headless. :688 creation/replacement tested before
  dumps and in profile_initialization; boot awaits skipped Web app. :732 rejected
  app arguments retaining a profile awaits application parser.
- :755 endpoint/managed credentials awaits hoh-sdk-app and credential integration;
  native environment provenance itself is covered by app-boot #1–33.
- :795 optional serving awaits SDK. :829 file hot reload and :880 watched bundle
  transactions not claimed (watcher MAP225 drop); :927 Node HMR explicitly dropped.
- :984,:1000,:1015,:1049 immutable args/composed fallback/help covered in cmdline.rs;
  Node .mjs loading excluded O6, watcher reload not claimed.
- :1062 pnpm auth/stdin/status,:1076 relative add,:1125 pnpm aliases,:1169 metadata
  dependency activation excluded O6/MAP226 redesign.
- :360,:646,:676,:813,:1207,:1218,:1229,:1241,:1254,:1306 are native binary tests
  with L2/L18 differences above; :657 installed symlink covered on Unix.

## Не перенесено

No sessions, SDK server, agent loop, MCP tool discovery, ACP or Web app is invented
for stage0. Owners/decisions are named per source file above. Real network/model,
GUI and external-service scenarios are excluded by PORTING §10.

## Исправления после прогона ведущего 19.11 (01:59)

Исходные девять отказов: logs/l1.lead-test-0159.txt. `ProfileHost::context()`
теперь даёт отдельный дочерний контекст профиля, в котором живут transport,
services и loader. Kernel uid 0 не выдаётся за disposable scope: по R15 он
всегда Active, shutdown снимает его эффекты и закрывает actor. Dispose-проверки
сохранены на настоящем контексте профиля и дополнены проверкой Closed ядра.
Native adaptation/public accessor указаны явно; чужой kernel не изменялся.

Boot-failure fixture использует required id webserver вместо optional failing:
L19 требует предупреждать, а не отвергать boot из-за optional строки. Сохранены
ожидание отказа, текст startup failed, освобождение дерева и transport. Для
ошибок причин добавлена проверка, что одинаковые сообщения **разных** ошибок
не исчезают даже под transparent aggregate wrapper с тем же адресом.

Schema Include/alias и память исправлены в loader; см. его дополнение от 19.11.
Исходные memory YAML и acceptance assertions не заменялись. Ведущий подтвердил
13/0 built_bin вне песочницы: EPERM у сигнала относится только к sandbox.

## Проверка

До исправлений этого раунда: ведущий выполнил test --no-fail-fast вне sandbox,
summary /mnt/toshiba/alexandrie/hoh/logs/l1.lead-test-0159.txt: built_bin 13/0,
девять отказов в остальных целях. EPERM UnixStream/self-pipe был только внутри
sandbox: Tokio signal/unix.rs:250–258 использует такую запись для уведомлений.
Тест сигнала сохранён включённым с явной preflight-проверкой ограничения.

После исправлений:
- Новый cargo check --offline -p hoh-loader -p hoh --all-targets дошёл до
  Checking hoh, затем более 25 минут без новых диагностик; прерван (exit130).
- Полный cargo test --offline --no-fail-fast -p hoh-loader -p hoh сначала
  ожидал build lock. После прерывания этой попытки lock оказался свободен;
  прежняя диагностика его владельца была недостаточной.
- Новый полный прогон с отдельным sampler действительно начал Compiling
  hoh-kernel, после чего **и cargo, и rustc остановлены в Tl**. Это подтверждает
  /tmp/hoh-l1-lead-fix-20261119/final/processes.log (PID 5 и 11 внутри
  namespace запуска). Старые и новые T/Tl не возобновлялись. Новая сессия
  прервана стандартным Ctrl-C (wrapper exit1), до выполнения тестов.
- cargo fmt -p hoh-loader -p hoh -- --check: **exit0** (19.11.2026).
- Clippy после текущих исправлений не запускался: pipeline не дошёл до него.
  Прежний clippy exit0 23m05s относится к версии до этих исправлений.

Журналы новой попытки: /tmp/hoh-l1-lead-fix-20261119/final/{test,processes}.log;
явные статусы рядом. Ни один runtime-тест текущей версии ещё не выполнен,
поэтому нового списка EPERM/socket failures или результата N/0 нет.
Все команды использовали CARGO_NET_OFFLINE=true, CARGO_BUILD_JOBS=2, target-l1;
compile/test/clippy также --offline. Ни один из девяти отказов не проигнорирован.
R54 nonrecursive-$ref default остаётся прежним единственным ignore.


## Профиль окна: подтверждение разрушающих команд внутри папки (`hoh-window-destructive-guard`)

Отличие профиля окна от dsh, у которого такого плагина нет. В dsh песочница `workspace-write` пускает
`rm` и перезапись файлов внутри папки сессии без вопроса. Для окна это решено иначе (условие вехи
«опасные действия подтверждаются», экран `design/build-a/approval-1440.png`): bash-команда, которая
может удалить или перезаписать существующий файл, сначала уходит на подтверждение.

- Подключение: `hoh:window-destructive-guard` в `src/registry.rs` и в
  `assets/bundle/window/cordis.patch.yml` (после `window-privileged-approver`). Только профиль окна;
  профили, сверяемые с dsh, не затронуты.
- Механика: `PreToolDecision::Ask` из слушателя `ToolsPreExecute` → `service_ask` → `hoh-user-approval`
  → в режиме host `approval/request` с mac; `rejected` — команда не запускается, `allowed-once` —
  запускается, следующий такой же вызов спрашивает снова. Правила и текст вопроса — в
  `crates/hoh-window-destructive-guard/PORT_NOTES.md`.
- `humanText` — русский текст под ключом `"en"` в `display_reason` (протокол хоста несёт один текст);
  сырая команда — в `reason`.
- Вызов с `sandbox_permissions: "danger-full-access"` (fix1): охрана задаёт вопрос сама, по-русски и в
  том же виде, что остальные («Помощник просит выполнить команду без ограничений песочницы: …»), а
  собственный вопрос bash об эскалации одобряется грантом `ApprovalService::pre_approve_escalation`
  (`hoh-user-approval`), который охрана записывает только после `allowed-once` хоста; грант привязан к
  (сессия, callId, `bash`, причина) и одноразовый (fix4). Один вопрос на вызов; `hoh-tool-bash` и
  `hoh-sandbox` не менялись. Тесты fix4: отклонённый/разрешённый bash и `write` с тем же callId.
- fix3: цель или опция со значением цели с подстановкой (`--file=$OUT`, `-o$OUT`, `-t$D`, `-$X`, маска) во всех ветках классификатора — вопрос, если режим команды может перезаписать/удалить.
- fix2: файловые проверки охраны идут на блокирующем потоке (`spawn_blocking`); `tar` определяет режим после всех опций; `env -C` / `sudo -D` переносят проверку целей в новую папку.
- fix1: ещё `curl -o/-O`, `wget -O`, `unzip` (без `-n`/`-l`), `patch`, `git apply`, `awk -i inplace`.
- fix0: охрана спрашивает и про `sed -i` / `perl -i`, `rsync --delete*`, `tar x…`, `ln -f`, `install`,
  `cp`/`mv` с `-f`, `python|node|perl|ruby` со встроенным кодом. Запуск файла-скрипта и сборки
  (`python x.py`, `node x.js`, `make`, `npm run`, `cargo`, `pytest`) сознательно без вопроса: вторая
  линия — песочница; внутри папки она ничего не спрашивает, и это принятое ограничение.
- Тесты: `tests/window_guard.rs` (удаление: отклонить / разрешить / повтор; перезапись; двенадцать
  обычных команд без единого `approval/request`; обе стороны эскалации) и два сценария в
  `tests/window_e2e.rs`. Файлы `tests/window_profile.rs`, `src/profile_boot.rs`, `src/window_glue.rs`
  этим изменением не тронуты.
