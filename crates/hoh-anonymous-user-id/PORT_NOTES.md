# hoh-anonymous-user-id ← packages/identity/anonymous-user-id @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Перенесён полностью |
| tests/anonymous-user-id.spec.ts | tests/anonymous-user-id.rs | Все исходные runtime-сценарии перенесены |
| README.md | PORT_NOTES.md | Прочитан; контракт учтён, README не копируется |
| package.json | Cargo.toml | Cargo зависимости; дополнительных runtime entry points нет |

## Отличия от образца
- Module-global memo перенесён в дополнительный публичный AnonymousUserIdStore (Default), принадлежащий контексту, по запрету глобального изменяемого состояния (раздел 6). Все потребители процесса должны разделять один экземпляр. get_or_create_anonymous_user_id принимает &store, метод с тем же именем также доступен.
- Mutex удерживает синхронный read/create/reread/overwrite/memoize как один шаг, как в однопоточном JS. Функция выполняет блокирующий файловый I/O под mutex, потому что исходник синхронный. Async-вызывающий код обязан вызывать её через spawn_blocking; это библиотека, не plugin.
- Options содержит env:Option<&HashMap<...>> и random_uuid:Option<&dyn Fn()->String>. None читает process environment/генерирует через hoh-util-crypto. Идентификатор — branded string newtype, format файла, регистр persisted UUID и ECMAScript trim сохранены.
- Дополнительный публичный AnonymousUserIdError::Home представляет ошибки разрешения пути через Result; ошибки persistence по-прежнему подавляются best-effort. random_uuid возвращает Uuid; недоступность OS RNG считается нарушением инварианта по решению ведущего, а не вариантом Random.
- Все 9 исходных it сохранены. Проверка process.env запускает отдельный test executable с временным DSH_HOME и marker HOH_ANONYMOUS_USER_ID_CHILD=1: helper process_environment_child помечен ignored для обычного прохода, но запускается и проверяется родительским тестом. Без marker helper сразу возвращает управление, в том числе при cargo test -- --ignored; отдельный тест подтверждает отсутствие записи без marker. Изменение окружения текущего процесса не требуется.

## Не перенесено
Глобальная изменяемая Map как механизм заменена owner-scoped store по разделу 6. Каталога locale нет.

## Не проверено
Windows/macOS не запускались. Недоступность записи проверяется regular-file-as-home; отдельные permission denied/read-only filesystem не инъецированы.

## Вопросы ведущему
Ведущему: при интеграции предоставить один AnonymousUserIdStore на процессный контекст, чтобы все consumers разделяли memo.

