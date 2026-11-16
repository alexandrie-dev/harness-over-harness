# hoh-error ← packages/util/error @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Перенесён полностью |
| tests/error.spec.ts | tests/error.rs | Все исходные runtime-сценарии перенесены |
| README.md | PORT_NOTES.md | Прочитан; контракт учтён, README не копируется |
| package.json | Cargo.toml | Cargo зависимости; дополнительных runtime entry points нет |

## Отличия от образца
- По решению ведущего (PORTING.md §7) все ошибки с кодом имеют один конкретный тип HarnessError с приватными полями. Нет CodedError и отдельных crate-local типов-обёрток над HarnessError; подкласс образца — пустая структура с NAME и конструктором, возвращающим HarnessError. Маршрутизация использует code(), проверки классов — is_named(). Общий SharedError служит только передаче бросаемого значения с сохранением идентичности.
- Перед проектированием просмотрены совпадения `extends HarnessError`, `instanceof HarnessError`, `.code ===` во всём packages с исключениями client/api/typert/host/experimental; в частности jobs/error, llm/error, llm/adapter-failure, boot/plugin-market/error, core/tools/pipeline, util/atomic-write. Они требуют стабильного code, имени и независимой от текста причины.
- Публичный API HarnessError: BoxError (Box<dyn Error + Send + Sync>), HarnessError (Debug/Clone), new/named, with_cause/with_detail, name/code/message/cause/detail/is_named/of и стандартные Display/Error. Display выводит только message. with_cause принимает конкретную ошибку и уже BoxError; cause/detail хранятся в Arc и разделяются при Clone. Типизированные дополнительные поля подклассов извлекаются detail::<T>().
- Дополнительный SharedError (Clone/Debug) по решению ведущего представляет бросаемую/передаваемую причину через Arc<dyn Error + Send + Sync>. API: new, ptr_eq, downcast_ref, get_ref, of, Display/Error. new(SharedError) и new(BoxError с SharedError) возвращают ту же общую ошибку, не создавая вложенную обёртку. Display и source() прозрачны: source() — причина внутренней ошибки, а не дополнительное звено SharedError. Сравнение идентичности использует ptr_eq.
- HarnessError::of проверяет сам динамический объект (в том числе BoxError.as_ref()) и значение внутри SharedError, но никогда не идёт по source(). SharedError::of также узнаёт только сам объект. Это сохраняет instanceof HarnessError для брошенной причины отмены. Произвольное JS thrown-значение, не являющееся ошибкой, нуждается в явном представлении у вызывающего кода.
- Пример шаблона подкласса:
```rust
pub struct ArchiveError;
pub enum ArchiveErrorCode { EntryNotFound }
impl ArchiveErrorCode {
    pub fn as_str(self) -> &'static str {
        match self { Self::EntryNotFound => "ARCHIVE_ENTRY_NOT_FOUND" }
    }
}
impl ArchiveError {
    pub const NAME: &str = "ArchiveError";
    #[allow(clippy::new_ret_no_self)] // Конструктор подкласса по PORTING.md §7.
    pub fn new(message: impl Into<String>, code: ArchiveErrorCode) -> hoh_error::HarnessError {
        hoh_error::HarnessError::named(Self::NAME, message, code.as_str())
    }
}
```

## Не перенесено
Нет. Каталога locale в образце нет.

## Не проверено
Нет отдельных платформенных веток. Тесты проверяют конкретную причину через downcast, of на глубине 0 и отказ для plain error с coded/shared source, BoxError, имена, наличие/отсутствие/тип detail, boxed cause и разделение cause/detail после Clone; для SharedError также identity через clone/BoxError/new, hit/miss downcast, прозрачные Display/source и HarnessError::of через SharedError и BoxError.

## Вопросы ведущему
Нет.

## Помощники Node (без пары в образце)

Отличие публичного API: добавлены и реэкспортированы из `src/node.rs`
`node_error_code(&io::Error)`, `node_error_description(code)`,
`node_fs_error_message(error, syscall, path, dest)`,
`node_spawn_error_message(error, command)`. Это общее поведение самой среды
Node/libuv, а не экспорты исходного пакета: имена и описания системных ошибок
и точные тексты для fs и неудачного старта дочернего процесса.
Все 85 описаний взяты из неизменённой фикстуры Node 22.23.1
`tests/fixtures/node_system_errors.json`; имена Unix errno сопоставлены
через именованные константы `rustix::io::Errno`, не через числа Linux.
Специальные libuv/resolver-коды имеют описание, но не притворяются errno.
Для EFTYPE предусмотрены константы BSD/Apple/newlib; для NONET, REMOTEIO,
UNATCH условная компиляция повторяет доступность констант rustix на Unix.
Ветки именованных констант macOS/BSD и остальных Unix не
проверены исполнением; не-Unix всегда возвращает None.

Если OS-номера нет, он неизвестен или хост не Unix, форматтеры используют
собственный Display ошибки: `<Display>, <syscall> ...` и `spawn <Display>`.
Эта ветка проверена только на согласованность с данным контрактом Rust,
с Node не сверена. Пути заключаются в одинарные кавычки без экранирования.
Для EACCES/EAGAIN/EMFILE/ENFILE/ENOENT spawn включает команду, для остальных
именованных ошибок — только код.

Слово syscall задаёт вызывающий код. Таблица непосредственно из fsMessages:

| Функция / случай | syscall |
| --- | --- |
| readFile: отсутствует / путь под файлом | open |
| readFile: каталог (ошибка чтения) | read |
| writeFile, open, truncate | open |
| mkdir (включая recursive) | mkdir |
| readdir | scandir |
| opendir | opendir |
| stat | stat |
| lstat | lstat |
| access | access |
| realpath | realpath |
| readlink | readlink |
| rename | rename |
| unlink | unlink |
| rmdir | rmdir |
| rm: отсутствующий путь | lstat |
| rm: каталог без recursive | rm (особая ERR_FS_EISDIR, см. ниже) |
| copyFile | copyfile |
| link | link |
| symlink | symlink |
| chmod | chmod |
| mkdtemp | mkdtemp |

`tests/node.rs` (Linux) проверяет все описания и все 67 Linux errno из таблицы,
а также отсутствие имени для 18 значений, не являющихся Linux errno.
Во временном каталоге воспроизводятся все 30 системных fsMessages через
std::fs и rustix access (каталог создаёт tempfile); syscall и пути берутся из фикстуры,
корень заменяется на `<root>`. Для mkdtemp воспроизводится ошибка внутреннего
mkdir под отсутствующим родителем; форматтер получает шаблон `tmp-XXXXXX`,
который Node показывает вместо случайного суффикса. Сам mkdtemp не вызывается:
tempfile оборачивает его ошибку и не сохраняет raw_os_error.
std::process::Command воспроизводит пять отказов старта: missing,
missing in PATH, not executable, directory, under a file.
Дополнительно проверяются обе формы spawn для всех 67 именованных errno.

Не покрыто этими форматтерами: `rm directory without recursive` —
Node создаёт отдельную ERR_FS_EISDIR с текстом
`Path is a directory: rm returned EISDIR (is a directory) <root>/dir`,
это не системная ошибка заданной формы; `exit 3` и `killed by TERM` —
ошибки уже запущенной команды, а не её старта. Эти три строки фикстуры
не воспроизводятся; не являются ошибками фикстуры и требуют логики вызывающего
кода. Поля ExecFile stdout/signal/thrownByExecFile не входят в строковый API.
Node для проверки не запускался.

