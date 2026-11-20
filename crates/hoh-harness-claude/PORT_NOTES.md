# hoh-harness-claude — исполнитель Claude Code для HoH (HOH-18, путь А)

Новый код HoH, не перенос dsh. Логика событий и разрешений и схемы сообщений взяты из T3 Code
(MIT, коммит 803f21f6, `/mnt/toshiba/alexandrie/third-party/t3code`); текст MIT и copyright — в
`NOTICE`. T3 ходит в CLI через `@anthropic-ai/claude-agent-sdk`; здесь тот же протокол
(`claude -p --input-format stream-json --output-format stream-json --verbose`) ведётся напрямую,
Agent SDK не используется и не поставляется. Всё про Claude — только в этом крейте; слот
(`hoh-external-harness`) и ядро ничего о Claude не знают.

## Границы

- Плагин `HarnessClaudePlugin` (имя `HarnessClaude`) регистрирует исполнителя `claude` через
  `ExternalHarnesses::register`. Исключение из профиля — не загружать плагин; другие крейты не
  правятся.
- CLI не запускается тестами: вместо него `tests/stub/claude.rs` (`claude-stub`).
- `~/.claude` и файлы с токенами не читаются. Вход определяется по ответу `initialize`
  (`account`), как в T3.

## Соответствие образцу (файл T3 : строки)

| Что | Где здесь | Образец T3 |
|---|---|---|
| Запуск `claude -p … stream-json --verbose`, `model`, `sessionId`/`resume` | `harness.rs` `session_args` | `ClaudeAdapterV2.ts:676-715`, `:1484-1555` (опции SDK, которые SDK превращает в эти флаги) |
| Политика → режим (всегда `default` + `can_use_tool`; `bypassPermissions` и `dontAsk` не используются) | `harness.rs` `launch_for` | `ClaudeAdapterV2.ts:3358-3407` (`permissionModeForClaudeRuntimePolicy`), `:3417-3468` (`claudeRuntimeQueryPolicyForRuntimePolicy`, `installPermissionCallback`) |
| Список read-only инструментов: только `--tools` (остальных инструментов у модели нет), без `--allowedTools` | `READ_ONLY_TOOLS` | `ClaudeAdapterV2.ts:3436-3466`, `CLAUDE_READ_ONLY_ALLOWED_TOOLS` (сокращён до инструментов чтения) |
| `can_use_tool` → запрос разрешения; allow с `updatedInput`, deny с сообщением, отмена с `interrupt` | `session.rs` `can_use_tool`, `respond` | `ClaudeAdapterV2.ts:12468-12585` (`canUseToolEffect`), `:4388-4430` (`permissionResultFromDecision`) |
| `AskUserQuestion` → вопрос; ответы по тексту вопроса через «, » | `session.rs` `questions_from`, `answers_by_question` | `ClaudeAdapterV2.ts:12378-12445`, `:5389-5401` (`claudeSdkUserInputAnswers`) |
| Причина запроса: prompt = command → title → description → decision_reason → сводка; reason = decision_reason, если он не повторяет prompt | `session.rs` `approval_prompt`, `approval_reason` | `ClaudeAdapterV2.ts:12511-12517`, `:4360-4387`; |
| Шаг, который CLI не выполнил (`tool_result_meta.non_execution_kind` cancelled/interrupted) → `Interrupted` | `session.rs` `non_execution_kind` | `ClaudeAdapterV2.ts:4327-4340`, тест `ClaudeAdapterV2.test.ts:8208-8285` |
| Текст запроса: команда, иначе путь, иначе описание | `session.rs` `action_for` | `ClaudeAdapterV2.ts:4120-4132` (`summarizeClaudeToolRequest`), `:3656` (`providerRequestKindFromClaudeTool`) |
| `thinking_delta` → рассуждение, `text_delta` → текст, `tool_use`/`tool_result` → шаги, `result` → конец хода | `session.rs` `handle_*` | `ClaudeAdapterV2.ts:10817-11003` |
| Вход выполнен: только `tokenSource: "none"` без ключа/почты/подписки опровергает | `harness.rs` `auth_from` | `ClaudeProvider.ts:213-228` (`claudeAuthStatus`) |
| Проба `initialize`: без сохранения сессии, без MCP, IDE выключен | `harness.rs` `probe_args`, `spec` | `ClaudeProvider.ts:271-306` (`buildClaudeCapabilitiesProbeQueryOptions`), `:520-604` |
| Модели из ответа `initialize`, `default` — не модель | `harness.rs` `models_from` | `ClaudeModelCatalog.ts:178-230` (`claudeReportedModels`) |
| Установлен ли CLI — `claude --version` | `harness.rs` `check_version` | `ClaudeProvider.ts:706-776` (по исследованию `hoh18-t3.md`) |
| Остановка: `interrupt` и жизненный цикл процесса | `session.rs` `interrupt`, `terminate` | `ClaudeAdapterV2.ts:14023-14060` (процессы добивает надзор слота) |

