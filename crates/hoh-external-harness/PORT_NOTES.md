# hoh-external-harness — нет образца в dsh (HOH-18, часть 1)

Новый код HoH: общее гнездо «внешний исполнитель сессии». Внешний CLI (Claude Code, Codex)
сам ведёт цикл агента, инструменты и продолжение, а сессия HoH записывает его события в свой
журнал. Ни Claude, ни Codex крейт не знает: адаптер CLI — отдельный пакет-плагин.

## Файлы
| файл | что |
|---|---|
| `src/contract.rs` | договор: версия `CONTRACT_VERSION = 1`, методы JSON-RPC, параметры, события `HarnessEvent`, ответы, коды отказов. Только serde-типы, имена полей camelCase. |
| `src/harness.rs` | трейты `ExternalHarness` (discover, start_or_resume) и `HarnessSession` (prompt, interrupt, respond, terminate) — то же, что договор, в процессе. |
| `src/supervisor.rs` | общий надзор за процессом: своя группа процессов, строки JSONL на stdout с пределом длины, ограниченные очереди, запись в stdin с подтверждением, stderr отдельно и с маской, остановка SIGTERM → ожидание → SIGKILL группы. |
| `src/stdio.rs` | `StdioHarness`: исполнитель-программа, говорящая договором по stdio (так подключается пакет из хоста пакетов, так работают тесты). |
| `src/redact.rs` | маска секретов: строки stderr, тексты ошибок, всё, что исполнитель пишет в журнал. |
| `src/agent.rs` | `ExternalAgent`: реализация `hoh_agent::Agent` поверх исполнителя: ход, «Стоп», запись событий в журнал. |
| `src/executor.rs` | `SessionExecutor` для реестра агентов: создание, продолжение после перезапуска, жизненный цикл. |
| `src/lib.rs` | плагин `ExternalHarness`, служба `externalHarnesses` (регистрация исполнителя по имени, discover, выбор пользователя по исполнителям). |
| `src/window.rs` | плагин `ExternalHarnessWindow`: методы окна `hoh/executor/discover`, `select`, `access/get`, `access/set`; файл выбора `external-harness.json` у профиля. |
| `tests/stub/harness.rs` | заглушечный исполнитель (бинарник `hoh-external-harness-stub`, только для тестов). |
| `tests/harness.rs` | сценарии гнезда целиком. |

## Как события попадают в журнал
Формы — те же, что пишет цикл агента, чтобы окно и проекции работали без изменений:
- ход: `turn/start`, `step/start` (шаг 1), `user/message`, …, `step/end`, `turn/end` с причиной
  `completed` / `aborted` (причина отмены) / `error` (`{message, code}` — код из договора);
- `text-delta` → `session.assistant-stream` (кадры Start/Chunk/End); финал (`text-final`) →
  долговечный `assistant/message` (рассуждение — блоком `reasoning` перед текстом), кадр End
  `Committed` с его seq; повторный финал того же сообщения в ходе отбрасывается. У каждого
  `assistant/message` поле `stream: []` (поле расчёта, которое хранилище требует при чтении) и
  `source.model` — выбранная модель или `default`, если не выбрана (пустую хранилище не берёт);
- текст без финала в завершённом ходе (`turn-completed`) записывается как обычный ответ; только
  после остановки или ошибки — с `interrupted: true`. Рассуждение без текста не теряется: оно
  уходит в сообщение следующего шага или отдельным `assistant/message` в конце хода;
- шаг (`item-started` / `item-completed`) пишется, как у цикла: `assistant/message` с блоком
  `tool-call` (id, имя, аргументы), затем `tool/call`, затем `tool/result`. Формат журнала v4 при
  чтении не принимает `tool/call` без такого объявления и `step/end` с вызовом без результата,
  поэтому все объявленные вызовы закрываются до `step/end` (незавершённые — результатом
  `interrupted`). Команда — `bash` `{command, workdir}`, правка — `edit_file`, вопрос —
  `user_question`; у команды к выводу добавляется `[Command finished with exit code N]`;
- запрос разрешения → классификатор опасных действий → существующий `approval/request`
  (`approval/asked` / `approval/decided` пишет служба одобрений);
- вопрос → `user-question/ask` через службу вопросов; его вызов открыт в ходе, результат пишет
  задача вопроса под замком, а конец хода закрывает неотвеченные — после `step/end` записей нет;
- нативный ID → запись `plugin:external-harness/session` `{harness, nativeSessionId[, replaces]}`.
  Первая такая запись (без ID) привязывает сессию к исполнителю: по ней реестр при продолжении
  выбирает исполнителя, а не цикл агента. Имя записи и её чтение из начала журнала
  (`EXECUTOR_BINDING_RECORD`, `bound_executor`, `stored_executor_binding`) объявлены в
  `hoh-agent`: реестр отказывает в продолжении такой сессии, если её исполнитель не загружен
  («session belongs to external harness <имя>»), а не отдаёт чужой журнал циклу;
- нарушение договора посреди хода (шаг без разрешения при `never`, повтор `requestId`) → шаг
  записывается с ошибкой `approval-bypassed` и пометкой «[The harness ran this step without asking
  for permission.]», ход — `turn/end` с ошибкой (`approval-bypassed` / `protocol-error`), процесс
  исполнителя останавливается; следующий ход запускает новый (с вопросом о расширении).

## Безопасность

### Режимы доступа (решение основателя 20.11 ~22:19; заменяет решение CEO 21:52)
- Два режима исполнителя, выбирает человек в окне. Тип `AccessMode` в `contract.rs`:
  - `confirm` («С подтверждением») — режим (а);
  - `full` («Полный доступ», как в T3) — без вопросов и без песочницы CLI.

  Умолчание — одна константа `DEFAULT_ACCESS = AccessMode::Confirm` в `src/contract.rs`.
  `Default` для `AccessMode` и `Choices::access` берут её же.
- Граница защиты: режим меняет только привилегированная команда окна `hoh/executor/access/set`.
  Она стоит в `privileged_methods` (`hoh-protocol`), SDK-сервер без одобрения хоста её не
  пропускает. Сеттер `Choices::set_access` — `pub(crate)`, его зовёт только `window.rs`.
  Наружу служба отдаёт режим только на чтение (`ExternalHarnessesService::access`).
  Режим читается при каждом запуске процесса исполнителя, и только из выбора. Ни чат, ни ответ
  модели, ни вызов инструмента (даже разрешённый человеком), ни файл в рабочей папке, ни
  `AgentOptions`/`meta` при создании сессии, ни `policy` конфигурации на него не влияют.
  Тест `nothing_in_a_session_turns_full_access_on`.
