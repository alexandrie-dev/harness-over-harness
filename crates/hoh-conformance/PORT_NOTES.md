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