Не перенесено из образца: planning-режим и `ExitPlanMode`, subagent-ветки, MCP elicitation,
`acceptForSession` и правила `updatedPermissions`, usage/rate-limit, bundled-каталог моделей по
версиям (`ClaudeModelCatalog.ts:300-380`) и `claudeHistoryWorker.ts` — историю ведёт HoH, нативный
ID нужен только для `--resume`.

## Отличия и отложенное (круг 1)

- Вместо резервного текста `Tool: …` (T3 `summarizeClaudeToolRequest`) контракт получает
  `HarnessAction::Tool{name,input}`; сводка используется только как `prompt` для выбора `reason`.
- Не перенесены тесты T3 про `acceptForSession`, `plan`, MCP elicitation, usage, каталог моделей
  (функции не перенесены). `denied`/`permission_denied` из `tool_result_meta` остаются `Failed`, как
  в T3 («без вывода отказа из текста»); `Declined` ставится только для запросов, отказанных здесь.
- `Cancelled`: разрешение → deny с `interrupt: true`, вопрос → обычный deny (T3 `:12433-12437`).
- После `--resume` в `prompt` уходит id из `system/init`, если CLI выдал новый.
- `ClaudeConfig` в `Debug` показывает только имена переменных окружения.
- Состояние `consumed`/`declined` очищается в конце хода.

## Версии CLI

Поддерживаются Claude Code `2.0.0` ≤ версия < `3.0.0` (`version.rs`). Протокол управления
Anthropic публично не закрепляет, поэтому диапазон проверяется при каждом `discover` и при каждом
старте: вне диапазона `discover` возвращает `detail` по-русски («Claude Code версии X не
поддерживается, нужна 2.0.0–2.x.»), а `start_or_resume` отказывает кодом `unsupported-version`.
Если `initialize` не ответил успешно, сообщение называет версию и диапазон. Границы выбраны по
знанию протокола, на живом CLI не проверены.

## Допущения о протоколе (проверить живым прогоном)

- `--permission-prompt-tool stdio` включает `can_use_tool` по stdin/stdout;
  `--include-partial-messages` даёт `stream_event`.
- `--session-id <uuid>` для нового сеанса и `--resume <id>` для продолжения; `--resume` с
  неизвестным ID завершает процесс с «No conversation found…» на stderr → `session-not-found`.
- `initialize` без параметров допустим; ответ содержит `models` и `account`.
- CLI шлёт по одному `assistant` на блок содержимого с общим `message.id`: id сообщений контракта
  — `<message.id>:<номер блока>`, одинаковые для дельт и итога.
- Сеть (`policy.network = denied`) CLI не отключает; отключаются только `WebFetch`/`WebSearch`.
  `bypassPermissions` — не песочница ОС.

## Не проверено

- Настоящий `claude` не запускался; схемы сообщений CLI взяты по T3 и знанию протокола.
- Связка с живым слотом (`ExternalHarnessPlugin` + ядро) тестом этого крейта не покрыта: тесты
  гоняют `ClaudeHarness` напрямую через `ExternalHarness`/`HarnessSession`.

