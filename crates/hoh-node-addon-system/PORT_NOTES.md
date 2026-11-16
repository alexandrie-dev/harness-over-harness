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
- `try_lock_exclusive` синхронный по заданию. На Unix принимает `AsFd`, возвращает
  `io::Result<()>`; положительный числовой syscall errno доступен через `raw_os_error()`:
  EAGAIN/EWOULDBLOCK — contention, остальные коды передаются без замены. JS-поля
  `code`, `errno`, `syscall` и текст `${code}: flock failed` заменены стандартной Rust-ошибкой.
  Вне поддерживаемых Linux/macOS x64/arm64 возвращается `ErrorKind::Unsupported` с
  `ERR_FLOCK_UNSUPPORTED_PLATFORM: flock is not supported on <platform>-<arch>`;
  на не-Unix параметр — `&File`, так как `AsFd` там отсутствует. macOS сохраняет flock,
  На macOS probe сохраняет запуск переданного launcher, как образец; стандартный launcher
  отсутствует, а запуск Rust-бинарника завершается с ошибкой unsupported Landlock.
  На не-Unix probe возвращает `Unusable` без запуска.
- Независимый C oracle тестов заменён установленным `/usr/bin/flock` (util-linux);
  он проверяет shared/exclusive locks в обе стороны через настоящий отдельный процесс.
  Неверный fd проверяется безопасно созданным `O_PATH` дескриптором: flock возвращает EBADF;
  передать отрицательный или закрытый fd через `AsFd` безопасный Rust не позволяет.
- `rustix` feature `stdio` включён только в dev-dependencies для передачи и закрытия
  унаследованного fd в дочерней тестовой программе. Новых зависимостей нет.

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

На Linux выполнены `cargo build`, `cargo test` (45 passed: entry 9, flock 16, launcher 20;
0 failed, 0 ignored), `cargo clippy --all-targets -- -D warnings`.
Ни один confinement-тест sandbox не заблокировал. Ревью исходного C и Rust выполнено только чтением.

- Ветки macOS (включая настоящий flock), Windows и прочих OS/архитектур не собирались и
  не запускались по заданию. Ошибка unsupported выражается в стандартном Rust API, как описано выше.
- Реальные ядра ABI 1–4, disabled/unsupported Landlock, отказы prctl/create/add/restrict,
  исчерпание памяти/дескрипторов не воспроизводились. Права и решение full/partial сверены с C.
- Отказ выделения памяти при разборе argv и резервировании grant-списков возвращает
  `landlock-run: out of memory` и 125, как C: 148–150. OOM внутри получения `args_os`,
  форматирования диагностики и `std::process::Command` следует политике allocator Rust:
  полностью fallible API для этих операций стандартная библиотека не предоставляет.

## Вопросы ведущему

Нет вопросов, блокирующих использование. Для следующего CI полезен запуск на ядрах ABI 1–4 и macOS.
