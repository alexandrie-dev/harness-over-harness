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
| Политика и режим гнезда → флаги (confirm: `default` + `can_use_tool`; full: `bypassPermissions`, для read-only `dontAsk`; см. «Режим только из гнезда») | `harness.rs` `launch_for` | `ClaudeAdapterV2.ts:3358-3407` (`permissionModeForClaudeRuntimePolicy`), `:3417-3468` (`claudeRuntimeQueryPolicyForRuntimePolicy`, `installPermissionCallback`) |
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
- Шаги и текст подагента (`parent_tool_use_id`) проходят как обычные события с необязательным
  `parentItemId`; общий слот пишет его как `parentCallId` в существующие `assistant/message`,
  `tool/call`, `tool/result`. Запрос права сохраняет id дочернего шага и тот же id запроса.
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

Режим выбирает человек в окне; адаптер получает его только аргументом гнезда
`start_with_access(params, AccessMode)` (см. «Режим только из гнезда»; поле `ClaudeConfig.access`
убрано). Больше нигде он не выводится: ни из конфигурации плагина, ни из политики `session/create`,
ни из ответов CLI. `approval: never` с белым списком
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
- `ClaudeConfig::argv(policy, access, model, resume, native_id)` (и `ClaudeHarness::argv`) возвращает
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

## Режим только из гнезда (21.11, выравнивание по main d88aef04)

Факт o1: режим брался из `Config.access` плагина, а не из гнезда; методы контракта
`start_with_access`, `launch_flags`, `discover_with_status` не были переопределены (выбор человека
не доходил до флагов, `effective.flags` пусты, `full` из окна давал `POLICY_UNAVAILABLE`).

- Типа `Access` и поля `ClaudeConfig.access` больше нет. Режим — только аргумент
  `start_with_access(params, AccessMode)`, который гнездо передаёт по выбору человека
  (`hoh/executor/access/set`). `start_or_resume` = `start_with_access(params, DEFAULT_ACCESS)`
  (confirm). Отказ `approval: never` в confirm тоже по аргументу гнезда.
- `launch_flags(policy, access)` (метод контракта и функция крейта) = `permission_args(launch_for(…))`,
  та же функция, что вставляет флаги в argv между потоковыми флагами и `--model`/идентификаторами:
  `effective.flags` окна = фактический argv.
  - confirm: `--permission-mode default --setting-sources "" --permission-prompt-tool stdio`
    (+ `--tools …` для read-only, + `--disallowedTools WebFetch,WebSearch` без сети);
  - full: `--permission-mode bypassPermissions --setting-sources "" --permission-prompt-tool stdio
    --allow-dangerously-skip-permissions`;
  - full + read-only: `--permission-mode dontAsk --setting-sources "" --tools Read,…` (full не
    расширяет read-only).
- `discover_with_status`: та же разведка (`claude --version`, затем `initialize` пробы; файлы
  входа и токены не читаются) и `versionStatus` по `version.rs`: в диапазоне — `supported`; версия
  прочитана, но вне диапазона — `unsupported`; не установлен, версия не читается или `--version`
  падает — `unknown`.
- **`access` в конфигурации плагина принимается и игнорируется с предупреждением в журнал**
  (`… names an access mode (…): it is ignored …`). Почему не отказ разбора: ядро проверяет
  конфигурацию по схеме, и отказ уронил бы профиль окна из-за строки старого профиля (до этой правки
  поле было законным): человек остался бы без Claude совсем. Игнорирование никогда не расширяет
  права: значение не читает никто, кроме предупреждения. Любое другое неизвестное поле — по-прежнему
  отказ (`deny_unknown_fields`).

Тесты `tests/slot.rs`: Claude смонтирован плагином в ядре со слотом и вызывается как
`dyn ExternalHarness` из `ExternalHarnesses` (так его зовут окно и исполнитель гнезда); файл
пользуется только трейтом, поэтому собирается и против кода main.
- (а) `the_slot_mode_decides_even_when_the_configuration_says_full`: конфигурация `access: full`,
  гнездо confirm → `--permission-mode default … --permission-prompt-tool stdio`, без bypass;
  `run: echo hi` → `ApprovalRequested`; предупреждение в журнале. На коде main (подмена `src/` на
  d88aef04) падает: `left: Some("bypassPermissions") right: Some("default")`.
