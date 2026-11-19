# hoh-subprocess-local ← packages/subprocess/subprocess-local @ 39163dd3

Состояние на 19.11: повторный прогон ведущего вне песочницы дал **237 passed / 2 failed**
(`logs/pl-110401-hoh-subprocess-local.lead-test.txt`). Первое падение было дефектом теста: fd-baseline смешивался с параллельными
control-тестами. Второе вскрыло production-race: отмена Context во время exec-ack превращала fallback startup rejection
в успешный `SIGTERM`. Проверка fd теперь идёт в отдельном test-процессе; ack различает Context и AbortSignal,
и fallback сохраняет rejection, тогда как native Linux scope по-прежнему может закончиться `SIGTERM`, как в
`local.spec.ts:1351–1375`. Повторный полный
прогон оставлен ведущему, поскольку bind Unix socket в песочнице возвращает `EPERM`.

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | `LocalSubprocessRuntime`, строка service `subprocess`, NoConfig, разрешение executable, terminal environment, владение процессами и teardown. |
| `src/spawn.ts`, `managed-owner.ts`, `control-spawn.ts` | одноимённые модули snake_case | Раздельные direct result / range exit, TERM → grace → KILL, AbortSignal, dispositions, fd 7, ограничения drain, cleanup после обоих результатов. |
| `src/output.ts` | `src/output.rs` | Byte-tail, whole-stream offsets, UTF-8 replacement, spill `wx`/0600, верхний предел полного файла, best-effort failures, sealed readers. Checked close и последняя попытка после failed discard-close — безопасный nix 0.28.0 (DEPS §5.6). |
| `src/linux-scope.ts`, `linux-scope-selector.ts` | одноимённые модули | Capability cache и invalidation, literal argv, private request, establishment, TasksCurrent, waking/backoff, failed-kill proof. |
| `src/linux-execve.ts`, `runner-launch.ts`, `runner-protocol.ts`, `spawn-runner.ts`, `bin.ts` | одноимённые модули и binary `hoh-subprocess-runner` | Rust bootstrap, строгие одноразовые JSON-файлы, env/cwd/argv, PATH/ENOEXEC, CLOEXEC и NONBLOCK только у принадлежащих child pipe ends. |
| `src/process-inspector.ts`, `terminal.ts`, `shell-activity.ts` | одноимённые модули | Linux /proc, identity fences, session/tree ownership, stdin-wait ABI probes, portable-pty, backpressure, Bash/Zsh activity files. |
| `locale/*.json` | `locale/*.json` | Все восемь файлов скопированы без изменения. |
| `tests/*.spec.ts` | `tests/*.rs`, внутренние unit tests | Проверяются stdio/control, хвосты/spill/close/retry, runner JSON/exec, Linux selector/scope, /proc ABI, ownership/teardown и PTY/activity. Матрицы it.each представлены таблицами внутри Rust tests; условия настоящих Zsh/systemd сохранены. Непереносимые случаи перечислены ниже. |

README прочитан. Пакет не содержит собственного LICENSE или указания на заимствованный код.
Зависимости — имеющиеся manifest/workspace и общие helpers, требуемые PORTING; корневые файлы не редактировались.

## Отличия от образца

1. `Plugin::NAME = "LocalSubprocessRuntime"`, `SubprocessRuntime`/service key импортированы из
   hoh-subprocess. Динамический Node loader заменён статическим portable-pty 0.9.0; Linux syscall adapters
   используют безопасный rustix. Здесь нет unsafe, изменения окружения родительского процесса или process::exit.
2. `spawn_subprocess`/`bind_managed_process` принимают Context для принадлежащих провайдеру задач. Shared done
   публикуется уже после начала producer; отмена wait возвращает false, не подменяя причину direct result.
   Kernel cancellation запускает TERM до ожидания фоновых задач. После подтверждённого пустого range сигналы
   больше не отправляются; провал range после done сохраняется и после удаления launch files.
   PTY allocation имеет context-owned lease до передачи созданного процесса его задачам: при отмене Rust
   future выгрузка дожидается blocking allocation и закрывает невостребованный результат. В Node Promise
   продолжает исполняться и без ожидающей стороны; lease сохраняет это владение при Rust cancellation.
