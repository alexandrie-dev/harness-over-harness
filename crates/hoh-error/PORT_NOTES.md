# hoh-error ← packages/util/error @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Перенесён полностью |
| tests/error.spec.ts | tests/error.rs | Все исходные runtime-сценарии перенесены |
| README.md | PORT_NOTES.md | Прочитан; контракт учтён, README не копируется |
| package.json | Cargo.toml | Cargo зависимости; дополнительных runtime entry points нет |

## Отличия от образца
- Наследование заменено композицией: crate-local enum/struct содержит `HarnessError`, реализует `CodedError::harness_error`; маршрутизатор принимает `&dyn CodedError`. Поля `name/code/message/cause` доступны единообразно, произвольные Rust-причины идут через `std::error::Error::source`.
- Перед проектированием просмотрены совпадения `extends HarnessError`, `instanceof HarnessError`, `.code ===` во всём packages с исключениями client/api/typert/host/experimental; в частности jobs/error, llm/error, llm/adapter-failure, boot/plugin-market/error, core/tools/pipeline, util/atomic-write. Они требуют стабильного code, имени и независимой от текста причины.
- Дополнительные публичные элементы: `CodedError` и его методы, конструктор `new`, builders `with_name/with_cause`. Rust не выводит имя содержащего типа автоматически: владелец явно задаёт имя класса источника. Causes ограничены `Error + Send + Sync`, произвольное JS thrown-значение нуждается в явном представлении у вызывающего кода.
- Пример: `let e = HarnessError::new("archive entry is missing", "ARCHIVE_ENTRY_NOT_FOUND").with_name("ArchiveError"); let routed: &dyn CodedError = &e; assert_eq!(routed.code(), "ARCHIVE_ENTRY_NOT_FOUND");`

## Не перенесено
Нет. Каталога locale в образце нет.

## Не проверено
Встраивание в будущие crate-local ошибки проверено на SampleError; динамический instanceof заменяется типизированной границей CodedError.

## Вопросы ведущему
Нет.

