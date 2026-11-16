# hoh-package-manifest ← packages/util/package-manifest @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | все реэкспорты |
| src/types.ts | src/types.rs | serde-типы, точные JSON-имена |
| src/client-module.ts | src/client_module.rs | типы функций и интерфейсы |
| tests/ отсутствует | tests/json_shapes.rs | 5 собственных тестов чтения/записи JSON |

## Отличия от образца
Имена полей Rust — snake_case, JSON сохраняет исходные имена. Union patch получает
имя BundlePatch; литерал 1 — ManifestVersion. Это вспомогательные публичные типы.
IndexMap сохраняет порядок записей. Не вводятся проверки SemVer, путей и значения
по умолчанию listedByDefault: ими, как и в образце, владеет читатель.
Функции client-module представлены Arc-замыканиями и трейтами; их невозможно
сериализовать в JSON, поэтому serde применяется только к данным types.ts.
StylesheetText — вспомогательный псевдоним типа замыкания. unknown представлен Value;
вызовы require/async не исполняют JavaScript, а описывают контракт для потребителя.

Среди переносимых пакетов используются DshBundleManifest, DshPackageManifest,
LocalizedText, PluginLocalizedMeta. Прямых потребителей остальных экспортов вне
исключённых client/api/typert/host/experimental нет; они всё равно перенесены.

## Не перенесено
Нет. Исходных тестов нет (0 перенесено, 0 пропущено); locale и отдельной лицензии нет.

## Не проверено
Исполнение браузерных JavaScript-фабрик: пакет содержит только объявления.

## Вопросы ведущему
Нет. Зависимостей от других HoH crates нет.
