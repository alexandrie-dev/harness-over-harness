# hoh-fs-local ← packages/fs/fs-local @ 39163dd3

## Fix поздней символьной ссылки

Для поздно созданной ссылки native file-watch теперь устанавливается до её
`add` callback. Установка выполняется на blocking worker, а callback ждёт её в
отслеживаемой Tokio-задаче; event callbacks сериализованы, как однопоточные
listener callbacks Chokidar. Поэтому синхронная запись цели из `add` callback
гарантированно даёт последующее уведомление. `FsIoInternals::inspect_watch_link`
— детерминированный test hook сразу после успешного `watch(link)`.

Новый `watch_coalescing` regression без sleep/retry и без повторных записей сначала
падал с `the add callback ran before the followed-link watch was installed`, затем
прошёл 1/0. Существующий тест поздней ссылки и весь coalescing-набор прошли 16/0.

## Fix взаимной блокировки watcher

Callback-поток notify больше не берёт `native` для проверки готовности: состояние
публикуется через `AtomicBool native_ready` после установки watcher и снимается до
close. Поэтому blocking `watch/unwatch`, который держит `native` и ждёт event loop,
не может взаимно заблокироваться с новым Add callback.

Детерминированный regression удерживает pre-watch gate после взятия `native`, создаёт
128 обычных файлов на blocking-потоке и требует, чтобы второй Add прошёл dispatch до
освобождения gate. Затем устанавливается link-watch, цель изменяется ровно один раз,
уведомление приходит, close завершается под timeout. На старой проверке через
`lock(&native)` тест падал 0/1 за 2 секунды; после AtomicBool проходит 1/0.
`before_watch_link` и двухфазный `inspect_watch_add` — только test seams для барьеров.

