# hoh-fs ← packages/fs/fs @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | FileSystem, сервис fs, четыре исходных события |
| src/types.ts | src/types.rs | бренды, метаданные, guards, outcomes, FsError и 16 кодов |
| src/errno.ts | src/errno.rs | классификация errno, исходная причина, пропуск дефектов |
| tests/errno.spec.ts | tests/errno.rs | оба объявления; 6 строк классификации и 4 identity |
| tests/service.spec.ts | tests/service.rs | все 13 объявлений |
| README.md | — | прочитан; LICENSE/locale в образце отсутствуют |

Все 15 исходных объявлений, то есть 23 случая с развёрнутыми it.each, сохранены. Таблицы исполняются внутри двух Rust-тестов errno; остальные 13 — отдельные tests/service.rs.

## Отличия от образца
- `FileSystem` — объектобезопасный trait, `Fs` — сервисный ключ `dyn FileSystem` с именем `fs`. Абстрактный пакет не имеет собственного plugin apply. Default watch сохраняет исходную проверку отмены и текст отказа; sandbox_mode/process_path_from_host_path возвращают None.
- У всех методов сохранены примитивы и guards; поля/строковые теги JSON и порядок outcomes совпадают. FsTargetKey/FsVersion объявлены общим branded_string; исходные одноимённые функции-конструкторы представлены From/brand_string. `FsMoveOutcome` — enum Deleted/Retained с ручной serde-реализацией исходного булевого тега sourceDeleted. before/sourceBefore None — явный null; остальные исходные optional-поля опускаются.
- `FsError` — фабрика общего HarnessError, NAME = FsError; `with_cause` соответствует options.cause. Для dynamic errno добавлен `ErrnoError` с optional string code, исходным message и io-причиной; from_io использует общие Node helpers с фактическими syscall/path/destination. fs_error_from_errno понимает SharedError и общие coded HarnessError. Голый io::Error без Node code/message не классифицируется автоматически: это дефект неперенесённой границы I/O и он возвращается неизменным; иначе в FS_IO_ERROR оказался бы Rust Display вместо Node-текста. Это дополнительно проверено на raw ELOOP.
- BoxError не может содержать брошенный JS primitive непосредственно: тест такой строки использует собственную Error-обёртку, числовой code — отдельную тестовую структуру с i32. Их исходная идентичность не меняется. Нестроковый code нельзя ошибочно трактовать как строковый ErrnoError::code.
- Дополнительные публичные имена для исходных анонимных типов/механики Rust: FsFuture, FsTextStream, FsChanged, FsWatcher (close вместо возвращаемой async-функции), FsActor (Arc<dyn Any>), FsVersionGuard, FsResolveOptions, FsByteRange, FsMoveOptions, FsWriteOperation, FsType, FsPathType, FsMoveSourceFailure. События — FsWriteIntentEvent, FsEditIntent, FsDeleteIntent, FsObserved; исходные имена fs/write-intent, fs/edit-intent, fs/delete-intent, fs/observed и режимы waterfall/emit сохранены. SandboxMode/SandboxExecutionPolicy реэкспортированы для implementors.
- Числа размеров и диапазонов — u64, типы событий несут opaque actor Arc. Невозможные в Rust отрицательные/дробные диапазоны, одиночные суррогаты не моделируются.
- Таблицы it.each представлены циклами без удаления строк. FakeFileSystem в tests — тот же неполный тестовый backend: patch/delete/move возвращают исходный unsupported и не являются заглушками production.
- Экспорты проверены по fs-local/fs-sandbox/fs-ssh, tool-fs/tool-str-replace-edit и observation-policy, skills/workspace/attachment. Все исходные экспорты сохранены; `FsTargetKey`/`FsVersion` как функции имеют только Rust-конструкторы. Внешних production-импортов нет у FsMoveSourceFailure; этот исходный экспорт тоже сохранён.

## Не перенесено
Нет исходных файлов или тестовых объявлений, исключённых из переноса.

## Не проверено
Нестандартные JavaScript thrown-values кроме документированной Rust-обёртки; платформенные errno за пределами Linux. Конкретные файловые операции проверяет hoh-fs-local.

## Вопросы ведущему
Нет отсутствующих API и shim. Общие типы отмены и ошибок импортированы, новые параллельные типы не введены.

## Проверка
Все cargo-команды: CARGO_BUILD_JOBS=2, CARGO_NET_OFFLINE=true, CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w4.
- `cargo check -p hoh-fs --tests`: exit 0.
- `cargo test -p hoh-fs`: exit 0; 15 passed, 0 failed суммарно (errno 2, service 13; unit/doc 0).
- После уточнения Node/Rust boundary: `cargo test -p hoh-fs --test errno`: exit 0, 2 passed, 0 failed, включая identity голого raw ELOOP без Node code/message.
- После переполнения диска проверены целостность файлов, JSON/TOML и окончания файлов; повреждений не обнаружено. `cargo fmt -p hoh-sandbox -p hoh-fs -p hoh-fs-local -- --check`: exit 0.
- `cargo clippy -p hoh-sandbox -p hoh-fs -p hoh-fs-local --all-targets -- -D warnings`: exit 0; `Finished dev profile [unoptimized + debuginfo] target(s) in 8m 41s`.
- Заключительный `cargo fmt -p hoh-sandbox -p hoh-fs -p hoh-fs-local -- --check` после lint-правок: exit 0.
