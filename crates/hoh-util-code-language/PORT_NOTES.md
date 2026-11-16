# hoh-util-code-language ← packages/util/code-language @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | Полностью: таблицы и все три экспорта. |
| `tests/code-language.spec.ts` | `tests/code-language.rs` | Все 15 `it`/`it.each`, все строки таблиц и проверки. |
| `README.md` | — | Прочитан; описанное поведение учтено. |

Проверка: `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c3 cargo test -p hoh-util-code-language` — `15 passed; 0 failed` (unit/doc: по 0). С теми же переменными окружения `cargo clippy -p hoh-util-code-language --all-targets -- -D warnings` и `cargo fmt -p hoh-util-code-language -- --check` завершились с кодом 0.

## Отличия от образца
- Имена функций приведены к snake_case; `undefined` представлен `None`, строковые результаты — `&'static str`.
- Закрытые таблицы языков и коротких имён объединены в одну неизменяемую таблицу. Порядок `CODE_HIGHLIGHT_EXTENSIONS` сохранён; глобального изменяемого реестра нет.
- Параметризованные `it.each` представлены циклами внутри соответствующего Rust-теста со всеми исходными строками.
- Дополнительных публичных символов нет.

## Не перенесено
Нет. Каталога `locale/` в образце нет.

## Не проверено
Отдельных платформенных веток нет. Unicode-приведение регистра использует таблицы Rust; отдельного дифференциального прогона с JavaScript вне исходных тестов не выполнялось. Все зарегистрированные расширения ASCII.

## Вопросы ведущему
Нет.
