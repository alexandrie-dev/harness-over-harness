# hoh-util-crypto ← packages/util/crypto @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Перенесён полностью |
| tests/uuid.spec.ts | tests/uuid.rs | Все исходные runtime-сценарии перенесены |
| README.md | PORT_NOTES.md | Прочитан; контракт учтён, README не копируется |
| package.json | Cargo.toml | Cargo зависимости; дополнительных runtime entry points нет |

## Отличия от образца
- bytesToBase64 → bytes_to_base64 использует base64 STANDARD с padding. randomUUID → random_uuid использует getrandom и те же pinned биты версии/варианта и lowercase hex.
- Uuid — прозрачный строковый newtype через hoh-brand вместо TS template-literal типа; wire-значение неизменно. Новых функций нет.
- random_uuid возвращает Uuid. По решению ведущего доступность криптографического OS RNG — инвариант; отказ getrandom вызывает panic через expect с однострочным объяснением инварианта.
- Дублирующий shape-тест mints_without_platform_random_uuid заменён unit-тестом приватного форматирования bytes: на байтах 0x00, 0xff и последовательных байтах проверены pinned version/variant bits и сохранение остальных битов. Rust не имеет ambient crypto.randomUUID, поэтому механизм подмены globalThis не переносится. Остальные исходные UUID/base64-тесты остаются в tests/uuid.rs.

## Не перенесено
Подмена браузерного globalThis не переносится как механизм; смысл сценария сохранён. Каталога locale нет.

## Не проверено
Отказ системного источника случайности не инъецирован; Windows/macOS getrandom отдельно не запускались.

## Вопросы ведущему
Нет.

