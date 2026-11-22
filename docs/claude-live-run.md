# Живой короткий прогон Claude (HOH-18): режимы «С подтверждением» и «Полный доступ»

`scripts/claude-live-smoke.sh` запускает собранный `hoh` с гнездом внешнего исполнителя
(`hoh-external-harness`) и пакетом Claude (`hoh-harness-claude`) на настоящем CLI `claude`.
Одна типовая задача решается дважды, в двух режимах доступа, которые человек выбирает в окне
(решение CEO 20.11):
- `confirm` («С подтверждением») — вопрос на каждую команду, правку и сеть;
- `full` («Полный доступ», как в T3) — без вопросов и без песочницы.

Задача: прочитать `first.txt`, дописать строку в `notes.txt`, выполнить `wc -l first.txt`.

Затем в режиме `confirm` проверяются личные настройки CLI (CEO 21.11): разрешающие правила, MCP и
хуки. Каждая строка гоняется в обоих значениях источников настроек пакета Claude — «как в T3» и
«изолированно» (раздел «Личные настройки CLI в confirm»).

И проектные инструкции и настройки папки задачи: `CLAUDE.md`, `.claude/rules`, навыки, хуки и
вложенный `CLAUDE.md`. «Как в T3» теперь означает только область `user` (решение CEO 21.11 ~02:00).
Основатель работает с проектными `CLAUDE.md` и правилами, поэтому нужен факт, грузит ли их CLI
(раздел «Проектные настройки при «как в T3»»).

Запускать только по команде ведущего, после чистого ревью обоих крейтов. До этого допустим
только `--dry-run`.

## Откуда берётся режим

Режим исполнителя меняет только человек, командой окна `hoh/executor/access/set`. Это
привилегированный метод: SDK-сервер просит одобрения хоста. Выбор сохраняется рядом с
профилем движка в `<DSH_HOME>/profiles/<профиль>/external-harness.json`, например
`{"access": {"claude": "full"}}`. Каждый запуск движка на этом профиле начинает с него.

В `headless` окна нет, поэтому скрипт перед каждой задачей сам пишет этот файл в профиль
прогона: так выглядело бы сохранённое действие человека. Ни чат, ни модель, ни файл в
рабочей папке, ни параметры сессии режим не меняют, это проверяют тесты гнезда.

`--approval-policy never` больше не используется.

## Что нужно до запуска

1. **Регистрация в `hoh` — сделано.** В `apps/hoh/Cargo.toml` три зависимости
   (`hoh-destructive-classifier`, `hoh-external-harness`, `hoh-harness-claude`), в
   `apps/hoh/src/registry.rs` четыре строки реестра:
   ```rust
   .add::<hoh_destructive_classifier::DestructiveClassifierPlugin>("hoh:destructive-classifier")
   .add::<hoh_external_harness::ExternalHarnessPlugin>("hoh:external-harness")
   .add::<hoh_harness_claude::HarnessClaudePlugin>("hoh:harness-claude")
   .add::<hoh_external_harness::ExternalHarnessWindowPlugin>("hoh:external-harness-window")
   ```
   - Окно: все четыре строки стоят в профиле окна (`apps/hoh/assets/bundle/window/cordis.patch.yml`,
     id `destructive-classifier`, `external-harness`, `harness-claude`,
     `external-harness-window`). Движок окна отвечает на `hoh/executor/discover`, `select`,
     `access/get` и `access/set`; это проверяет `apps/hoh/tests/window_executor.rs` на
     поддельном `claude`.
   - `headless`: состав по умолчанию не менялся. Скрипт подключает три строки своим наложением
     (`--patch`), строка окна там не нужна.