3. Rust bootstrap — отдельный executable **рядом с приложением**. При сборке приложения нужно также собрать
   `cargo build -p hoh-subprocess-local --bin hoh-subprocess-runner`. Сборка binary зависимости не происходит
   автоматически от одной сборки приложения. Для собственного упакованного launcher экспортирован
   `spawn_runner::run_selected_subprocess_runner`: его вызывают до создания многопоточного runtime.
   При отсутствии helper остаётся PGID fallback без fd 7; запрос control требует helper и получает явный отказ.
   Этот режим без helper сохраняет родительскую SID (std Command умеет process_group, но не setsid):
   это ослабление относительно Node detached spawn. Для session parity helper обязателен; native capability
   также проверяет его наличие, включая случай отдельного custom target runner.
   Образец наследует fd 7 из Node spawn; безопасный Rust transport соединяет private Unix socket и дублирует
   принадлежащий helper дескриптор на 7. Однопоточный helper атомарно заменяет уже открытый
   inherited fd 7 через `dup2`, как Node. Control socket находится в отдельном коротком `/tmp/dsh-ctl-*`
   каталоге с режимом 0700, поэтому длинный `TMPDIR` не упирается в предел `sun_path`; socket и каталог
   удаляются и при startup rejection до connect. Байты control, half-close и обычные stdout/stderr не смешиваются.
   Для обычного fallback private FIFO `exec-status.fifo` даёт барьер exec, который у Node уже встроен в
   uv_spawn: синхронный spawn возвращает управление после exec либо записи startup error. FIFO имеет 0600,
   находится внутри 0700 launch directory, закрывается при exec через CLOEXEC и не меняет target argv/env.
   Немедленный teardown поэтому сохраняет ENOENT целевой команды, не подменяя его SIGTERM bootstrap.
   Native launcher тоже проходит helper в private `--hoh-detached-exec` mode: setsid выполняется до exec
   systemd-run, сохраняя direct PID как SID/PGID, как Node `detached: true`. Отдельный CLOEXEC FIFO
   подтверждает именно exec launcher, не ждёт запуска target и не потребляет target request. Внутренний
   target helper наследует session/group launcher; private bootstrap marker удаляется при переходе к
   полному target env из request. В fallback helper создаёт собственную session, как прямой detached spawn.
   При наличии helper launch request и exec FIFO всё же обязательно создаются под Node-style `TMPDIR`; поэтому
   полный/readonly TMPDIR может отклонить launch там, где исходный Node spawn не нуждается в этих файлах.
   Ожидание exec acknowledgement асинхронное и прерывается AbortSignal/отменой Context, не блокируя current-thread executor.
   Настоящие SID/PGID и отсутствие marker в target env проверяются без живого systemd.
4. POSIX pipe ends — Unix socket pairs, как у Node, а не std::process FIFO. PTY читает blocking worker,
   собирает неполные UTF-8 кодовые точки и выдаёт те же UTF-8 bytes, которые Node получает из строки node-pty.
   Асинхронные inspections/resize/signalling имеют общий барьер с terminate; завершение cleanup ждёт workers.
5. `SpillStorage` явно принадлежит runtime и передаётся через `SpawnInternals`; для standalone spawns его
   можно разделять явно. Изменяемого process-global singleton нет (PORTING §6). Единственный static — счётчик
   уникальных имён. Пустая private directory удаляется при Drop; завершённые spill files сохраняются.
6. `SpillFailureReporter` возвращает Result вместо JavaScript throw. Неудача reporter не прекращает tail;
   исходные raw stderr diagnostics сохранены как write_all, без println/eprintln и без stdout. Callback получает
   уже зафиксированный tail; это позволяет не держать нерекурсивный mutex при вызове пользовательского кода.
   `SpillIo`/`NativeSpillIo`, `new_with_io`, `new_with_observations` — явные seams вместо глобальных vi mocks.
   `SpillCloseFailure` явно переносит незакрытый File либо номер дескриптора в последнюю попытку seal;
   native close использует `nix::unistd::close` после передачи владения из File, без unsafe и без fsync.
   Как и образец, после последнего failed close seal забывает дескриптор и снимает advertised path.
