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
- Mutex удерживает синхронный read/create/reread/overwrite/memoize как один шаг, как в однопоточном JS. Дисковый I/O остаётся синхронным, как у boot-time исходника; это библиотека, не plugin.
- Options содержит env:Option<&HashMap<...>> и random_uuid:Option<&dyn Fn()->String>. None читает process environment/генерирует через hoh-util-crypto. Идентификатор — branded string newtype, format файла, регистр persisted UUID и ECMAScript trim сохранены.
- Дополнительный публичный AnonymousUserIdError (Home/Random) представляет ошибки разрешения пути и генерации через Result; ошибки persistence по-прежнему подавляются best-effort.
- Все 9 исходных it сохранены. Проверка process.env запускает отдельный test executable с временным DSH_HOME: helper process_environment_child помечен ignored для обычного прохода, но запускается и проверяется родительским тестом. Изменение окружения процесса не требуется.

## Не перенесено
Глобальная изменяемая Map как механизм заменена owner-scoped store по разделу 6. Каталога locale нет.

## Не проверено
Windows/macOS не запускались. Недоступность записи проверяется regular-file-as-home; отдельные permission denied/read-only filesystem не инъецированы.

## Вопросы ведущему
Ведущему: при интеграции предоставить один AnonymousUserIdStore на процессный контекст, чтобы все consumers разделяли memo.

