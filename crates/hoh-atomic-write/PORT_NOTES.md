# hoh-atomic-write ← packages/util/atomic-write @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Протокол и native Unix process probe перенесены; Windows probe ждёт этапа 5 |
| tests/atomic-write.spec.ts | tests/atomic-write.rs | Все исходные случаи и дополнительные проверки причин отмены, Drop, PID и raw errno |
| README.md | PORT_NOTES.md | Прочитан; контракт учтён, README не копируется |
| package.json | Cargo.toml | Cargo зависимости; дополнительных runtime entry points нет |

## Отличия от образца
- Async I/O через tokio; mode/dir_mode — u32, wait_ms — u64. with_file_lock и его operation возвращают Result<T, hoh_error::BoxError>: IO и произвольные ошибки операции передаются без обёртки. Временный enum AtomicWriteError удалён: ошибки отмены теперь имеют общий тип hoh_timeout::AbortError или представляют исходный SharedError причины.
- По решению ведущего PORTING.md §7 FileLockError — пустая структура с NAME и new(PathBuf), возвращающим конкретный HarnessError::named. Дополнительный FileLockErrorCode::FileLockTimeout.as_str() задаёт FILE_LOCK_TIMEOUT; FileLockErrorDetails { lock_path: PathBuf } прикрепляется через with_detail и читается detail::<FileLockErrorDetails>(). name/code/message совпадают с образцом. За BoxError таймаут распознаётся напрямую через HarnessError::of; CodedError, Deref и обёртки ошибок удалены. Протокол записи/takeover/retry при этой адаптации не менялся.
- FileLockOptions.signal — Option<hoh_timeout::AbortSignal>. Оба throwIfAborted (до acquisition и перед operation) выбрасывают сам SharedError причины с сохранением ptr_eq. Abortable wait, как node:timers/promises, возвращает новый общий AbortError с точным текстом "The operation was aborted" и cause = исходный SharedError; он узнаётся через is_abort_error. После входа cancellation принадлежит операции. Дополнительный AbortError::with_cause введён в hoh-timeout именно для этого контракта.
- По решению ведущего добавлен приватный LockGuard: после acquisition он владеет записью держателя, при Drop future синхронно best-effort удаляет lock, только если запись всё ещё та же. На Unix дополнительно сверяются dev/ino и удерживается File исходного inode до окончания cleanup, чтобы отмена старого удаления не задела нового держателя с тем же PID. На обычном завершении сохраняется async remove, guard снимается после его успеха. Тест timeout вокруг уже начатой незавершающейся operation проверяет немедленное повторное acquisition; другие тесты сохраняют подменённые record/inode.
- transient на Unix принимает ровно raw EACCES, EBUSY, EPERM (rustix Errno::ACCESS/BUSY/PERM); ErrorKind без raw code не подходит. На Windows явно сопоставлены Win32 5 (access denied), 1314 (privilege not held), 32/33 (sharing/lock violation), 170 (busy); широкие проверки PermissionDenied/ResourceBusy удалены. Windows mapping не запускался на Windows, дополнительный cfg(windows) тест готов.
- Тестовые filesystem/probe hooks приватны; tests/atomic-write.rs подключается модулем unit tests, autotests=false исключает повторную компиляцию файла как integration crate. Тесты таймеров работают с paused tokio clock. Параметризованные записи/платформы сохранены циклами.
- Дополнительное поле FileLockOptions::process_exited позволяет владельцу переопределить проверку PID (true только при ESRCH). По умолчанию на Unix используется безопасный rustix::process::test_kill_process: успех означает живой процесс, ESRCH — завершившийся, EPERM — живой процесс другого пользователя; все остальные ошибки, как в образце, не доказывают завершение (false). Протокол повторно читает record и проверяет PID под hash-claim. На Windows default probe возвращает прежний Unsupported до этапа 5.

## Не перенесено
Нативный Windows signal-zero probe — ждёт этапа 5 по решению ведущего; до этого без переопределения process_exited возвращается Unsupported. Реальный exited-process тест включён на Unix (cfg(unix)); как и в образце, на Windows он не запускается из-за быстрого переиспользования PID. Каталога locale нет.

## Не проверено
Windows/macOS runtime не запускались. Windows retry/EPERM проверены инъекцией raw Unix errno на Linux, не реальными Win32 sharing violations. Реальный Unix probe проверен на завершившемся процессе, живом родителе и PID 1 под non-root; root-вариант последнего теста печатает причину пропуска и не доказывает EPERM. Drop cleanup best-effort: при отказе файловой системы удалить lock гарантировать освобождение нельзя. На Windows guard проверяет запись, без Unix-проверки inode.

## Вопросы ведущему
Новых вопросов нет. Windows default probe отложен до этапа 5 по решению ведущего.