- Выбор хранится рядом с профилем движка: `<профиль>/external-harness.json`
  (`{"access": {"claude": "full"}, "selected": {...}}`), запись атомарная, права 0600. Читает
  его плагин `ExternalHarness` при подъёме, если запускающий назвал профиль
  (`ProfileContextKey`). Поэтому выбор действует во всех запусках движка на этом профиле,
  `headless` тоже. Тест `the_saved_choice_is_where_a_new_run_starts`.

  Остаётся риск: файл профиля может переписать сам исполнитель в режиме `full`, ведь он и так
  может всё. В `confirm` такая запись — вопрос человеку.
- Передача режима исполнителю. Трейт `ExternalHarness` получил три метода со значением по
  умолчанию, поэтому `hoh-harness-claude` (s4) собирается без правок:
  - `start_with_access(params: StartParams, access: AccessMode)` — так гнездо запускает каждый
    процесс. По умолчанию `Confirm` идёт в `start_or_resume`, а `Full` получает отказ
    `policy-unavailable` («this harness has no full-access launch»);
  - `launch_flags(policy: &HarnessPolicy, access: AccessMode) -> Vec<String>` — флаги CLI для
    окна, по умолчанию пусто;
  - `discover_with_status() -> (Discovery, VersionStatus)` — по умолчанию `unknown`.

  По stdio режим уходит полем `access` в `harness/start`. `StdioHarnessConfig.full_access_args`
  — аргументы программы для `full`; `None` означает отказ в `full`.
- `confirm`: `policy` конфигурации понижается до безопасной (`full-access` → `workspace-write`,
  сеть закрыта, `read-only` остаётся). Понижение явное: строка `warn` в журнале ядра, вопроса
  нет. Тест `a_configured_wider_policy_is_capped_to_the_chosen_mode`. В `full` настроенный
  `read-only` тоже остаётся (ревью 232500 №4, ниже).

  Автоматического «да» нет: каждый нативный запрос (команда, правка, сеть, инструмент, право)
  идёт вопросом человеку через службу одобрений. `allow` уходит только по `allowed-once` от
  человека, без отвечающего — `deny`. Единственный автоматический `allowed-once` службы
  одобрений — гранты стража окна для `bash` собственного цикла; запрос слота их не находит.
  Тест `without_never_every_command_write_and_network_step_is_asked`: обычная политика и
  настроенная `full-access` + сеть, вопрос на всё.
- `full`: процесс запускается с политикой `full-access`, `on-request`, сеть открыта, и
  `start_with_access(…, Full)`. Вопроса-согласия перед запуском нет: согласием было действие
  человека в окне. Текст режима для окна — «Полный доступ: Claude выполняет команды без
  вопросов», для `confirm` — «С подтверждением: команды, правки файлов и сеть Claude выполняет
  после вашего подтверждения» (`access_text`; имя — из имени исполнителя с заглавной).
- Смена режима не обрывает идущий ход. Процесс исполнителя перезапускается перед следующим
  ходом: `LiveSession.access` расходится с выбором. До этого `access/get` показывает в
  `effective` режим и флаги работающего процесса.
- `hoh/executor/access/get` → `{executor, mode, effective: {mode, flags, text}, selected}`:
  - `effective` — запуск последнего работающего процесса этого исполнителя
    (`Shared::launched`, слабая ссылка на сессию); если его нет — то, с чем запустится
    следующий (`launch_flags(launch_policy(...), mode)`);
  - `text` — добавленное поле, его нет в договоре u1.

  Тест `the_window_turns_full_access_on_and_shows_the_switches_that_run`: `effective.flags`
  равны хвосту `argv` процесса заглушки.
- (б) `approval: never` по-прежнему выключен флагом сборки `never-mode`. Без флага отказ
  `policy-unavailable` с текстом для окна «Этот режим пока недоступен для Claude: действия идут
  через подтверждение» (`never_off`), до запуска процесса и без вопроса. Тест
  `never_is_refused_before_the_harness_starts`.

  Вопрос-согласие (`ask_escalation`) остался только для `never` в сборке с `never-mode` и
  только в `confirm`. При `full` правило `never` не действует (`TurnLog.never` выключено): CLI
  ничего не спрашивает.
- Код (б) сохранён за флагом: правило путей и сети (`within_workspace`, `command_reach`), фраза
  окна согласия, проверка исполнителя, выполнившего шаг без запроса. Его тесты помечены
  `#[cfg(feature = "never-mode")]` и проходят с `--features never-mode`. Всё, что ниже сказано
  про `never`, относится к этому выключенному коду.
- Выбор исполнителя для новых чатов — `hoh/executor/select {executor|null, model}`,
  привилегированный. Он хранится в реестре агентов (`AgentRegistry::select_executor`,
  `hoh-agent`) и в том же файле у профиля. Новая сессия верхнего уровня (без родителя,
  `origin` и seed) идёт к выбранному исполнителю с выбранной моделью; подагенты и форки
  остаются у цикла. Если выбранный исполнитель не загружен, `create` отказывает, тихой
  подмены на цикл нет. Тест `the_window_sends_new_chats_to_the_selected_executor`.
- `hoh/executor/discover {executor}` → `{executor, installed, version, versionStatus, auth,
  models, detail}`:
  - каждый вызов — новый `discover_with_status()`;
  - `versionStatus` — от исполнителя, `unknown` без установленной версии;
  - `unsupported`/`unknown` — не отказ, а предупреждение в `detail`;
  - строки маскируются.

  Файлы входа CLI не читаются ни гнездом, ни окном. Тест
  `discover_reports_four_states_and_reads_no_sign_in_files`: четыре состояния заглушки и
  приманка в её `HOME`.
- Каждый нативный запрос разрешения сначала проходит общий классификатор
  (`hoh-destructive-classifier`): его вопрос показывается для опасного действия, для остальных —
  простой вопрос с самим действием.
- `approval: never` по договору не выключает вопросы у исполнителя: CLI работает в режиме,
  который спрашивает (у Claude — `default` с обратным вызовом разрешений по stdio, не
  `bypassPermissions`), и пересылает каждую команду и правку гнезду как `approval-requested`
  с `itemId` шага; выполняет только после `allow`. Решает гнездо по правилу ниже. Гнездо
  проверяет это: команда или правка с выводом (`item-output`) или с завершением
  `completed`/`failed` без разрешённого запроса на её `itemId` — нарушение (`approval-bypassed`),
  исполнитель останавливается. Завершение без начала считается командой, если у него есть
  код выхода. Не проверяются вызовы прочих инструментов (`tool`): какие из них требуют вопроса,
  решает CLI (чтение у Claude в режиме `default` идёт без вопроса). Шаг может прийти как
  `item-started` до запроса (так делает Codex) — это не нарушение. Проверка обнаруживает
  выполненное, а не предотвращает его: она останавливает исполнитель, который не держит
  договор, после первого же такого шага. Вне `never` вопросы задаёт сам CLI в своей песочнице,
  и гнездо этого не проверяет.
