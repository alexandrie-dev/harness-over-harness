# hoh-conformance ← packages/test-support/session-snapshot, snapshots/{session,sdk,acp} @ 39163dd3

## Круг исправлений cf1

1. Headless по-прежнему требует записанный exit code 0/1. ACP и SDK после успешного протокола
   ждут завершения без проверки числового кода. ACP: `session-snapshot/src/launcher.ts:271–294,445–448`.
   SDK: `sdk/client/src/client.ts:263–267` сохраняет код только для диагностики; `:494–510`
   закрывает процесс, а `sdk/client/src/dispose.ts:82–98` ждёт exit, не проверяя его значение;
   `snapshots/sdk/sdk.snapshot.ts:1042–1056` вызывает этот close, не добавляя exit assertion.
   Shutdown в SDK — best effort с исходным пределом 1000 ms (`client.ts:499–503`), после него
   закрывается stdin. Неудавшийся initialize/незавершённый протокол остаются отказами.
   Перед исправлением настоящий runner с golden ACP handshake и exit 1 дал phase=exit;
   после исправления этот regression проходит. SDK double также проверяет shutdown/EOF.
2. `Normalizer::legacy_log` переносит `normalize.ts:359–387`: сначала собирает UUID/short job ids
   из job/registered и job/settled, резервирует все уже встречающиеся в **сыром** журнале
   `{{id:N}}`, заменяет job ids во всех строковых листьях и только затем выполняет legacy UUID scrub.
   Этот общий путь используют и закрепляющие request/header, и живые header/prompt comparisons.
   Regression с одним UUID одновременно в job event, header и prompt и с заранее занятым
   `{{id:1}}` сначала упал (получалось `{{sessionId}}`), после исправления везде получается `{{id:2}}`.
   Новый публичный legacy_log — общий интерфейс сравнения для этих потребителей; отдельного
   независимого публичного API такого имени у образца нет.
3. До удаления поля version проверяются две отдельные гарантии: headless/SDK writer пишет V4
   **для каждой роли** (`headless.snapshot.ts:199–205`, `sdk.snapshot.ts:362–367`);
   SDK wire roles имеют одно общее поколение, отдельно на actual и expected стороне
   (`sdk.snapshot.ts:533–541`). Диагностики: `current writer generation: ...; expected v4` и
   `SDK wire goldens require one common writer generation (actual|expected): ...`.
   ACP не получает дополнительного writer-generation ограничения, которого в его harness нет.
   Публичные compare::current_writer_generation и compare::sdk_wire_generation — интерфейсы
   runner и тестов для этих source assertions; самостоятельных экспортов с такими именами у dsh нет.
4. Executable double принимает настоящий `--profile/--patch/task`, независимо проверяет
   argv/материализованные патчи, выбранные input bytes и переменные README 1.2; клиентские процессы
   наследуют контрольные NODE_OPTIONS/telemetry/key и отравленные proxies, double проверяет
   их наследование/перекрытие/удаление. Fixture plans строятся из read-only corpus вне runner.
   Double пишет parent/child outputs в настоящие storage roots, исполняет SDK/ACP stdio-обмен,
   проверяет prompt inputs/attachments, cancellation, permission answers и stdin EOF/shutdown.
   Реальный runner записывает `checks.jsonl` с выполненными сравнениями. Неперенесённые
   setup/migration/scenario gates применяются после доступных сравнений: они всё равно запрещают
   pass, но больше не прячут ошибки сравнения за преждевременным capability failure.
   `tests/runner_process.rs` запускает runner CLI для identity и шести мутаций всех 242 сценариев;
   identity обязана дойти до точного объявленного gate или пройти два ACP случая без модели.
   Все 14 SDK result.expected.json должны получить выполненный `SDK result: pass`.
   Сам result.expected.json не входит в fixture plans и double его не читает: результат строит
   настоящий runner из receipt/events протокола, затем сравнивает выбранный последний SDK result.
   Для двух ACP случаев без model events мутации отсутствующих tool/usage/prompt/events повреждают
   соответствующий protocol oracle и должны отказать в frames; extra file всегда проверяется в tree.
   Headless/SDK без model call также проходят log comparison. Если нет tool/result, usage или
   system/message, соответствующая мутация добавляет событие с
   неожиданным значением: утверждение о sensitivity не предполагает наличия этих полей во всех логах.
   DSH_MCP_DEMO_URL для двух сценариев с startHttpMcpFixture пока отсутствует; double проверяет это
   состояние, а точный соответствующий missing check по-прежнему запрещает pass.
   ACP читает turn boundaries с диска во время протокола: double держит исходный журнал до stdin EOF
   и только затем пишет мутированный harvested output. Иначе reorder turn/end проверял бы таймаут
   протокола вместо требуемой sensitivity сравнения журналов. Аргументы, env и протокол остаются
   обязательными assertions для каждого мутированного запуска.
   Double не подменяет модель/инструменты в настоящем hoh и никогда не подключается runner автоматически.

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `session-snapshot/src/session-files.ts`, README 1.1 | `src/corpus.rs`, `runner::harvest` | выбор максимального поколения каждой непрерывной роли, проверка имени/заголовка; последние поколения хранилища по физическому каталогу |
| `manifest.ts`, README 4 | `src/yaml.rs`, `corpus::discover` | JSON_SCHEMA без временных меток и merge keys; закрытые поля манифеста; прочитаны все 242 манифеста и строки EXPECTED.tsv |
| `launcher.ts`, `headless.snapshot.ts`, `sdk.snapshot.ts`, README 1.2 | `src/runner.rs`, `corpus::patch_sources`, `yaml::materialize` | профили, порядок overlays, SDK legacy context и дополнительные overlays, изолированные cwd/home-data/spill; фиксированный cwd длиной 192 единицы UTF-16 JSON-строки; HOME наследуется |
| `headless.snapshot.ts:workspaceSetups`, `workspace.ts`, README 1.2, 3.3 | `src/workspace.rs` | копирование без разыменования симлинков; девять подготовок из десяти; text/binary/base64/symlink/empty-directory, точные корневые исключения, `.empty` только в expected-дереве |
| `headless.snapshot.ts:taskFromSession`, README 1.3 | `compare::task` | первое принятое одноблочное пользовательское сообщение inbox, затем user/message, затем manifest.input.task |
| `sdk.snapshot.ts:sdkActions/materializeInput`, SDK client, README 1.3, 5.1 | `src/protocol.rs` | initialize, session/query, receipt по messageId, ожидание idle и turn/end, команды, вложения, session/wait, shutdown, approval/request и plan-review answers |
| `harness.ts:runStep`, README 1.3, 5.2 | `protocol::acp` | все семь операций корпуса: initialize, newSession, newSessionExpectError, prompt, promptContent, promptAndCancel, waitForTurnEnd; также waitForFile; permissionAnswers по kind |
| `identity.ts`, README 2.1 | `normalize::identities` | типизированные пространства session/message/approval/workflow/command/rpc/retry/id, первое появление, общие связи родителя и детей; нет произвольного маскирования UUID в tool/user text |
| `normalize.ts`, README 2.1–2.4, 2.6 | `src/normalize.rs` | cwd aliases/границы/ограниченная канонизация путей, spillLocator, строго ограниченное окно event-read, служебные часы, конверты и inclusive sourceEventSeqs, RPC id, usage_update; остальные числа/тексты/порядок сохраняются |
| `suite.ts:verifyHeaders`, headless/sdk verifyHeaders, README 2.5, 3.2 | `src/compare.rs` | промпты с маркерами изменений сравниваются как строки, схемы как JSON-значения, полные request/header, child sidecars, порядок system/message; ACP revisions исключают series из закрепляющего набора |
| `headless.snapshot.ts:stderrFromSession/finalTextFromSession`, README 3.4 | `compare::{stderr,headless_stdout,expected_exit}` | точные stdout/stderr/exit, reasoning через границы compact streams, error code/message |
| `normalizeNotifications/normalizeResult`, README 5.1 | `normalize::notifications`, `runner::execute` | отдельная нормализация SDK events без удаления seq/time-конвертов, typed feedback/job identities, notification JSON goldens и строковый result golden |
| `suite.ts:reconcileCatalogCreationTimes/unknownToolCallIds`, README 1.7, 3.1 | `compare::catalog_creation_times`, `runner::execute` | до нормализации проверяются часы catalog/child header в SDK/ACP; UNKNOWN_TOOL запрещён только ACP; script exhausted проверяется только SDK |
| README 3.5, решение ведущего о миграциях | `compare::migration_gate` | настоящая V0→V1 через `hoh-session-format-v0-to-v1`, completion projected/packed envelopes и tool placeholders; отсутствующие V1→V2/V2→V3/V3→V4 дают точную причину, без pass |
| README 1.4, 1.6, 1.7 | launch environment и materialized patches | replay остаётся плагином внутри hoh; скрипты модели, !!js и привязка по первому model call не исполняются прогонщиком |
| brief: интерфейс и отчёт | `src/main.rs`, `src/runner.rs` | повторяемые suite/scenario selectors, jobs, таймауты, пять TSV-полей; отдельный runner-errors.tsv; diff.txt через hoh-jsdiff; exit 0 при полном учёте pass/fail, exit 1 при runner error |
| brief: identity/sensitivity/normalization | `src/bin/fake.rs`, `src/bin/fake/*`, `tests/{runner_process,fix_round,corpus,normalization}.rs`, `tests/support/plans.rs` | drop-in fake запускается настоящим runner CLI; независимый fixture oracle проверяет argv/env/протокол, пишет весь корпус с живыми id/cwd/часами/сателлитами; result goldens и шесть мутаций проходят через execute |

## Отличия от образца

- Это самостоятельная Rust CLI, а не перенос всех публичных API test-support/Vitest. Публичные
  `Scenario`, `Options`, `Outcome`, `Failure`, `Normalizer`, `Context`, `Action`, `Rpc`, вспомогательные
  модули и операции чтения/сравнения нужны двум бинарникам и integration tests; такого единого API у dsh нет.
  Диагностические причины чистых помощников представлены `Result<T, String>`; сценарный отказ отдельно
  хранит phase/reason/diff и не смешивается с runner error.
- Никаких record/refresh/write-back в образец. Исходники и EXPECTED.tsv не изменяются. Retained input
  читается отдельно от `writer[.k].expected.jsonl`, который служит только выходным оракулом.
- `hoh-loader::yaml::parse_value` приватен. Публичные parse_entries/parse_patches не подходят для
  snapshot.yml: они требуют массив loader rows и якорят относительные имена в file URLs. Другой crate
  не менялся. Здесь событийный saphyr-parser читает JSON-представимое подмножество JSON_SCHEMA;
  неизвестные explicit tags отвергаются. Все YAML, выбранные для запуска корпуса, проверяются тестом.
  Это не новая общая YAML-библиотека: предел вложенности 128; циклические aliases не поддержаны.
- Имена/id строк и package specifiers сохраняются. Только относительные plugin names вставок и их групп
  материализуются в `local:<clone-relative-path>`, по решению ведущего. JSON как подмножество YAML
  сохраняет `__jsExpr`-представление `!!js`; вычисляет выражения loader, не runner.
  Node node_modules symlinks не создаются: bare package names разрешает реестр/alias table Rust-бинарника.
- Входы проверяются на доступность миграций после результата запуска: реальный отказ launch/exit
  не подменяется сообщением о ещё недоступном следующем формате. V0→V1 действительно исполняется;
  gate стоит **перед любыми сравнениями**, чтобы diff не скрывал missing migration.
  Тест проверяет missing-edge reason для каждой исторической роли. ACP без model turn миграция не нужна.
- Сравнения JSON используют JS reader/writer и deep_equal_json: порядок ключей объекта несущественен,
  порядок массивов и событий существенен. Полный промпт, stdout/stderr и SDK result остаются строками.
  ACP-кадры сравниваются как значения JSON, что разрешено README 6.2.
- Каждая попытка имеет отдельный `runs/<suite>/<scenario>/run-*/`, cwd и spill внутри неё; ссылка на
  последнюю попытку — latest-run.txt. diff.txt лежит также непосредственно в каталоге сценария.
  outside-temp проверяется по реальному cwd: корень прогонов должен находиться вне временных write grants.
  При неподходящем корне это setup failure, а не pass. Данные попыток оставляются для диагностики.
- Таймауты доступны как `--timeout-ms` (30000), `--request-timeout-ms` (110000, SDK),
  `--wait-timeout-ms` (10000, ACP); polling журнала/файла — 10 ms. На Unix дочерний процесс получает
  отдельную process group; завершение/таймаут убирает оставшихся потомков безопасным rustix API.
- Skip не превращается в pass: отсутствующие host tools/неподходящая платформа приводят к отказу.
  `acp/handshake` и `acp/reject-extra-dirs` не чувствительны к tool/missing/reorder/usage/prompt:
  их persisted log содержит только заголовок, без tool results, usage, prompts и двух body rows для
  удаления/перестановки. Эти десять пар mutation/scenario явно учитываются как not sensitive;
  agentInfo.name больше не подменяет отсутствующие targets. Tree mutation применима к обоим.
- Identity retained-сценария проверяет materialized текущий writer oracle, а не воображаемый результат
  ещё отсутствующей полной миграции. Тест доступной миграции независим и никогда не объявляет retained
  сценарий прошедшим.

### Инструменты хоста

Для самого прогонщика: построенный `hoh`, readable clone dsh, файловая система с симлинками, место для
изолированных run directories. Fake/сравнения используют только Rust и corpus, не Node или сеть.
Для настоящих tool-сценариев нужны инструменты команд записанного корпуса: `bash`, `/bin/sh`, обычные
POSIX utilities (`cat`, `mkdir`, `rm`, `sleep`, `sed`, `chmod` и т.п.), `python3`, `node`; для pwsh-сценариев —
работающий `pwsh`. Office-retired-ptc находит node в PATH, создаёт два shell wrapper и не запускает Node
в setup. Git потребуется настоящему git-worktree setup; в этом writer lane state-changing git запрещён.
MCP/office/CUA/PTC требуют соответствующих портированных пакетов и stand-in plugins; наличие host tool
не заменяет их реализацию. Ничего не устанавливалось, JS и источники dsh не запускались.