2. **Пакет Claude из хода s4 21.11** (access-from-nest, access-add, personal-settings): режим
   только из гнезда (`start_with_access`, `launch_flags`, `discover_with_status`) и источники
   настроек `settingSources`. В main d88aef04 этого ещё нет. На пакете из main:
   - `full` отказывает с `policy-unavailable` («this harness has no full-access launch»);
   - строки личных настроек не стартуют: поле `settingSources` в конфигурации `harness-claude`
     пакет из main не знает и отказывает в разборе;
   - `<прогон>.flags` не совпадут: там `--setting-sources ""` вместо выбранного значения.

   Скрипт ждёт для `personal` флаг `--setting-sources user` (s4 меняет на него после cand/i4). На
   пакете cand/i4 там `user,project,local`, и `<прогон>.flags` строк `.t3`, `confirm` и `full` не
   пройдут.
3. **Собранный `hoh`**: `$CARGO_TARGET_DIR/debug/hoh` (по умолчанию `target-o1`). Другой путь —
   `--hoh PATH`, сборка — `--build` (`timeout 1800 cargo build --offline -p hoh`).
4. **CLI `claude` версии 2.x**, в `PATH` или `--claude PATH`, вход в нём уже выполнен
   пользователем. Скрипт ключи и `~/.claude` не читает и не копирует.

## Как запускать

```bash
scripts/claude-live-smoke.sh --dry-run              # план: пути, наложения, фикстуры, команды, флаги
scripts/claude-live-smoke.sh                        # оба режима, 6 строк личных и 10 проектных настроек
scripts/claude-live-smoke.sh --only confirm         # один прогон: confirm, full или имя строки
scripts/claude-live-smoke.sh --only personal        # только шесть строк личных настроек
scripts/claude-live-smoke.sh --only project         # только десять строк проектных настроек
scripts/claude-live-smoke.sh --only confirm-project.rules-t3
scripts/claude-live-smoke.sh --only confirm-hooks-t3
scripts/claude-live-smoke.sh --build --timeout 600
```

Прогоны: `confirm`, `full`, `confirm-<allow|mcp|hooks>-<t3|isolated>`,
`confirm-project.<claude_md|rules|skills|hooks|nested_claude_md>-<t3|isolated>`. `--dir PATH` — другая
родительская папка прогона (только символы `A-Za-z0-9_./-`: пути попадают в JSON фикстур).

Проверка самого скрипта без настоящего CLI — заглушка
`scripts/tests/claude-live-smoke/fake-claude` (повторяет только то, что проверяет отчёт: вопрос
`can_use_tool`, правила allow, хуки PreToolUse/PostToolUse, сервер из `.mcp.json`, и как их
отключают `--setting-sources` и `--strict-mcp-config`). При области `project` в источниках она
показывает и проектные: метки `CLAUDE.md` и `.claude/rules/*.md` в ответе, метку `sub/CLAUDE.md`
после чтения файла из `sub/`, навык в `system/init.skills`, хуки `SessionStart` и
`UserPromptSubmit`. При `user` и `""` она их не показывает. Это только проверка логики скрипта;
грузит ли их настоящий CLI, покажет живой прогон.

```bash
scripts/claude-live-smoke.sh --hoh <hoh с пакетом s4> --dir /dev/shm/hoh/live-test \
  --claude "$PWD/scripts/tests/claude-live-smoke/fake-claude" --timeout 60
```

С `FAKE_CLAUDE_IGNORES_SOURCES=1` заглушка грузит настройки папки, что бы ей ни сказали:
строки `confirm.personal.allow.isolated` и `.hooks.isolated` должны упасть, а все строки
`confirm.project.*` показать «загружено: да».

Коды возврата:
- `0` — все проверки прошли;
- `1` — хотя бы одна проверка не прошла;
- `2` — неверные флаги, нет `hoh` или `claude`, не прошла сборка;
- `3` — в `hoh` нет плагинов Claude, ничего не запускалось.

## Что делает

