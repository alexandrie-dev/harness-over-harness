# hoh-node-addon-system ← native/system @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `packages/entry/src/main.c` | `src/launcher.rs`, `src/bin/landlock-run.rs` | грамматика, диагностика, confinement и exec перенесены |
| `packages/entry/src/index.ts` | `src/entry.rs`, реэкспорты `src/lib.rs` | константы, grants, probe, путь launcher перенесены |
| `packages/entry/src/flock.c`, `flock.ts` | `src/flock.rs` | один `LOCK_EX | LOCK_NB`, заимствованный fd |
| `docs/cli-contract.md`, `flock-contract.md`, `support-matrix.md` | реализация и интеграционные тесты | контракты учтены |
| `test/launcher.test.js` | `tests/launcher.rs` | все поведенческие случаи и дополнительные проверки задания |
| `test/entry.test.js` | `tests/entry.rs` | константы, grant argv, probe; разрешение npm заменено проверкой sibling-пути |
| `test/flock.test.js` | `tests/flock.rs` | все применимые syscall/process-сценарии; исключения перечислены ниже |
| `LICENSE` | `LICENSE` | BSD-3-Clause, копия без изменений |

### Права и ABI

Источник таблицы — `packages/entry/src/main.c`; номера строк относятся к образцу.
В Rust используется только `AccessFs::from_all(ABI::V5)`, никогда актуальная максимальная версия
библиотеки: права ABI 6–9, TCP-права, scopes и новые флаги не запрашиваются.

| ABI ядра | handled / directory `--rw` | directory `--ro` | non-directory `--rw` | non-directory `--ro` | строки C |
| --- | --- | --- | --- | --- | --- |
| 1 | EXECUTE, WRITE_FILE, READ_FILE, READ_DIR, REMOVE_DIR, REMOVE_FILE, MAKE_CHAR, MAKE_DIR, MAKE_REG, MAKE_SOCK, MAKE_FIFO, MAKE_BLOCK, MAKE_SYM; биты 0–12 (`0x1fff`) | EXECUTE, READ_FILE, READ_DIR (`0x000d`) | EXECUTE, WRITE_FILE, READ_FILE (`0x0007`) | EXECUTE, READ_FILE (`0x0005`) | 71–88, 184–189, 207–208, 244–250 |
| 2 | предыдущие + REFER (`0x3fff`) | как ABI 1 | как ABI 1 | как ABI 1 | 84, 187, 207–208, 244–250 |
| 3 | предыдущие + TRUNCATE (`0x7fff`) | как ABI 1 | EXECUTE, WRITE_FILE, READ_FILE, TRUNCATE (`0x4007`) | как ABI 1 | 85, 188, 207–208, 244–250 |
| 4 | как ABI 3 (`0x7fff`); TCP не обрабатывается | как ABI 1 | как ABI 3 | как ABI 1 | 85, 184–189, 238–240 |
| 5 и выше | предыдущие + IOCTL_DEV (`0xffff`), максимум ABI 5 | как ABI 1 | EXECUTE, WRITE_FILE, READ_FILE, TRUNCATE, IOCTL_DEV (`0xc007`) | как ABI 1 | 86, 94, 189, 207–208, 237–250 |

Первое `handle_access(V1)` с `HardRequirement` отвергает недоступный Landlock **до открытия grants**
(C: 231–235). Затем `BestEffort` пересекает запрошенные права V5 с возможностями ядра.
`PathFd` открывает root с `O_PATH | O_CLOEXEC`; права файла сужаются явно по `fstat`
до `AccessFs::from_file(V5)`. Сначала добавляются все `--ro`, затем все `--rw`, как C: 244–252.
`no_new_privs(true)` и успешный `restrict_self()` обязательны, `NotEnforced` не допускает exec.

Статус протокола берётся из ABI в `RestrictionStatus.landlock`: ABI ≥ 5 → `full`,
ABI 1–4 → `partial`, отсутствие enforcement → ошибка 125. Не используется сам по себе
`RulesetStatus::PartiallyEnforced`: библиотека может считать автоматическое удаление
directory-only прав у файлов частичным enforcement, тогда как C этого не делает.
Поэтому права файлов сужаются до передачи библиотеке; последняя также согласует права со старым ABI.
Probe пишет ровно строку C: 281, обычный partial-run — строку C: 292.

