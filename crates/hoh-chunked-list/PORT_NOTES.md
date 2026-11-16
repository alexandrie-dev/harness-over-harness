# hoh-chunked-list ← packages/util/chunked-list @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Перенесён полностью |
| tests/chunked-list.spec.ts | tests/chunked-list.rs | Все исходные runtime-сценарии перенесены |
| README.md | PORT_NOTES.md | Прочитан; контракт учтён, README не копируется |
| package.json | Cargo.toml | Cargo зависимости; дополнительных runtime entry points нет |

## Отличия от образца
- `Option<Arc<ChunkedList<T>>>` представляет пустой список; Arc разделяет старые чанки и значения. При append копируется максимум 64 Arc, T не требует Clone. Поля приватны для readonly-контракта; дополнительные методы `values/previous` предоставляют чтение.
- `append_chunked_list` принимает Option<&Arc<_>>, итератор принимает Option<&ChunkedList<_>> и возвращает &T в порядке вставки.
- `chunked_list_schema<T>(Value)` возвращает Result<Arc<_>, serde_json::Error>. Deserialize типа T заменяет Zod-схему владельца (для дополнительной валидации владелец пишет Deserialize). Прямой Deserialize также проверяет 1..64 и deny_unknown_fields на всех уровнях; previous:null отклоняется, отсутствующий previous допустим. Порядок JSON: values, previous.
- Параметризованные случаи it.each сохранены циклом: 4 неверных head + 4 неверных previous; добавлена проверка null.

## Не перенесено
Zod как механизм заменён serde по заданию. Каталога locale нет.

## Не проверено
Глубокие рекурсивные JSON-checkpoints ограничены стеком/лимитом serde_json; как и образец, нет безлимитной рекурсивной валидации.

## Вопросы ведущему
Нет.

