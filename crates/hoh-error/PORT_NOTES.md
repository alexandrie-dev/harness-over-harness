# hoh-error ← packages/util/error @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Перенесён полностью |
| tests/error.spec.ts | tests/error.rs | Все исходные runtime-сценарии перенесены |
| README.md | PORT_NOTES.md | Прочитан; контракт учтён, README не копируется |
| package.json | Cargo.toml | Cargo зависимости; дополнительных runtime entry points нет |

## Отличия от образца
- По решению ведущего (PORTING.md §7) все ошибки с кодом имеют один конкретный тип HarnessError с приватными полями. Нет CodedError и типов-обёрток; подкласс образца — пустая структура с NAME и конструктором, возвращающим HarnessError. Маршрутизация использует code(), проверки классов — is_named().
- Перед проектированием просмотрены совпадения `extends HarnessError`, `instanceof HarnessError`, `.code ===` во всём packages с исключениями client/api/typert/host/experimental; в частности jobs/error, llm/error, llm/adapter-failure, boot/plugin-market/error, core/tools/pipeline, util/atomic-write. Они требуют стабильного code, имени и независимой от текста причины.
- Полный публичный API: BoxError (Box<dyn Error + Send + Sync>), HarnessError (Debug/Clone), new/named, with_cause/with_detail, name/code/message/cause/detail/is_named/of и стандартные Display/Error. Display выводит только message. with_cause принимает конкретную ошибку и уже BoxError; cause/detail хранятся в Arc и разделяются при Clone. Типизированные дополнительные поля подклассов извлекаются detail::<T>().
- HarnessError::of проверяет только сам динамический объект (в том числе BoxError.as_ref()), не ищет HarnessError по source(). Произвольное JS thrown-значение нуждается в явном представлении у вызывающего кода.
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
Нет отдельных платформенных веток. Тесты проверяют конкретную причину через downcast, of на глубине 0 и отказ для plain error с coded source, BoxError, имена, наличие/отсутствие/тип detail, boxed cause и разделение cause/detail после Clone.

## Вопросы ведущему
Нет.