### Линковка launcher

Поставляется **только статический** файл, собранный ведущим через `scripts/build-landlock-run.sh`:
`-C target-feature=+crt-static`, отдельный target, release; скрипт проверяет отсутствие `INTERP`
и `NEEDED` в ELF. Обычный `cargo build`/`cargo test` создаёт динамический `landlock-run` для тестов:
этот файл **нельзя поставлять**, его loader обрабатывает LD-переменные до установки ruleset.
Статический файл не загружает библиотеки из `LD_PRELOAD`, `LD_LIBRARY_PATH`, `LD_AUDIT`.

Для запуска тестов с поставляемым файлом служит только тестовый `HOH_LANDLOCK_RUN`:
в `tests/launcher.rs` и `tests/entry.rs` по одному helper выбирает этот путь либо
`CARGO_BIN_EXE_landlock-run`. Сам launcher не использует эту переменную и не меняет окружение.
`static_launcher_ignores_loader_variables` — единственный ignored-тест обычной сборки;
скрипт запускает его с `--include-ignored` и статическим файлом.

Статическая glibc здесь отличается от статической musl образца. После установки ruleset
glibc `execvp` при ENOEXEC запускает файл без shebang через `/bin/sh` (ведущий измерил exit 0);
musl возвращает `Exec format error`, и исходный launcher завершился бы с 125.
При отсутствующем `PATH` применяется default search path glibc вместо musl;
тексты некоторых редких errno тоже отличаются. Кроме того, glibc обрабатывает
`GLIBC_TUNABLES` при старте даже статического executable; отсутствие динамического loader
не означает полного игнорирования этой переменной. Это остаётся отличием от musl.

### Проверка чувствительности новых тестов

Тесты написаны до временных изменений реализации. Каждая мутация после наблюдаемого
падения восстановлена; в поставляемом коде мутаций нет.

| тест | временное нарушение | наблюдаемое падение |
| --- | --- | --- |
| `static_launcher_ignores_loader_variables` | обычный динамический бинарник | stderr содержит ошибки loader для LD_AUDIT и LD_PRELOAD вместо пустой строки |
| `refer_allows_links_inside_read_write_grant_and_refuses_imports` | REFER удалён из handled и `--rw` | `os.link('W/a/f', 'W/b/g')` получил EXDEV (18) вместо успеха |
| `truncate_by_path_requires_the_truncate_right` | TRUNCATE удалён из handled и `--rw` | path-truncate вернул 0 вместо EACCES (13) |
| `ioctl_on_devices_requires_the_ioctl_dev_right` | IOCTL_DEV удалён из handled и `--rw` | FIONREAD вернул ENOTTY (25) вместо EACCES (13) |
| `full_or_partial_notice_strictly_matches_the_probe` | partial notice печатается и при Full | лишняя строка stderr вместо пустой строки |
| `read_write_regular_file_grant_allows_read_overwrite_and_truncate_but_no_sibling` | file-права сужены до чтения | `open('target', 'r+b')` получил EACCES |
| `read_only_regular_file_grant_allows_reading_but_not_writing` | file-grant получил write-права | запись успешна (0) вместо EACCES (13) |
| `commands_without_execute_permission_or_naming_a_directory_fail` | exec failure возвращает 0 | статус 0 вместо 125 |
| `command_sees_sigpipe_with_default_disposition` | перед командой shell явно игнорирует SIGPIPE | `SigIgn` содержит бит 13 (`0x1000`) |
| `grant_args_preserves_non_utf8_paths` | lossy UTF-8 преобразование пути | `0xff` заменён на U+FFFD |

REFER, path-TRUNCATE и device-IOCTL тесты требуют probe Full (ABI ≥ 5) и явно падают
при любом другом результате. Прямые системные операции выполняет `/usr/bin/python3 -I`;
отсутствие Python приводит к ошибке с именем инструмента. Link выполняется через `os.link`,
truncate — через `os.truncate(path, 0)` без открытия файла на запись, ioctl — FIONREAD на `/dev/zero`.
Все обычные файлы, hard links и каталоги принадлежат `tempfile`; device-тест открывает `/dev/zero`
только для чтения. Проверки stderr успешных confined-команд строго следуют результату probe:
Partial требует ровно notice, Full — пустую строку.