- (б, за флагом) В `confirm` без вопроса разрешается только при `approval: never` и только шаг,
  который классификатор не считает опасным и который целиком внутри папки сессии: команда
  с рабочим каталогом в ней, у которой все слова-пути тоже в ней и нет сети (см. ниже),
  добавление и правка файлов в ней. Путь разбирается так, как его
  пройдёт система: ссылки в существующей части раскрываются по ходу, и `..` после ссылки
  уходит из той папки, куда она ведёт (`link/../x` при ссылке наружу — снаружи). `..` в ещё не
  существующей части, битая ссылка и нечитаемая часть пути — не разобрано, значит вопрос.
  Удаление, `permission`, вызов инструмента и всё вне папки спрашиваются всегда. Настроенные
  сеть или песочница в `confirm` понижаются и ничего без вопроса не разрешают.
- Команда в этом режиме проверяется по словам (ревью `hoh-harness-claude` 214304, п. 1;
  ведущий 20.11): `command_reach` из `hoh-destructive-classifier` разбирает строку тем же
  разбором оболочки, что и классификатор, и отдаёт все слова, которые могут быть путями, —
  аргументы, значения флагов (`-C /etc`, `-C/etc`, `--prefix=/usr`, `-o../x`, `-xzf/tmp/a.tar`),
  значения присваиваний (`GIT_DIR=/x`, `of=/dev/sda`), цели перенаправлений (кроме
  `/dev/null`, `/dev/std*`, дескрипторов), путь самой программы — и сетевые программы (`curl`,
  `wget`, `nc`, `ssh`, `scp`, `rsync`, `docker`, `gh`, …; `git push/pull/fetch/clone/remote/…`,
  установка пакетов у `npm`/`pnpm`/`yarn`/`pip`/`uv`/`poetry`/`cargo`/`gem`/`go`/`bundle`).
  Без вопроса — только если сети нет и каждое слово, открытое от рабочей папки команды так,
  как его откроет система (со ссылками), лежит в папке сессии; слово с `~` в начале — вне
  папки. Проверяются все слова, не только похожие на путь: `foo` в `rg foo src` не существует
  и потому внутри, а `cat link` при ссылке наружу — снаружи.
- Не разобрано, значит вопрос: строка, которую разбор не читает; слово, известное только при
  запуске (`$X`, `$(…)`, обратные кавычки, `<(…)`, шаблон `*`, `~`, `{a,b}`); смена папки
  (`cd`, `pushd`); обёртки, запускающие другую команду (`sudo`, `env`, `xargs`, `timeout`,
  `nohup`, `exec`, `eval`, `source`, …); код в аргументах (`sh -c`, `python -c`/`-m`,
  `node -e`, интерпретатор без файла скрипта, `awk`, `find -exec/-ok/-delete/-fprint`,
  любой `sed`, кроме `sed -n '<строки>p'`). Цена — вопрос на `ls *.rs`, `cat $F`, `cd sub && ls`
  и любой `sed -i`; это сознательно (fail-closed). Не закрыто: код самого проекта (`make`,
  `cargo test`, `./script.sh`, хуки git) может делать что угодно — это предложение CEO
  о файлах, выполняемых позже (отчёт o1 к ревью 212418).
- Окно согласия на `never` говорит дословно, что пройдёт без вопроса: в заголовке — «без
  вопросов только о командах и правках файлов, которые остаются в папке сессии: всё, что
  удаляет или перезаписывает файлы, касается путей вне папки или сети, он спросит», в объяснении —
  полное правило (`NEVER_RULE` в `agent.rs`).
- Вопрос о правке файлов показывает в подробностях все изменения списком («удалить: …»,
  «изменить: …», «переместить: … → …»), а не только то, что классификатор счёл опасным: его
  заголовок остаётся, подробности заменяются полным списком, потому что одобряется вся правка.
- Вопрос, в который действие не поместилось (команда, ввод инструмента, описание или список
  изменений длиннее 3600 знаков), помечается в заголовке «(показано не полностью)» и строкой
  с полным размером в подробностях. Отличие от предложения ревью «отказать»: предел окна
  около 8192 единиц UTF-16, и отказ запретил бы любую длинную команду с heredoc; решает
  пользователь, видя пометку.
- Без службы одобрений или классификатора плагин не поднимается (зависимости обязательны).
- stderr не попадает в журнал, кроме короткого хвоста в тексте ошибки при падении. Строка stderr
  длиннее предела обрезается по последнему пробелу до маски: ключ, разрезанный границей,
  выбрасывается целиком.
- Каждый `requestId` используется в ходе один раз. Повтор (даже после ответа) — нарушение
  договора: ход завершается `protocol-error` («the harness repeated request id …»), исполнитель
  останавливается. Ответить повтору нельзя без двусмысленности: второй ответ на тот же id мог бы
  одобрить другое действие, а молчание оставило бы исполнитель ждать до «Стоп».
- Маска (`redact`): блоки закрытых ключей (`-----BEGIN … PRIVATE KEY-----` … `-----END …`)
  целиком, блок без конца — до конца текста, конец блока без начала (обрезанный хвост вывода) —
  от начала текста; известные формы ключей, `Bearer`, пароль в URL (`scheme://user:…@`, `/` и
  `?` в пароле тоже; до последнего `@` перед хостом: `u:p@ss@h` → `u:[redacted]@h`; часть после
  `@` с `/`, `?` или `#` — уже путь, как в `…@registry/@scope/pkg`), присваивания с именем,
  оканчивающимся словом секрета (`OPENAI_KEY=`, `DB_PASSWORD="…"`, `x-api-key:`, `--password=`)
  или содержащим сильное слово где угодно, в том числе слитно (`secret`, `password`, `passwd`,
  `passphrase`, `private_key`, `credential`: `SECRET_KEY_BASE=`, `PGPASSWORD=`), имена camelCase
  (`clientSecret`, `apiKey`), длинные непрозрачные токены. Значение без кавычек идёт до пробела
  или конца, `,` и `;` его не обрывают (`DB_PASSWORD=ab;cd` → `DB_PASSWORD=[redacted]`);
  кавычка внутри такого значения — его часть, кавычка в конце остаётся (закрывает кавычки вокруг
  всего присваивания, `-H 'Authorization: Bearer …'`). Значение в кавычках остаётся в кавычках,
  поэтому JSON не ломается; у JSON-значений (`redact_value`) маскируется каждая строка, ключи не
  трогаются. Пароль URL, в котором есть и `@`, и `/` после него, разбирается неоднозначно:
  маскируется часть до первого такого `@`.