## Раунд 2 полного ревью (решения)

- Процесс, убитый сигналом: `SessionEnded.message` = «Claude Code process terminated by signal
  SIGABRT» (T3 `ClaudeAdapterV2.test.ts:22384`), далее хвост stderr, если он есть; номер без имени
  выводится числом. Ход получает это сообщение через ядро слота. Тест: сценарий `kill-self`.
- `reason`/промпт запроса очищаются от управляющих последовательностей терминала и обрезаются
  (T3 `:4367`, `stripVTControlCharacters`).
- Шаги подагента (`tool_use` с `parent_tool_use_id`) отфильтрованы, как и их текст: T3 вкладывает их
  в родительский поток, у слота такого уровня нет. Запрос на право подагента приходит без `item_id`.
- Стат файловой системы (`Write`: Add/Update, проверка рабочей папки) не блокирует рантайм:
  `tokio::fs`.
- Открытые запросы снимаются (`RequestResolved`) при `terminate`, конце потока и нарушении
  протокола (T3 `:7713`).
- `initialize`: `select!` предпочитает ответ завершению процесса (`biased`).
- Ошибка отправки: `interrupt` сбрасывает `interrupting` и запись `host`; `prompt` после
  `TurnStarted` отправляет парный `TurnFailed`.
- Ответ на запрос: `updatedInput` всегда объект (`{}` без `input`); `toolUseID` и
  `decisionClassification` (`user_temporary`/`user_reject`) как в T3 `:4426-4428`.
- `claude-stub` собирается только с функцией `stub`; она включена dev-зависимостью крейта на себя,
  поэтому обычная сборка (`apps/hoh`) заглушку не строит, а `cargo test` строит.
- Закрыто тестами и остальные пробелы ревью: `multiSelect`/`custom`, `--disallowedTools`
  (`network: Denied`), `--model` сквозным запуском, неизвестный `control_request`,
  `control_cancel_request` для вопроса, запросы при `result`, `stream_event` подагента, обрезанный
  stdout.
- Не покрыто тестом: ошибки отправки в `interrupt`/`prompt` (нужен мёртвый stdin при живом
  читателе), порядок `select!` в `initialize`, неблокирующий стат.

## Раунд ревью 212419 (решения ведущего)

Аргументы CLI по режимам (общее для всех: `-p --input-format stream-json --output-format
stream-json --verbose --include-partial-messages --setting-sources ""`, затем `--model`, если не
`default`, и `--resume`/`--session-id`):
- по умолчанию и `approval=never`, полный доступ: `--permission-mode default
  --permission-prompt-tool stdio` (+ `--disallowedTools WebFetch,WebSearch` при закрытой сети);
- read-only: `--permission-mode dontAsk --tools/--allowedTools Read,Glob,Grep,LS,NotebookRead,TodoWrite`
  (без `--permission-prompt-tool` при `never`).

- Отличие от T3: `approval=never` НЕ включает `bypassPermissions` (T3 `:3358-3407` в полном доступе
  одобряет всё сам). Иначе CLI исполнял бы `rm -rf` молча, и правило слота («без вопроса только
  неопасный шаг внутри папки») было бы недостижимо. Теперь отвечает слот.
- `--setting-sources ""`: пользовательские, проектные и локальные файлы настроек не загружаются,
  их `allow: Bash(*)` не обходит слот; политика администратора действует. Флаг есть в
  `claude --help`; пустое значение как «ни одного источника» — по поведению SDK, на живом CLI не
  проверено.
- `reason` обрезается до 400 символов (как `summarize_tool`); C1-CSI `\u{9b}` вырезается.
- `cwd` команды: папка сессии, либо цель ведущего `cd <простой путь> &&|;`; любой другой
  `cd/pushd/popd/chdir` (в том числе внутри `bash -c`) даёт `/` — заменитель «неизвестно»,
  вне любой папки сессии, поэтому слот спрашивает. Контракт признака «неизвестен» не имеет.