- Папка прогона: `/mnt/toshiba/alexandrie/hoh/scratch-live/<время>/`:
  - `<прогон>/` — cwd задачи, в нём `first.txt` (метка `HOH-LIVE-…` в первой строке),
    `notes.txt` и фикстуры строки (`.claude/`, `.mcp.json`);
  - `logs/` — для каждого прогона `*.out` (события `--json`), `*.err`, `*.code`, `*.cmd` и
    `*.argv` (командная строка процесса `claude`, снятая из `/proc`);
  - `dsh-home/` — данные `hoh`: профиль `headless` с `external-harness.json`, журналы сессий;
  - `patches/<прогон>.yml` — наложение профиля;
  - `fixtures/` — `hook.sh`, `mcp_server.py` и прокладка `claude-tap` с `tap.json` (вне cwd
    задачи);
  - `markers/<прогон>/` — метки, которые оставляют хуки и сервер MCP, и `system-init.jsonl`
    (строки `system/init` CLI, их копирует прокладка в проектных строках).
- Изоляция: `DSH_HOME` и `DSH_AGENTS_HOME` указывают в папку прогона. `HOME` не меняется: по
  нему CLI находит свой вход, а подмена разлогинила бы его, копировать же ключи запрещено.
  `TMPDIR` — `/dev/shm/hoh`.
- Профиль `headless` с наложением:
  - строки `hoh:destructive-classifier`, `hoh:external-harness` и `hoh:harness-claude`
    (`executable` — найденный `claude`, в проектных строках — прокладка `fixtures/claude-tap`
    перед ним; в строках личных и проектных настроек ещё `settingSources`:
    `personal` для `.t3`, `isolated` для `.isolated`; в `confirm` и `full` значение не задано,
    действует умолчание пакета `personal`);
  - `agent-default-model` переключён на `provider: claude`, `model: default`;
  - выключены `session-title-llm` и `session-telemetry-otel`, плюс `DSH_TELEMETRY_DISABLED=1`.
- Ответов на вопросы в `headless` нет: окна нет, `--approval-policy` не задан. Каждый вопрос
  заканчивается `unavailable`, гнездо отвечает CLI `deny`. Поэтому в `confirm` правка и
  команда не выполняются, а число вопросов — это сколько раз CLI спросил на этой задаче.
- Каждая задача ограничена `timeout` (по умолчанию 300 с, затем `KILL` через 15 с).
- После задач скрипт находит оставшиеся процессы с cwd внутри папки прогона и шлёт им `TERM`.
  Чужие процессы он не трогает.
- Сеть: сам `hoh` в сеть не ходит (строки выше выключены). CLI ходит в Anthropic. В `confirm`
  его веб-инструменты выключены, в `full` — нет. Изоляции сети на уровне ОС нет.

## Флаги CLI по режимам

Флаги выставляет `hoh-harness-claude` (s4): `launch_for` и `session_args` в
`crates/hoh-harness-claude/src/harness.rs`. Гнездо передаёт ему выбранный режим через
`start_with_access(params, AccessMode)` и показывает окну `launch_flags(policy, AccessMode)`
(`hoh/executor/access/get` → `effective.flags`). Ожидаемые флаги (порядок не сравнивается):

| Режим | Флаги режима |
|---|---|
| `confirm` | `--permission-mode default --permission-prompt-tool stdio --disallowedTools WebFetch,WebSearch` |
| `full` | `--permission-mode bypassPermissions --permission-prompt-tool stdio --allow-dangerously-skip-permissions` |

| Источники настроек (`settingSources`) | Добавляет |
|---|---|
| `personal` — «как в T3»: только `user` (CEO 21.11 ~02:00), умолчание пакета (`DEFAULT_SETTING_SOURCES`) | `--setting-sources user` |
| `isolated` — «изолированно» | `--setting-sources "" --strict-mcp-config` |

- Значение источников задаёт профиль (конфигурация `harness-claude`), не чат и не папка.
- `--disallowedTools WebFetch,WebSearch` в `confirm` есть всегда: гнездо в этом режиме
  запрещает сеть (`launch_policy`), без сети пакет выключает оба веб-инструмента.
- В `full` `--permission-prompt-tool stdio` остаётся: то, что CLI не разрешает даже в
  `bypassPermissions` (защищённые пути), приходит вопросом к человеку.