7. Остальные дополнительные Rust имена описывают владение: LocalError / RunnerFailure, enum причин selector,
   Scope internals/DirectRange/SystemdScopeOwner/Startup и закрытый LinuxScopeKind, ProcStat/FileStatus/Snapshot, TerminalControl,
   binding/storage и их constructors. Ошибки простого Error остаются thiserror enum, Harness subclasses
   берутся из hoh-subprocess. JSON и Node path semantics используют общие helpers.
   Selector/probe/unit_stem возвращают Result для исключений образца, включая отказ randomBytes:
   ошибка не превращается в слабый fallback или повторяющееся имя unit. Текст ошибки native entropy ниже.
   Чтение UTF-8 в протоколе, /proc и activity files использует replacement, как Node; PATH runner намеренно
   собирается конкатенацией из образца и сохраняет symlink-sensitive `link/..`.
   Пути временных каталогов, напротив, проходят Node join/normalize до mkdir, как `join(tmpdir(), prefix)`:
   `//`, `.` и `..` в TMPDIR не меняют видимый текст пути и не требуют существования удаляемого join сегмента.
8. `prepare_linux_terminal_scope` выделен отдельно; обычный `launchLinuxScope` объединён с launch/bind внутри
   `spawn_subprocess` при `SpawnInternals.linux_scope`. Наблюдаемая последовательность подготовка storage →
   launch → direct result / range observation сохранена. Производственные потребители исходных exports:
   ssh/helper использует runtime; subprocess-ssh и ssh/helper-processes — OutputCollector, logSpillFailure,
   prepareManagedProcessBinding; ssh-helper-runtime — runner. Остальные helper exports нужны самому пакету/тестам.
   Rust eagerly наблюдает `range_exit()` после каждого `done`, чтобы гарантировать cleanup owner. В исходнике
   range запрашивается только через terminate/`waitForExit`; адаптация может сделать лишний `systemctl show` или
   `/proc` polling, пока жив оторванный потомок.
9. SubprocessDone принимает SharedError. Primitive/null/undefined причины представлены оболочкой
   SubprocessAbortReason с исходным значением, Display и shared identity; JavaScript typeof не представлен
   типом Future. Proxy caller context отсутствует по PORTING §8: сервис пишет через явно сохранённый
   provider context, поэтому имя logger принадлежит провайдеру. Формат и error logSpillFailure собираются
   в одну строку по docs/kernel.md §7; журнал — диагностический поток, его record count/stack не сравниваются
   с отдельными Node logger arguments. Тексты stderr reporter сохраняются отдельно, без этой адаптации.
10. Внешние production imports используют `LocalSubprocessRuntime`, `runSelectedSubprocessRunner`,
    `OutputCollector`, `logSpillFailure`, `prepareManagedProcessBinding` и тип `SpillFailureReporter`.
    Остальные экспорты модулей `managed-owner`, `control-spawn`, `shell-activity`, `runner-launch`,
    `runner-protocol`, `spawn`, `terminal`, `linux-execve`, `linux-scope`, `linux-scope-selector`,
    `process-inspector`, а также output-типы `SpillOptions`/`ManagedProcessBinding` нужны внутри пакета
    и его тестам, отдельно другими production пакетами не импортируются.
    `controlPipe`/`runnerStdio` объединены с typed stdio/control внутри `launch_and_bind`,
    `loadLinuxExecve` — с `exec_linux_file` без lazy factory, `signalLinuxDirectProcess` —
    с `spawn::signal_direct_process`, `reportSpawnRunnerFailure` — с catch в `run_selected_subprocess_runner`.
    Node `SpawnRunnerInternals` с заменяемыми модулями заменён private `exec_target_with` test seam;
    `LiteralArgvOptions` представлен `Vec<String>`. Это адаптации Node-механизмов, не отдельные Rust exports.