- Тесты слота: классификатор слота (`hoh-destructive-classifier`) на действиях, которые отдаёт
  крейт; сквозного прогона через `ExternalHarnessPlugin` и агента нет (нужна сборка ядра из
  8 плагинов, это тесты крейта `hoh-external-harness`).

## Раунд ревью 212948

- `cwd` команды известен только если `may_change_folder` не находит смены каталога. Кавычки и
  `\` стираются (`\cd`, `c''d`, `c\d` = `cd`), затем по простым командам строки: слово `cd`,
  `pushd`, `popd`, `chdir`, `eval`, `exec`, `source` где угодно (в том числе в `bash -c '…'` и
  `$(…)`), первое слово `.` или начинающееся с `$`/обратной кавычки → «неизвестно» = `/`, слот
  спрашивает. Исключение: ведущий `cd <простой путь> &&|;` без кавычек и подстановок.
- `--setting-sources ""` теперь и в `probe_args`. Живая проверка (`claude` 2.1.329, без сети и
  модели): `claude --setting-sources "" --version` принимается (код 0); неверное значение
  (`bogus`) отвергается до `--version` («Invalid setting source … Valid options are: user, project,
  local»), значит пустая строка прошла проверку источников. Что пустое значение не загружает
  ничего, по поведению не проверено (для этого нужен запрос модели).
- Сквозной тест со слотом («`rm -rf` спрашивается, `ls` внутри папки нет») делает o1 в
  `hoh-external-harness`; здесь проверено только на классификаторе слота.

## Раунд ревью 213901

- `cwd` команды: разбор shell убран, правило по белому списку. Папка сессии — только если в строке
  нет ни одного из `$ ` \ ' " ; & | ( ) { } < >` и перевода строки и ни одно слово не равно
  `cd`, `pushd`, `popd`, `builtin`, `command`, `eval`, `exec`, `source`, `.`, `env`, `xargs`,
  `sh`, `bash`, `zsh`, `time`, `nohup`, `sudo` (плюс `chdir`). Иначе `/` («неизвестно»),
  слот спрашивает. Принято: под `approval=never` спрашивается чаще (любая цепочка `&&`, конвейер,
  кавычки). Ведущий `cd x && …` больше не разбирается.
- Что `--setting-sources ""` реально не грузит пользовательские правила, проверяет живой прогон o1
  (`docs/claude-live-run.md`); у этого крейта такого теста нет.

## Раунд ревью 214304

- «Каталог неизвестен» (`cwd = /`) теперь и для аргументов, меняющих рабочую папку: слово `-C`
  (и `-C<путь>`, но не `-C3` у grep), `--directory`, `--prefix`, `--git-dir`, `--work-tree`,
  `--cwd`, `--chdir`, `-chdir` (также `--опция=путь`).
- Пути в аргументах (`cat ~/.ssh/id_rsa`, `curl -T …`), чтение и сеть через `Bash` — задача слота
  (`within_workspace`, o1), в этом крейте не проверяются.
- Живые проверки (`Bash(*)` из пользовательских настроек при `--setting-sources ""`; read-only +
  `on-request` под `dontAsk`: вопрос не доходит) делает живой прогон o1 (`docs/claude-live-run.md`).

## Решение CEO 21:52: приёмка делится

- **(а) Обычный режим (этот пакет).** Любое действие с командой, записью и сетью, которое CLI не
  разрешает сам, идёт вопросом гнезду: `--permission-mode default --permission-prompt-tool stdio
  --setting-sources ""` (+ `--disallowedTools WebFetch,WebSearch` при закрытой сети). Автоматического
  «да» в адаптере нет: `respond` отвечает только тем, что решил человек или слот. `bypassPermissions`
  и `--allow-dangerously-skip-permissions` в коде отсутствуют.
- **Read-only:** `--permission-mode dontAsk --tools/--allowedTools Read,Glob,Grep,LS,NotebookRead,TodoWrite
  --permission-prompt-tool stdio`. «Да» только на чтение этим набором; команда, запись и сеть
  CLI не получает вовсе.