Проверка `<прогон>.flags` сравнивает их с флагами живого процесса из `logs/<прогон>.argv`.
Командная строка там записана по словам в кавычках оболочки, так что пустое значение
`--setting-sources` сохраняется.

## Личные настройки CLI в confirm

Настройки человека (`~/.claude`, `~/.claude.json`) скрипт не читает и не меняет. Проверка идёт на
собственных фикстурах в cwd задачи: области `project` и `local`, которые CLI грузит так же, как
`user`, если флаги их не отключили. Каждая строка гоняется в `confirm` дважды: `.t3`
(`settingSources: personal`) и `.isolated`.

| Строка | Фикстура в cwd | `.isolated`: PASS, если | `.t3`: факт |
|---|---|---|---|
| `allow` | `.claude/settings.json`: `permissions.allow` `Bash(wc:*)` (область project); `.claude/settings.local.json`: `Edit`, `Write` (область local) | спрошены и правка (`edit_file`), и команда (`bash`); `notes.txt` не изменён, команда не выполнена | выполнены ли команда и правка без вопроса окна — видно, какая область сработала |
| `mcp` | `.mcp.json`: сервер `hoh-live` (`fixtures/mcp_server.py`, stdio) с инструментом `hoh_live_write`, который пишет метку; `.claude/settings.local.json`: `enableAllProjectMcpServers`, `enabledMcpjsonServers` | метки вызова нет и (CLI не запрашивал список инструментов или вызов спрошен окном) | запущен ли сервер, запрошен ли список, спрошен ли вызов, есть ли метка |
| `hooks` | `.claude/settings.json`: на `PreToolUse` (`Bash\|Edit\|Write\|MultiEdit`) хук-метка и хук, отвечающий `permissionDecision: allow`; на `PostToolUse` хук-метка | меток нет, вопрос окна задан, `notes.txt` не изменён | сработали ли хуки, выполнены ли правка и команда без вопроса |

- Задача строки `mcp` — вызвать `hoh_live_write`; у `allow` и `hooks` — обычная задача.
- `.isolated` — проверка (`PASS`/`FAIL`). `.t3` — сбор факта (`INFO`): «действие выполнено без
  вопроса окна: да/нет» и что именно произошло. При «как в T3» = только `user` фикстуры областей
  `project` и `local` грузиться не должны; прогон это подтверждает или опровергает.
- В `.t3` действуют и настоящие настройки человека из `~/.claude` (это и есть «как в T3»), так
  что факт складывается из фикстуры и личных настроек. Отчёт перечисляет вопросы каждого
  прогона.
- `managed` (`INFO`): есть ли `/etc/claude-code/managed-settings.json` и
  `/etc/claude-code/managed-mcp.json`. Проверяется только наличие, содержимое не читается.
  Управляемые настройки флагами не отключаются.
- Модель по ответу CLI: CLI сообщает её в `system/init.model`, но в контракте гнезда нет поля,
  куда её передать (s4 предложил `HarnessEvent::ModelReported`). Пока отчёт пишет, что модель не
  передаётся.

## Проектные настройки при «как в T3»

Те же условия: `confirm`, фикстуры в cwd задачи, `~/.claude` не читается и не меняется. Каждая
строка — отдельный прогон, дважды: `.t3` (`--setting-sources user`) и для сравнения `.isolated`.
Каждая даёт `INFO confirm.project.<строка>.<t3|isolated>: загружено: да/нет; <доказательство>`.

| Строка | Фикстура в cwd | Задача | «Загружено: да», если |
|---|---|---|---|
| `project.claude_md` | `CLAUDE.md`: «In every answer, add the word HOH-MARK-PROJ at the end of your reply.» | прочитать `first.txt`, ответить его первой строкой | `HOH-MARK-PROJ` в финальном тексте (`final.text`) |
| `project.rules` | `.claude/rules/hoh.md`: то же с `HOH-MARK-RULES` | та же | `HOH-MARK-RULES` в финальном тексте |
| `project.skills` | `.claude/skills/hoh-live-skill/SKILL.md` (`name: hoh-live-skill`) | та же | имя `hoh-live-skill` в `system/init.skills` (или в `system/init.slash_commands`) |
| `project.hooks` | `.claude/settings.json`: хуки `SessionStart` и `UserPromptSubmit`, пишущие файлы-метки | та же | есть метка `markers/<прогон>/hook-start` или `hook-prompt` |
| `project.nested_claude_md` | `sub/CLAUDE.md` с `HOH-MARK-NESTED`, `sub/note.txt` (строка `SUB-<метка прогона>`) | прочитать `sub/note.txt`, ответить его первой строкой | `HOH-MARK-NESTED` в финальном тексте |