- (б) `full_from_the_slot_runs_full_whatever_the_configuration_says`: конфигурация `{}` и
  `access: confirm`, гнездо full → bypass, вопроса нет, команда выполнена (на main —
  `POLICY_UNAVAILABLE`).
- (в) `the_launch_flags_are_the_flags_the_process_gets`: 2 режима × 3 политики, argv заглушки =
  поток + `launch_flags` + `--session-id`; точные флаги трёх случаев. Юнит-тест
  `the_launch_flags_are_the_slice_of_the_argv…` — то же для `argv()` с `--model` и `--resume`.
- (г) `discover_with_status_places_the_version_against_the_checked_range`: 2.0.31 → supported
  (вход, модели, совпадает с `discover`); 3.1.0 и 1.0.128 → unsupported; нечитаемая версия и
  отсутствие программы → unknown.
- Тесты файла идут по одному (общий замок): каждый пишет скрипт-запускатель, и форк соседнего
  теста в момент записи дал бы «text file busy».
- Новые dev-зависимости (крейты рабочего пространства, уже в `Cargo.lock`): hoh-agent,
  hoh-session, hoh-session-projection, hoh-user-approval, hoh-user-questions.

Вопрос о версии вне диапазона закрыт: окно показывает `unsupported` с текстом «Версия {version} не
поддерживается: чат с ней не начнётся» (`window.rs`), старт такую версию отклоняет
(`UNSUPPORTED_VERSION`, условие CEO), `detail` разведки говорит «не поддерживается». Текст окна и
поведение старта совпадают.

### Дополнение (аудит стыков гнезда, CEO 21.11)

От аргумента гнезда, а не от конфигурации, зависят все производные режима.
`launch_for(&params.policy, access)` решает:
- запрет сети (`--disallowedTools WebFetch,WebSearch`);
- ветку read-only (`default` + callback или `dontAsk`);
- наличие канала вопросов.

Признак `bypass` сессии (`settings_of(&config, launch.bypass)`) берётся из того же `launch`. От него
зависит автоотказ проверки удаления (`session.rs` `is_removal_safety_refusal`), поэтому и он идёт от
режима гнезда.

Тесты `tests/slot.rs`: конфигурация везде `access: full`, гнездо confirm.
- (1) `confirm_from_the_slot_keeps_the_network_tools_off…` — сеть закрыта → `--disallowedTools
  WebFetch,WebSearch`. На main: `left: None`.
- (2) `confirm_from_the_slot_keeps_a_read_only_session_asking…` — read-only → `default`,
  `--permission-prompt-tool stdio`, `--tools Read,…`, без `dontAsk`. На main: `left: Some("dontAsk")`.
- (3) `under_confirm_from_the_slot_the_removal_check_is_a_question…` — `Dangerous rm` →
  `ApprovalRequested` с командой, без автоотказа. На main: автоотказ, тест падает.

**MCP пользователя при `--setting-sources ""`.** Сам по себе этот флаг MCP пользователя не
отключает (код CLI в `refs/` нет, вывод по T3 и SDK):
- SDK, `claude_agent_sdk/types.py:2721-2726`: `setting_sources` управляет только файлами
  `settings.json` (user, project, local).
- `types.py:2439-2444`: отдельный `strict_mcp_config` (`--strict-mcp-config`) отбрасывает MCP из
  `.mcp.json`, настроек user/global и плагинов.
- Свои e2e-тесты SDK для изоляции передают оба флага (`e2e-tests/test_run_end.py:34-35`,
  `test_verbatim_prompts.py:43-44`).
- T3, `ClaudeProvider.ts:287-297`: проба исключает MCP файловой системы через
  `mcpServers: {}` + `strictMcpConfig: true`. Флаг не передаётся, если `managed-mcp.json` его
  запрещает. MCP claude.ai отключается переменной `ENABLE_CLAUDEAI_MCP_SERVERS=false` («discovered
  outside filesystem config»).
