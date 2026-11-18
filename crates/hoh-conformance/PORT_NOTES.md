# hoh-conformance ← packages/test-support/session-snapshot, snapshots/{session,sdk,acp} @ 39163dd3

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
| brief: identity/sensitivity/normalization | `src/bin/fake.rs`, `tests/{corpus,normalization}.rs` | fake-процесс записывает весь корпус, реальные вместо токенов id/cwd/часы/конверты/сателлиты, stdout/stderr и exit; обе стороны сравнений проверяются без hoh |

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
  тест проверяет missing-edge reason для каждой исторической роли. ACP без model turn миграция не нужна.
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
  Каталог только с заголовком не содержит переставляемых model events: sensitivity для handshake и
  reject-extra-dirs переставляет два различных ACP response frames и требует phase=frames.
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

## Проверка

Все Cargo-команды: `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-cf1`.

- `cargo test -p hoh-conformance` — **25 passed, 0 failed, 0 ignored**:
  `tests/corpus.rs`: `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`;
  `tests/normalization.rs`: `test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
  Unit/doc targets содержат 0 тестов. Identity и sensitivity обходят все 242 сценария, без исключений.
- `cargo clippy -p hoh-conformance --all-targets -- -D warnings` — exit 0,
  `Finished dev profile [unoptimized + debuginfo]`.
- `cargo fmt -p hoh-conformance -- --check` — exit 0.
- Построенный `/mnt/toshiba/alexandrie/hoh/target-cf1/debug/hoh-conformance` запущен с
  `--hoh /bin/false --refs /mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`
  `--runs /mnt/toshiba/alexandrie/hoh/target-cf1/runs/bin-false-final`
  `--suite session --suite sdk --suite acp --jobs 4`
  `--results /mnt/toshiba/alexandrie/hoh/target-cf1/runs/bin-false-final/RESULTS.tsv`.
  Exit 0; **`scenarios: 0 of 242 pass, 242 fail, 0 runner errors`**.
  Отдельно прочитан TSV: 242 строки, 192 session + 41 sdk + 9 acp, все status=fail/phase=exit,
  все строки имеют пять полей; RESULTS.runner-errors.tsv пуст.