11. Strict startup-error parser возвращает `unknown error result` и для нестрокового `type`, как TypeScript.
    Linux realtime signal ≥ 32 не имеет статического имени в `signal_name`: Rust outcome для завершения таким сигналом
    отдаёт `{ exit_code: None, signal: None }`, тогда как Node отдаёт `signal: null` со своим `exitCode`; это пограничное отличие сохранено.

## Не перенесено

- Windows Job/Win32/inspector и связанные ветки runner/terminal: **ждёт этапа 5**, по заданию Linux only.
- macOS process inspection/platform selection исключены этим Linux-only заданием.
- node-pty/koffi lazy loading, tsx/npm source-versus-built resolution, vi module replacement и упаковочные
  npm e2e как механизм не переносятся (PORTING §8/§10); executable/PTY/runner contracts проверяются в Rust.
- Пять развёрнутых сценариев `process-exit.spec.ts` и проверки порядка/снятия process exit listeners
  из `local.spec.ts`: требуют host-exit hook, см. вопрос 1. Прямые force-kill и обычный async teardown
  проверяются; это не считается проверкой процесса, завершившегося через exit/uncaught rejection.
- Утверждения, относящиеся только к Node EventEmitter/Stream internals (повторный error/exit callback,
  pause/resume call counters, mutable stdio tuple и загрузка addon), заменены проверками соответствующих
  результатов и владения Rust streams/oneshot/PTY workers; невозможные в закрытых Rust типах null поля не
  фабрикуются через ослабление типов. Исходная проверка Windows Job/console host здесь не заявляется.

Сценарии из `spawn.spec.ts`, отмеченные ревью, покрыты так:

- :278 — `spawn::tests::cancels_escalation_when_the_group_vanishes_after_term_before_pipes_drain`;
- :347 и :1549 — `kills_the_process_group_with_sigterm_when_the_signal_fires`;
- :549 — `output::contains_close_failures_and_drops_the_spill_path`;
- :906 — `terminates_the_whole_process_group_grandchildren_die_too`;
- :962 — `an_aborted_wait_reports_false_while_the_tree_lives`, затем terminate и true;
- :1335 — `repeated_terminate_after_exit_never_signals_a_reused_process_group`;
- :1349 — Linux-адаптация `force_kills_the_current_process_tree_without_waiting_for_the_normal_grace`;
  process-exit hook остаётся отложен вопросом 1;
- :1538 — public `kill_group` игнорирует любую ошибку signal, а unit-матрица
  `final_kill_targets_group_and_direct_and_probes_after_failed_direct_submission` проверяет ветку отказа, включая `PERM`.

## Проверка

- Полный прогон ведущего до текущих test/code fixes: **237/2**,
  `logs/pl-110401-hoh-subprocess-local.lead-test.txt`; обе причины описаны в шапке и исправлены.
- После test-fixes `cargo check -p hoh-subprocess-local --tests` и clippy завершились с exit 0; fmt check чист,
  `cargo test -p hoh-subprocess-local --test local` — **12/0**, фокусный delayed-ack/AbortSignal — **1/0**.
  В предыдущем sandbox-safe наборе пройдены ещё 62/0: lib 50/0, runner_protocol 10/0 и два фокусных spawn-сценария 2/0.
- Новые regression tests на 50 startup failures/control-fd leak, 100-byte `TMPDIR` и inherited open fd 7
  скомпилированы. Они не ослаблены и не ignored; fd-leak сценарий изолирован от параллельных tests.
  Их bind Unix socket в песочнице даёт `EPERM`;
  свежий полный прогон этого круга должен выполнить ведущий вне песочницы.
- В source tests находится 341 объявление `it` до раскрытия `it.each`, в том числе 37 объявлений
  в трёх отдельных Windows-only suites. Число Rust test functions не является числом исходных
  случаев: таблицы объединены, часть Linux проверок находится в unit tests. Полная численная
  сверка каждого исходного случая пока не подтверждена; полного покрытия всех 341 не заявляем.
- Настоящий user-systemd и reparented setsid descendants требуют доступного user manager; unit seams не
  считаются проверкой живого manager. Финальный прогон отметит доступные real-process/PTY случаи.