- Маскируется всё, что исполнитель пишет в журнал: текст ошибок (`turn-failed`, ответ JSON-RPC
  с ошибкой, ошибки разбора ответа и события, отказ при старте или prompt), текст ответов
  и рассуждение, аргументы шагов, вывод команд и правок, ответ на вопрос, текст вопроса
  (подписи вариантов — нет). Отличие от цикла агента dsh, который вывод `bash` не маскирует:
  здесь процесс внешний, его вывод HoH не видел до журнала, а `cat .env` у CLI — обычный шаг.
  Цена — лишняя маска значений вида `key=…`, `password_min_length=8`, порта и пути в
  `http://host:8080/a@b`, текста между маркерами PEM в объяснении формата, хвоста после `,`/`;`
  в значении секрета (`token=a1,next=2` теряет `,next=2`); обычные слова (`monkey=`,
  `tokenizer=`, `token_count=`) не задеваются.
- Поток ответа (`text-delta`) удерживает хвост, который может быть началом секрета, до
  следующего куска или конца сообщения (решение CEO): последнее слово вместе с именем,
  разделителем или `Bearer`, значением которого оно может быть; значение в кавычках с именем —
  до закрывающей кавычки; блок `-----BEGIN` — до своего `-----END`. Показанное режется только
  между словами и не внутри того, что маскирует `redact`, и каждый показанный кусок
  маскируется сам: склеенный поток равен маске всего текста (проверено посимвольной нарезкой).
  Цена — поток отстаёт на одно слово, а незакрытая кавычка после `:`/`=` или `-----BEGIN` без
  конца задерживают показ до конца сообщения.

## Отличия от образца
Образца в dsh нет. Устройство договора (ход, шаги, запросы, «Стоп» через нативное прерывание)
сверено с разбором T3 Code (`docs/hoh18-claude-codex.md`; T3: `CodexAdapterV2.ts`,
`ClaudeAdapterV2.ts`). Код T3 не переносился, NOTICE не нужен.

Решения, о которых стоит знать:
- Исполнитель выбирается маршрутом провайдера: имя исполнителя = `provider` в `AgentOptions`.
  В `CreateAgentOptions` нового поля нет: структуру строят литералами в чужих крейтах.
- Очередь сообщений агента — в памяти, как у цикла; `steer` уходит в следующий ход
  (у внешнего CLI нет вставки посреди хода), `inject` — без пробуждения.
- Смена модели перезапускает процесс исполнителя перед следующим ходом; модель другого
  провайдера игнорируется с предупреждением в журнале ядра.
- Потерянная нативная сессия (`session-not-found`) → новая, в журнал пишется `replaces`.
- После аварийного выхода HoH посреди хода ход закрывает штатное восстановление хранилища
  (`interrupted_turn_closers`): шаги объявлены так же, как у цикла, поэтому начатые получают
  `TOOL_OUTCOME_UNKNOWN`, неначатые — `TOOL_NOT_STARTED`. Собственного «закрывателя» больше нет.
- Ответ на `harness/prompt` ждётся вместе с событиями хода: исполнитель может ответить на prompt
  до или после того, как прислал ход. «Стоп» до ответа на prompt не шлёт `harness/interrupt` сразу
  (хода у исполнителя ещё нет): прерывание уходит, как только prompt принят; срок прерывания
  считается от нажатия «Стоп». Если исполнитель не ответил на prompt в этот срок — процесс
  останавливается.
- Если события начала хода (`turn/start`, `step/start`, `user/message`) не записались, ввод
  возвращается в очередь на прежние места, ход закрывается `turn/end` с кодом
  `turn-open-failed` (если `turn/start` записан), и очередь ждёт следующего пробуждения, а не
  повторяет ход сама.
- Выгрузка сессии закрывает агента до остановки водителя: после неё ход не начинается, даже из
  ввода, лежавшего в очереди.

## Не перенесено
Ничего: образца нет. Регистрация в профиле окна — за ведущим (apps/hoh не трогал по заданию).
Адаптеры Claude и Codex — отдельный пакет, пишется позже.

## Не проверено
- Настоящие CLI (Claude, Codex) — только заглушка по договору.
- Сквозной сбой самого HoH посреди хода (убитый процесс): JSONL-хранилище во время хода ещё не
  сбрасывает журнал на диск. Проверено продолжение журнала, оставленного открытым
  (`resume_closes_a_turn_the_host_left_open`), и продолжение сессии с шагами и вопросами.
- Перенос договора на хост пакетов (`docs/packages.md`): типы и методы готовы к той же
  передаче (serde, явная версия), но сам мост в хост пакетов не написан.
- Платформы кроме Linux (группы процессов и сигналы — unix).

## Замечания ревью без исправления (круг 1)
- Насос stdout ждёт место в очереди событий (256). Пока водитель не читает события, ответы на
  запросы к исполнителю не разбираются. Взаимной блокировки нет: ответ на prompt теперь ждётся
  вместе с событиями; задачи разрешений и вопросов после отзыва ничего не отправляют
  исполнителю; `interrupt` и `harness/stop` ограничены сроком. Худший случай — ожидание
  `harness/stop` до `terminateGraceMs`, затем остановка группы.
- Если `assistant/message` объявления записался, а `tool/call` — нет (сбой записи), вызов
  остаётся объявленным без начала; закрытие хода тогда не пишет точное восстановление
  `TOOL_NOT_STARTED`. Это тот же случай отказа диска, что и у цикла агента.
- Текст, который ещё идёт потоком, когда исполнитель начинает шаг в том же шаге журнала, окно
  (`ui/src/screens/chat/projection.mjs`, `settleLive` по ключу ход:шаг) уберёт до финала и
  покажет заново по финалу. Codex и Claude завершают текстовый блок до начала шага.

## Замечания ревью (круг 2)
- №4 (гипотеза о немаскированных ошибках разбора): в журнале текст и раньше проходил `redact`
  перед `turn/end`. Утечка подтвердилась на другом пути: ошибка разбора ответа на
  `harness/discover` (serde цитирует неизвестный вариант) уходила вызывающему без маски.
  Теперь тексты ошибок разбора ответа, события и неизвестной нотификации маскируются в `stdio.rs`
  при создании. Тест `protocol_errors_quoting_a_key_are_masked` проверяет оба пути.

## Замечания ревью (круг 3)
- №1 (`..` после ссылки), №2 (вопрос по смешанной правке), №3 (PEM, `SECRET_KEY_BASE`),
  №4 (`/` и `?` в пароле URL), окно потока — исправлены, см. «Безопасность».