- Сессии T3 `strictMcpConfig` не передают.

У нас `--strict-mcp-config` стоит у пробы и в режиме `isolated` (сессии `personal` его не
передают). В confirm вызов инструмента MCP идёт через
`can_use_tool` (правил разрешения нет: настройки не загружаются), в full он выполняется без вопроса.
Проверить вживую можно по `system/init.mcp_servers[].source`.

**Модель при `default`.** Прочитать можно:
- `system/init.model` (SDK `types.py` `SystemInitData.model`). Приходит в начале хода, а не при
  старте процесса: T3 `isClaudeTurnStartMessage`, `ClaudeAdapterV2.ts:3725-3727`.
- `assistant.message.model` корневых сообщений, кроме `<synthetic>`: T3 `executedModel`,
  `:10770-10775`, отдаёт его в ход, `:6342`, `:6984-6986`.
- Ключи `result.modelUsage`.

Ответ `initialize` даёт только список с `default` и не говорит, во что тот развернулся. Отдать
модель гнезду нельзя без правки контракта: в `StartResult` и событиях `HarnessEvent` поля модели нет.

## Личные настройки человека (решение основателя и CEO 21.11)

**Решение 21.11 (основатель и CEO), уточнено CEO ~02:00:** по умолчанию действуют личные настройки
человека, но «как в T3» = только `user` (см. раздел «"Как в T3" = только `user`» ниже). Режим
«изолированно» остаётся параметром. Умолчание задано в одном месте: `DEFAULT_SETTING_SOURCES =
SettingSources::Personal` (`harness.rs`). Значение берётся из конфигурации плагина в профиле:
`settingSources: "personal" | "isolated"` (`ClaudeConfig.setting_sources`). Workspace и чат его не
задают, режим доступа на него не влияет. С этого раунда `--setting-sources ""` из прошлых раундов
стоит только в «изолированно».

- `personal`: `--setting-sources user` (решение CEO 21.11 ~02:00; у первой версии раунда было
  `user,project,local`, как T3 читает сам, `ClaudeAdapterV2.ts:5595`, `:5628`). Причина:
  настройки рабочей папки (`project`, `local`) не должны отключать режим «Спрашивать разрешение»:
  файл мог положить чужой человек или сама модель. Флаг передаётся явно: по `claude --help` он грузит только
  перечисленные источники, и окно видит выбор в `effective.flags`. Без флага CLI грузит все
  источники (SDK `types.py:2728`, «matches CLI defaults»). Что передаёт TS SDK T3 по умолчанию, по
  `refs/` не установлено. MCP и хуки человека не отключаются.
- `isolated`: `--setting-sources "" --strict-mcp-config`. По help: «Only use MCP servers from
  --mcp-config, ignoring all other MCP configurations». `--mcp-config` не передаётся.
- Флаги стоят сразу после `--permission-mode` и входят в `launch_flags`, поэтому
  `effective.flags` окна = argv.
- `--safe-mode` не используется. Help 2.1.329 говорит: «Auth, model selection, built-in tools and
  plugins, and permissions work normally». О `-p` и stream-json там ничего нет, так что условие
  ведущего не доказано. `--bare` не годится: OAuth не читается.
- Проба (discover) не изменилась: `--setting-sources "" --strict-mcp-config --mcp-config {}`.
  Разведка не запускает хуки человека; T3 для пробы ставит `disableAllHooks`.

Что из личного действует в confirm:
- `permissions.allow` (в `personal` только user; project и local не грузятся). В `personal` разрешённое правилом выполняется **без
  вопроса окна**: «can_use_tool… is *not* invoked for… `permissions.allow` rules in settings»
  (SDK `types.py:2584-2597`, `:2310`). В `isolated` правила не загружаются.
- Хуки. `PreToolUse` с ответом allow тоже минует вопрос (`types.py:2595-2597`). Хуки из файлов
  настроек в `isolated` не грузятся. Хуки установленных плагинов в `isolated` не установлены.