## Отличия от образца

- `run_launcher(args, stdout, stderr)` — дополнительный публичный Rust API вместо C `main`;
  `args` не содержит argv[0], тип элементов `OsString`, writers — `&mut dyn Write`, результат `u8`.
  Неверную грамматику можно проверять без процесса. Успешный вызов необратимо ограничивает вызывающий
  поток; для него необходим отдельный процесс. Тесты никогда не ограничивают поток test runner.
  Бинарный `main` из трёх строк возвращает `ExitCode`, без `process::exit`.
- `LauncherGrants` содержит `Vec<PathBuf>` вместо необязательных массивов строк; пустой вектор
  эквивалентен отсутствию поля. `grant_args` возвращает `Vec<OsString>`, сохраняя байты Unix-путей.
- `launcher_path() -> io::Result<PathBuf>` возвращает `landlock-run` рядом с текущим executable,
  не проверяя существование. Нpm platform-package resolution отсутствует по заданию;
  ошибка `current_exe` выражается через `Result`, probe превращает её в `Unusable`.
- `probe(Option<&Path>, Option<Duration>)` синхронный; `None` выбирает launcher_path и 2000 мс.
  Нулевой timeout отключает deadline, как Node. Читает только stdout, ищет подстроку
  `partially enforced` при exit 0; пустой/иной stdout означает `Full`. Неуспех, сигнал,
  timeout, spawn/read failure или превышение Node maxBuffer 1 МиБ означают `Unusable`.
  Pipe читается неблокирующе, дочерний процесс завершается и reap-ится при отказе.
  После timeout отправляется SIGTERM; затем `probe` ждёт завершения ребёнка без ограничения,
  как `spawnSync`: ребёнок, игнорирующий SIGTERM, может задержать возврат дольше timeout.
- `try_lock_exclusive` синхронный по заданию. На Unix принимает `AsFd`, возвращает
  `io::Result<()>`; положительный числовой syscall errno доступен через `raw_os_error()`:
  EAGAIN/EWOULDBLOCK — contention, остальные коды передаются без замены. JS-поля
  `code`, `errno`, `syscall` и текст `${code}: flock failed` заменены стандартной Rust-ошибкой.
  На любом Unix вызывается `flock`, без ограничения архитектуры или списка npm-пакетов.
  На не-Unix возвращается `ErrorKind::Unsupported` с
  `ERR_FLOCK_UNSUPPORTED_PLATFORM: flock is not supported on <platform>-<arch>`;
  на не-Unix параметр — `&File`, так как `AsFd` там отсутствует.
  На macOS probe сохраняет запуск переданного launcher, как образец; стандартный launcher
  отсутствует, а запуск Rust-бинарника завершается с ошибкой unsupported Landlock.
  На не-Unix probe возвращает `Unusable` без запуска.
- Независимый C oracle тестов заменён установленным `/usr/bin/flock` (util-linux);
  он проверяет shared/exclusive locks в обе стороны через настоящий отдельный процесс.
  Неверный fd проверяется безопасно созданным `O_PATH` дескриптором: flock возвращает EBADF;
  передать отрицательный или закрытый fd через `AsFd` безопасный Rust не позволяет.
- `rustix` feature `stdio` включён только в dev-dependencies для передачи и закрытия
  унаследованного fd в дочерней тестовой программе. Новых зависимостей нет.
- Rust runtime при старте открывает `/dev/null` вместо закрытого стандартного дескриптора;
  C launcher передаёт закрытый дескриптор команде как есть. SIGPIPE игнорируется Rust runtime,
  но `CommandExt::exec` восстанавливает default перед `execvp`: бит SIGPIPE в `SigIgn` команды
  проверяется тестом. Маска заблокированных сигналов наследуется без изменений.

## Не перенесено