- **(б) `approval: never` — ВЫКЛЮЧЕН** до отдельного пакета: старт отказывает кодом
  `escalation-denied` («Режим без вопросов для Claude Code пока недоступен…») до запуска CLI, для
  любого sandbox, в том числе read-only.
- `command_cwd` (белый список, `FOLDER_WORDS`, `FOLDER_OPTIONS`) — **для пакета (б)**. В режиме
  (а) он только заполняет `cwd` действия для показа и слота; решения «да» не принимает, и на
  вопрос не влияет (слот при `on-request` спрашивает всегда).
- Остаточный риск, не закрываемый адаптером: сам CLI в `default` может разрешить часть
  безвредных команд без `can_use_tool` по своим встроенным правилам; проверяется живым прогоном.

## Раунд ревью 215107 (режим (а))

Аргументы CLI: **обычный** `--permission-mode default --permission-prompt-tool stdio
--setting-sources ""` (+ `--disallowedTools WebFetch,WebSearch` при закрытой сети); **read-only** то
же самое + `--tools Read,Glob,Grep,LS,NotebookRead,TodoWrite`, **без** `--allowedTools` и без
`dontAsk`: чтение вне папки идёт вопросом, как в обычном режиме (раньше `--allowedTools` отвечало
«да» на чтение любого файла). `approval: never` по-прежнему выключен.

- Ответ человека не теряется молча: `respond` возвращает `Err(closed)`, если в stdin записать нельзя.
  Ограничение: очередь записи `Supervisor` асинхронная, самая первая запись в уже мёртвый канал
  успех лишь ставит в очередь; ошибку видит следующая отправка. Нужно от o1: `Supervisor::send`
  должен подтверждать запись (или сообщать об ошибке записи событием/флагом).
- Ответ на вопрос и approval: `toolUseID` всегда строка (id вызова, при его отсутствии id запроса),
  тело ответа на вопрос равно T3 `:12438-12445` (тест сравнивает JSON целиком).
- `command_cwd` сравнивает имя программы, не путь (`/bin/sh`, `/usr/bin/env`).
- `check_version`: ненулевой код выхода — не версия; `Discovery.detail` проходит `redact`.

### Карта «тест T3 → тест Rust» (§14.7)

| T3 (`ClaudeAdapterV2.test.ts`) | Rust (`tests/claude.rs`, `session.rs`) |
|---|---|
| политика → режим, `:558-660` | `approval_never_is_refused_and_no_mode_gets_a_way…`, `no_policy_gives_the_cli_a_way…` |
| allow/deny с `toolUseID`, `:992-1022` | `the_answer_to_a_request_carries_an_object_and_the_tool_use_id`, `an_allowed_command_runs_and_a_denied_one_is_declined` |
| вопрос: ответы по тексту, `:349-` | `a_question_reaches_the_user…`, `the_answer_to_a_question_has_the_body_of_t3`, `answers_join_several_choices…` |
| отмена запроса, `Cancelled` | `a_cancelled_permission_request_denies_with_interrupt`, `a_cancelled_question_is_a_plain_deny` |
| `tool_result_meta`, `:8208-8285` | `interrupt_during_a_command…`, `an_interrupted_step_keeps_the_output_of_its_own_result` |
| процесс убит сигналом, `:22384` | `a_process_killed_by_a_signal_names_the_signal` |
| открытая карточка при остановке, `:7713` | `ending_the_session_releases_the_open_requests`, `a_process_that_dies_releases…` |
| текст/инструменты подагента | `text_of_a_subagent…`, `a_step_of_a_subagent_is_not_a_step_of_the_turn`, `a_stream_delta_of_a_subagent…` |
| `decisionReason` без ESC, `:4367` | `terminal_escapes_are_stripped…`, `the_single_character_csi…` |

### Отложено (с причинами)

- **Дозапись и перенаправление хода (steering)**, T3 `:6161`, `:16127`: контракт слота не имеет
  такого вызова; второй `prompt` во время хода отклоняется (`a_second_prompt_during_a_turn_is_refused`).