`DSH_MCP_DEMO_URL` **требуется** dsh для `session/plugin-manager-mcp` и `session/mcp-template-error`:
headless.snapshot.ts:1400–1401 создаёт startHttpMcpFixture, :1448 передаёт mcpDemo.url поверх manifest env.
packages/mcp/mcp-client/tests/http-fixture.ts поднимает localhost HTTP MCP endpoint на свободном порту,
реализует ping/pong (либо deliberate rejection), фиксирует calls/auth и закрывается после сценария.
Runner пока не задаёт переменную: эта fixture не перенесена; произвольный URL не воспроизводит её
протокол, учёт и lifecycle. В этом задании запуск серверов запрещён. Оба сценария остаются fail-closed
на `pluginManagerMcp`/`verifyMcpTemplateError`; fake проверяет фактическое отсутствие переменной,
а не подтверждает точную env parity этих двух сценариев. Перенос HTTP fixture остаётся ведущему.

### Ответы README 7.5 и 7.6

5. В V4 codec делегирует V3, тот V2. `session-format-v1-to-v2/src/codec.ts:114–127`
   устанавливает inheritedEventCount равным **seq маркера** `session/end-seed` с `data.inherited:true`,
   не seq+1. Для unseeded cut=0; seeded без маркера и unseeded с inherited-маркером отвергаются.
   V3 дополнительно учитывает принятый валидатором маркер при recoverable scan
   (`session-format-v2-to-v3/src/codec.ts:45–60`). Replay читает этот cut из artifact каталога;
   runner его не угадывает и не привязывает параллельных детей новым правилом.
6. `profile-context.ts:resolveTelemetryPatch` отключает session-telemetry-otel при **любом непустом**
   DSH_TELEMETRY_DISABLED, включая `0`/`false`. ACP runner эту переменную не задаёт и наследует HOME/env.
   Базовый bundle монтирует `otel` и `session-telemetry-otel`; default mode=FEEDBACK_ONLY,
   default endpoint=https://dsh-otel-collector.deepseeksvc.com/v1/logs. Backend создаёт reporter,
   но capture='on-demand': выгрузку разрешают только canonical feedback events/feedback committed
   (`packages/session/session-telemetry-otel/src/index.ts:192–242`). В девяти ACP recordings таких
   feedback events нет: при default env payload для экспорта не возникает. Это вывод из исходников,
   не сетевой эксперимент; наследуемые overrides сохранены, дополнительного opt-out runner не добавляет.

## Не перенесено

Каждый недостающий check закрывает путь к pass. После более раннего фактического расхождения результат
содержит его фазу; если общие сравнения совпали — `check <name> not implemented`.

- `strictV4Catalog`: в workspace есть общий каталог и V0→V1; полная строгая released V4
  schema/relationships ещё не портирована. Сравнительная проекция не выдаётся за такую проверку.
  Все model-сценарии блокируются этим check, если не остановились раньше на другом check/расхождении.
- `workspace.setup.git-worktree`: создание Git-репозитория/commit/filter не выполняется из-за запрета
  state-changing git в brief. Проверки самого worktree также перечислены ниже.
- Headless checks из тела controller:
  `verifyProviderCwdResume`, `verifyWorktree`, `verifyWorkingDirectory`, `verifySessionQuerySpill`,
  `sessionReferenceSpillExpected`, `verifyTeamTreeMessage`, `verifyTeamTaskInbox`, `verifyAdvancedToolchain`,
  `verifyMcpResources`, `verifyMcpTemplateError`, `verifyNoMcpServers`,
  `verifyBackgroundConfinementFailure`, `pluginManagerMcp`, `verifyToolResultWriterParity`,
  `packedWriterParity`, `toolcallUnusedArgs`, `emptyResponseRetryCurrent`, `officeProgressive`,
  `subagentDepthTwoRejection`. pluginManagerMcp/verifyMcpTemplateError включают отсутствующий
  startHttpMcpFixture и его URL/calls/auth inspection; provider-cwd требует ещё два запуска.
- SDK controller assertions учтены отдельной именованной группой для **каждого** сценария;
  matching golden не заменяет этих проверок. Имена checks:
  `sdk.account-provider-signout`, `sdk.approval`, `sdk.background-wait-steer`, `sdk.bash-tool`,
  `sdk.command-attachments`, `sdk.command-identity`, `sdk.cua-run-code`, `sdk.desktop-context`,
  `sdk.desktop-ptc-context`, `sdk.dynamic-tool-updates`, `sdk.goal-rounds`, `sdk.inline-image-prompt`,
  `sdk.manual-compact`, `sdk.max-tokens-continue`, `sdk.minimal-image-compaction`, `sdk.multi-turn`,
  `sdk.permission-switch`, `sdk.persistent-tools`, `sdk.plan-review`, `sdk.ptc-cancel`, `sdk.ptc-turn`,
  `sdk.serial-created`, `sdk.session-resume`, `sdk.session-title-after-turn`, `sdk.session-title-stability`,
  `sdk.subagent-activation-limit`, `sdk.subagent-continuable`, `sdk.subagent-continuable-inheritance`,
  `sdk.subagent-dsh-sdk-diagnostic`, `sdk.subagent-dsh-sdk-dynamic-route`, `sdk.subagent-fork-in-process`,
  `sdk.subagent-list-agents`, `sdk.subagent-mixed`, `sdk.subagent-send-message`, `sdk.subagent-spawn-in-process`,
  `sdk.system-prompt-in-history`, `sdk.team-default-enabled`, `sdk.team-session-mode`, `sdk.text-turn`,
  `sdk.tool-error-details`, `sdk.tool-scheduler-recovery`.
  Они включают точные model request capture assertions, minimal prompt/bash description/tool requirements,
  ordering/working-directory/title assertions и дополнительные процедуры session-resume/continuable.
  Общие SDK JSON-RPC ввод и goldens уже сравниваются; успешность этих групп не предполагается.
- ACP operations вне выбранного корпуса: `acp.input.promptAndWaitForAgentMessage`,
  `acp.input.promptExpectError`, `acp.input.waitForTurnStart`, `acp.input.waitForSubagentTurnEnd`,
  `acp.input.waitForGoalPhase`, `acp.input.waitForInboxMessage`, `acp.input.waitForTitleAfterTurnEnd`,
  `acp.input.waitForEventAfterTurnEnd`, `acp.input.cancel`. Все семь используемых корпусом операций есть.
- Полная цепочка после V1, модельный replay plugin и local stand-ins — другие задачи согласно brief.
  Никакие recorded tool results не подставляются настоящему процессу.
- Vitest record/refresh, проверки JS export shapes, исходные TypeScript-only corpus guards и web-сценарии
  не входят в CLI replay assignment. Своего LICENSE у session-snapshot нет; package.json объявляет MIT,
  заимствований с отдельным LICENSE в перенесённых файлах не найдено.

## Не проверено

- Настоящего построенного hoh пока нет; positive end-to-end model/SDK/ACP execution не заявляется.
  Проверены реальный fake-процесс и реальный /bin/false. Протоколы сохранены по исходникам,
  а не заменены результатами fake в настоящем runner.
- Native Windows/macOS filesystem/process поведение не запускалось. POSIX node_path helper используется
  для clone-relative plugin names; Windows junction/process-group и skill-loop требуют native lane.
  `workspace.setup.skill-root-isolation.windows` явно возвращает not implemented.
- Проверка JavaScript numeric/Unicode edge cases общего YAML вне corpus не заявляется. JSON reader/writer
  берутся из hoh-util-values с его задокументированными ограничениями Value/одиночных суррогатов.
- Identity проходит все 242 сценария, включая parent/child logs, спутники и доступные wire goldens.
  Зафиксированных отличий данных корпуса не найдено. Исправлены ошибки runner: SDK pin lookup должен
  видеть session corpus; ACP pinning headers исключают series. Они не объявлены отличиями recordings.

## Вопросы ведущему

Новых вопросов нет. Отсутствующие форматы/плагины и перечисленные fail-closed checks явно учтены;
никаких решений о смягчении сравнения не принято.

## Исправления fix2

1. Log mismatch сохраняет полный diff и отдельно называет row count, event type/order либо JSON pointer
   с expected/actual. Executable sensitivity требует для missing точный -1 count; для reorder — типы
   первой/последней body row; для tool — изменённый путь и marker; для usage — числовой путь и delta +1.
   Синтетические fake/tool и fake/usage имеют разные точные reasons.
2. ACP без model log больше не повреждает agentInfo.name. Десять неприменимых пар перечислены выше;
   они остаются identity pass, а tree mutation должна давать собственный tree mismatch.
3. Executable matrix отдельно считает реальные существующие tool text (>100) и usage numeric (>200),
   не засчитывая синтетические fallback и миграционно заблокированные сравнения.
4. Reorder выполняется только при last > 1; binary unit test проверяет два/три rows, matrix считает
   применимые сценарии (>200). На текущих fixtures первая/последняя body row имеют разные типы.
5. Executable regression пишет верные headless logs и завершает bash-tool-turn с кодом 1:
   требуется phase exit, expected 0, persisted sessions 1, до любых сравнений.
6. Migration gate восстановлен после launch/exit, перед comparisons. Historical text-turn с tool mutation
   должен назвать migration v1->v2 not ported; checks trace содержит только отказ migration.
   Identity не заявляет недостижимые SDK result comparisons, matrix отдельно считает blocked inputs.
7. Требование DSH_MCP_DEMO_URL и причина отсутствия реальной HTTP fixture разобраны выше по исходнику dsh.

## Проверка fix2

Среда: CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-cf1. Регулятор вручную не обходился.
`cargo check -p hoh-llm-replay -p hoh-conformance --tests` — exit 0 (19m02s).
`cargo test -p hoh-llm-replay -p hoh-conformance -- --test-threads=2 --nocapture` — exit 0.
Conformance: **28 passed, 0 failed, 0 ignored**: binary 1; corpus 3; fix_round 3;
normalization 20; runner_process 1. Matrix завершилась за 354.82s.
Identity: 242 достигнутых gates, 14 SDK results сравнены. Для tool/missing/reorder/usage/prompt:
234 detected + 6 migration-blocked + 2 not sensitive; tree: 236 + 6 + 0.
Реальные поля: **tool=197, usage=218, reorder=234**. Exit/migration и три generation negatives прошли.
`cargo fmt -p hoh-llm-replay -p hoh-conformance -- --check` — exit 0.
Логи: target-cf1/cf1-{check,tests,clippy}.log.
`cargo clippy -p hoh-llm-replay -p hoh-conformance --all-targets -- -D warnings` — exit 0,
`Finished dev profile [unoptimized + debuginfo] target(s) in 3.64s`.
Единственное замечание fix2 clippy — items_after_test_module: неизменённый unit-test блок
перенесён в конец peer.rs; это последняя нефункциональная правка после зелёных тестов.
Итоговый лог: target-cf1/cf1-clippy-final.log. Повторный fmt-check обоих crates — exit 0.
Контрольный binary запуск: `target-cf1/debug/hoh-conformance --hoh /bin/false`
с --refs закреплённого клона, --jobs 2, --runs/--results в
target-cf1/runs/bin-false-fix2-20261119. Exit 0:
`scenarios: 0 of 242 pass, 242 fail, 0 runner errors`.
TSV прочитан отдельно: 242 строки по 5 полей, все fail; 192 exit, 50 launch;
RESULTS.runner-errors.tsv пуст. Вывод сохранён в target-cf1/cf1-false.log.

## Проверка предыдущего fix round (до fix2)

Все Cargo-команды: `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-cf1`.

