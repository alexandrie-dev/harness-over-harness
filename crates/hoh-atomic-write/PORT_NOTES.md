# hoh-atomic-write ← packages/util/atomic-write @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Протокол и native Unix process probe перенесены; Windows probe ждёт этапа 5 |
| tests/atomic-write.spec.ts | tests/atomic-write.rs | 29 passed, 0 ignored на Unix; добавлена проверка release при ошибке операции |
| README.md | PORT_NOTES.md | Прочитан; контракт учтён, README не копируется |
| package.json | Cargo.toml | Cargo зависимости; дополнительных runtime entry points нет |

## Отличия от образца
- Async I/O через tokio; mode/dir_mode — u32, wait_ms — u64. with_file_lock и его operation возвращают Result<T, hoh_error::BoxError>: IO и произвольные ошибки операции передаются без обёртки. Дополнительный публичный enum AtomicWriteError содержит только Aborted для обычной некодированной ошибки отмены.
- По решению ведущего PORTING.md §7 FileLockError — пустая структура с NAME и new(PathBuf), возвращающим конкретный HarnessError::named. Дополнительный FileLockErrorCode::FileLockTimeout.as_str() задаёт FILE_LOCK_TIMEOUT; FileLockErrorDetails { lock_path: PathBuf } прикрепляется через with_detail и читается detail::<FileLockErrorDetails>(). name/code/message совпадают с образцом. За BoxError таймаут распознаётся напрямую через HarnessError::of; CodedError, Deref и обёртки ошибок удалены. Протокол записи/takeover/retry при этой адаптации не менялся.
- AbortSignal заменён watch::Receiver<bool>: true отменяет acquisition/wait, после входа cancellation принадлежит операции. Произвольный JS abort reason не представлен.
- Тестовые filesystem/probe hooks приватны; tests/atomic-write.rs подключается модулем unit tests, autotests=false исключает повторную компиляцию файла как integration crate. Тесты таймеров работают с paused tokio clock. Параметризованные записи/платформы сохранены циклами.
- Дополнительное поле FileLockOptions::process_exited позволяет владельцу переопределить проверку PID (true только при ESRCH). По умолчанию на Unix используется безопасный rustix::process::test_kill_process: успех означает живой процесс, ESRCH — завершившийся, EPERM — живой процесс другого пользователя; все остальные ошибки, как в образце, не доказывают завершение (false). Протокол повторно читает record и проверяет PID под hash-claim. На Windows default probe возвращает прежний Unsupported до этапа 5.

## Не перенесено
Нативный Windows signal-zero probe — ждёт этапа 5 по решению ведущего; до этого без переопределения process_exited возвращается Unsupported. Реальный exited-process тест включён на Unix (cfg(unix)); как и в образце, на Windows он не запускается из-за быстрого переиспользования PID. Каталога locale нет.

## Не проверено
Windows/macOS runtime не запускались. Windows retry/EPERM проверены инъекцией на Linux, не реальными sharing violations. Cleanup после уничтожения Rust future извне не эквивалентен JS promise: вызывающий код должен отменять через signal и дождаться возврата.

## Вопросы ведущему
Новых вопросов нет. Windows default probe отложен до этапа 5 по решению ведущего.