- **Откат на свежую сессию при «conversation gone»**, T3 `:14708`: слот ожидает
  `session-not-found` и сам решает (создать новую сессию); адаптер молча подменять сессию не
  должен, иначе пропадёт история.
- **Уже прерванный сигнал запроса**, T3 `:1607`, `:7669`: у адаптера нет `AbortSignal`; роль
  играют `control_cancel_request` и `HarnessResponse::Cancelled` (покрыто).
- **`acceptAlways` / `acceptForSession`**, T3 `:7612`: контракт слота не знает «разрешить на
  сессию», а `updatedPermissions` дали бы CLI собственное постоянное правило (в обход слота).
- Отложено прежнее: план, MCP-elicitation, usage, каталог моделей.

## Раунд ревью 215810

- `terminate` не зависит от читателя событий: `RequestResolved` кладётся в очередь без ожидания
  (`try_send`), что не поместилось, доставляет отдельная задача (до 30 с на событие); SIGTERM идёт
  сразу. Тест: `event_queue = 1`, два открытых запроса, события не читаются, `terminate` ≤ 3 с.
- Остановка без ответа ограничена (T3 `ClaudeAdapterV2.test.ts:14176`): через
  `ClaudeConfig.interrupt_timeout` (10 с) после `interrupt` без `result` ход закрывается один раз
  как `TurnAborted`, открытые шаги — `Interrupted`, запросы снимаются, процесс закрывается,
  `SessionEnded{Terminated}`. Тест: заглушка `silent-interrupt`.
- Проекция `AskUserQuestion` как у T3 `:5339-5388`: пустой вопрос отброшен, `header` = `Question N`
  (N по исходному списку), `description` = метка, пустая метка отброшена, в ответе нет ключа
  `questions`, если его не было во входе. Ключ ответа — обрезанный текст вопроса.
- `StreamClosed` с пустым хвостом: `message = None`.
- Потеря ответа в мёртвый stdin (закрыто): `Supervisor::send` у o1 возвращает `Ok` только после
  `write_all`+`flush`, ошибка (`closed`/`io-error`) идёт отправителю. `reply`/`respond` передают её
  наверх без изменений; запрос при этом остаётся открытым (не отвечен, шаг не помечается
  отклонённым) и снимается `RequestResolved` при завершении сессии. Исключение: ответ-ошибка на
  неизвестный `control_request` (`reply_error`) идёт из читателя кадров, где нет вызывающего, и его
  сбой не доходит ни до кого; он сам по себе признак мёртвого процесса, о котором скажет
  `SessionEnded`. Тест `an_answer_to_a_closed_stdin…` в этом дереве подготавливает мёртвый канал
  первой отправкой (`interrupt`), потому что дерево s4 пока со старым `Supervisor`; с подтверждением
  записи (дерево o1) первая же отправка вернёт ошибку.
- Не покрыто тестом (заметки ревью, не требования круга): `cwd` с сохранённой папкой CLI (для (б));
  `Debug` у `ProcessSpec` (чужой крейт); `NotebookEdit`, вывод stdout/stderr команды, 403 в
  сообщении хода, `HarnessClaudePlugin`/`Config`.

## Два режима доступа (решение основателя 20.11)

Режим выбирает человек в окне; адаптер получает его полем `ClaudeConfig.access` (`Access::Confirm`
по умолчанию, `Access::Full`; в конфигурации плагина `access: "confirm" | "full"`). Больше нигде он
не выводится: ни из политики `session/create`, ни из ответов CLI. `approval: never` с белым списком
по-прежнему выключен в обоих режимах.

- **confirm** (режим (а), без изменений): `-p --input-format stream-json --output-format
  stream-json --verbose --include-partial-messages --permission-mode default --setting-sources ""
  --permission-prompt-tool stdio` (+ `--tools …` для read-only, + `--disallowedTools
  WebFetch,WebSearch` при закрытой сети) `--session-id <uuid>` | `--resume <id>`.