- Настоящие delayed EIO от close и отказ системного источника entropy не инъецируются в ядро ОС;
  close/retry и отказ reporter проверяются fallible filesystem seam.

## Вопросы ведущему

1. **SHIM(hoh-kernel), `src/index.ts:104–115, 122–141`.** Node prepend exit listener синхронно force-kills
   retained ranges при process.exit/uncaught/default-unhandled-rejection, оставаясь установленным до quiescence.
   Kernel имеет async fiber teardown, но не такой host-exit emitter. Публичный terminate_for_host_exit выполняет
   тот же обход и содержит отказ одного target; приложению нужен pre-exit hook, вызывающий его перед выходом.
   Нормальный async disposal реализован; прямой exit/abort процесса Rust этот shim не перехватывает.
2. **SHIM(hoh-subprocess), `src/runner-protocol.ts:242–255`, `src/linux-scope.ts:271–273`.** Strict runner record
   содержит name/message/code/syscall/path. Shared classifier узнаёт SubprocessForeignError::Coded, который
   хранит только message/code. Для startup rejection shim сохраняет именно их, чтобы bash получал тот же
   SUBPROCESS_SPAWN_NOT_FOUND/FAILED; syscall/path/nondefault name остаются доступны в отдельном RunnerFailure
   при явном decode, но не у этого classified rejection. Минимальное дополнение — общий foreign diagnostic
   record с этими полями, распознаваемый classifier без превращения обычной Error в HarnessError.
3. `terminalEnvironment` использует SHELL, затем Linux `/etc/passwd` по uid. У node:os.userInfo используется
   системный passwd/NSS resolver. Private shim не моделирует NSS providers и исключение userInfo;
   требуется общий Node adapter для NSS и его ошибок. Нестандартные NSS providers не проверены.
4. Proxy installation в hoh-subprocess передаётся явно. Здесь child_env пока вызывает scrubbed_parent_env(None),
   а явные caller env overlays сохраняются. Нужен путь передачи provider-owned installation от bootstrap;
   ambient process.env и global dispatcher по правилам переноса не изменяются.
5. **SHIM(hoh-util-values), `src/runner-launch.ts:284–289`.** Private inspect_string воспроизводит выбор
   кавычек и базовые escapes NUL/newline/tab. Полный node:util.inspect(string), включая остальные control
   escapes, maxStringLength и перенос длинных многострочных строк, требует общего adapter; эти пограничные
   diagnostics пока не заявлены byte-for-byte.
6. **SHIM(hoh-util-values), `src/runner-protocol.ts:105–124`, `src/output.ts:37–39`, `src/linux-scope.ts:97–99`.**
   Общего Node host adapter для `os.tmpdir`/ошибок `mkdtemp` и `crypto.randomBytes` нет. Private helper выбирает
   TMPDIR → TMP → TEMP → /tmp без изменения env; успешные private dirs — tempfile, entropy — getrandom.
   Редкие тексты отказа tempfile/getrandom пока нативные Rust, совпадение с Node/OpenSSL не заявлено.
   Нужен общий adapter с ошибками Node; при отказе entropy target не запускается и ID не подменяется.
7. **SHIM(node:child_process), `src/control-spawn.ts:12–20`, `src/runner-launch.ts:119–140`.**
   Node передаёт анонимный duplex child fd 7 непосредственно через stdio mapping. Разрешённый safe Rust
   spawn API поддерживает только fd 0/1/2; прямого атомарного extra-fd mapping в используемых rustix/nix
   и portable-pty API нет. Текущий private Unix socket bridge в коротком 0700 каталоге сохраняет duplex stream,
   но требует bind/connect; в этой среде bind возвращает EPERM. Helper-side `dup2` теперь точно заменяет
   inherited fd 7, но не устраняет сам мост. Для устранения этого
   ограничения нужен согласованный safe spawn adapter с передачей owned fd в child fd 7 (например,
   через posix_spawn file actions), без временной замены fd или CLOEXEC в многопоточном родителе.
   Новая зависимость и unsafe в своём crate не добавлялись; ограничения среды не обходились.