- №5 (аргументы команды в режиме без вопросов) — было принято правилом CEO; закрыто позже
  проверкой слов команды (ревью 214304, п. 1), см. «Безопасность». Сквозной тест
  `under_never_only_commands_that_stay_in_the_folder_pass_without_a_question`: `ls`,
  `cat a.txt`, `rg foo src`, `sed -n '1,20p' a.txt` — без вопроса; `rm -rf ~/x`, запись вне папки,
  `cat ~/.ssh/id_rsa`, `curl -T ~/.ssh/id_rsa …`, `make -C /etc install`, `git -C /x …`,
  `tar -C / -xf a.tar`, ссылка наружу, `$HOME`, `sh -c`, `git push`, `wget`, рабочая папка `/` —
  вопрос.

## Замечания ревью 212418
- №1 (`never` держится на запросах исполнителя): договор `never` переписан (см. «Безопасность»),
  гнездо ловит шаг без разрешения. Тесты `a_harness_that_runs_a_step_without_asking_under_never_is_stopped`,
  `without_never_the_harness_decides_what_to_ask`. Заглушка теперь шлёт `itemId` в запросе.
- №2 (`,`/`;` в значении), №3 (`@` в пароле URL) — исправлены, случаи в
  `masks_keys_tokens_and_assignments` и в посимвольной проверке потока. Попутно: слитное
  `PGPASSWORD=` раньше не маскировалось вовсе.
- №4 (продолжение сессии без её исполнителя) — отказ в реестре `hoh-agent`, тест
  `hoh-agent/tests/executor.rs::resume_refuses_a_session_bound_to_an_external_harness_that_is_not_loaded`.
- №5 (повтор `requestId`) — ошибка хода вместо молчания, тест
  `a_repeated_request_id_fails_the_turn_instead_of_hanging`.
- №6 — страж окна уже вызывает `decide_command` (в пакет входит `hoh-window-destructive-guard`).
- №7 (правка `.git/hooks`, `package.json`, `.envrc` без вопроса) — правило CEO в этом ходе не
  менялось; предложение — в отчёте o1.

## Запись в stdin и ответы исполнителю (блокер режима (а), ревью 215107 п. 1)
- `Supervisor::send(&self, message: &Value) -> Result<(), HarnessFailure>` — сигнатура прежняя,
  смысл новый: `Ok` только после того, как строка записана в stdin и сброшена (`write_all` +
  `flush`; процесс мог её ещё не прочесть). Иначе ошибка именно этой записи тому, кто отправил:
  `closed` — stdin закрыт (процесс закрыл его или вышел, канал сломан — `BrokenPipe`, был
  `terminate`), `io-error` — иная ошибка записи. После первой ошибки писатель останавливается,
  строки в очереди не пишутся, их отправители тоже получают `closed`. Порядок строк — порядок
  отправки; `send` ждёт строки перед своей. Молчаливого успеха больше нет.
- Запрос гнезда (`request_value`) ждёт подтверждение записи не дольше срока ответа; ответ насоса
  на запрос исполнителя пишется отдельной задачей, чтобы насос не ждал stdin, который не читают.
- `Supervisor::abandon()` — убить группу сразу (без SIGTERM), если не было `terminate` и процесс
  жив; закрывает stdin. Его вызывает `Drop` надзора и владелец `Owner` в `stdio.rs`: последний
  `Link` на процесс ушёл без остановки (например, «Стоп» во время `initialize` или
  `harness/start`) — процесс убит, а не ждёт stdin вечно (насос держит сам надзор, а не владельца).

## Замечания ревью 215609
- №1 утечка процесса при «Стоп» во время запуска — `Owner` (выше). Тест
  `stop_while_the_harness_starts_kills_its_process` (заглушка с `--slow-initialize` и `--slow-start`).
- №2 `create` без `session_id` — id выдаёт хранилище (`prepare(None)`), как у цикла. Тест
  `a_session_created_without_an_id_gets_one`. Через цикл (`deepseek`) это проверяют его тесты.
- №3 исполнитель без ответа — вопрос, который журнал не записал, отзывается `cancelled`; задача
  ответа, которую не удалось запустить (контекст агента уходит), завершает ход с остановкой
  исполнителя; ушедший агент (`as_dyn()` пуст) отвечает `cancelled`. Тест
  `a_question_that_cannot_be_recorded_is_withdrawn`.
- №4 запрос между ходами — отзывается `cancelled`, когда начинается следующий ход (события между
  ходами читаются только тогда). Тест `a_request_between_turns_is_withdrawn`.
- №5 одобрение привязано к действию: `allowed` хранит `itemId` → разрешённое действие; шаг, начатый
  другим действием (`ls` спрошен, `rm` выполнен), — обход. Сравнение точное: адаптер обязан
  присылать одно и то же действие в запросе и в шаге. Это код `never` (выключен), тест
  `a_step_run_as_another_action_than_allowed_is_a_bypass` идёт с `--features never-mode`.

## Замечания ревью 220312 и 221315
Каждому пункту — тест; новые тесты проверены на старом коде (падают).
- 220312 №1 (текст согласия на `full-access`/сеть) — снят решением основателя: настроенные
  `full-access`/сеть в `confirm` понижаются, `full` выбирает человек в окне, вопроса нет
  (раздел «Режимы доступа»).
- 220312 №2 = 221315 №1: событие между ходами пишется в журнал ядра только маскированным JSON
  (`redact_value`). Тест `an_event_between_turns_reaches_the_log_masked`: перехват логгера
  (`Logger::exporter`), ключ из `text-delta` после хода не виден, `OPENAI_KEY=[redacted]` виден.
- 220312 №3: вопрос, вызов которого журнал не записал, отзывается `cancelled` (`refuse`). Тест
  `a_question_that_cannot_be_recorded_is_withdrawn` (хранилище отказывает на `assistant/message`).
- 220312 №4: запрос между ходами — №4 ревью 215609 (`a_request_between_turns_is_withdrawn`).
- 220312 №5: `resume`, который исполнитель не подтвердил (`resumed: false`), записывается с
  `replaces` и строкой `warn`. Тест `a_resume_the_harness_did_not_confirm_is_recorded`
  (заглушка `--fresh-on-resume`).
- 220312 №6 = 221315 №4: вопросы классификатора к файловой системе для правки (`deletion`,
  `overwrite` → `fs.exists`) идут на блокирующем потоке (`spawn_blocking`). Если пул откажет,
  они выполняются на месте, ответ тот же. Тест `file_change_checks_run_off_the_runtime_thread`
  (наблюдающий `Fs`).
- 220312 №7 = 221315 №6: исполнитель не может взять имя поставщика модели (`Llm::list_providers`
  при `register`), иначе его сессии ушли бы от цикла. Тест
  `a_harness_cannot_take_a_model_providers_name`. Создание без `session_id` закрыто ранее
  (215609 №2). Поставщик, зарегистрированный позже исполнителя, не проверяется.