- **full** (выбор человека; T3 включает bypass только при `never`/`dangerFullAccess`, а у нас выбор режима заменяет `never`; `ClaudeAdapterV2.ts:3358-3407` `permissionModeForClaudeRuntimePolicy`
  → `bypassPermissions`; `:3417-3468` `claudeRuntimeQueryPolicyForRuntimePolicy` →
  `allowDangerouslySkipPermissions: true`, без callback; `:1255`, `:2329` передают флаг CLI):
  те же общие флаги + `--permission-mode bypassPermissions --allow-dangerously-skip-permissions
  --setting-sources "" --permission-prompt-tool stdio` (колбэк остаётся, см. раунд 223024), **без** `--tools`, `--disallowedTools`. Песочница и
  сеть политики сессии режим не сужают. `--setting-sources ""` оставлен и здесь (наше добавление: T3
  его не передаёт; разрешения при `bypassPermissions` от него не зависят).
- `ClaudeConfig::argv(policy, model, resume, native_id)` (и `ClaudeHarness::argv`) возвращает
  фактический argv вместе с программой; запуск использует ту же функцию, окно показывает режим по ней.

## Раунд ревью 221515

Перенесено (§14.7): T3 `:8766` — истёкший вход называется (`message.error ==
"authentication_failed"` → «run `claude auth login`», папка сессии и `CLAUDE_CONFIG_DIR`), тест
`an_expired_login_is_named_…`; `:8821` — при 403 показывается текст CLI, не подсказка про вход,
тест `a_403_shows_…`; `:11253` — `NotebookEdit` называет блокнот, тест `a_notebook_edit_names_…`;
`:15269` — вывод инструмента идёт как вывод шага (`ItemCompleted.output` из `tool_result`; тесты
`an_allowed_command_runs_…`); `:14671`, `:14752` — процесс вышел до ответа, ход не остаётся
открытым: старт отказывает (`a_cli_that_dies_at_start…`), падение в ходе даёт `SessionEnded`
(`a_crash_in_a_turn…`, `a_process_killed_by_a_signal…`).

Отложено с причиной: `:15795` (результат на ранее записанный prompt без начатого хода): у адаптера
нет журнала prompt uuid, `result` без своего хода игнорируется (`handle_result` возвращает
сразу); `:8287` (ход после диалога resume): диалог resume — особенность Agent SDK, у CLI в режиме
stream-json его нет.

- Гонка `respond`/`terminate`: если сессия уже снята (`State.released`), неудавшийся ответ
  освобождает свой запрос одним `RequestResolved`, а не возвращает его в `pending`.
- Папка команды в режиме (а) показывается настоящая (папка сессии): подставной `/` вводил бы
  в заблуждение окно и классификатор слота (относительные пути разбираются от неё). Белый список
  `command_cwd` оставлен для пакета (б) за `STRICT_FOLDER = false`.
- Сторожа `interrupt_timeout` хранят `AbortHandle` и отменяются в `terminate` и при конце потока;
  сессия не живёт до конца сна.
- `strip_terminal_escapes` — шаблон ansi-regex Node (`ESC(B`, C1, OSC); голые `\r` и BEL
  остаются, как у Node.
- `summarize_tool`: ключи в порядке CLI (`serde_json` с `preserve_order` в workspace), запись как
  `JSON.stringify` (`hoh-util-values`), длина и срез в единицах UTF-16.
- `prompt` при ошибке отправки сообщает её один раз: `Err` (слот закрывает им ход); парное
  `TurnFailed` убрано (отмена решения раунда 2).

## Раунд ревью 222318

- **full ≠ «как T3».** T3 включает `bypassPermissions` только при `approval: never` (или полном
  доступе песочницы, `ClaudeAdapterV2.ts:3364-3378`); у нас `full` — выбор человека в окне и
  заменяет `never`. `full` расширяет только обычную (workspace-write) сессию. Read-only сессия
  (например, ревьюер) остаётся read-only: `--permission-mode dontAsk --tools
  Read,Glob,Grep,LS,NotebookRead,TodoWrite`, без вопросов (T3 для read-only: `:3380-3386`).
  Тест `full_does_not_widen_a_read_only_session…`.