- `cargo test -p hoh-conformance` — **27 passed, 0 failed, 0 ignored** (exit 0):
  `tests/corpus.rs`: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`;
  `tests/fix_round.rs`: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`;
  `tests/normalization.rs`: `test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`;
  `tests/runner_process.rs`: `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
  Последний executable matrix завершился за 220.52 s. Unit/doc targets содержат 0 тестов.
  Identity и каждая из шести sensitivity-мутаций запускают все 242 сценария, без исключений;
  три дополнительных executable negatives проверяют headless/SDK writer V3 и смешанные SDK поколения.
  Identity исполнила все 14 SDK result goldens и дошла до ожидаемых gates: 233 scenario-check,
  6 missing migration, 1 git-worktree setup; два ACP случая без модели действительно pass.
  Это успешность проверки runner/double, а не заявление о прохождении этих 240 сценариев настоящим hoh.
- `cargo clippy -p hoh-conformance --all-targets -- -D warnings` — exit 0,
  `Finished dev profile [unoptimized + debuginfo]`.
- `cargo fmt -p hoh-conformance -- --check` — exit 0.
- Построенный `/mnt/toshiba/alexandrie/hoh/target-cf1/debug/hoh-conformance` запущен с
  `--hoh /bin/false --refs /mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`
  `--runs /mnt/toshiba/alexandrie/hoh/target-cf1/runs/bin-false-round-final`
  `--suite session --suite sdk --suite acp --jobs 4`
  `--results /mnt/toshiba/alexandrie/hoh/target-cf1/runs/bin-false-round-final/RESULTS.tsv`.
  Exit 0; **`scenarios: 0 of 242 pass, 242 fail, 0 runner errors`**.
  Отдельно прочитан TSV: 242 строки, 192 session + 41 sdk + 9 acp, все status=fail:
  192 phase=exit (headless), 50 phase=launch (SDK/ACP без initialize).
  Все строки имеют пять полей; RESULTS.runner-errors.tsv пуст.

## Папка прогона через симлинк (sg2-conformance, 19.11)

Симптом полного `cargo test --workspace` чистой выгрузки HEAD (7403/2): `corpus` —
`session compaction-recovery fail setup fixed cwd length mismatch`, `runner_process` —
`fake: spill root is not isolated`. Причина не в выгрузке и не в длине её пути: тесты запускали
прогонщик с `--runs /mnt/toshiba/alexandrie/hoh/target-cf1/runs`, а `target-cf1` с 16:29 —
симлинк на `/dev/shm/hoh/target-cf1`. Прогонщик строил из этого пути (1) префикс `cwd-----…` для
сценариев с cwd фиксированной длины 192 и (2) каталог `spill-*`, а cwd потом канонизировал
(`fs::canonicalize`). С симлинком канонический cwd (`/dev/shm/hoh/…`) отличался от
построенного по неканоническому пути: длина cwd ≠ 192 (проверка в `runner.rs`), родитель
spill ≠ родитель cwd (проверка двойника в `bin/fake/contract.rs`). Пока `target-cf1` был обычной
папкой, обе проверки проходили, поэтому в приёмке тесты были зелёными.

Исправление (`runner.rs`, `run`): `dir` прогона канонизируется один раз, до создания `run-*`,
cwd и spill; всё остальное строится от него. Оракул сравнивает пути, которые видит дочерний
процесс, с путями, построенными прогонщиком, и написание `--runs` (симлинк, `..`) на них
влиять не должно.
Тесты: `corpus.rs` и `runner_process.rs` больше не берут жёсткий путь `target-cf1/runs`, а
создают папку под `CARGO_TARGET_TMPDIR` и проходят к ней ЧЕРЕЗ симлинк `l → real` (имена разной
длины, как macOS `/var` → `/private/var`); сами тесты проверяют, что длина `canonicalize(root)`
отличается от длины `root`. Поэтому случай воспроизводится при любом `CARGO_TARGET_DIR`, в том
числе без симлинков. Без канонизации в `runner.rs` (проверено временным удалением строки при
канонической `CARGO_TARGET_DIR`): `corpus.rs` падает с `session compaction-recovery fail setup
fixed cwd length mismatch`, `runner_process.rs` — со «fake: spill root is not isolated» на
сценариях с `spill` (до identity-проверок). Оба теста с симлинком только под `cfg(unix)`
(`runner_process.rs` целиком, `corpus.rs` — один тест); Windows не проверялся.
Остаётся фиксированным только расположение `refs` (`/mnt/toshiba/alexandrie/hoh/refs/…`) — это
данные образца, а не выгрузка. Ограничение прежнее: если `CARGO_TARGET_TMPDIR` внутри `/tmp`,
сценарии `outside-temp` честно отвечают «falls within temporary write grant».

## Подключение к бинарнику hoh и прогон списка V1 (s2, 20.11)

Запуск: `scripts/conformance-v1.sh [in|extra|all] [флаги прогонщика]` собирает `hoh` с функцией
`conformance-fixtures` и `hoh-conformance`, затем вызывает `hoh-conformance --v1 conformance/V1-SCENARIOS.tsv
--group … --out /mnt/toshiba/alexandrie/hoh/conformance-runs/<время>`. Результат — `summary.tsv`
(`suite scenario group status phase body_check detail`), `diffs/<suite>/<сценарий>.diff|.stderr`, `runs/`.
HOME изолирован (`/dev/shm/hoh/conformance-home-<время>`), сеть не нужна: модель — `llm-replay`.

Статус сценария (`src/v1.rs`, `classify`):
- **не запустилось** — ошибка прогонщика, фазы `input`/`setup`/`launch`, либо в stderr `startup failed`
  (профиль не загрузился);
- **расхождение** — процесс стартовал, но один из сравниваемых артефактов отличается (`exit`, `stdout`,
  `frames`, `log`, `headers`);
- **прошло** — все сравнения совпали; завершающий барьер `scenario-check` (проверки тела TS-сценария,
  которые не перенесены, см. «Не перенесено») не считается расхождением, его имя пишется в столбец
  `body_check`. В первом прогоне до барьера не дошёл ни один сценарий, поэтому «прошло» = 0.
  Это число нельзя читать как «всё совпало»: пока строгий V4-каталог и проверки тела не перенесены,
  «прошло» означает только совпадение сравнений прогонщика.

Что понадобилось в `apps/hoh` (бинарнику, не крейту прогонщика):
- `src/v1_registry.rs` и 60 строк в `Cargo.toml` (зависимости и функция `conformance-fixtures`): плагины v1, уже лежащие в дереве, но не заведённые в
  таблицу (`headless`, `headless/startup`, `llm-replay`, `agent-*`, `subagent*`, `goal*`, `spill-*`,
  `session-*`, `settings`, `workspace`, `schedule`, `office-*` и др., всего 56 пакетов). `hoh-jobs-local`
  в таблицу не включён: на момент работы крейт не собирался (`Config: Serialize`), его писатель — x9.
- `profile_boot.rs`: (1) профиль не должен подменять `cordis.yml` на `cordis.snapshot.yml` — подмена
  `resolveConfigPath` есть только у явного пути конфига; из-за неё `headless` не стартовал
  (`config file not found …/cordis.snapshot.yml`); (2) профили `sdk`/`sdk-minimal` берут тот же
  stdio-клей, что окно (единственный читатель stdin, выход после `shutdown`).
- `v1_registry` при `--features conformance-fixtures` регистрирует Rust-аналоги `local:`-фикстур из
  `hoh_conformance::fixtures`.

### Нормализация первой версии

Файл `NORMALIZATION.md` — полный перечень (что, где, почему, тест); тест
`every_rule_has_a_section_in_normalization_md` не даёт добавить правило без раздела. Правила
нормализуют **только записанную сторону** и только то, что вносит плагин вне 118 строк
(`research/v1-engine-scope.codex.md`):

| правило | что убирается | почему | тест |
|---|---|---|---|
| `N-BOOT-FOREIGN-PLUGINS` | последний `--patch` отключает 21 строку профиля (`connection`, `file-upload`, `deepseek-account`, `llm-pi-ai`, `hooks-*`, `plugin-manager`, `ptc-runtime`, `workflow-ptc`, `tool-workflow`, `tool-fs-search`, `repeat-tool-reminder`, `plan-mode`, `typert*`, `remote-access`, `timer`, …) | пакеты вне 118 без Rust-реализации; без этого профиль sdk не стартует (обязательная `connection`); `tool-jobs`/`jobs-local` (внутри 118) не отключаются | `exclusion_patch_disables_every_foreign_row_and_nothing_else`, `no_foreign_plugin_is_inside_the_118`, контракт fake (`contract.rs`) |
| `N-PROMPT-FOREIGN` | три абзаца системного промпта: `glob`, `grep` (tool-fs-search), `workflow` (tool-workflow) | пишут плагины вне 118; абзац `tool-jobs` не трогается | `foreign_prompt_paragraphs_are_dropped_and_others_kept`, `a_dropped_last_paragraph_keeps_the_closing_newline`, `a_similar_but_different_paragraph_is_not_dropped` |
| `N-TOOLS-FOREIGN` | инструменты `glob`, `grep`, `workflow` в наборах схем и в `tools` строк `request/header` (имена или определения) | регистрируют плагины вне 118 | `foreign_tools_are_dropped_from_a_schema_set_and_the_rest_is_untouched`, `foreign_tools_are_dropped_from_recorded_header_rows_in_both_forms` |

Нормализация провайдера (условие CEO): **правил нет**. Записи сделаны `llm-deepseek-api-key`, но патч
сценария заменяет его `llm-replay` под тем же идентификатором провайдера `deepseek-official`, поэтому
заголовок запроса, поля модели и `thinking`/`reasoningEffort` совпадают без правок. Первый прогон не
показал расхождений, вызванных заменой провайдера. Если они появятся — правило и строка в
`NORMALIZATION.md` вместе (`provider_replacement_adds_no_normalization_rule` закрепляет текущее состояние).
Диагностический флаг `--disable-plugin ID` не является нормализацией: прогоны с ним помечаются как
диагностические (без него полный прогон даёт то, что в summary).

### Фикстуры `local:`

Перенесено 8 из 22 TS-фикстур отобранных сценариев (`src/fixtures.rs`, регистрация под `local:`-именем
файла; поведение дословное): `compaction-working-directory`, `parent-agent-preset`,
`parent-sandbox-override`, `in-history-prompt-update`, `session-write-grant`, `child-question-tripwire`,
`subagent-activation-limit`, `subagent-settlement-barrier` (`src/fixtures/agents.rs`). Тесты: `tests/fixtures.rs` (имя и существование
исходного файла; непортированная фикстура не разрешается). Эффект проверен прогоном: `working-directory/change`
больше не пропадает в сценариях `compaction-*`.

Не перенесено (остаток, сценарии затронуты): `workspace-context-compaction`, `workspace-context-global-failure`,
`working-directory-admission`, `partial-landlock-sandbox`, `in-history-series-start`,
`dynamic-tool-updates`, `dynamic-tool-prompt-updates`, `subagent-structured-parent`,
`subagent-send-message-fence`, `subagent-durability-failure`,
`provider-cwd`, `provider-unload`, `pending-confinement`,
`async-confinement-failure` (packages/shell/bash-sandbox), и `.mjs`-фикстуры SDK (`capture-requests`, `fixture`,
`driver`, `settlement-order`, `runtime`, `scheduler-failure`, `model-switch-driver`, `execution-order`,
`web-fetch-*`, `deepseek-fetch-fixture`). Каждая использует внутренние события крейтов-владельцев
(`tools/post-execute`, `agent/pre-step`, `llm/stream`, `loader.entries`, subagent-сервис); перенос
требует чтения API по каждой и не уместился в этот ход.

### Первый прогон (`conformance-runs/20261120-182636/summary.tsv`)

- **110 in: прошло 0, расхождение 108, не запустилось 2**; **9 extra: прошло 0, расхождение 9, не запустилось 0**.
- До нормализации и подключения плагинов (первый заход, 16:42): 0/91/28 — 27 sdk не загружались
  (`connection` обязательна).
- Фазы: log 79 (71 in + 8 extra), headers 26, exit 10, frames 2, input 2.
- Не запустилось: `session/error-finish` (`migration v1->v2 not ported`), `sdk/multi-turn`
  (`migration v2->v3 not ported`) — исторические поколения журнала, их декодеры в дереве нет.
- Чаще всего в записанном журнале есть, а у нас нет: `job/registered`+`job/settled` (21 сценарий,
  `tool-jobs`/`jobs-local`, строки внутри 118, ещё пишутся), `session/title` (17), `user/message`
  (18), `agent/inbox/spliced` (10), `request/header`, `compaction/*` (4), `model/selection` (4).
  В заголовках: 20+ сценариев расходятся только из-за абзаца `Track every background job id…`
  (tool-jobs; не нормализуется намеренно).
- Расхождения прогонщик не чинит, разбор — у ведущего.

### Наблюдения для ведущего

- `hoh-settings`: задача `settings legacy import` ждёт `loader.settled()` без отмены. Если профиль не
  оседает (в голом запуске без overlay из N-BOOT-FOREIGN-PLUGINS, при нерезолвящихся строках), выход после
  `shutdown` подвисает; с overlay в полном прогоне не воспроизводится (2 `process timeout` в `session`:
  `read-image-attachment-path`, `subagent-parallel`).
- `cargo test -p hoh-conformance` при `TMPDIR`, лежащем над симлинком `target-*`, роняет четыре
  `outside-temp` сценария (`cwd is temporary`) — свойство среды, не кода; зелёный прогон:
  `TMPDIR=/mnt/toshiba/alexandrie/hoh/scratch-s2 cargo test --offline -p hoh-conformance`.

## strictV4Catalog и миграции (s2, 20.11 вечер)

Заглушка «check strictV4Catalog not implemented» удалена. Настоящая проверка — `compare::strict_catalog`
(фаза `input`, имя в `checks.jsonl` — `strict-catalog`, раньше `migration`); выполняется для каждой
воспроизводимой фикстуры (все роли) в `runner::execute` на прежнем месте. TS-образец — разбор фикстуры в
`llm-replay`, т. е. то, что делает каждый сценарий при загрузке записи:
- `packages/test-support/llm-replay/src/index.ts:218-288` `parseSessionFixture`: первая строка — заголовок,
  `createSessionFormatCatalogWithChildren([]).createRestore(header, {recovery:'strict', validation:'current'})`
  (`:245-248`), каждая следующая строка — `restore.decodeRow` (`:279`), в конце `restore.finish()` (`:288`);
- `:389-398` `normalizeProjectedHeader` (`delegationDepth: 0` у v0, `{{cwd}}` → `/dsh-snapshot-cwd`);
  `:367-387` `normalizeProjectedRow` (токен `{{tools}}` убирается, список имён → определения);
  `:260-277`, `:401-407` плотные `seq` и нулевой `time` у строк без конверта, кардинальность packed-строк;
- `:410-431` `fixtureFormatError`: префикс `session snapshot line N:`; строка для ошибки `finish` берётся по
  `Session event N` / ` at seq N` / `inherited Session cut N` / `seed event at index N`.
Реализация использует `hoh_session_format_catalog::create_session_format_catalog_with_children` и
`SessionFormatCatalog::create_restore` (Strict + Current); та же цепочка выполняет миграции v0→v1→v2→v3→v4,
поэтому прежний `migration_gate` (останавливавшийся на v1→v2 и v2→v3) удалён: `session/error-finish` и
`sdk/multi-turn` доходят до запуска. Отдельная проверка v0 на `restore_v0_to_v1` больше не нужна.
Остальные барьеры (`sdk.*`, `verify*`, git-worktree и др.) не тронуты, остаются fail-closed.
Барьер ACP «strictV4Catalog» тоже снят: у ACP-сценариев нет других именованных проверок тела.

Тесты (`tests/corpus.rs`): `every_historical_and_current_fixture_reads_strictly_under_the_catalog` (все роли
всех 242 сценариев читаются), модуль `strict_catalog` — запись v4 проходит, историческая запись мигрируется и
проходит; испорченные: неизвестный тип события (`format v4 contains unknown event type "no/such-event" at seq 2`,
строка 4), нарушенная связь (`tool/result … has no advertised tool lifecycle`), версия 9 (`stored Session uses
newer format v9; this build writes v4`), смесь projected/complete строк, битый JSON, пустой файл.
`runner_process.rs`: ожидаемый барьер считается по настоящему `strict_catalog`, v1-сценарий `text-turn`
проходит миграцию и сравнение.

Нормализация: новых правил нет.

## Тела сценариев: `src/body/` (f1, 20.11)

Проверки после запуска, которых нет в общих сравнениях runner. Вход — `body::body_check(suite, name, &BodyContext)`;
`None` — у сценария нет тела в этом модуле. Тела `sdk` — только для 26 сценариев группы `in`
(`body::sdk::SCENARIOS_IN`), остальным возвращается `None`.

### headless (`body/session.rs`, 10 из 10), `headless.snapshot.ts`

| сценарий | образец | примечание |
|---|---|---|
| empty-response-retry-current | :1447–1453 | порядок четырёх типов событий |
| tool-call-turn | :171–183 (частично) | `tool/result` без `id` из мигрированного `session.v3.jsonl` (`compare::migrate_fixture`) равны выводу writer. НЕ перенесён второй полный прогон сценария на V3-входе (:1349–1350, :1476–1478: exit, stdout, журнал, заголовки) — сценарий идёт как «прошло, тело не проверено» (`body::unchecked`) |
| toolcall-unused-args | :1485–1503 | |
| office-progressive-native / -ptc | :1504–1519 | у `-ptc` нет ветки с отказом render |
| subagent-depth-two-rejection | :1523–1531 | |
| provider-cwd | :944–1008 | запускает `hoh` ещё два раза (`BodyContext::launch`: бинарь, профиль, патчи, env, задача) из временного каталога; `--session-id` и `--json` как в образце. Env — окружение запуска runner с подменённым `DSH_SNAPSHOT_PROVIDER_CWD`; образцовый набор `REPLAY_CREDENTIAL_ENV`/`DSH_HOME` не воспроизведён |
| background-confinement-failure | :862–879 | читает `confinement-audit.json` из `cwd` |
| working-directory | :881–904 | |
| worktree | :906–942 | git с детерминированными именем и датой, без пользовательской конфигурации |
| packed-chunks | :1122–1150 | три упакованных типа в фикстуре; упакованный журнал и распакованный (построчный декодер V0 `RELEASED_V0_SESSION_FORMAT_CODEC`, `run.expand()`) мигрируются каталогом и сравниваются по событиям без `seq`/`time` |

### sdk (`body/sdk.rs`, 26 из 26 перечислены; свои тела у 10), `sdk.snapshot.ts`

Тела: command-attachments :1253–1297, command-identity :1348–1365, permission-switch :1366–1409,
manual-compact :1410–1465, dynamic-tool-updates :1535–1568, tool-scheduler-recovery :1569–1587,
subagent-activation-limit :1588–1595, system-prompt-in-history :1707–1726, desktop-context (`runtimeContext`
в SDK_ASSERTIONS, :1738–1753), serial-created :1766–1772. Остальные 16 сценариев группы — только общий
хвост :1773–1778 (при нескольких Session: `subagent.started`/`finished` и `parentSessionId` ребёнка).

Не перенесено (в runner нет данных):
- утверждения над `modelRequests` (файлы `*-requests.jsonl` профиля): command-attachments :1286–1291 (`modelRequests`
  длины 1, сообщение с картинкой, «You are in plan mode.»), command-identity :1360–1364,
  permission-switch :1380–1408, manual-compact :1448–1464;
- `results[].events` (результаты вызовов SDK): dynamic-tool-updates и tool-scheduler-recovery берут события из
  журнала на диске (`finalResponse` второго вызова — текст последнего `assistant/message`);
- `expectedFinalResponse` — в образце проверяется всегда (:1617), не только при записи. Не перенесено для desktop-context, manual-compact, permission-switch, command-identity, dynamic-tool-updates; эти сценарии идут в отчёте как «прошло, тело не проверено» (`body::unchecked`, `body::gate`);
- внутри прогона (управляющий код, `protocol.rs`): порядок завершений subagent-continuable :1000–1015 и
  feedback-события :1018–1023 — вне этого модуля.

Подключение (делает s2): в `runner.rs::scenario_checks` до заглушек строится `BodyContext` и вызывается
`body::body_check(&s.suite, &s.name, &ctx)`; `Some(check)` возвращается как результат, `None` — прежняя логика.
Поля: `dir = s.dir`, `manifest = s.manifest`, `actual`, `expected = s.expected_logs()?`, `notifications` (для SDK —
накопленные уведомления, для headless `&[]`), `cwd`, `harvest = || harvest(&sessions, &s.suite)`,
`launch = Launch { hoh: &options.hoh, profile: s.profile(), patches: &patches, env: &env, task }`.
Тесты: `tests/body.rs` — проход на записанных журналах корпуса и отказ на испорченной копии.

### Прогон после снятия барьера (`conformance-runs/20261120-193810`, `--timeout-ms 180000`)

in 110: прошло 0 (тело не проверено 0), расхождение 110, не запустилось 0; extra 9: 0 / 9 / 0.
По фазам: log 69+8, headers 32, exit 7+1, frames 2. Против `20261120-182636` (0/108/2; log 71+8, headers 26,
exit 9+1, frames 2, input 2): `error-finish` и `multi-turn` теперь доходят до сравнения; 74 сценария,
остановленных барьером, перестали быть «барьерными» — все они оказались настоящими расхождениями
(в основном отсутствуют `job/*` из tool-jobs и абзац его промпта, `session/title`, `agent/inbox/spliced`).
При нагрузке системы (load ≈ 20) лимит процесса 30 с даёт ложные `process timeout` (17 из 110): итоговый
прогон сделан с `--timeout-ms 180000`, таймаутов нет.

## Круг исправлений 1 по ревью прогонщика (s2, 20.11)

- №2 `src/manifest.rs`: полный порт `parseSnapshotManifest` (manifest.ts:201-432), сообщения как в образце; профиль `web`
  принят, `scenario` необязателен; `corpus::discover` вызывает `manifest::parse`. Тесты `tests/manifest.rs`.
- №3 `compare::restore_pinned_tool_schemas` (suite.ts:532-548) в `compare::headers`: `tools` пина — `{{tools}}`
  или упорядоченные имена sidecar, иначе ошибка; пин без `tools` при непустом sidecar — ошибка. Имена пина проходят
  `N-TOOLS-FOREIGN` (`normalize::v1::header_tools`) так же, как sidecar. Тесты `tests/round1.rs`.
- №6 таймаут SDK — на каждый запрос (`Rpc::finish`, `Rpc::until`), а не на весь контроллер; ACP сохраняет общий лимит.
- №7 `normalize::clocks` не добавляет `data: null` строкам без `data`.
- №1 `tests/workspace.rs` (4 из 4), `tests/manifest.rs` (7 `it` из `manifest.spec.ts` + `writesCurrentSessionFixtures` (`manifest.rs: writes_current_session_fixtures`) + таблица недопустимых метаданных + тест `js_trim`); остальные спеки — список ниже.
- Тела сценариев f1 (`body::body_check`) подключены в `runner::scenario_checks`: `Some` решает, `None` — прежняя логика.
  `tests/runner_process.rs`: сценарий с телом должен дойти до `scenario-check`; исход тела на fake не проверяется.
- №4 блокирующий ввод-вывод: `wait_boundary` (опрос журналов каждые 10 мс) вынесен в `spawn_blocking`. Не вынесены:
  jsdiff в `compare::equal`, синхронные `fs` в `runner`, `workspace::capture` — они идут один раз на сценарий; детерминированного
  теста нет (поведение не наблюдаемо без измерения занятости воркеров), поэтому теста нет.
- №5 `promptAndCancel`: пока ждём границу хода/файл, stdout дочернего процесса не читается. Скрытый риск: если до
  `session/cancel` процесс выведет в stdout больше буфера канала (≈64 КиБ), он заблокируется на записи и ожидание
  закончится таймаутом. На корпусе не срабатывает (кадры до отмены короче); исправление — читать stdout параллельно с ожиданием.
- №8 отложено: заметок нет.

### Список случаев TS-спеков: перенесён / отложен, причина

`manifest.spec.ts` (все 7 `it` и `it.each`, включая случай «keeps explicitly retained generations read-only…» через
`writes_current_session_fixtures`) и `workspace.spec.ts` перенесены целиком (`tests/manifest.rs`, `tests/workspace.rs`).

Источник: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/test-support/session-snapshot/tests/`. Rust: `/mnt/toshiba/alexandrie/hoh/wt/s2/crates/hoh-conformance` (tests/*.rs и `#[cfg(test)]` в src). manifest.spec.ts и workspace.spec.ts не включены (уже перенесены). Тесты не запускались, статус выставлен чтением тел тестов.

Критерий: «перенесён» — Rust-тест реально проверяет то же поведение (центральное утверждение случая). Если покрыта лишь часть или функция есть без теста — «отложен» с пометкой «частично»/причиной. Строки `it.each` считаются блоком; в итогах учтено число строк таблицы (N).

#### Итоги по спекам

| спек | перенесён | отложен | всего |
|---|---|---|---|
| normalize.spec.ts | 88 | 0 | 88 |
| identity.spec.ts | 4 | 0 | 4 |
| storage-policy.spec.ts | 8 | 0 | 8 |
| harness.spec.ts | 70 | 2 | 72 |
| suite.spec.ts | 113 | 4 | 117 |

Примечание: `describe` без `it` в suite.spec.ts (строки 173, 179, 183) лишь регистрируют defineAcpSnapshotSuite в режимах replay/record/refresh; их динамические случаи не пронумерованы и не учтены (mode record/refresh не портирован, replay-аналог — прогон корпуса `hoh-conformance`).

#### normalize.spec.ts

| строка | случай | статус | причина/тест |
|---|---|---|---|
| 30 | rewrites JSON-RPC ids to a stable first-seen sequence | перенесён | tests/normalization.rs::rpc_ids_share_one_namespace_preserve_string_vs_number |
| 43 | scrubs the cwd and session id anywhere they appear | перенесён | tests/normalization.rs::cwd_aliases_mac_alias_and_path_boundaries (cwd) + acp_usage_tokens_and_legacy_message_ids (sessionId) |
| 55 | keeps standard message identity distinct from session identity | перенесён | tests/normalization.rs::acp_usage_tokens_and_legacy_message_ids (messageId и sessionId раздельно) |
| 75 | stabilizes path-dependent context occupancy without hiding capacity | перенесён | tests/normalization.rs::acp_usage_tokens_and_legacy_message_ids (used -> {{usedTokens}}, емкость сохранена; ключ там total, а не size) |
| 96 | scrubs cwd at file URI and chained-punctuation boundaries | отложен | частично: cwd_aliases_mac_alias_and_path_boundaries проверяет file:// и границы -other/.txt; цепочка пунктуации `cwd.,` -> `{{cwd}}.,` не проверена |
| 118 | scrubs every filesystem spelling of the cwd longest-first | отложен | частично: alias проверен, но longest-first и Windows-написания (RUNNER~1) не проверены |
| 136 | canonicalizes only cwd-rooted path separators | перенесён | tests/normalization.rs::canonical_paths_do_not_rewrite_regex_or_commands (разделители только у cwd-rooted, regex/printf не тронуты) |
| 160 | canonicalizes generated relative path fields and text markers without rewriting other text | отложен | частично: canonical_paths_... проверяет `path`-ключ и `<path>`/`Additional instructions from:` для абсолютных путей; относительные `nested\AGENTS.md` и `.\nested\task.txt` не проверены |
| 179 | can preserve native cwd-rooted separators for a platform golden | отложен | режим cwdPathMode:"native" не портирован (grep native в src/normalize.rs пуст); Windows-golden не используется |
| 186 | scrubs a stray UUID not in the known list | отложен | нет теста на «чужой» UUID -> {{sessionId}} в stdout (normalizer.string его не проверяет) |
| 191 | leaves notification frames without an id untouched in id-space | отложен | нет теста: фрейм-уведомление без id не проверяется (rpc_ids_... идёт только по фреймам с id) |
| 197 | stabilizes only the top-level event timestamp and spill byte count in event-read text | отложен | частично: event_read_time_and_omitted_bytes_only_in_target_envelope проверяет верхний time и Omitted, но не что вложенный `data.time` (31337) и текст после `After:` остаются |
| 223 | preserves event-like timestamps in unrelated output text | перенесён | tests/normalization.rs::event_read_time_and_omitted_bytes_only_in_target_envelope (последний assert: без Target event ничего не меняется) |
| 247 | throws on a non-JSON stdout line (the purity check) | перенесён | tests/normalization.rs::rpc_ids_share_one_namespace_preserve_string_vs_number (frames("ordinary stdout") -> Err) |
| 252 | ignores blank lines | перенесён | tests/normalization.rs::review_220412::blank_lines_in_a_frame_stream_are_ignored |
| 259 | keeps $format distinct in single-log and single-snapshot paths (строк в таблице: 2) | отложен | частично: tests/fix_round.rs::legacy_job_uuid_in_header_and_prompt_skips_reserved_id_token (литерал {{id:1}} сохранён, один job -> {{id:2}}); два разных job-id ({{id:2}}/{{id:3}}), tool/call.arguments, короткие UUID, идемпотентность не проверены |
| 286 | normalizes only message-feedback item clocks | отложен | частично: feedback_clocks_and_version_identity зануляет createdAt у feedback/message-put, но не проверяет, что у tool/result с тем же item часы НЕ меняются |
| 297 | keeps unexpected request-header fields observable in comparisons | отложен | нет теста: неожиданные поля request/header остаются наблюдаемыми (в Rust заголовки сравниваются через compare::headers e2e) |
| 309 | zeroes the header createdAt | перенесён | tests/normalization.rs::session_event_and_embedded_stream_clocks (createdAt заголовка = 0) |
| 315 | preserves event sequence and zeroes event time | отложен | поведение другое: Rust-logs() удаляет seq/time (envelopes_are_removed_references_and_event_order_remain), seq не «сохраняется»; кейс TS-нормализатора session-log в Rust не воспроизводится |
| 322 | normalizes a projected event without adding a persistence envelope | отложен | нет теста: проецируемое событие без persistence envelope; Rust-аналог — только clocks_do_not_add_data_to_rows_without_it (round1.rs), другая проверка |
| 331 | scrubs cwd and session id deep inside event data | отложен | частично: cwd/sessionId проверены на уровне string()/value() (cwd_aliases_..., acp_usage_...), обход data событий в logs() отдельно не проверен |
| 341 | scrubs cwd at file URI and chained-punctuation boundaries in event data | отложен | частично: см. строку 96 (цепочка пунктуации в event data не проверена) |
| 357 | scrubs random local spill paths under the snapshot cwd | перенесён | tests/normalization.rs::local_and_snapshot_spill_locators_keep_only_filename (`{{cwd}}/.spill/session-…/…-result.txt` -> {{spillLocator:result.txt}}) |
| 373 | scrubs macOS /private aliases for local spill paths | отложен | частично: /private-алиас проверен для обычного пути (cwd_aliases_...), для spill-пути /private+cwd+/.spill — нет |
| 388 | scrubs macOS /private prefix on cwd-rooted fs tool result paths | перенесён | tests/normalization.rs::cwd_aliases_mac_alias_and_path_boundaries (/private/tmp/... -> {{cwd}}/x) |
| 403 | scrubs fixed snapshot spill paths | отложен | нет теста: фиксированный /tmp/dsh-acp-snapshot-spill (в Rust проверен только dsh-acp-snap-<id>) |
| 418 | scrubs scenario-owned snapshot spill paths | перенесён | tests/normalization.rs::local_and_snapshot_spill_locators_keep_only_filename (/tmp/dsh-acp-snap-012345678/…) |
| 433 | scrubs scenario-owned snapshot spill paths with Windows drive and separators | перенесён | tests/normalization.rs::local_and_snapshot_spill_locators_keep_only_filename (C:\t\dsh-acp-snap-… с обратными слэшами) |
| 448 | shares cwd-rooted path handling with stdout normalization | отложен | нет cwdPathMode/native в Rust-нормализаторе; проверка совместного canonical-поведения со stdout не воспроизведена |
| 460 | scrubs the session id in the header | отложен | нет прямого теста id заголовка session; sessionId-скраб проверен только для ACP-значений (acp_usage_tokens_...), в logs() идентичности — {{session:N}} (typed_identities_...) |
| 465 | zeroes a hook/result durationMs (run-to-run noise) but keeps its decision | перенесён | tests/normalization.rs::hook_job_goal_and_catalog_clocks_are_scoped (hook/result durationMs -> 0; соседние поля сохранены) |
| 476 | preserves a packed chunk row\'s sequence, zeroes time, and zeroes volatile dt gaps | перенесён | tests/normalization.rs::packed_row_clocks_preserve_lengths (time0 -> 0, dt -> нули, texts сохранены по длине) |
| 490 | normalizes timing inside an embedded Assistant stream and ignores opaque members | отложен | частично: session_event_and_embedded_stream_clocks проверяет text-chunks/chunk внутри stream, но не opaque-члены (null, строка) и usage с time0/dt |
| 527 | normalizes a headerless packed-like stream record without decoding it | отложен | нет теста: безголовая packed-подобная запись с нестроковым data |
| 534 | leaves a non-hook event durationMs untouched (only hook/result is scrubbed) | перенесён | tests/normalization.rs::review_220412::a_non_hook_duration_is_left_untouched |
| 540 | normalizes goal lifecycle clocks without scrubbing unrelated payload timestamps | отложен | частично: goal/change createdAt/updatedAt обнуляются (hook_job_goal_and_catalog_clocks_are_scoped), но что tool/result.createdAt и goal без часов не меняются — не проверено |
| 555 | normalizes job lifecycle clocks while preserving user timestamps and uncertainty | отложен | частично: round1.rs::clocks_do_not_add_data_to_rows_without_it обнуляет startedAt у job/settled; не проверены null/строковый data, пользовательские startedAt в tool/result и producerReported |
| 575 | normalizes subagent catalog child creation clocks | перенесён | tests/normalization.rs::hook_job_goal_and_catalog_clocks_are_scoped (subagent/catalog childCreatedAt -> 0) |
| 591 | handles complete envelopes when optional normalized fields are absent | перенесён | tests/round1.rs::clocks_do_not_add_data_to_rows_without_it |
| 609 | normalizes, scrubs, and projects each parsed body record | отложен | частично: куски раздельно (system_prompt_bulk_is_scrubbed_only_in_first_block, envelopes_are_removed_...); целиком проекция+cwd+{{tools}} в заголовке без developer-сообщения не проверена; {{tools}}-токен проверен только e2e |
| 635 | normalizes an already-projected packed row | перенесён | tests/normalization.rs::packed_row_clocks_preserve_lengths (data.dt обнуляется у packed-строки) |
| 646 | retains historical packed-row boundaries while normalizing their timing | отложен | нет теста границ packed-строк: миграция/ре-пакинг делает hoh_session_format_catalog (strict_catalog), нормализатор Rust их не меняет; проверяется только e2e прогоном корпуса |
| 672 | preserves adjacent catalog facts in parent event order | отложен | нет теста порядка соседних subagent/catalog в логе родителя |
| 691 | preserves malformed catalog payloads | отложен | нет теста: некорректные catalog-payload сохраняются как есть |
| 708 | preserves source references and catalog order: $sources (строк в таблице: 2) | отложен | частично: envelopes_are_removed_references_and_event_order_remain проверяет разворот sourceEventSeqs; порядок catalog, surfaceOp и идемпотентность не проверены |
| 730 | migrates and re-packs multi-session fixtures after relationship-preserving id redaction | отложен | миграция v0->v3 и ре-пакинг в Rust выполняет hoh_session_format_catalog (strict_catalog, tests/corpus.rs); нормализатор не мигрирует — аналог только e2e (every_historical_and_current_fixture_reads_strictly_under_the_catalog), id v2-to-v3-system-… не проверяется |
| 782 | normalizes an already-projected snapshot without a released-format field | отложен | нет теста: уже спроецированный снимок без поля версии |
| 794 | rejects an empty snapshot before classifying its released format | перенесён | tests/corpus.rs::strict_catalog::mixed_projected_and_complete_rows_and_bad_json_are_rejected (strict_catalog("") -> "session snapshot must start with a session header") |
| 799 | rejects a nonempty snapshot whose first record is not a session header | перенесён | tests/normalization.rs::review_220412::a_snapshot_must_start_with_a_session_header |
| 816 | compares each strictly restored input using its own native delivery generation | отложен | частично: native_delivery_generation_only_matches_source_generation проверяет токен для текущей v4; историческая v3 -> тот же токен и сравнение «historical === current», а также дефолтный (не native) режим не проверены |
| 826 | keeps source-versus-migrated-artifact delivery comparison generation-exact by default | отложен | нет теста: сравнение source vs migrated по умолчанию (prepareSessionSnapshotFixtureForComparison не портирована) |
| 834 | keeps a wrong native delivery generation distinguishable from historical writer output | перенесён | tests/normalization.rs::native_delivery_generation_only_matches_source_generation (delivery 3 в логе v4 остаётся числом 3, не токеном) |
| 842 | refuses source delivery claiming V4 before creating comparison tokens | отложен | нет теста: отказ при delivery, заявляющем V4 в источнике v3 (сообщение "format v3 delivery marker claims target format v4") |
| 847 | retains future delivery generations without renaming their event identity | отложен | нет теста: будущие поколения (99) без переименования; prepareSessionSnapshotFixtureForComparison не портирована |
| 861 | keeps captured generations and delivery lookalikes numeric in versioned inputs | отложен | частично: числа вне delivery-маркера не проверены (custom/event capturedFormatVersion/sessionFormatVersion) |
| 872 | preserves delivery and captured-source generations without a validated source generation | отложен | normalizeSessionFormatMetadata не портирована (в Rust эквивалент встроен в Normalizer::logs, отдельного теста без исходной версии нет) |
| 881 | preserves opaque generation qualifiers and their lookalikes | отложен | функция нормализации session-reference/capturedFormatVersion не портирована (grep capturedFormat в src пуст); lookalike-кейсы без теста |
| 955 | preserves an unexpected session-reference payload instead of omitting its fields | отложен | session-reference payload не нормализуется в Rust; нет теста |
| 963 | keeps session-reference lookalikes outside Message source positions unchanged | отложен | session-reference lookalike: нет теста; normalizeSessionFormatMetadata не портирована |
| 986 | projects persisted source-event ranges back to logical seq arrays | перенесён | tests/normalization.rs::envelopes_are_removed_references_and_event_order_remain (sourceEventSeqs [0,[1,3]] -> [0,1,2,3]) |
| 999 | rejects headerless input | перенесён | tests/normalization.rs::review_220412::a_snapshot_must_start_with_a_session_header |
| 1006 | tokenizeSessionFixtureCwd: macOS / Linux / Windows (строк в таблице: 3) | отложен | tokenizeSessionFixtureCwd не портирована (инструмент записи фикстур record/refresh; Rust replay-only) |
| 1066 | collapses a residual macOS realpath prefix around an existing cwd token | отложен | tokenizeSessionFixtureCwd не портирована (record/refresh) |
| 1084 | rejects a log without a session cwd | отложен | tokenizeSessionFixtureCwd не портирована (record/refresh) |
| 1092 | recognizes %s locators with %s separators in nested JSON omissions (строк в таблице: 3) | отложен | extractSnapshotSpillPaths не портирована; вложенные JSON-омиссии с locator/omittedBytes не проверены (в Rust spill-тест только на строках) |
| 1112 | normalizes nested Windows local spill locators with %s paths (canonical, native) (строк в таблице: 2) | отложен | нет cwdPathMode:"native" в Rust-нормализаторе; вложенные Windows spill-locators в JSON-тексте не проверены |
| 1122 | maps each spill filename to its full matched path, last match wins per name | отложен | extractSnapshotSpillPaths не портирована (refresh-инструмент) |
| 1134 | returns an empty map when the log carries no snapshot spill paths | отложен | extractSnapshotSpillPaths не портирована (refresh-инструмент) |
| 1163 | replaces system/message text and header tools with tokens, keeping config and reason | отложен | частично: {{system}} проверен (system_prompt_bulk_is_scrubbed_only_in_first_block); {{tools}} в заголовке без developer-сообщения и сохранность config/reason — только e2e; scrubModelRequestBulk как отдельная строчная функция не портирована (scrub_bulk приватна, работает над Value) |
| 1177 | keeps an absent tools field absent and an empty system prompt empty (presence is behavior) | перенесён | tests/normalization.rs::review_220412::an_absent_tools_field_stays_absent_and_an_empty_system_prompt_empty |
| 1184 | leaves malformed records with no scrubbable payload byte-identical | перенесён | tests/normalization.rs::review_220412::malformed_records_without_a_scrubbable_payload_are_unchanged |
| 1195 | passes every other line through byte-for-byte and is idempotent | отложен | байт-в-байт и идемпотентность построчной функции scrubModelRequestBulk не портированы (Rust работает над Value, не над сырой строкой) |
| 1206 | writes expanded source-event references and remains idempotent | отложен | частично: разворот sourceEventSeqs проверен (envelopes_are_removed_...), идемпотентность scrubSessionSnapshot нет |
| 1217 | writes stable feedback clocks while retaining notes and version identity | перенесён | tests/normalization.rs::feedback_clocks_and_version_identity (createdAt -> 0, version -> {{id:…}}; updatedAt/note/идемпотентность отдельно не проверены) |
| 1230 | normalizes Timeline task clocks and receipts without changing descriptions | перенесён | tests/normalization.rs::timeline_tree_tasks_receipts_and_tool_meta (часы задач и receipt обнуляются, count и текст сохранены) |
| 1247 | preserves the header while projecting and scrubbing each body record | отложен | scrubSessionSnapshot как отдельная функция не портирована; проекция+скраб покрыты кусками, не целиком |
| 1268 | rejects headerless input | перенесён | tests/normalization.rs::review_220412::a_snapshot_must_start_with_a_session_header (один путь: Normalizer::logs) |
| 1275 | scrubs only system/message text while keeping header tools verbatim | отложен | scrubSystemPrompts как отдельный режим не портирован (в Rust один проход Normalizer::logs); отдельного негативного/идемпотентного теста нет |
| 1298 | retains historical tool names when developer messages reference declarations | перенесён | tests/normalization.rs::developer_messages_retain_ordered_tool_names (tools -> ["bash","write"] при developer/message) |
| 1315 | scrubs only tool-schema payloads while keeping prompts verbatim | отложен | scrubToolSchemas как отдельный режим не портирован (в Rust один проход Normalizer::logs); сохранность промптов при скрабе tools не проверена |

#### identity.spec.ts

| строка | случай | статус | причина/тест |
|---|---|---|---|
| 13 | preserves feedback versions and target relationships without redacting unrelated prose | отложен | частично: feedback version -> {{id:…}} (feedback_clocks_and_version_identity) и прозаический UUID в tool-тексте (arbitrary_tool_text_and_usage_are_preserved); messageId -> {{message:1}} в feedback-put/delete и идемпотентность не проверены |
| 30 | preserves typed relationships across parent and child logs | отложен | частично: typed_identities_preserve_shared_parent_child_relationships (session/parentSession/childId) и messages_approvals_workflows_commands_rpc_retries_and_jobs_have_distinct_spaces (message/approval/workflow по отдельности); общий message id родитель+потомок, requestId/echoed -> {{id:1}}, `session {{session:2}}` внутри текста, идемпотентность не проверены |
| 74 | classifies semantic text plus command, RPC, and retry identity fields | отложен | частично: commandId/rpcId/retryId -> {{command:1}}/{{rpc:1}}/{{retry:1}} проверены (messages_approvals_...); семантический текст `as message …`, `Anonymous user: {{id:1}}`, неизменный `requestId` и отсутствие завершающего \n не проверены |
| 102 | keeps a canonical token first seen through a generic id key | отложен | нет теста: канонический токен {{message:7}}, увиденный через общий ключ id, нумерация {{message:8}} |

#### storage-policy.spec.ts

| строка | случай | статус | причина/тест |
|---|---|---|---|
| 49 | checks only the highest generation of every parent and child role | отложен | assertSessionFixtureStorage не портирована: проверка хранимых фикстур (скраб промпта/схем, fixed point редакции id, UNKNOWN_TOOL, /private{{cwd}}) — часть record/refresh на Node; в Rust только replay |
| 53 | preserves a selected historical child without imposing current prompt or identity storage | отложен | assertSessionFixtureStorage не портирована: проверка хранимых фикстур (скраб промпта/схем, fixed point редакции id, UNKNOWN_TOOL, /private{{cwd}}) — часть record/refresh на Node; в Rust только replay |
| 57 | rejects a selected historical child with %s (unknown tool, noncanonical cwd) (строк в таблице: 2) | отложен | assertSessionFixtureStorage не портирована; UNKNOWN_TOOL проверяется только inline по журналу прогона (src/runner.rs:825), unit-теста нет; /private{{cwd}} в хранимых фикстурах не проверяется |
| 64 | rejects an unredacted identity in a selected current child beside historical residue | отложен | assertSessionFixtureStorage не портирована: проверка хранимых фикстур (скраб промпта/схем, fixed point редакции id, UNKNOWN_TOOL, /private{{cwd}}) — часть record/refresh на Node; в Rust только replay |
| 69 | rejects a selected child with %s (missing prompt, unscrubbed prompt, unscrubbed tools) (строк в таблице: 3) | отложен | assertSessionFixtureStorage не портирована: проверка хранимых фикстур (скраб промпта/схем, fixed point редакции id, UNKNOWN_TOOL, /private{{cwd}}) — часть record/refresh на Node; в Rust только replay |

#### harness.spec.ts

| строка | случай | статус | причина/тест |
|---|---|---|---|
| 96 | keeps scenario-owned snapshot spill root length stable across platforms | отложен | snapshotSpillRoot не портирована (длина корня spill зависит от платформы; в Rust путь spill определяет сам hoh) |
| 115 | surfaces an asynchronous child spawn failure through startup and close | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 131 | centralizes ACP boot, captures, updates, fail-closed permissions, and shutdown | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 178 | builds dsh profile argv and rebases relative modules in live and replay patches | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 282 | waits for inherited stdio and buffered ACP parsing after the parent exits | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 306 | rejects promptly when fallback termination is refused | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 335 | preserves the child error when the requested signal sets an exit marker | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 358 | preserves the child error when the requested signal publishes its exit marker later | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 380 | preserves the child error when fallback refusal races with an exit marker | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 404 | preserves the child error after accepted fallback termination drains | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 426 | rejects promptly when fallback termination emits an error | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 456 | waits for in-flight client callbacks after the ACP stream closes | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 491 | includes agent stderr when the ACP connection closes during startup | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 499 | preserves launch-resolution errors when no child process exists | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 516 | drives a full turn: initialize, session, prompt, permission stub, harvest | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 547 | drives a structured prompt-content step without flattening its wire blocks | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 565 | forwards override/child fixture paths into the child env and captures stderr | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 596 | cleans acquired workspace and session roots when spill allocation fails | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 615 | gives concurrent runs of the same scenario private temporary spill roots | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 632 | seeds the workspace dir into the temp cwd before the run | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 646 | prepares the generated workspace after copying committed fixtures | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 670 | creates the generated workspace under an explicit parent | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 686 | promptAndCancel waits for the durable turn start, cancels, and settles the prompt | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 697 | promptAndCancel can wait for cwd-relative readiness before cancelling | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 728 | promptAndWaitForAgentMessage keeps the app live through a matching later update | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 743 | waitForTurnEnd holds cancellation open through the persisted closing boundary | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 762 | waitForInboxMessage holds the app through a matching durable insertion | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 790 | waitForInboxMessage times out when the session log or matching insertion is absent | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 811 | waitForTitleAfterTurnEnd holds the app through a standalone durable title | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 837 | waitForTurnStart can require a later durable turn before continuing | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 862 | waitForTurnStart rejects missing, earlier, and malformed durable turns | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 939 | waitForTurnEnd times out for a missing log and an open logged turn | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 969 | waitForGoalPhase requires the requested durable goal phase | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1001 | identifies the $label wait when its first log harvest outlasts the deadline (строк в таблице: 7) | отложен | нужен vitest с vi.spyOn(fs.readdir); wait-операции waitForTurnStart/GoalPhase/InboxMessage/… в Rust не реализованы (PORT_NOTES «ACP operations вне выбранного корпуса») |
| 1047 | waitForSubagentTurnEnd requires a closed child work turn | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1135 | waitForTitleAfterTurnEnd times out when the title precedes the boundary | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1161 | waitForEventAfterTurnEnd holds the app for a typed post-boundary record and times out otherwise | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1211 | promptExpectError swallows a model-error response as the expected outcome | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1220 | promptExpectError throws when the prompt unexpectedly succeeds (and teardown kills the live child) | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1228 | reports scenario and cleanup failures together | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1246 | reports cleanup failure after an otherwise successful scenario | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1261 | newSessionExpectError swallows the rejection, with and without extra dirs | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1279 | newSessionExpectError throws when session/new unexpectedly succeeds | отложен | частично реализовано: src/protocol.rs ("expected session/new to be rejected" / newSessionExpectError), unit-теста нет; проверяется только e2e |
| 1287 | a plain cancel step is forwarded (and ignored by an idle agent) | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1296 | a standalone cancel can wait for cwd-relative readiness | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1309 | waitForFile holds the next input step behind cwd-relative readiness | отложен | нужен Node/vitest + TS fake-acp-agent (tests/fixtures/fake-acp-agent.ts, vi.mock/vi.spyOn node:fs): runScenario/harness.ts не портирован; Rust-драйвер ACP (src/protocol.rs) проверяется только end-to-end (tests/runner_process.rs, fix_round.rs, corpus) |
| 1322 | rejects %j before newSession (строк в таблице: 12) | отложен | нужен fake-acp-agent; в Rust ошибка «prompt before newSession» есть только для prompt/promptContent/promptAndCancel (src/protocol.rs), остальные op дают «not implemented»; unit-теста нет |
| 1343 | rejects an unknown input op | отложен | нет теста: неизвестный op в Rust даёт "check acp.input.<op> not implemented" (src/protocol.rs), другое сообщение и другая семантика (fail-closed) |
| 1352 | harvests all logs primary-first, children by createdAt then id, skipping filesystem noise | отложен | harvest логов (runner::harvest) реализован, unit-теста нет; пустое/несовпадающее поколение и исчезнувший корень sessions не проверяются; нужен fake-acp-agent |
| 1381 | rejects an empty persisted generation | отложен | harvest логов (runner::harvest) реализован, unit-теста нет; пустое/несовпадающее поколение и исчезнувший корень sessions не проверяются; нужен fake-acp-agent |
| 1389 | rejects a persisted filename/header generation mismatch | отложен | harvest логов (runner::harvest) реализован, unit-теста нет; пустое/несовпадающее поколение и исчезнувший корень sessions не проверяются; нужен fake-acp-agent |
| 1399 | yields no logs when the sessions root vanished | отложен | harvest логов (runner::harvest) реализован, unit-теста нет; пустое/несовпадающее поколение и исчезнувший корень sessions не проверяются; нужен fake-acp-agent |
| 1408 | answers permission requests from the scripted queue by option kind, falling back to cancelled | отложен | очередь permissionAnswers по kind и ошибка «permission kind … was not offered» реализованы в src/protocol.rs:105-112, но unit-теста нет; fake-acp-agent и сценарии вручную не портированы |
| 1425 | selects a non-first offered option by kind | отложен | очередь permissionAnswers по kind и ошибка «permission kind … was not offered» реализованы в src/protocol.rs:105-112, но unit-теста нет; fake-acp-agent и сценарии вручную не портированы |
| 1434 | rejects the run on a scripted permission kind the agent never offered | отложен | очередь permissionAnswers по kind и ошибка «permission kind … was not offered» реализованы в src/protocol.rs:105-112, но unit-теста нет; fake-acp-agent и сценарии вручную не портированы |

#### suite.spec.ts

| строка | случай | статус | тест Rust / категория §10 |
|---|---|---|---|
| 188 | rewrites stdout and comparable logs from a replay-mode child run | не переносится | проверки генераторов: refresh-режим defineAcpSnapshotSuite: читает файлы, которые refresh-запуск записал в каталог снимков (результат записи fake-агентом) |
| 234 | uses complete UUIDs for the shared parent and child system message | не переносится | проверки генераторов: record-режим: проверяет входные данные behavior.json фикстуры record-suite, из которых генератор записи пишет фикстуры |
| 244 | creates a missing primary fixture and preserves generations for a retired child role | не переносится | проверки генераторов: record-режим: проверяет файлы, которые record-запуск записал (создание первичной фикстуры, сохранение поколений) |
| 255 | retains an unchanged message relationship across the recorded parent and child fixtures | не переносится | проверки генераторов: record-режим: проверяет фикстуры, записанные record-запуском (сохранность связи сообщений) |
| 270 | throws when a scenario's header class has no pinning scenario | перенесён | tests/registration.rs::throws_when_a_scenarios_header_class_has_no_pinning_scenario |
| 293 | throws when two scenarios pin the same header class | перенесён | tests/registration.rs::throws_when_two_scenarios_pin_the_same_header_class |
| 307 | throws when scenario names are duplicated | перенесён | tests/registration.rs::throws_when_scenario_names_are_duplicated |
| 321 | rejects %s away from a header pin (строк в таблице: 4) | перенесён | tests/registration.rs::rejects_sources_and_counts_away_from_a_header_pin |
| 345 | rejects an unknown or non-pinning sidecar source | перенесён | tests/registration.rs::rejects_an_unknown_or_non_pinning_sidecar_source |
| 380 | rejects a sidecar source that redirects the same artifact | перенесён | tests/registration.rs::rejects_a_sidecar_source_that_redirects_the_same_artifact |
| 409 | rejects shared sidecars with different header-change counts | перенесён | tests/registration.rs::rejects_shared_sidecars_with_different_header_change_counts |
| 436 | rejects shared prompt sidecars with different prompt-change counts | перенесён | tests/registration.rs::rejects_shared_prompt_sidecars_with_different_prompt_change_counts |
| 465 | accepts identical claims and rejects order-dependent shared output | перенесён | tests/registration.rs::accepts_identical_claims_and_rejects_order_dependent_shared_output |
| 478 | rejects identical committed content under different paths | перенесён | tests/registration.rs::rejects_identical_committed_content_under_different_paths |
| 493 | keeps native writer expectations outside historical replay selection | перенесён | tests/session_files.rs::keeps_native_writer_expectations_outside_historical_replay_selection |
| 500 | stdout.expected.jsonl (строк в таблице: 4) | перенесён | tests/session_files.rs::rejects_invalid_writer_snapshot_roles |
| 506 | selects the highest generation for each contiguous role while ignoring other files | перенесён | tests/session_files.rs::selects_the_highest_generation_for_each_contiguous_role_while_ignoring_other_files |
| 519 | accepts a primary-only scenario | перенесён | tests/session_files.rs::accepts_a_primary_only_scenario_and_rejects_a_missing_primary_or_a_gap |
| 523 | rejects a directory without the primary fixture | перенесён | tests/session_files.rs::accepts_a_primary_only_scenario_and_rejects_a_missing_primary_or_a_gap |
| 527 | rejects gapped child fixtures | перенесён | tests/session_files.rs::accepts_a_primary_only_scenario_and_rejects_a_missing_primary_or_a_gap |
| 532 | session.0.jsonl (строк в таблице: 7) | перенесён | tests/session_files.rs::rejects_invalid_fixture_names |
| 548 | rejects duplicate role generations | перенесён | tests/session_files.rs::rejects_duplicate_role_generations |
| 555 | renders canonical parent, child, raw, and compressed names | перенесён | tests/session_files.rs::renders_canonical_parent_child_raw_and_compressed_names |
| 564 | b/session.v1.jsonl (строк в таблице: 5) | перенесён | tests/session_files.rs::rejects_invalid_numeric_filename_components |
| 574 | parses canonical fixture and persistence names without admitting lookalikes | перенесён | tests/session_files.rs::parses_canonical_fixture_and_persistence_names_without_admitting_lookalikes |
| 585 | selects one highest persisted generation per directory and compression | перенесён | tests/session_files.rs::selects_one_highest_persisted_generation_per_directory_and_compression |
| 599 | validates filename and header generation equality | перенесён | tests/session_files.rs::validates_filename_and_header_generation_equality |
| 618 | ? (строк в таблице: 8) | перенесён | tests/session_files.rs::rejects_malformed_headers |
| 640 | adds the native sidecar after the shared golden on Windows | перенесён | tests/sidecars.rs::adds_the_native_sidecar_after_the_shared_golden_on_windows |
| 647 | keeps only the shared golden on other platforms or without the declaration | перенесён | tests/sidecars.rs::keeps_only_the_shared_golden_on_other_platforms_or_without_the_declaration |
| 668 | skips authored scenarios only while recording | перенесён | tests/sidecars.rs::skips_authored_scenarios_only_while_recording |
| 673 | skips a retained historical generation while recording but replays and refreshes it | перенесён | tests/sidecars.rs::skips_a_retained_generation_while_recording_but_replays_it |
| 678 | skips posixOnly scenarios on Windows and nowhere else | перенесён | tests/sidecars.rs::skips_posix_only_scenarios_on_windows_and_nowhere_else |
| 685 | skips pwshOnly scenarios when the host lacks pwsh, and runs them otherwise | перенесён | tests/sidecars.rs::skips_pwsh_only_scenarios_when_the_host_lacks_pwsh |
| 694 | reads the fixture header id and cwd | перенесён | tests/sidecars.rs::fixture_context_reads_the_header_id_and_cwd |
| 699 | yields no session ids for a header without a string id | перенесён | tests/sidecars.rs::fixture_context_yields_no_session_ids_without_a_string_id |
| 703 | falls back to an impossible sentinel cwd (never the empty string) | перенесён | tests/sidecars.rs::fixture_context_falls_back_to_an_impossible_sentinel_cwd |
| 709 | treats an empty fixture as an empty header | перенесён | tests/sidecars.rs::fixture_context_treats_an_empty_fixture_as_an_empty_header |
| 719 | extracts every request/header payload in log order, normalized | перенесён | tests/sidecars.rs::extracts_every_request_header_payload_in_log_order_normalized |
| 730 | yields nothing for a log without header events | перенесён | tests/sidecars.rs::yields_nothing_for_a_log_without_header_events |
| 739 | extracts every system/message text in log order, normalized, and reads empty content as no prompt | перенесён | tests/sidecars.rs::extracts_every_system_message_text_and_reads_empty_content_as_no_prompt |
| 751 | omits system/message records without a text block | перенесён | tests/sidecars.rs::omits_system_messages_without_a_text_block |
| 769 | accepts a log whose first request/header follows a system/message | перенесён | tests/sidecars.rs::accepts_a_log_whose_first_request_header_follows_a_system_message |
| 773 | accepts a log without any request/header | перенесён | tests/sidecars.rs::accepts_a_log_without_any_request_header |
| 778 | rejects a request/header with no preceding system/message | перенесён | tests/sidecars.rs::rejects_a_request_header_with_no_preceding_system_message |
| 785 | extracts normalized schema arrays, preserves empty headers, and omits invalid fields | перенесён | tests/sidecars.rs::extracts_normalized_schema_arrays_preserves_empty_headers_and_omits_invalid_fields |
| 803 | adds a missing terminal newline without changing an existing one | перенесён | tests/sidecars.rs::adds_a_missing_terminal_newline_without_changing_an_existing_one |
| 808 | renders readable changed-prompt sections | перенесён | tests/sidecars.rs::renders_readable_changed_prompt_sections |
| 813 | does not double the newline of a changed prompt | перенесён | tests/sidecars.rs::does_not_double_the_newline_of_a_changed_prompt |
| 820 | splits a formatted sidecar back into its initial prompt and each replacement | перенесён | tests/sidecars.rs::splits_a_formatted_sidecar_back_into_its_initial_prompt_and_each_replacement |
| 828 | reads a single-prompt sidecar as having no changes | перенесён | tests/sidecars.rs::reads_a_single_prompt_sidecar_as_having_no_changes |
| 836 | accepts a distinct non-empty canonical child prompt | перенесён | tests/sidecars.rs::accepts_a_distinct_non_empty_canonical_child_prompt |
| 842 | rejects an empty or non-canonical child prompt | перенесён | tests/sidecars.rs::rejects_an_empty_or_non_canonical_child_prompt |
| 849 | rejects a child prompt that duplicates its class pin | перенесён | tests/sidecars.rs::rejects_a_child_prompt_that_duplicates_its_class_pin |
| 862 | counts changed request headers, ignoring anchors, blanks, and other lines | перенесён | tests/sidecars.rs::counts_changed_request_headers_ignoring_anchors_blanks_and_other_lines |
| 877 | formats and parses canonical structured JSON | перенесён | tests/sidecars.rs::formats_and_parses_canonical_structured_json |
| 883 | rejects invalid top-level and field shapes | перенесён | tests/sidecars.rs::rejects_invalid_top_level_and_field_shapes |
| 892 | restores initial schemas into the pinned header token | перенесён | tests/sidecars.rs::restores_initial_schemas_into_the_pinned_header_token |
| 897 | preserves absent tools only for an empty sidecar | перенесён | tests/sidecars.rs::preserves_absent_tools_only_for_an_empty_sidecar |
| 903 | restores ordered tool names only when they match the sidecar | перенесён | tests/sidecars.rs::restores_ordered_tool_names_only_when_they_match_the_sidecar |
| 914 | rejects invalid headers and a missing tool token | перенесён | tests/sidecars.rs::rejects_invalid_headers_and_a_missing_tool_token |
| 923 | returns structured UNKNOWN_TOOL call ids and ignores other results | перенесён | tests/sidecars.rs::returns_structured_unknown_tool_call_ids_and_ignores_other_results |
| 938 | returns no failures for ordinary tool results | перенесён | tests/sidecars.rs::returns_no_failures_for_ordinary_tool_results |
| 944 | reuses one committed message UUID across fixture-ready parent and child logs | перенесён | tests/refresh.rs::reuses_one_committed_message_uuid_across_fixture_ready_parent_and_child_logs |
| 967 | stabilizes developer message identities through their explicit surface case | перенесён | tests/refresh.rs::stabilizes_developer_message_identities_through_their_explicit_surface_case |
| 976 | rewrites only complete messages carried by surface events or durable inbox splices | перенесён | tests/refresh.rs::rewrites_only_complete_messages_carried_by_surface_events_or_durable_inbox_splices |
| 1033 | matches cwd-bearing messages only after the fresh log reaches fixture-ready form | перенесён | tests/refresh.rs::matches_cwd_bearing_messages_only_after_the_fresh_log_reaches_fixture_ready_form |
| 1060 | rejects a fingerprint connected to an id that also identifies different content | перенесён | tests/refresh.rs::rejects_a_fingerprint_connected_to_an_id_that_also_identifies_different_content |
| 1079 | leaves fresh fixtures unchanged when no committed counterpart exists | перенесён | tests/refresh.rs::leaves_fresh_fixtures_unchanged_when_no_committed_counterpart_exists |
| 1086 | maps fresh ids and cwd values to the existing fixture values, skipping non-replacements | перенесён | tests/refresh.rs::maps_fresh_ids_and_cwd_values_to_the_existing_fixture_values_skipping_non_replacements |
| 1103 | stabilizes moved snapshot spill paths by filename while skipping unchanged or unmatched names | перенесён | tests/refresh.rs::stabilizes_moved_snapshot_spill_paths_by_filename_while_skipping_unchanged_or_unmatched_names |
| 1126 | leaves complete message ids out of the literal refresh replacement list | перенесён | tests/refresh.rs::leaves_complete_message_ids_out_of_the_literal_refresh_replacement_list |
| 1152 | preserves unpacked member times when refresh first packs a chunk run | перенесён | tests/refresh.rs::preserves_unpacked_member_times_when_refresh_first_packs_a_chunk_run |
| 1176 | preserves packed member times without flattening fresh chunk boundaries | перенесён | tests/refresh.rs::preserves_packed_member_times_without_flattening_fresh_chunk_boundaries |
| 1195 | reads projected packed fixtures while retaining their data gaps | перенесён | tests/refresh.rs::reads_projected_packed_fixtures_while_retaining_their_data_gaps |
| 1214 | {"type":"session","id":"same","createdAt":200} (строк в таблице: 4) | перенесён | tests/refresh.rs::keeps_fresh_packed_gaps_when_the_old_run_cannot_supply_them |
| 1240 | aligns volatile times across a newly inserted log event | перенесён | tests/refresh.rs::aligns_volatile_times_across_a_newly_inserted_log_event |
| 1270 | retains unchanged message ids across an unrelated inserted event | перенесён | tests/refresh.rs::retains_unchanged_message_ids_across_an_unrelated_inserted_event |
| 1320 | leaves an aligned complete message id to the fixture-ready structural pass | перенесён | tests/refresh.rs::leaves_an_aligned_complete_message_id_to_the_fixture_ready_structural_pass |
| 1339 | keeps volatile fixture fields while preserving fresh meaningful payloads | перенесён | tests/refresh.rs::keeps_volatile_fixture_fields_while_preserving_fresh_meaningful_payloads |
| 1371 | keeps job identities consistent across registration, tool text, and later calls during refresh | перенесён | tests/refresh.rs::keeps_job_identities_consistent_across_registration_tool_text_and_later_calls_during_refresh |
| 1388 | preserves normalized volatile fields while accepting fresh semantic fields | перенесён | tests/refresh.rs::preserves_normalized_volatile_fields_while_accepting_fresh_semantic_fields |
| 1491 | normalizes fresh cwd aliases before reusing existing paths | перенесён | tests/refresh.rs::normalizes_fresh_cwd_aliases_before_reusing_existing_paths |
| 1519 | preserves one correlated volatile id through a consistent log-wide mapping | перенесён | tests/refresh.rs::preserves_one_correlated_volatile_id_through_a_consistent_log_wide_mapping |
| 1543 | keeps fresh correlated ids when record alignment is structurally ambiguous | перенесён | tests/refresh.rs::keeps_fresh_correlated_ids_when_record_alignment_is_structurally_ambiguous |
| 1567 | keeps fresh ids when existing records remain unmatched | перенесён | tests/refresh.rs::keeps_fresh_ids_when_existing_records_remain_unmatched |
| 1587 | {"type":"session","id":"same","createdAt":100,"cwd":"/old"} (строк в таблице: 2) | перенесён | tests/refresh.rs::keeps_fresh_ids_when_the_mapping_would_not_be_a_bijection |
| 1626 | rejects mismatched raw clocks before normalization can erase them | перенесён | tests/refresh.rs::rejects_mismatched_raw_clocks_before_normalization_can_erase_them |
| 1631 | keeps retained child clocks in newly generated catalog fixtures | перенесён | tests/refresh.rs::keeps_retained_child_clocks_in_newly_generated_catalog_fixtures |
| 1638 | preserves partial corpora and unrelated rows verbatim | перенесён | tests/refresh.rs::preserves_partial_corpora_and_unrelated_rows_verbatim |

- `--scenario` при `--v1` теперь сужает список (раньше список его затирал и запускался весь прогон): `main.rs`.

## Ревью конвейера 213901 (s2, 20.11)

- №1 ошибки тел сценариев имеют фазу `body` (`body::fail`, `body::expect_eq`), а не `scenario-check`; `v1::classify`
  относит `body` к расхождению, `scenario-check` («check … not implemented») остаётся «прошло, тело не проверено».
- №2 `--v1` с пустой выборкой (`--scenario nope`, `--suite acp`) — ошибка `no scenarios selected`, а не «все сценарии».
- №3 проверки тел идут через `block_in_place` (многопоточный рантайм), процессы `git` и повторный `hoh` — через
  `body::run_bounded` с лимитом `--timeout-ms`: по истечении процесс убивается, проверка падает фазой `body`.
- №4 `session-write-grant`: в Rust `Disposer::dispose()` выполняет синхронную отмену внутри самого вызова (`effects.rs
  begin_inner → synchronous`), поэтому и прежний `tokio::spawn(disposer.dispose())` снимал грант до следующего запроса;
  дефекта поведения нет. Код сделан явным (`fixtures::release_grant`, без spawn), добавлен тест-страж на многопоточном
  рантайме (на прежнем коде он тоже зелёный).
- №5 в `tests/fixtures.rs::PORTED` добавлены четыре зарегистрированные фикстуры; список «не перенесено» исправлен.
- №6 случай `writesCurrentSessionFixtures` портирован.
- №7 `compare::strict_catalog`: ошибка `finish` берёт текст из причины (`source`), `released Session row N` → строка
  `row_lines[N]` (`finish_error_line`). Реальный журнал не воспроизводит эту ошибку (v0-декодер в строгом режиме
  падает раньше, в `decode_row`), поэтому тест — юнит на функцию выбора строки.
- №8 `manifest.rs`: `trim()` → `js_trim` (BOM `\u{feff}` — пробел, `\u{85}` — нет).

## Ревью конвейера 220412 (s2, 20.11, fix3)

- №1 `body::run_bounded`: потомок — лидер своей группы процессов; по сроку и после нормального выхода группа убивается
  (`kill_process_group`), чтение труб ограничено 1 с после этого. Тесты: `body::tests` (фоновая работа держит трубы —
  возврат < 1 с; таймаут убивает группу; вывод и код быстрого потомка).
- №2 `body::unchecked`/`body::gate`: SDK-тела с непереносимыми частями (`modelRequests`: command-attachments,
  command-identity, permission-switch, manual-compact; `expectedFinalResponse`: desktop-context, manual-compact,
  permission-switch, command-identity, dynamic-tool-updates) дают `scenario-check` «check sdk.<name>: <части> not
  implemented» → «прошло, тело не проверено». Тест `tests/body.rs::a_partially_carried_sdk_body_names_what_it_leaves_unchecked`.
- №3 `packed-chunks` — полное сравнение (см. таблицу выше); тест `packed_chunks_checks_the_row_types_and_the_unpacked_equality`.
- №4 `tool-call-turn` — через удерживаемый V3; тест `tool_call_turn_compares_the_writer_with_the_migrated_retained_v3_input`
  (подмена `content` во входе V3, версия не 3).
- №5 `runner::off_the_workers`: на current-thread рантайме тело отказывается с ошибкой, на multi-thread — `block_in_place`.
  Тесты `round1.rs` (оба рантайма, таймер идёт).
- №6 перенесено 10 случаев (normalize.spec: 252, 534, 799, 999, 1177, 1184, 1268; suite.spec: 519, 523, 1626).
  НЕ закрыто: у ~30 случаев функция в Rust есть, а теста нет (строки normalize.spec 186, 191, 297, 322, 403, 527, 646, 672,
  691, 782, 826, 842, 847, 955, 963; identity 102; suite.spec 694–709 `compare::context`, 719/730 `header_values`, 785
  `schema_sets`, 828 `prompt_parts`, 923/938 UNKNOWN_TOOL, 1352–1399 `harvest`, 1408–1434 `permissionAnswers`). Они остаются
  в таблице отложенных с прежней причиной; тесты не написаны в этом ходе.

## Ревью конвейера 222017 (s2, 20.11, fix4)

- №1 `Rpc::next_line` пропускает пустые строки stdout (и в `close`); тест `tests/protocol.rs::blank_lines_between_frames_are_ignored`.
  Чтение строки cancel-safe (`fill_buf`, остаток в `partial`).
- №2 `Normalizer::logs` вызывает `scrub_bulk`, нестроковый `text` остаётся; тест `a_non_string_system_text_is_not_replaced_by_the_token`.
- №3 тексты и сроки приведены к `harness.ts`: префикс `snapshot-harness: `, `within Nms`, `workspace file "p" did not appear`,
  `session "id" did not persist turn/…`, `expected session/new to be rejected but it succeeded`, `<op> before newSession`
  (сразу, без ожидания; в т.ч. `waitForTurnEnd`), неотвеченный scripted-вид разрешения: агенту уходит `cancelled`, прогон
  отвергается после шага с текстом TS; `timeoutMs` шага (и `waitForFile.timeoutMs`) учитывается. Тесты — `tests/protocol.rs`.
  Остаётся отличием: неизвестный/неперенесённый `op` даёт `check acp.input.<op> not implemented` (fail-closed), не `unknown input op`.
- №4 `Rpc::serving`: ожидание файла/границы в `promptAndCancel` идёт вместе с чтением stdout; тест — агент пишет 300 КБ до маркера.
- №5 тела идут через `block_in_place` (`runner::off_the_workers`): он отдаёт очередь воркера другому потоку, воркер не
  блокируется. Тест `bodies_blocking_every_worker_do_not_delay_the_timers_of_other_scenarios` (два тела держат оба воркера, таймер соседа вовремя).
  `spawn_blocking` невозможен: тело заимствует данные прогона.
- №6 закрыто: `identities` берёт `id` только у `session` (тест `a_first_record_that_is_not_a_session_reserves_no_identity`);
  тай-брейк по `id` у всех сьютов; `DSH_SNAPSHOT_CHILD_FILES` с `:` — ошибка; `\` в имени файла на POSIX сохраняется.
  Отложено: `yaml.rs` — нестроковые ключи (js-yaml приводит к строке) и `1_000`; в корпусе не встречаются, YAML-манифесты
  закрыты `exact_keys`. `corpus::pin` берёт первый подходящий pin без проверки уникальности (спек: `suite.spec.ts`, контракт
  регистрации, отложен как весь блок). «Refuses source delivery claiming V4» (`normalize.spec.ts:842`) не реализовано:
  `prepareSessionSnapshotFixtureForComparison` не портирована, `logs()` оставляет число; строка 842 таблицы остаётся «отложен».

## Ревью конвейера 224637 (s2, 20.11, fix5; блокирующие)

- №1 `body::unchecked` расширен управляющими проверками драйвера: `approval` (параметры `approval/request`), `session-resume`
  (закрытие, повторный запуск, `missing-session`), `subagent-continuable` (порядок завершений, результат `session/wait`),
  `session-title-stability` (ожидание provider-title), `text-turn` (цепочка `feedback/*`, `DSH_SNAPSHOT_FEEDBACK`). Все они
  идут как «прошло, тело не проверено». Тесты `tests/body.rs::controller_only_sdk_scenarios_are_passed_body_not_checked`,
  `scenarios_without_a_body_pass_and_unlisted_ones_are_not_claimed` (прежний «approval — pass» заменён).
- №2 `finish_error_line`: шаблоны пробуются по порядку источника (`Session event` → ` at seq` → `inherited Session cut`);
  `seed event at index` убран. Юнит `the_event_patterns_are_tried_in_the_order_of_the_source`.
- №3 `waitForTurnEnd` учитывает `timeoutMs` шага; тест `wait_for_turn_end_uses_the_timeout_of_its_step` («within 50ms»).

## Ревью конвейера 230043 (s2, 20.11, fix6; блокирующие)

- №1 `tool-call-turn`: второй прогон на удерживаемом V3 не портирован; `body::unchecked("session","tool-call-turn")` →
  «прошло, тело не проверено». Таблица тел выше исправлена (не «перенесено»). Тест `tool_call_turn_is_passed_body_not_checked`.
- №2 `Rpc::serving`: с `work` соревнуется только чтение строки (cancel-safe); прочитанный кадр обрабатывается и отвечается до
  конца вне `select!`. Тест `a_permission_request_racing_the_marker_is_answered` (25 кругов).
- №4 `Normalizer::logs`: журнал V2/V3, где delivery-маркер уже заявляет следующее поколение, отвергается текстом
  `format vN delivery marker claims target format vN+1` (как миграции `v2-to-v3`, `v3-to-v4`). Тест `a_source_delivery_marker_claiming_the_target_generation_is_refused`.
- №5 `packed-chunks`: оба журнала (упакованный и распакованный по V0-декодеру с исходным заголовком) идут через `migrate_fixture`
  и `Normalizer::logs` (`compare::normalized_session`). Отличие от TS: нормализуется мигрированный артефакт, а не сырой текст.
  Тест `packed_comparison_sees_chunk_contents_but_not_clocks` (другие `dt` не видны, другой текст чанка виден; сдвиг границ
  чанков ломает ссылки `assistant/message`, поэтому до сравнения не доходит — строгий каталог отвергает его раньше).
- №6 `toolcall-unused-args`: `!result.isError` с JS-истинностью (`js_truthy`); тест `toolcall_unused_args_accepts_a_tool_result_without_is_error`.
- №8 `run_bounded`: выход потомка наблюдается через `waitid(WNOWAIT)`, группа убивается до `wait()`; пока процесс не собран,
  его pid не может быть переиспользован. Отличие от источника: `snapshotGit` — `spawnSync` без таймаута, здесь каждый процесс тела ограничен.


## Перенос тестов образца (s2, fix7)

Новые модули: `storage.rs` (`unknown_tool_call_ids`, `system_prompt_precedes_requests`,
`assert_session_fixture_storage`), `session_files.rs` (имена поколений Session), `sidecars.rs`
(`scenario_skipped`, `stdout_expected_variants`, `fixture_context`, `normalized_headers/system_prompts/
tool_schemas`, форматы `system-prompt.*.md` и `tool-schemas.*.json`, `header_change_count`).
Тесты: `tests/identity.rs` (4), `storage_policy.rs` (8 случаев), `normalize_spec.rs`, `session_files.rs` (12),
`sidecars.rs` (35, строки suite.spec 640–940), `harness_harvest.rs` (4), добавки в `protocol.rs` (1408, 1425).
У каждой функции комментарий `/// :NNN` — строка случая в TS-спеке.

Найдено тестами и исправлено в коде: `runner::harvest` сообщал `empty persisted session` /
`filename/header version mismatch`; теперь использует `session_files::assert_persisted_session_version`
(тексты образца `session.jsonl: session fixture is empty`, `filename declares Session format vN, header declares vM`).
Из хвоста: `command-attachments` без `attachments` в манифесте не требует `attachmentId: null` (ключ опускается);
`yaml.rs` сохраняет `-0` числом -0, поэтому `sessionFormat.version: -0` отвергается.

### Не перенесено (с причиной из PORTING.md §10)

- suite.spec: не перенесены 4 случая — 188, 234, 244, 255 (категория §10 «проверки генераторов»): они читают файлы,
  которые записали record- и refresh-запуски `defineAcpSnapshotSuite`, либо входные данные фикстуры записи. Остальные
  113 перенесены: функции, которые выполняет и воспроизведение или вызывают сами тесты (`stabilize*`,
  `refreshFixtureReplacements`, `stabilizeRefreshLog`, `reconcileCatalogCreationTimes`, контракт регистрации,
  общие снимки), портированы в `src/refresh.rs` и `src/registration.rs` и проверены тестами `tests/refresh.rs`
  (27) и `tests/registration.rs` (11). Построчная таблица — выше (`#### suite.spec.ts`).
- harness.spec: 2 случая НЕ перенесены как тест — 1228 («сбой сценария и сбой очистки вместе») и 1246
  («сбой очистки после успешного сценария»). Это `AggregateError` над очисткой TS `runScenario`; в Rust очистки,
  которая может упасть, нет: корни прогона (`cwd-*`, `spill-*`) намеренно остаются для разбора (`runner.rs`,
  «Persist run data for diagnosis»), группа процессов убивается в `Drop` без результата. Поэтому вердикт сценария
  не может быть заменён или склеен с ошибкой очистки; единственная ошибка прогона не теряет остальных артефактов.
  Наблюдаемое поведение покрывают `a_failed_run_keeps_its_roots_and_records_the_failure`,
  `a_run_with_a_protocol_error_still_writes_every_artifact` и `an_unallocatable_runs_directory_is_a_runner_error…`.
  Причина по §10 — подмена `node:fs/promises` (приём TS-тестов, не поведение продукта).
- Перенесены наблюдаемым поведением в `tests/harness_process.rs`: 96 (`spill_locator_root`, длина `/tmp` ↔ `/t`),
  596 (сбой выделения: ошибка выделения без мусора; сбойный прогон оставляет корни), 615 (приватные корни
  каждого прогона), 670 (рабочий каталог — потомок каталога прогона), 131/178 (целый прогон через
  `hoh-conformance-fake`, материализация патчей, перенос относительных модулей, текст ошибки патча), 646
  (setup после копирования), 306/426 (застрявший потомок убит вместе с группой, `exit`: `process timeout`),
  335/358/380 (смерть по сигналу: `frames`, `protocol stdout closed before completion`, exit `None`), 456
  (запрос «в полёте» при закрытии потока). Часть 178 про ссылки `node_modules` пакетов — механика Node
  (`hoh` собственных пакетов Node не резолвит), в Rust не существует.
- Найдено и исправлено: запись в умершего ребёнка (`Broken pipe`) подменяла первую ошибку; теперь она
  равна `protocol stdout closed before completion`. `yaml::materialize` для не-массива теперь говорит
  `snapshot profile patch must be a top-level array: <путь>`.
- Перенесены через двойник `hoh` (`tests/harness_process.rs`): 115, 499 (сбой запуска), 491, 282, 404.
  Найдено тестом 491: отказ при старте не включал stderr агента; теперь причина `launch` содержит
  `agent stderr:\n…` (меняет только текст причины).
- Живые wait-операции перенесены: `src/waits.rs` + операции `protocol::acp` (`cancel`, `promptExpectError`,
  `promptAndWaitForAgentMessage`, `waitForTurnStart`, `waitForGoalPhase`, `waitForInboxMessage`,
  `waitForTitleAfterTurnEnd`, `waitForEventAfterTurnEnd`, `waitForSubagentTurnEnd`); тесты `tests/harness_steps.rs` (28).
  Различие: журналы заранее лежат в корне сессий (fake-агент TS пишет их при отмене), а неизвестная
  операция теперь даёт текст образца `snapshot-harness: unknown input op {…}`.


## Ревью конвейера 000320 (s2, 21.11, fix9): suite.spec

- Блокирующее: 59 случаев `suite.spec` были записаны целыми диапазонами «генераторы». Теперь перенесено 113 из 117,
  не перенесено 4 (188, 234, 244, 255 — «проверки генераторов»), таблица построчная.
- `src/refresh.rs`: `stabilize_fixture_message_ids`, `refresh_fixture_replacements`, `stabilize_refresh_log`,
  `reconcile_catalog_creation_times` (`ClockPolicy::{Validate, PreserveHeaders}`). Вызов `Validate` добавлен в раннер
  после сбора журналов (`runner.rs`, фаза `log`) — как `reconcileCatalogCreationTimes(..., 'validate')` в suite.ts:1349;
  до этого расхождение сырых часов каталога родителя и потомка при воспроизведении не проверялось.
- `src/registration.rs`: `validate_registration` (дубликаты имён, класс заголовка без пина и пин-дубль, источники и
  счётчики сайдкаров), `claim_shared_snapshot`, `assert_unique_snapshot_contents`. Таблица `Declaration` собирается
  вручную: в Rust сценарии берутся из манифестов, а `corpus::pin` не проверяет уникальность пина — это известное
  отличие, строка в вопросы ведущему.
- Различие: `stabilize*` в TS вызываются только режимом refresh, но функции чистые и их поведение сохранено полностью
  (порядок ключей JSON, числа, ограничение безопасных целых для времён пакетных строк).