- 221315 №2: эффект жизненного цикла на `shared.ctx` больше не держит сессию после ухода
  владельца: снявшаяся сторона снимает и эффект другой (`Lifecycle::leave`, повторный `dispose`
  своего — сразу). Тест `a_session_whose_owner_ends_is_released`.
- 221315 №3: `Debug` у `ProcessSpec` печатает имена переменных окружения без значений, аргументы
  маскированы. Тест `supervisor::tests::a_spec_prints_no_environment_values`.
- 221315 №5: привязка ищется во всех записях до первого события хода (`turn/*`, сообщения,
  `tool/result`), страницами по 16, а не в первых 16 (`hoh-agent`). Тест
  `a_binding_after_many_leading_records_still_binds_the_session` (40 записей до привязки).

## Итоговое ревью 232500
- №1 (текст одобрения `access/set`, `select`) — `hoh-user-approval/src/method.rs`, см. его PORT_NOTES.
- №2 (ожидания текстов стража в `apps/hoh/tests/window_guard.rs`) — обновлены по разрешению ведущего.
- №3: ошибка чтения журнала при поиске привязки — отказ `resume` (`claims` передаёт ошибку
  `stored_executor_binding`), см. PORT_NOTES `hoh-agent` п. 13.
- №4: `full` не расширяет настроенный `read-only` (сессия-ревьюер остаётся ревьюером, как в
  `hoh-harness-claude`): `launch_policy` оставляет `read-only` и сеть конфигурации, а
  `AccessMode::widens(policy)` — общее правило, когда нужен запуск без вопросов: `StdioHarness` не
  добавляет `full_access_args`, умолчание `start_with_access` не отказывает такой сессии. В
  `harness/start` уходит `access: "full"` с политикой `read-only`. Тесты:
  `the_launch_policy_follows_the_users_mode` (юнит) и `full_access_keeps_a_read_only_session_read_only`
  (политика и argv заглушки, `effective.flags` пусты, вопросов нет); на прежнем коде падают.

## Флак `a_request_between_turns_is_withdrawn` (интеграция i3) — гонка в продукте
- Причина: запрос, который исполнитель шлёт после `turn-completed` (между ходами), отзывался только
  если к началу следующего хода он уже лежал в канале событий (`ensure_live`, `try_recv`). Пришедший
  позже — после отправки следующего промпта — разбирался как запрос нового хода: уходил человеку и
  получал ответ `approval`. Воспроизведено на прежнем коде под нагрузкой: 3 падения из 42 прогонов,
  тот же вывод, что в i3 (`left: "approval"`, `right: "cancelled"`; `plans/w-o1-flake-withdrawn.log`).
- Исправление: ход открывает `turn-started` с его id (правило договора, `HarnessEvent::TurnStarted`;
  заглушка и `hoh-harness-claude` шлют его первым, Claude — до передачи сообщения CLI). Запрос до него
  (`TurnLog.opened == false`) отзывается `cancelled` (`refuse`), никого не спрашивая, когда бы он ни
  дошёл. Исполнитель, не шлющий `turn-started`, получает отказ на каждый запрос (закрыто, не обход).
- Тест `a_request_from_between_turns_that_arrives_in_the_next_turn_is_withdrawn` (сценарий заглушки
  `late request after prompt`: запрос придерживается до прихода следующего промпта): на прежнем коде
  падает 10 из 10, с исправлением — 200 прогонов обоих тестов под нагрузкой без падений.

## Вопросы ведущему
См. отчёт исполнителя o1.

## Режим открытого разговора (i7, решение CEO 21.11 ~03:40)

Находки: claude-executor-01, window-ui-02/-03, logic-e2e-02/-03 — окно показывало режим и
исполнителя «для новых чатов» или процесса, запущенного последним, а не разговора на экране.

- `hoh/executor/access/get` принимает необязательный `sessionId`. С ним в ответе есть `session`:
  `{executor, model, privacy, mode, running, pending}`. Без него ответ прежний, `session` нет.
  - `executor`, `model`: у живого агента — маршрут провайдера и модель (`agent.model().current()`),
    у не открытого — по записи привязки журнала (`stored_executor_binding`), модель тогда `null`.
    Выбор «для новых чатов» (`selected`) на них не влияет.
  - `privacy`: `"remote"` для разговора внешнего исполнителя (слова уходят в интернет).
  - `mode`: режим, в котором пойдёт СЛЕДУЮЩИЙ ход (выбор человека для исполнителя).
    `running`: режим живого процесса этого разговора или `null`. `pending`: `running ≠ mode`.
  - Разговор без внешнего исполнителя (цикл агента) или неизвестный: `executor`, `model`,
    `privacy`, `mode`, `running` равны `null`, `pending` = `false`. Это отличие от контракта
    ведущего: `mode` и `running` допускают `null`.
  - `effective` с `sessionId` — процесс этого разговора (режим и флаги его запуска), а при
    отсутствии процесса — то, с чем стартовал бы следующий ход.
- Запись о запуске ведётся по разговору: `Shared.launched` — список `(id разговора, Launched)`,
  запись разговора заменяется при его новом запуске, записи вымерших процессов отбрасываются.
  `running_launch()` без `sessionId` — запущенный последним, как раньше.
- Смена режима по-прежнему вступает в силу в начале следующего хода (`ensure_live`); процесс при
  `access/set` не останавливается, расхождение отдаётся полем `pending`.
- Заглушка окна (`hoh-window-stub`) приведена к тому же: привязка разговора к Claude при первом
  сообщении нового чата, `running` по разговору, `selected` без Claude — `{executor: null,
  model: null}`, как отвечает движок.
- Тесты: `apps/hoh/tests/window_executor.rs` `s1_…` (смена режима туда и обратно, `pending`,
  `effective` = argv поддельного claude) и `s2_…` (два чата, возврат «для новых чатов»); заглушка
  — `each_conversation_keeps_its_own_executor_model_and_mode`.

## i7-add (verify-4 CEO verdict)