- MCP. Серверы человека в `personal` стартуют сами, а вызов их инструментов в confirm — вопрос, если
  нет allow-правила. Коннекторы claude.ai T3 отключает отдельно: `ENABLE_CLAUDEAI_MCP_SERVERS=false`
  (`ClaudeProvider.ts:296-297`). У нас переменная стоит только у пробы (`harness.rs`, `spec(..,
  probe: true)`); в запуске хода, в том числе в `isolated`, её нет. Покрывает ли коннекторы claude.ai
  `--strict-mcp-config` — открыто: решает строка `mcp.claudeai.isolated` живого прогона (решение
  CEO).
- Управляемые (managed) настройки действуют всегда, флагами не отключаются (help). На этой машине
  их нет (ведущий).
- `permissions.defaultMode` человека. По коду CLI 2.1.329 он учитывается, только когда режим не
  задан флагом: явный `--permission-mode` важнее. Подтвердить живым прогоном.

Живая проверка: в `system/init` есть `mcp_servers[].source`, `plugins`, `skills`.

Тесты:
- `tests/slot.rs`: `the_setting_sources_of_the_profile_reach_the_command_line` (оба значения, оба
  режима); `the_window_shows_the_setting_sources_the_session_starts_with` — реальное окно
  (`access/get` через SDK-сервер): `effective.flags` содержит выбор и равны срезу argv;
  `the_launch_flags_are_the_flags_the_process_gets` — для обоих значений.
- Юнит-тест `launch_flags` = срез argv, 2 значения × 3 политики × 2 режима × resume;
  `the_setting_sources_are_named_in_the_configuration`.

Фактическая модель: в контракте нет места (ни поля `StartResult`, ни события `HarnessEvent`), см.
отчёт для o1.

## «Как в T3» = только `user` (решение CEO 21.11 ~02:00)

*Отменено 22.11 (HOH-40, основатель: «так же, как в T3»): см. раздел «HOH-40» ниже.*

**Решение:** `personal` передаёт `--setting-sources user`. Настройки рабочей папки (`project`:
`.claude/settings.json`, `local`: `.claude/settings.local.json`) не действуют.

**Причина:** режим «Спрашивать разрешение» не должен отключаться файлом из папки. Правило `allow`
или хук там мог положить чужой человек или сама модель. Оба ревью кандидата cand/i4 сочли это
блокирующим.

- Источники названы в одном месте: `PERSONAL_SETTING_SOURCES = "user"` (`harness.rs`), его берёт
  `SettingSources::args`. `DEFAULT_SETTING_SOURCES = Personal` не менялся, «изолированно» тоже.
- **Вариант Б** (как T3 читает сам, `ClaudeAdapterV2.ts:5595`): смена `PERSONAL_SETTING_SOURCES`
  на `"user,project,local"`, плюс тесты, где это значение записано буквально.
- Следствие по SDK (`types.py:2731-2732`, «Must include "project" to load CLAUDE.md files»):
  `CLAUDE.md` проекта при `user` не грузится, личный `~/.claude/CLAUDE.md` — по источнику `user`.
  Подтвердить живым прогоном (`get_context_usage.memoryFiles`).
- Не установлено: грузятся ли MCP-серверы проекта (`.mcp.json`) без источника `project`. Проверка —
  `system/init.mcp_servers[].source`.
- Тесты:
  - argv `personal` = `--setting-sources user` (`tests/claude.rs`
    `the_setting_sources_are_the_same_for_every_policy`): `project` и `local` не передаются,
    флаг один;
  - `launch_flags` = argv (юнит-тест и `tests/slot.rs`), окно `effective.flags`
    (`the_window_shows_the_setting_sources…`).

## i8b (итоговое ревью Fable cand/i8, 144603)

- «Стоп» во время запуска: у `ClaudeSession` есть `Drop` — последний дескриптор, брошенный без остановки (слот отменил `start` до ответа на `initialize`), убивает группу процесса (`Supervisor::abandon`), как сторож `Owner` у `StdioHarness`. `read_loop` держит `Inner`, а не сессию, и процесс больше не переживает отмену. Тест: `apps/hoh/tests/window_executor.rs` `t9_…` (поддельный claude не отвечает на `initialize`, отмена, pid мёртв; до правки падал).
- `--resume` получает только id формы UUID (`is_uuid`); сохранённый id другой формы не попадает в argv (его можно принять за ключ) и даёт `SESSION_NOT_FOUND`, после чего слот начинает новую сессию. Тест `only_a_session_id_of_the_cli_form_is_resumed`; `a_session_resumes_by_its_native_id_and_a_missing_one_is_reported` берёт несуществующий id правильной формы, чтобы отказ по-прежнему шёл от CLI.

