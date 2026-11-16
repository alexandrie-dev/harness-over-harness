# hoh-util-workspace-path ← packages/util/workspace-path @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | Полностью: все экспорты, строковые пути и WHATWG-разрешение `fileMediaUrl` через `url` 2.5.8. |
| `src/file-address.ts` | `src/file_address.rs` | Полностью, включая `FileAddress` и три функции; модуль публичен и реэкспортирован из корня. |
| `tests/index.spec.ts` | `tests/index.rs` | Все 10 `it`/`it.each`, все строки и проверки. |
| `tests/file-address.spec.ts` | `tests/file-address.rs` | Все 13 `it`/`it.each`, все строки и проверки. |
| `tests/file-media-url.spec.ts` | `tests/file-media-url.rs` | Все 4 `it.each`, все строки и проверки (полный URL сравнивается с ожидаемым); ещё 11 тестов на WHATWG-разбор, ошибки и порядок проверок. |
| `README.md` | — | Прочитан; описанное поведение учтено. |

Проверка: `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c3 cargo test -p hoh-util-workspace-path` — `file-address`: `13 passed; 0 failed`, `file-media-url`: `15 passed; 0 failed`, `index`: `10 passed; 0 failed` (unit/doc: по 0). С теми же переменными окружения `cargo clippy -p hoh-util-workspace-path --all-targets -- -D warnings` и `cargo fmt -p hoh-util-workspace-path -- --check` завершились с кодом 0.

## Отличия от образца
- Функции и поля Rust используют snake_case. `FileAddress` — закрытый enum с `serde(tag = "scope")`; поля JSON сохраняют `sessionId`, `path` и исходный порядок. Идентификатор Session в образце не branded, поэтому остаётся строкой.
- Добавлен публичный `PathParts`: именованный Rust-тип для исходного анонимного объекта `{ directory, name }`. Дополнительных функций в публичном API нет.
- Строки принимаются как `&str`, необязательные аргументы — `Option<&str>`; возвращаемые строки владеют памятью, `undefined` представлен `None`.
- Rust-строки не представляют одиночные UTF-16-суррогаты JavaScript. Для корректного Unicode percent-кодирование совпадает с `encodeURIComponent` (в адресах `:` остаётся буквальным); percent-декодирование строго проверяет UTF-8, не заменяя ошибки символом замены.
- `file_media_url` сохраняет исходные проверки транспорта, абсолютного пути, сетевого префикса и управляющих символов до разбора URL. `url::Url::parse(base)?.join(relative)` переносит `new URL(relative, base).href`, включая WHATWG-нормализацию host, path и query. Параметр `path` сначала кодируется как `encodeURIComponent`, без form-urlencoded-преобразования пробелов в `+`; дополнительное кодирование query определяется схемой, как в образце.
- Результат `file_media_url` — `Result<Option<String>, FileMediaUrlError>`: обычный исходный `undefined` представлен `Ok(None)`, URL — `Ok(Some(url))`, выброшенный при неверной базе `TypeError` — `Err(FileMediaUrlError::InvalidUrl(...))`. Добавлен публичный enum `FileMediaUrlError` с `thiserror` по разделу 7 правил; текст ошибки `Invalid URL`, исходный `url::ParseError` доступен как причина. Это Rust-представление исключения, а не дополнительный исходный экспорт.
- Параметризованные `it.each` представлены циклами внутри соответствующего Rust-теста со всеми исходными строками. В URL-тестах независимые ожидаемые строки проверяют одновременно pathname, точный percent-encoding и значение параметра `path`.

## Не перенесено
Нет. Каталога `locale/` в образце нет.

## Не проверено
- Отдельных cfg-веток нет: POSIX, Windows drive и UNC обрабатываются строками и проверяются на одном хосте без файловой системы.
- Добавленные тесты проверяют userinfo, IPv6, IDNA, percent-encoded hostname, десятичный/шестнадцатеричный/восьмеричный IPv4, C0/ASCII whitespace, базы без обычного `://`, dot-сегменты, query/fragment и некорректные базы. Полный набор Web Platform Tests и отдельный дифференциальный прогон с JavaScript не запускались; WHATWG-разбор делегирован библиотеке `url`.

## Вопросы ведущему
Нет.

## Помощники Node (без пары в образце)

Отличие публичного API: добавлен модуль `node_path` (`src/node_path.rs`) с
`normalize(path)`, `join(parts)`, `resolve(cwd, parts)`, `relative(cwd, from, to)`,
`dirname(path)`, `basename(path)`, `extname(path)`, `is_absolute(path)`.
Это общая реализация поведения самой среды Node (`path.posix`), отсутствующая
как набор экспортов в пакете-образце. Она нужна для одинаковых видимых путей
у всех потребителей; существующие функции пакета не заменены.
Вход — строки UTF-8, выход — строки (у `is_absolute` — bool), разделитель
только `/` на любом хосте; `std::path` не используется. `cwd` — обязательный
абсолютный путь вместо скрытого `process.cwd()`.

Нормализация сохраняет завершающий слеш, join пропускает пустые части,
resolve отбрасывает части слева от последней абсолютной и завершающий слеш.
dirname сохраняет исходное написание, в том числе особое `//`; basename
и extname не разрешают точки.
`tests/node_path.rs` сравнивает результат с каждой из 8835 строк неизменённой
фикстуры Node 22.23.1 `tests/fixtures/node_path.jsonl`.
Непроверено: одиночные UTF-16-суррогаты (непредставимы в `str`), нестроковые
аргументы JavaScript; невалидный относительный `cwd` вне контракта API.
Платформенных веток нет. Node для проверки не запускался.