- `test/package-matrix.test.js`, `test/release-packing.test.js`, проверка отсутствия root npm export,
  инъекция resolver платформенного npm package, fallback в `node_modules`: npm packaging и
  механизм установки отсутствуют в Rust (разделы 8, 10 правил; замена launcher_path дана заданием).
- Из `test/flock.test.js`: `import succeeds with native addons disabled`, подмена `process.platform`
  для `win32`/`freebsd`, асинхронность Promise, ошибки JS-аргументов, callback invocation/throw и
  worker termination в `queued`/`callback`. Это исключительно Node-API/JS lifecycle без аналога
  в синхронном типизированном вызове Rust. Ошибки syscall, конкуренция, advisory I/O, наследование
  fd, закрытие последнего владельца, нормальное завершение и SIGKILL проверяются процессами.
- C/Node-API binding, сборка C oracle, platform artifacts, npm release scripts не нужны порту.

## Не проверено

### Стабилизация полного workspace-прогона 2026-11-19

- Тест последней унаследованной ссылки на flock иногда получал `EWOULDBLOCK` после подтверждённого
  `close`: параллельный тест мог сделать `fork` в момент, когда transport-fd ещё существовал, и
  его ребёнок временно держал ту же open-file-description до закрывающего CLOEXEC `exec`.
  Верхнеуровневые flock-сценарии одного test binary теперь сериализованы; их собственные потоки и
  дочерние процессы по-прежнему выполняются конкурентно. Это убирает только постороннее
  наследование между параллельными тестами; проверяемый ребёнок всё ещё получает настоящий fd,
  parent закрывает свою ссылку, lock остаётся до команды child `close`, после чего contender
  обязан немедленно захватить lock.

Счётчики `cargo test` включают `flock_child`: это entry point дочернего helper-процесса,
а не самостоятельный поведенческий случай. Итоги fix-round на Linux:

| команда | результат |
| --- | --- |
| `cargo test -p hoh-node-addon-system` | entry: `10 passed; 0 failed; 0 ignored`; flock: `16 passed; 0 failed; 0 ignored`; launcher: `28 passed; 0 failed; 1 ignored`; library/bin/doc-tests: по 0 тестов |
| `bash scripts/build-landlock-run.sh .` | exit 0; entry: `10 passed; 0 failed; 0 ignored`; launcher: `29 passed; 0 failed; 0 ignored`; ELF без loader и shared libraries |
| `cargo clippy -p hoh-node-addon-system --all-targets -- -D warnings` | `Finished dev profile`, exit 0, предупреждений нет |
| `cargo fmt -p hoh-node-addon-system -- --check` | exit 0, вывода нет |

Каждая команда выполнена с `CARGO_NET_OFFLINE=true` и
`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-p1`; скрипт дополнительно с
`HOH_LANDLOCK_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-p1-static`.
Проверенный статический launcher:
`/mnt/toshiba/alexandrie/hoh/target-p1-static/x86_64-unknown-linux-gnu/release/landlock-run`.
Обычный прогон имеет ровно один ignored-тест — только проверку LD-переменных для статической сборки.
Ни один confinement-тест sandbox не заблокировал. Ревью исходного C и Rust выполнено только чтением.

- Ветки macOS (включая настоящий flock), Windows и прочих OS/архитектур не собирались и
  не запускались по заданию. Ошибка unsupported выражается в стандартном Rust API, как описано выше.
- Отказ на ядре без Landlock не проверен: такого ядра в окружении нет.
  Реальные ядра ABI 1–4, disabled Landlock, отказы prctl/create/add/restrict,
  исчерпание памяти/дескрипторов не воспроизводились. Права и решение full/partial сверены с C.
- Отказ выделения памяти при разборе argv и резервировании grant-списков возвращает
  `landlock-run: out of memory` и 125, как C: 148–150. OOM внутри получения `args_os`,
  форматирования диагностики и `std::process::Command` следует политике allocator Rust:
  полностью fallible API для этих операций стандартная библиотека не предоставляет.

## Вопросы ведущему

Нет вопросов, блокирующих использование. Для следующего CI полезен запуск на ядрах ABI 1–4 и macOS.