- Доказательство в строке отчёта: финальный текст (до 200 символов) или что нашлось в
  `system/init` и в метках. У `nested_claude_md` ещё «`sub/note.txt` прочитан: да/нет» (его
  строка в ответе): без чтения вложенный `CLAUDE.md` не грузится, и «нет» тогда ничего не значит.
- `system/init` читает только пакет Claude, в журнал сессии он не попадает. Поэтому в проектных
  строках `executable` — прокладка `fixtures/claude-tap`. Она запускает настоящий `claude` с теми
  же аргументами, stdin и stderr, передаёт stdout построчно и копирует только строки
  `system/init` в `markers/<прогон>/system-init.jsonl`. Отчёт не печатает имена навыков
  человека, только их число и есть ли среди них `hoh-live-skill`.
- В `.t3` CLI грузит и `~/.claude/CLAUDE.md` и навыки человека (область `user`). Метки и имя
  навыка есть только в фикстурах, так что на факт это не влияет.
- В отчёте после перечня вопросов — таблица «строка × t3/isolated» со значениями «да/нет» («—»,
  если прогон не запускался).

## Проверки

Источники: строки `--json` в `logs/*.out` (`session`, `status`, `tool_call`, `tool_result`,
`final`), журнал сессии `dsh-home/sessions/**.jsonl[.zstd]`, найденный по `sessionId`, и
`logs/*.argv`.

| Проверка | Ожидается |
|---|---|
| `version` | `claude --version` ≥ 2.0.0 и < 3.0.0 (тот же диапазон, что `version.rs`) |
| `managed` (INFO) | наличие управляемых настроек CLI |
| `<прогон>.bound` | в журнале `plugin:external-harness/session` с `harness: "claude"`: ход шёл через Claude, а не через цикл агента |
| `<прогон>.turn` | `final.reason.kind == "completed"`, код `hoh` 0 |
| `<прогон>.flags` | флаги процесса `claude` совпадают с таблицами выше |
| `confirm.asked` | не меньше двух `approval/asked`: правка `notes.txt` и команда |
| `confirm.denied` | ни одного `approval/decided` с `allowed-once` |
| `confirm.unchanged` | `notes.txt` не изменён |
| `full.questions` | `approval/asked` нет ни одного |
| `full.edited` | в `notes.txt` появилась строка `edited` |
| `secrets` | ни в журналах сессий, ни в `logs/*` нет форм ключей: `sk-ant-`, `sk-…`, `ghp_`, `github_pat_`, `AKIA`, `AIza`, `xox?-`, JWT, `PRIVATE KEY`, `Bearer <длинный>`, `"accessToken"`/`"apiKey"` со значением. В отчёте только файл и строка, само значение не печатается |
| `confirm.personal.<allow\|mcp\|hooks>.isolated` | см. «Личные настройки CLI в confirm» |
| `confirm.personal.<allow\|mcp\|hooks>.t3` (INFO) | факт: «действие выполнено без вопроса окна: да/нет» |
| `confirm.project.<claude_md\|rules\|skills\|hooks\|nested_claude_md>.<t3\|isolated>` (INFO) | факт: «загружено: да/нет» и доказательство (раздел «Проектные настройки при «как в T3»») |
| `leftovers` (WARN) | после задач процессов прогона не осталось (в том числе сервера MCP фикстуры) |

## Отчёт прогона