- **full и сеть.** В `full` при `network: Denied` `--disallowedTools` не передаётся: `WebFetch` и
  `WebSearch` разрешены — это выбор человека (тест там же). `full` + `--resume`: те же флаги, тест
  `full_access_resumes_a_session_with_the_same_flags`.
- **`approval: never`:** в confirm отказ с текстом слота «Этот режим пока недоступен для Claude:
  действия идут через подтверждение»; в full не отказывает (вопросов там нет по определению).
- `command_cwd` (для пакета (б)): `.` как аргумент — путь (`ls .`, `grep -r x .`), слово `.` как
  программа — `source`; кавычки снимаются перед поиском слов (`c''d` = `cd`), поэтому
  `git commit -m "x"` остаётся в папке сессии.
- `failed_start`: хвост stderr проходит `redact`; отказ CLI от `bypassPermissions` под root
  называется (T3 `:383-385`, `:765`: «Claude Code exited before it started: …» + что делать).
- `AskUserQuestion` в full: колбэка нет, CLI не шлёт `can_use_tool`, `QuestionAsked` не приходит
  (тест `in_full_access_a_question_is_not_asked`); как CLI отвечает модели, проверит живой прогон.
- Гонка `respond`/`terminate` закрыта в раунде 221515 (`State.released`).

## Раунд ревью 223024

- stderr процесса проходит `redact` везде: `SessionEnded.message` (`end_of_stream`) и
  `diagnostics()` (контракт требует маскирования); тест `the_stderr_of_a_dying_process_is_masked…`.
- Причина запроса как у T3 `:12511-12517`, `:4367`: prompt — команда (если не пуста, как есть), иначе
  `title ?? description ?? decision_reason` (пустой `title` выбирается), без очистки и обрезки;
  очищается и обрезается только `reason`, сравнение с сырым prompt (юнит-тесты `title:""`, escape).
- Гонка `interrupt_timeout`: расчёт хода (`ItemCompleted(Interrupted)`, `RequestResolved`,
  `TurnAborted`, `SessionEnded`) идёт отдельной задачей, которую конец потока не отменяет; ход
  получает ровно один терминальный исход (юнит-тест с очередью в одно событие).
- **full оставляет колбэк** (`--permission-prompt-tool stdio`), как T3 (`:3466-3467`): то, от чего
  отказывается сам CLI под bypass (защищённые пути и т.п.), доходит до человека обычным
  `ApprovalRequested` (тест `in_full_access_what_bypass_refuses…`). Отказ «removal safety»
  (`Dangerous rm…`, два сообщения `shell -c`, без `matched_ask_rule`) адаптер сам отклоняет словами
  T3 `:12494-12510`, человека не спрашивает: это автоматический отказ, не «да»; только в bypass (тест
  там же, и в confirm тот же запрос — обычный вопрос).

### Отложено (с причинами)

- **`fullAccessOverride`** (T3 `:952-982`, `:4898`): отметка карточки «provider_check/ask_rule» —
  поле презентации T3; контракт слота его не имеет, текст причины CLI показывается как есть.
- **Понижение bypass при политике администратора** (T3 `:623`, `:4822`): T3 по ответу SDK
  переключается на `auto`/`default`. У нас CLI, запрещённый политикой, не стартует с bypass и
  сообщает это в stderr: старт падает с этим текстом (`failed_start`); автопонижения нет, решение
  остаётся за человеком (выбрать «С подтверждением»).
- **TodoWrite и задачи** (T3 `:11412-12194`): план/задачи — узел плана T3; контракта плана у
  слота нет, `TodoWrite` идёт как обычный шаг (`HarnessAction::Tool`).
- **`api_retry` и Stop во время повтора** (`:12214`, `:12430`): повтор запроса CLI виден только как
  служебные кадры; остановка и так ограничена по времени (`interrupt_timeout`).
- `preview` у вариантов `AskUserQuestion` отбрасывается (контракт вопроса его не имеет).