## Файлы
| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs, src/locks.rs, src/watch.rs | LocalFileSystem, Config, plugin, eager FIFO locks, watcher |
| src/fsio.ts | src/fsio.rs | Linux resolve/probe/lstat/list/read/stream/edit/write/delete |
| src/win32.ts | — | Windows исключён заданием |
| tests/filesystem.spec.ts | tests/filesystem.rs | 79 из 81 объявлений; два Windows исключены |
| tests/fsio.spec.ts | tests/fsio.rs | 76 из 82; шесть Windows исключены |
| tests/patch-mutations.spec.ts | tests/patch_mutations.rs | 19 из 20; один Windows исключён |
| tests/watch.spec.ts | tests/watch.rs | все 7 объявлений, обе file/missing строки; плюс два native notify теста |
| tests/win32.spec.ts | — | четыре Windows-объявления исключены |
| locale/*.json | locale/*.json | восемь файлов скопированы без изменений |
| README.md | — | прочитан; собственного LICENSE нет |
| требование PORTING §6 | tests/short_write.rs | изолированный child с RLIMIT_FSIZE=64 и SIGXFSZ ignore |

Всего сохранено 181 из 194 исходных тестовых объявлений, включая все строки их таблиц; 13 исключений относятся только к Windows. Дополнительные проверки не входят в это число. Тестовые файлы/каталоги создаются только tempfile. Permission-тесты, как в образце, допускают привилегированное окружение, в котором mode=000/0555 не запрещает доступ.

С развёрнутыми it.each это 200 из 213 исходных случаев: filesystem 79, fsio 79, patch-mutations 34, watch 8. Те же 13 Windows-случаев исключены. Дополнительно: два native notify теста, три fsio регрессии (shared/coded ошибки, fallible temp names, Node rm/lstat тексты), один RLIMIT parent-тест и его исполняемый в child ignored helper.

## Отличия от образца
- Linux-only по заданию. Plugin-тип `FsLocalPlugin` с исходным class-row name `LocalFileSystem` предоставляет сервис `fs`; `apply` не создаёт дополнительного сервиса. Config сохраняет cwd и diffBasisMaxBytes, runtime default process.cwd() и 10 MiB; предел Linux Node MAX_STRING_LENGTH = 536870888. cwd: None означает отсутствие настройки при программном new; явный JSON null отвергается. Deserialize заполняет cwd фактическим process.cwd(), поэтому сериализуемая kernel config содержит default; runtime default вычисляется при разборе config вместо загрузки исходного ES-модуля. Схема объявляет cwd optional string с фактическим default cwd при генерации. ResolvedConfig хранит фактический cwd/целый лимит. f64 на входе нужен для исходного отказа fractional/nonfinite/unsafe, в работе лимит u64.
- Абстрактные методы реализованы через текущий hoh-fs. LocalFileSystem::new(Config) создаёт standalone provider, FsLocalPlugin регистрирует его в kernel. Дополнительные публичные Rust-имена: Config/ResolvedConfig, FsLocalPlugin/apply, DEFAULT_DIFF_BASIS_MAX_BYTES/MAX_DIFF_BASIS_BYTES, mutation_lock_count (диагностика исходной private locks.size), watch::{WatchFactory,WatchRequest,WatchRegistration} и методы ignored/notify_path/report_error для исходных spies на chokidar. FsWatcher::close возвращает awaitable освобождение.
- Async I/O идёт через tokio::fs; notify setup/drop и access системного watcher вынесены на blocking worker. FIFO first-key резервируется при вызове метода, затем остальные distinct keys берутся в исходном UTF-16 lexical order. Задача операции владеет locks до завершения, даже если вызывающий уронил future: исходный Promise тоже продолжает работу. Source remove failure после публикации destination остаётся частичным FsMoveOutcome; дефект пробрасывается с исходной SharedError identity.
- Atomic write сохраняет исходный private sibling tmpdir, owner-only 0700/0600, exclusive temp open, fsync, сохранение mode и rename либо hard-link no-replace. Перед sync_all явно вызывается flush (PORTING §6); EFBIG тест проверяет исходные bytes и удаление temp. Как в образце, directory fsync нет, межпроцессного replacement-lock нет, post-commit cleanup best effort. Temp-файл закрывается явно через безопасную `nix::unistd::close(OwnedFd::into_raw_fd())` до публикации (fsio.ts:648): ошибка close отклоняет запись `cannot write "…": EIO: i/o error, close`, прежний файл не тронут; вторая ошибка cleanup-close даёт исходный составной текст `write failed (…) and temp close failed (…)`. Rust-отличие: дескриптор после первого close уже освобождён ядром, поэтому повторный cleanup-close в Rust повторяет только результат хука `FsIoInternals::close_file` (в Node это второй вызов `handle.close()`). Хук `close_file` заменяет результат каждой попытки close (аналог шпиона на дескрипторе); cleanup-failure текст и cause реализованы.
- Версия — та же строка dev:ino:size:mtimeNs:ctimeNs из Linux metadata. ReadWholeText и stream проверяют NUL только в первых 8192 байтах; edit/diff — во всём buffer. UTF-8 fatal, BOM удаляется, результаты edits/moves LF, дисковые CRLF восстанавливаются. Длину выборки line-endings считаем в UTF-16. Bounded bytes читают не более maxBytes+1; diff проверяет opened fd и exclusive bound, chunks <=64 KiB, abort checkpoints до выделения buffer.
- Node path и сообщения errno взяты из общих crates. Для errno без ОС-номера используются исходные синтетические ErrnoError. Публичные дополнительные помощники `*_with_internals`, expand_home_prefix[_with_override] и типы hook (`PathHook`, `TargetHook`, `OperationHook`, `MetadataHook`, `SizeHook`, `NameHook`, `LinkHook`, `PublicationHook`, `StagedHook`, `DiffMetadataHook`, `StagedPaths`, `AtomicWriteOptions`) дают имена исходным анонимным типам и заменяют Node module/descriptor spies. FsIoInternals расширен per-instance metadata/diff/read/defect hooks; home_directory заменяет небезопасный process-wide stub HOME в тестах.
- chokidar заменён notify 8.2.2 по DEPS decision 6. File watcher наблюдает родителя, directory watcher — сам каталог, Access игнорируется; пути фильтруются до callback, initial scan событий не выдаёт. Для missing parent notify наблюдает ближайшего существующего ancestor рекурсивно с точным фильтром; для существующего root NonRecursive. ready означает установленный native watch, close ждёт native drop. События notify приводятся к виду chokidar 5 (`watch::Coalescer`, чистая машина состояний над явными часами; `EventPipeline` — обвязка на tokio-времени): unlink откладывается на 100 мс (`atomic: true`), add внутри окна превращается в change, change по одному пути пропускается с передним фронтом и порогом 50 мс (`_throttle(EV.CHANGE, path, 50)`), flush отдаёт все ещё ожидающие unlink. Отображение notify → kind: Create(Folder)/Remove(Folder) — `addDir/unlinkDir` (без задержки), Create/Remove/rename From/To — add/unlink, остальное — change, `Name(Both)` игнорируется как повтор From+To. Цели символьных ссылок на файлы среди прямых детей наблюдаемого каталога получают собственные native-watch (chokidar `followSymlinks`: `fs.watch(link)` следует по ссылке); для поздней ссылки native-watch устанавливается до `add` callback, а после unlink набор обновляется асинхронно. Event callbacks сериализованы; callback читает готовность watcher из AtomicBool без `native` mutex, `inspect_watch_link` наблюдает успешную установку. Ссылки на каталоги и ссылки глубже depth 0 не отслеживаются (как и события внутри них при `depth: 0`). Публичные `Coalescer`, `EventPipeline`, `WatchEventKind`, `Outcome`, `ATOMIC_WINDOW`, `CHANGE_THROTTLE` — дополнение для тестов с управляемыми часами. Точное число и порядок событий inotify всё же не равны fs.watch Node. Здесь нет исходной настройки awaitWriteFinish/usePolling. Инициализационные ошибки callback/отказ разделяют SharedError, после ready ошибки нормализуются к Error. Сигнал действует только на initialization; после ready ресурс принадлежит close handle, как в образце.
- Таблицы it.each представлены циклами со всеми Linux строками. Строковые JS throws обёрнуты Error на Rust-границе; ошибки сохраняют identity через SharedError. Node module mocks заменены локальными hooks без изменения окружения процесса.
- temp_dir_name/temp_name — fallible `NameHook: Fn(&str) -> Result<String, BoxError>`: ошибка фабрики имени до создания staging возвращается неизменной. Добавлены регрессии на её identity и на shared errno в write/remove/diff. LocalTarget — alias FsTarget; EditText/LiteralEdit — дополнительные имена анонимных исходных результатов readForEdit/applyLiteralEdit. При отмене ожидания ready используется общий AbortError("The operation was aborted") с причиной сигнала, как node:events.once; pre-abort/после metadata используют исходный throwIfAborted.
- В cancellation-тестах отложенный Node I/O представлен отдельным current-thread runtime с одним blocking worker и управляемым gate: abort происходит после первого poll и до завершения I/O, без ожидания по реальному времени. Запись из hook общего coded HarnessError классифицируется как исходный Error с полем code; AbortError при staging становится FsError/FS_ABORTED.
- Native I/O всегда проходит `ErrnoError::from_io` с реальными syscall/путями. Голый Rust io::Error из пользовательского hook остаётся исходным дефектом, а не превращается в FsError с неправильным текстом ОС; регрессия проверяет его identity. Для прочих дефектов без code optional diff-read также не скрывает ошибку.
- Внешний production-потребитель исходных именованных экспортов: fs-sandbox импортирует LocalFileSystem и Config; Rust-наследование заменяется делегированием/trait implementation. Все Linux fsio exports сохранены, но вне самого пакета production-код их не импортирует: FsIoInternals, LocalTarget, PathInfo, PathLinkInfo, LocalDirEntry, expandHomePrefix, localDisplayPath, resolveLocalTarget, probe, probeNoFollow, listDirectory, readWholeText, readWholeBytes, readByteWindow, streamWholeText, writeFileAtomic, removeFile, LineEndings, readForEdit, readTextForDiff, applyLiteralEdit, normalizeLineEndings, restoreLineEndings. Windows readFileDaclWin32/copyFileDaclWin32/replaceFileWin32 отсутствуют согласно заданию; остальных намеренно удалённых экспортов нет.

## Не перенесено
- src/win32.ts и четыре его теста; drive-relative/case/separator ветки; пять atomic Windows DACL/secure-replace тестов, один Windows move-DACL тест и три других Windows path-теста — всего 13 объявлений. Это явное исключение текущего задания, Windows-поддержка не заявляется.

## Не проверено
- Имя класса ошибок Node (`RangeError`, `TypeError`) в `ErrnoError` не хранится; сверяются `code` (`ERR_OUT_OF_RANGE`, `ERR_INVALID_ARG_VALUE`) и текст. Разбиение длинной строки с переводами строки в `util.inspect` (>76 символов) в тексте ERR_INVALID_ARG_VALUE не воспроизводится. Нулевой байт в пути распознаётся при формировании ошибки ввода-вывода (`errno`), а не до вызова, поэтому проверка срабатывает во всех операциях с путём, а имена аргументов (`path`, `oldPath`, `newPath`, `existingPath`) выбираются по имени вызова.
- Windows/macOS, non-UTF8 имена Linux (to_string_lossy соответствует Node utf8 replacement, разные некорректные имена могут совпасть), одиночные UTF-16-суррогаты, NFS/FS с необычным fsync/hard-link/metadata поведением. JavaScript/zod/chokidar для дифференциального исполнения не запускались.
- Сортировка имён: `feruca` 0.13.0 (UCA, таблица CLDR root, non-ignorable, без byte-tiebreak) ≈ ICU `en-US` у `localeCompare`; ожидаемые порядки в `tests/review_round.rs` выведены по правилам UCA/CLDR root (диакритика — вторичный уровень, строчные перед прописными, пунктуация не игнорируется, кириллица по алфавиту, `ё` после `е`), на самом Node не снимались. Другие локали Node не проверены. Native watcher не обещает совпадения количества/порядка callback с chokidar; parent delete/recreate, как и в исходном README, не гарантирован.

## Вопросы ведущему
- `src/shim.rs`: SHIM(hoh-util-values) — общий ICU-compatible Node String.localeCompare пока в этом crate на `feruca` (DEPS 5.6); перенести в общий helper, когда он появится, и снять фикстуру с Node. Другие crates не редактировались.
## Проверка
Актуальные cargo-команды выполнены через `timeout 5m` с CARGO_BUILD_JOBS=2,
CARGO_NET_OFFLINE=true, CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c4.
- `cargo test --offline -p hoh-fs-local`: exit 0; суммарно **229 passed,
  0 failed, 2 ignored**. `watch_coalescing`: 17/0. Вывод `/tmp/c4-fs-local-tests3.log`.
- `cargo clippy --offline -p hoh-fs-local --all-targets --no-deps -- -D warnings`:
  exit 0, предупреждений нет. Вывод `/tmp/c4-fs-local-clippy3.log`.
- `cargo fmt -p hoh-fs-local -- --check`: exit 0, без вывода.
  Вывод `/tmp/c4-fs-local-fmt3.log`.

История исходного порта ниже сохранена как более ранняя проверка до review-regressions.
- `cargo check -p hoh-fs-local --tests`: exit 0.
- Ранний `cargo test -p hoh-fs-local`: exit 0; суммарно 187 passed, 0 failed, 1 ignored. Targets: filesystem 79, fsio 79, patch_mutations 19, short_write 1 passed/1 ignored, watch 9; unit/doc 0.
- ignored limited_child запускается parent-тестом в отдельном процессе: EFBIG, исходный файл и отсутствие temp проверены. Два дополнительных native notify теста прошли.
- Первый прогон filesystem дал 78 passed/1 failed из-за лишнего `/` в helper Env.path(""); helper исправлен, полный заключительный прогон выше прошёл. Прерванная лимитом времени компиляция не считалась ошибкой теста. Остановленные регулятором процессы самостоятельно не возобновлялись.
- После переполнения диска проверены целостность файлов, JSON/TOML и окончания файлов; повреждений не обнаружено. Все восемь locale-файлов побайтно совпадают с образцом. `cargo fmt -p hoh-sandbox -p hoh-fs -p hoh-fs-local -- --check`: exit 0.
- `cargo clippy -p hoh-sandbox -p hoh-fs -p hoh-fs-local --all-targets -- -D warnings`: exit 0; `Finished dev profile [unoptimized + debuginfo] target(s) in 8m 41s`. После ENOSPC запуск повторён; три нулевых permissions-литерала в tests/fsio.rs записаны как 0o0 по требованию Clippy, значение не менялось.
- Заключительный `cargo fmt -p hoh-sandbox -p hoh-fs -p hoh-fs-local -- --check` после lint-правок: exit 0.