## i10b (живой пилот 22.11)

- `ClaudeHarness::stop_ends_process() = true`: «Стоп» завершает CLI и всё под ним, следующий ход — `--resume` того же разговора (подробно — PORT_NOTES `hoh-external-harness`, i10b). Нативное прерывание (`ClaudeSession::interrupt`, сторож неподтверждённого прерывания) остаётся в крейте как у T3, но слот его для Claude больше не зовёт.

## i12b (предварительная живая проверка 939feabd, `qa/pilot-pre-20261122-1744`, s3)

- **Чужой `result` не закрывает ход.** Пилот: «Стоп» во время команды; в беседе оставался фоновый `sleep 120`, который ушёл вместе с процессом. Следующее сообщение запустило claude с `--resume`, и CLI сначала ответил на уведомление о потерянной фоновой команде — `result` без хода (`origin: task-notification`, `num_turns: 0`, пустой). Сессия приняла его за ответ, ход закрылся сразу (`completed`, без текста). Окно ушло в «готов», и его «Стоп» уже ничего не слал (поэтому нет и пометки «Вы остановили ответ»). CLI отвечал на сообщение между ходами, и следующий ход ушёл в тот же процесс.
- Теперь, как в T3 (`ClaudeAdapterV2.ts:4688-4725`, `:12196-12310`):
  - каждый запрос уходит со свежим `uuid`;
  - `result` не закрывает ход, если он называет другие запросы (`user_message_uuids` / `user_message_uuid` без нашего);
  - не закрывает и пустой результат уведомления без хода (`origin.kind = task-notification`, `success`, `num_turns: 0`, пустой `result`);
  - не закрывает и результат с `origin.kind` не `human` у CLI, который уже показал эхо запросов (эхо в `result` или `command_lifecycle` с нашим `uuid`). Без эха такое происхождение не доказывает чужой ход: CLI может влить запрос в свой ход.
  - Такой `result` ничего не меняет в состоянии хода.
- Отличие от T3: кадры чужого хода (текст, шаги) не откладываются, а идут в текущий ход; T3 буферизует их (`heldRootFrames`, «wake»).
- Тесты: `tests/claude.rs` `a_result_of_another_turn_does_not_end_the_prompts_turn` (заглушка: `wake-first` — пустой результат уведомления до ответа; `wake-turn-first` — `command_lifecycle`, ход CLI с `origin`, результат с чужим `uuid`, затем ответ с нашим). Сквозной — `apps/hoh/tests/window_executor.rs` `t16_…`: «Стоп» посреди хода; claude, возобновлённый с `--resume`, сначала отдаёт результат уведомления; «Стоп» во время команды следующего хода закрывает ход как `aborted`, убивает claude и команду (по PID), третий ход — новый запуск с `--resume`. До правки оба падали (`completed` сразу).

## HOH-40 (решение CEO 22.11 21:50): источники настроек и уровень усилия, как в T3

- **Источники.** `PERSONAL_SETTING_SOURCES = "user,project,local"`: `personal` (умолчание) передаёт
  `--setting-sources user,project,local`. Это вариант Б из раздела «"Как в T3" = только `user`»:
  решение CEO 21.11 отменено словами основателя «так же, как в T3». T3 своему SDK источники не
  передаёт (`buildClaudeQueryOptions`, `ClaudeAdapterV2.ts:2283-2367`, поля `settingSources` нет),
  а сам, когда ему надо знать, чему подчиняется сессия, читает `["user","project","local"]`
  (`ClaudeAdapterV2.ts:5595`, `:5628`, `ClaudeDriver.ts:164`; с собственным домом
  `CLAUDE_CONFIG_DIR` — только `project`, `local`; своего дома у нас нет). Без флага CLI грузит все
  три (SDK `types.py:2728`, «matches CLI defaults»). Флаг по-прежнему передаётся явно: окно видит
  источники в `effective.flags`, и подпись «Настройки Claude из папки проекта не применяются»
  (`ui/src/lib/executor.mjs`, `projectSettingsIgnored`) пропадает сама.
