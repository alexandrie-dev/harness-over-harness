# hoh-file-reference ← packages/context/file-reference @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | `FILE_REFERENCE_PROMPT`, абстрактный сервис и ключ `fileReferences` перенесены |
| `src/grammar.ts` | `src/grammar.rs` | обе функции перенесены с UTF-16-смещением и JavaScript-классом `\\s` |
| `src/types.ts` | `src/types.rs` | обе публичные записи и литералы `file`/`directory` перенесены |
| `tests/service.spec.ts` | `tests/service.rs` | 1 / 0 исходных случаев перенесено |
| — | `tests/grammar.rs` | 6 дополнительных проверок грамматики, UTF-16, сериализации и точных литералов |
| `README.md`, `package.json` | эти заметки и `Cargo.toml` | прочитаны; собственного `LICENSE` и `locale/` нет |

Все runtime-экспорты используются переносимым `packages/context/file-reference-local`; неиспользуемых
переносимыми пакетами публичных символов не найдено.

## Отличия от образца

- Абстрактный класс `FileReferenceService extends Service` представлен объектно-безопасным trait с тем же
  именем; дополнительный ключ `FileReferences` публикует `dyn FileReferenceService` под неизменным именем
  сервиса `fileReferences`. Отдельного плагина у образца нет, поэтому в переносе он не вымышлен.
- `Promise<FileReferenceCandidate[]>`, который может быть отклонён реализацией поставщика, представлен как
  `BoxFuture<Result<Vec<FileReferenceCandidate>, hoh_error::BoxError>>`. Аргументы заимствуются на время
  future; `Agent` остаётся общим `Arc<dyn Agent>`, а отмена — общим `hoh_timeout::AbortSignal`.
- Анонимное объединение поля `kind` получило необходимое Rust-имя `FileReferenceKind`; его wire-литералы и
  порядок полей `path`, `kind` сохранены. Default export в Rust совпадает с корневым `FileReferenceService`.
- `cursor_col` имеет тип `usize` и считается в единицах UTF-16. Отрицательные, дробные и бесконечные значения
  JavaScript Number не входят в контракт редакторской колонки. Позиция внутри surrogate pair возвращает
  `None`: Rust `String` не может представить получаемый JavaScript lone surrogate; реальные позиции курсора
  редактора проходят по границам символов. Смещение за концом строки, как `slice`, означает всю строку.

## Не перенесено

README-переводы, npm `exports` для `package.json`/`src/*`, source maps и метаданные публикации не являются
runtime-кодом Rust. Исходных runtime-файлов, экспортов или тестовых случаев не пропущено.

## Не проверено

Платформенных веток нет. Входной JavaScript-текст с уже содержащимся lone surrogate и курсор внутри surrogate
pair не представим типом Rust `str`; обычный Unicode, включая astral-символы до токена, проверен по UTF-16.
Фактическое сканирование файлов принадлежит поставщику `file-reference-local`, а не этому контрактному crate.

## Вопросы ведущему

Нет. Все нужные API зависимых crates присутствуют; правок вне `hoh-file-reference` не требуется.