- **Idle process ends with the mode choice.** `access/set` ends the Claude processes of the executor that idle between turns (`Shared::retire_idle`, `terminate_grace`), so no turn started in the background runs in the old mode. A conversation that runs a turn keeps its process until the turn ends (the mode applies from its next turn, as before). The window then shows no live process (`running` null) until the next message.
- **Choices are written before they apply.** `Window::commit` takes one lock, writes the file, and only then applies the change. A refused write keeps the previous state and returns "the choice could not be saved to …"; two decisions in a row take effect in order (the old apply-then-revert left `full` active).
- **Folder is fixed.** A `working-directory/access` listener (prepend) refuses a folder *set* for a conversation of a registered harness. The event's agent is the folder service's model-less adapter, so the conversation's own agent is looked up by id. The JSON-RPC wire carries only the message text, so the code `EXECUTOR_FOLDER_FIXED` is the prefix: `EXECUTOR_FOLDER_FIXED: Чат с Claude работает в одной папке. Чтобы работать в другой, начните новый чат`.
- **Approval window wording.** Headline is by meaning with the display name ("Claude просит выполнить команду: …" / "… изменить файл …"), no "Исполнитель"/quoted name; explanation is empty when there is nothing to add. Risk of a request the classifier did not judge is `unassessed` (protocol `Risk::Unassessed`), marked by `display_reason["x-risk"] = "unassessed"` and mapped in `hoh-user-approval/src/host.rs::risk_of` (only that file of the crate changed). A request the classifier called dangerous stays `high`.

## i8 (independent review of cand/i7)