- Что ещё могло бы гасить проектное (`claude --help` 2.1.x): `--strict-mcp-config`, `--mcp-config`,
  `--bare`, `--safe-mode`, `--restricted`, `--disable-slash-commands`. В запуске сессии `personal`
  ни одного из них нет. `-p` (stream-json, как у SDK T3) пропускает диалог доверия к папке, как и у
  T3. Защита окна не изменилась: режим по-прежнему только из окна (`--permission-mode`,
  `--permission-prompt-tool stdio`, `--tools`/`--disallowedTools`). Но теперь `allow`-правила и хуки
  папки (`.claude/settings*.json`) отвечают раньше вопроса окна, как в терминале человека и в T3.
  Это и было причиной решения 21.11.
- Проба `discover` не менялась (`--setting-sources "" --strict-mcp-config --mcp-config {}`, во
  временной папке). T3 у пробы читает все три источника с `disableAllHooks`, но проба у нас нужна
  только для входа и списка моделей, проект в ней не участвует.
- **Уровень усилия.** `EFFORT_LEVELS = low, medium, high, xhigh, max` (`claude --help`: «--effort
  <level> Effort level for the current session (low, medium, high, xhigh, max)»; T3
  `CLAUDE_EFFORT_LEVELS`, `ClaudeModelCatalog.ts:366`). `ClaudeHarness::effort_levels` отдаёт их
  слоту, `StartParams.effort` идёт в argv как `--effort <level>` после `--model` (T3 отдаёт SDK
  опцию `effort`, `ClaudeAdapterV2.ts:2296`). Уровень не из списка отклоняется до запуска
  (`code::PROTOCOL`): слот такой не передаёт. `--effort` не входит в `launch_flags`: от него не
  зависит, что сессия делает без вопроса.
- Отличие от T3. T3 сужает уровень по каталогу модели (`normalizeClaudeCatalogEffort`, у модели может
  не быть `xhigh`/`max`). Живую смену уровня у T3 делает `applyFlagSettings` (`effortLevel`), а
  новый процесс он запускает только когда уровень снимают или меняется thinking
  (`canApplyClaudeSelectionLive`, `:2067`). У нас любая смена уровня — новый процесс с
  `--resume` (решение CEO), каталога уровней по моделям нет.
- Тесты: `the_effort_follows_its_switch`, `a_level_claude_does_not_have_starts_nothing`;
  `launch_flags` = срез argv (с `--effort high`); `user,project,local` в `tests/claude.rs`,
  `tests/slot.rs`; сквозной `apps/hoh/tests/window_executor.rs` `t18_…` (PORT_NOTES
  `hoh-external-harness`, HOH-40).

## HOH-41: четыре режима (i7, решение CEO 23.11 00:07)

- `launch_for` соответствует T3 (`ClaudeAdapterV2.ts:3395-3404`, `permissionModeForClaudeRuntimePolicy`):

  | режим окна | `--permission-mode` |
  |---|---|
  | `confirm` | `default` |
  | `acceptEdits` | `acceptEdits` |
  | `auto` | `auto` |
  | `full` | `bypassPermissions` + `--allow-dangerously-skip-permissions` |

  Оба значения есть в `claude --help` 2.1.329 (choices: acceptEdits, auto, bypassPermissions, manual, dontAsk, plan). `default` в этом списке нет, но CLI его принимает и сам называет режим `default` (снимок q1 22.11).
- Остальные ключи новых режимов как у `confirm`: `--permission-prompt-tool stdio`, без bypass, `--disallowedTools WebFetch,WebSearch` (сеть закрыта). Сеанс только для чтения под ними остаётся запуском `confirm` только для чтения (`default` + `--tools`).
- Тест — `each_mode_of_the_window_is_its_permission_mode`.