Отчёт печатается и сохраняется в `report.txt` папки прогона. В нём:
- строка на каждую проверку (`PASS`, `FAIL` или `WARN`) и на каждый факт (`INFO`);
- `в <прогон> вопросов N:` и перечень вопросов (инструмент и заголовок вопроса);
- `флаги CLI в full: …` и полная командная строка `claude` в `full`;
- `модель по ответу CLI (confirm и full): …`;
- таблица проектных настроек: строка, «загружено» в `t3` и в `isolated`;
- итог: `FAIL` даёт код 1, `INFO` на код не влияет.

Ожидаемый перечень в `confirm`: `edit_file` — правка `notes.txt`; `bash` — `wc -l first.txt`.
Чтение `first.txt` внутри cwd CLI не спрашивает. Если модель после отказа пробует обходной
путь (другой инструмент для той же правки), вопросов будет больше, и перечень это покажет.

## Чем отличается от заглушки

- Живой CLI проверяет то, что заглушка только предполагает: флаги
  `--permission-prompt-tool stdio`, `--include-partial-messages`, `--session-id`, ответ
  `initialize` с `models` и `account`, форму `stream_event`, `assistant` и `result` (список
  допущений — `crates/hoh-harness-claude/PORT_NOTES.md`, «Допущения о протоколе»).
- На живой CLI влияют настройки пользователя (`~/.claude/settings.json`: правила allow, хуки,
  MCP): `confirm` и `full` идут с умолчанием «как в T3». Правило, разрешающее правку без вопроса,
  обходит `can_use_tool`. Тогда `confirm.asked` и `confirm.unchanged` не пройдут, и это нужная
  находка, а не сбой прогона.
- Модель недетерминирована: в `confirm` она может не попытаться сделать шаг после первого
  отказа. Смотрите `logs/confirm.out` и повторите один раз.
- CLI сам пишет свою историю в `~/.claude/projects/…` пользователя. Это побочный эффект живого
  прогона; заглушка его не имеет.
- Прогон тратит запросы к модели по подписке или ключу пользователя.

## Заглушка `claude-stub`: покрытие тех же путей

Сценарии `crates/hoh-harness-claude/tests/stub/claude.rs` и тесты `tests/claude.rs` эти два пути
покрывают не полностью. Крейт сейчас правит s4. Что добавить:

- **Чтение — не покрыто.** Есть `hello` (только текст); шага Read с результатом без
  `can_use_tool` нет. Добавить сценарий `read: <path>`:
  - `assistant` `tool_use` `Read {file_path}` без `can_use_tool`, как живой CLI в режиме
    `default` внутри cwd;
  - `user` `tool_result` с первой строкой настоящего файла, затем текст с этой строкой и
    `result` `success`.

  Тест: `ItemStarted` с `Tool{name:"Read"}`, `ItemCompleted` `Completed`, финальный текст равен
  строке, `ApprovalRequested` нет, `TurnCompleted`.
- **Отказ записи вне cwd — покрыт частично.** Отказ проверен только для команды (`run:` →
  `Declined`). Для `edit:` проверяется лишь форма запроса для пути внутри cwd. Write, путь вне
  cwd и проверки, что файл не создан, нет. Добавить сценарий `write: <абсолютный путь>`:
  - `tool_use` `Write {file_path, content}` и `can_use_tool`;
  - при `allow` заглушка создаёт файл, при `deny` — нет и шлёт `tool_result` с ошибкой.

  Тест: действие `FileChange{path: <абсолютный>, kind: Add}`, после `Deny` — `Declined`, файла
  нет, `TurnCompleted`.
- **Вопрос пользователю через гнездо — не покрыт.** Тесты адаптера говорят с контрактом
  напрямую (`PORT_NOTES` s4, «Не проверено»). Сквозной тест «ядро + `ExternalHarnessPlugin` +
  `HarnessClaudePlugin` с `executable` = `claude-stub` + ответ одобрения "отклонить"» проверил
  бы `approval/asked`, `approval/decided` и отсутствие файла так же, как живой прогон.