- **Folder of a new chat.** The folder refusal above now applies only once the conversation has worked in its folder: its native session is recorded in the log (`plugin:external-harness/session` with a `nativeSessionId`) or its process runs (`Shared::started_in_folder`). A new chat with Claude (no turn yet) takes another folder, which is what the window's «Новый чат в другой папке» does. The agent no longer keeps the folder it was created in: `ExternalAgent::cwd()` reads the session's folder where it is used (start, workspace checks), so the first turn runs in the folder chosen before it.
- **Model after a restart.** The binding record also names the conversation's model: `{harness, nativeSessionId[, model][, replaces]}`, written at creation and whenever the native id or the model changes. A conversation keeps the model of its route only when that route names the harness (a new chat); otherwise (a resume, which the server hands the engine's route, e.g. `window-mock`) it takes the model of its last binding record, or none (the CLI's default). Another provider's model never reaches `--model` or `access/get{sessionId}.session.model`. Logs written before have no `model`: such a conversation continues on the CLI's default.
- Tests: `apps/hoh/tests/window_executor.rs` `t6_…` (new chat takes a folder, its turn runs there, then `EXECUTOR_FOLDER_FIXED`) and `t7_…` (restart: `access/get{sessionId}` before and after `session/resume` keeps executor, mode and data line, the model after the resume, and the next start has the same `--model`). Both failed before the change.
- **Headline of a question (final review Fable, B).** `one_line` turns every character the protocol refuses in `humanText` (checked with `hoh_protocol::approval::validate_human_text` one character at a time: controls, spacing other than a plain space, invisible marks such as U+FE0F, U+00A0, U+200B) into a space and joins runs of spaces, as the classifier's `text.rs::clean` does. Before, one such character in a command or a file name made the host replace the headline with "Allow tool action bash?". Test: `t4b_…`.
- **Saved choices after a restart (final review Fable, C).** `t8_…`: `access/set full` and `select` are saved to `external-harness.json`; the next engine starts with mode `full`, Claude selected, and Claude runs with the full-access switches; a damaged file leaves `confirm` and no executor. `restore_choices` now warns when no profile is named (the launcher always names it before the plugins start, so the window's path reads the file).

## i9b (narrow review Fable cand/i9, 152628)

- **Mode changed and changed back between turns.** `retire_idle` ends an idle process outside the driver and forgets its launch; a terminate may end the events without `SessionEnded` (the contract allows it for a clean close), so `ensure_live` no longer took the session as alive once the mode was back to its own, and the next message failed with "harness stdin is closed". Now a live session without its launch (`Shared::running_launch_of`) is restarted: the next message starts Claude anew in the mode in effect, with `--resume`. Test `t10_…` in `apps/hoh/tests/window_executor.rs` (also: the same mode chosen again ends nothing; a change during a turn leaves that turn's process, the next message starts in the new mode); it failed before the change.
- `t11_…`: the CLI names its session `--dangerously-skip-permissions`; after a mode change the next start has `--session-id`, no `--resume` and not that word (the UUID check of `hoh-harness-claude`; without it the test fails).

## i10b (живой пилот 22.11, `qa/pilot-20261122-1640`)

- **«Стоп» завершает процесс исполнителя и всё под ним.** `ExternalHarness::stop_ends_process()` (по умолчанию `false`): у исполнителя, чьё нативное прерывание может оставить команду (Claude Code 2.1.329 в пилоте: `sleep 120` жил после «Стоп», её поздний результат стал следующим ответом), слот при отмене хода сразу делает `terminate` (SIGTERM, `terminateGraceMs`, KILL) и помечает сессию мёртвой; ход закрыт как `aborted`, следующий ход запускает процесс заново с прежним нативным id (`--resume`). У `StdioHarness` прерывание по-прежнему нативное. «Стоп» во время запуска — как в i8b (брошенный старт → `abandon`).
- **Дерево, а не только группа.** Claude Code запускает команды в собственной сессии (так же делает подставной claude `scripts/tests/claude-live-extra/fake-claude`, `start_new_session`), сигнал группе их не достаёт. `Supervisor::terminate` и `abandon` теперь снимают список всех потомков процесса по `/proc` (родитель → дети, до сигнала: после смерти процесса его потомки уже не под ним) и шлют сигнал группе и каждому из них; перед KILL список дополняется повторным обходом.
- **Смерть движка (в т.ч. KILL -9).** К каждому процессу исполнителя `Supervisor::spawn` запускает сторожа: `/bin/sh` в собственной группе процессов, stdin — труба, второй конец которой есть только у движка (CLOEXEC, держит задача, ждущая процесс). Конец трубы приходит, когда движок перестаёт следить за процессом: процесс вышел, или движок умер любым способом. Сторож обходит `/proc` от процесса, убивает найденное дерево, сам процесс и его группу (`kill -9`) и выходит. Процесс перед обходом не останавливается: группа, осиротевшая со смертью движка при остановленном участнике, получает от ядра SIGHUP, процесс умирает раньше обхода, и его команды уходят к другому родителю (так `t14_…` падал примерно в каждом третьем полном прогоне; механизм проверен отдельным опытом). `PR_SET_PDEATHSIG` не взят: он срабатывает при смерти потока-родителя (рабочий поток tokio, ушедший после `block_in_place`, убил бы живой claude), сбрасывается у детей claude (команды переживают) и не достаёт команды в чужих сессиях. Не запустился сторож — не запускается и исполнитель (`code::IO`).
- **Дубль короткого ответа в окне.** Живой текст удерживает незаконченное слово (`ready_to_show`: хвост может оказаться началом ключа), поэтому однословный ответ («ok1») целиком доходил до окна куском *после* записанного `assistant/message`: окно (`ui/src/screens/chat/projection.mjs:334`, `settleLive`, и `:368`) ставит запись на место показанного живого текста, а когда показанного ещё нет — добавляет запись отдельно; кусок после неё рисовался вторым ответом. После перезапуска окна — только запись, один раз. Теперь `final_text` и `finish` досылают остаток живого текста до записи ответа (`shown_whole`), конец потока (`End{Committed}`) — после неё. Окно не менялось; для u1: `settleLive` мог бы списывать попытку и без показанного текста (`retired`), тогда поздний кусок не создал бы вторую запись.
- Тесты (`apps/hoh/tests/window_executor.rs`): `t12_{confirm,full}_…` — «Стоп» посреди команды в собственной сессии: нет ни claude, ни команды (по PID), следующий ход — новый запуск с `--resume` того же id, в нём ничего от прерванного хода; `t13_{confirm,full}_…` — «Стоп» на запуске (после смены режима): процесс мёртв, следующий ход с `--resume`; `t14_…` — KILL -9 движка посреди команды: claude и команда исчезают; `t15_…` — однословный ответ из `stream_event` приходит в окно живым текстом до записи, запись одна. `t2_…` переписан: после «Стоп» процесс завершается и у слушающего CLI. Юнит: `supervisor::terminating_ends_a_command_in_a_session_of_its_own`.

## i11b (итоговое ревью Fable cand/i11, 173026)

- **Обход `/proc` вне потоков рантайма (PORTING.md §6).** `Supervisor::terminate` читает дерево через `spawn_blocking`. `abandon` (зовётся и из `Drop`) отдаёт обход и KILL блокирующему потоку рантайма (`Handle::try_current`), без рантайма — своему потоку `std::thread`, а если поток не создался — делает это сам. Если блокирующая задача не успела до остановки рантайма, процесс добивает сторож: его труба закрывается вместе с задачей ожидания. Поведение «Стоп» и сторожа то же; `t12_…`–`t15_…` зелёные.
- **Рекомендации.**
  - `terminate` процесса, который уже вышел сам, ничего не шлёт: его pid и группа могли достаться другим, а свою группу при выходе он оставил сторожу. Повторный обход перед KILL — только пока процесс жив.
  - `terminate`, брошенный между SIGTERM и KILL (тайм-аут вызывающего), всё равно убивает найденное (`KillOnDrop`, без обхода `/proc`). Тест `a_terminate_dropped_in_its_grace_still_kills` (раньше процесс, не слушающий SIGTERM, оставался жить).
  - Сторож читает `stat` целиком: имя процесса (`comm`) может содержать перевод строки, и тогда `stat` занимает две строки. Тест `a_name_with_a_line_break_does_not_hide_a_process` (со старым чтением одной строки падает), там же разбор движка (`descendants`).
  - После «Стоп» слот сразу отпускает живую сессию: `access/get` больше не показывает `running` у мёртвого процесса до следующего сообщения (проверка в `t12_…`).
  - Дерево и сторож опираются на `/proc`. Вне Linux остаются только процесс и его группа: обход ничего не находит, сторож убивает процесс и группу.
  - `assert_full` в `window_executor.rs` проверяет, что в full нет `--tools` и `--disallowedTools`.
  - Не сделано (не тривиально): команда, ушедшая от родителя (`setsid nohup … &` с выходом промежуточного процесса), уже не под CLI и переживает «Стоп» и смерть движка; надёжнее cgroup или subreaper.

## i12c (итоговое ревью Fable 174941)

- **Имя процесса не в UTF-8.** Ядро хранит в `comm` первые 15 байт имени файла, которым процесс запущен: `проверка.sh` (19 байт) режется посреди `а`, и `/proc/<pid>/stat` уже не UTF-8. `descendants` читал `stat` через `read_to_string`, такой процесс пропускался, и `terminate`/`abandon` («Стоп», брошенный старт) не находили команду в собственной сессии. Теперь `stat` читается байтами (`std::fs::read`), родитель берётся после последней `)` (`parent_in_stat`; в имени бывают `)`, пробелы и переводы строки). Других чтений `/proc` в `hoh-external-harness` и `hoh-harness-claude` нет. Сторож (`dash` без окружения, без локали) читает байты и без правки: проверено тестом.
- Тесты: `supervisor::terminating_ends_a_command_whose_name_is_cut_in_a_character` (`terminate`) и `a_name_cut_in_a_character_does_not_hide_a_process` (обход движка и сторож; сторож в тесте запускается без окружения, как в `spawn_guard`), в `apps/hoh/tests/window_executor.rs` `t17_…` — «Стоп» посреди команды `проверка.sh` (подставной claude: `long <имя>`). Со старым чтением `terminate`, обход и `t17_…` падают. Проверки «жив ли процесс» в тестах (`alive`) тоже читают `stat` байтами: иначе такой процесс считался бы мёртвым и тест проходил бы ложно.

## HOH-40 (решение CEO 22.11 21:50): уровень усилия из `session/prompt`

- Окно (u3) называет уровень в параметрах `session/prompt` полем `effort` (строка). SDK-сервер это поле не читает. Слушатель `HohRpcMethodRegistry::on_prompt` (крейт `hoh-sdk-jsonrpc-server`, отдельный пакет) отдаёт параметры каждого `session/prompt` до того, как сообщение может начать ход. `ExternalHarnessWindowPlugin` регистрирует слушатель и записывает уровень последнего запроса каждого разговора в `Choices` (по id сессии HoH). Без `effort` или с `null` записывается «нет уровня»; не строка — то же, с предупреждением в журнале.
- `ExternalHarness::effort_levels()` (по умолчанию пусто) — уровни, которые исполнитель принимает в `StartParams.effort` (новое поле, на проводе `effort`, без значения не пишется). На каждом старте хода (`ensure_live`) слот берёт записанный уровень. Уровень не из списка исполнителя не передаётся, ход идёт на собственном уровне исполнителя, в журнал идёт `harness "…" has no effort level "…": the turn runs without one`.
- Живой процесс с другим уровнем (в том числе уровень сняли) завершается, и ход запускает исполнителя заново на том же нативном разговоре (`--resume`), как при смене режима. Тот же уровень сохраняет процесс.
- Уровень — последнего запроса разговора. Если два запроса стоят в очереди, ход первого начнётся с уровнем второго. Окно шлёт уровень с каждым запросом.
- Тест `apps/hoh/tests/window_executor.rs` `t18_…`: `high` → `--effort high` и `--setting-sources user,project,local`; тот же уровень — тот же процесс; `max` — новый запуск с `--resume` того же id, режим прежний; `turbo` — новый запуск без `--effort` (запись в журнале движка тестом не проверяется: профиль окна журнал не выводит); запрос без уровня — процесс тот же (уровня не было и у него). Слушатель — `crates/hoh-sdk-jsonrpc-server/tests/window.rs` `a_prompt_listener_hears_the_params_of_every_prompt`.
