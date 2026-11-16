# hoh-brand ← packages/util/brand @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Перенесён полностью |
| tests/brand.spec.ts | tests/brand.rs | Все исходные runtime-сценарии перенесены |
| README.md | PORT_NOTES.md | Прочитан; контракт учтён, README не копируется |
| package.json | Cargo.toml | Cargo зависимости; дополнительных runtime entry points нет |

## Отличия от образца
- `Branded` и `BrandedNumber` стали traits-контрактами newtype, helpers `brand_string/brand_number` остаются без валидации. Сравнение/JSON сохраняют исходный примитив; арифметика требует явного снятия бренда.
- До реализации просмотрены `Branded<`, `BrandedNumber`, `brandString/brandNumber` во всём разрешённом packages; показательные владельцы: core/session/types (SessionId, SessionSeq, SessionLogOffset), llm/brand, jobs/brand, boot/plugin-market/detail.
- Дополнительные публичные макросы `branded_string!` и `branded_number!` объявляют доменные newtype; скрытый re-export `__serde` нужен derive в вызывающем crate. Макрос предполагает стандартное имя зависимости `hoh-brand`.
- Строковый пример в одну строку: `hoh_brand::branded_string!(pub SessionId);` Затем `let id: SessionId = hoh_brand::brand_string("session-1");`. Макрос содержит `#[serde(transparent)]`, conversions String/&str, AsRef<str>, Display, сравнение/сортировку/hash. Сериализация остаётся строкой.
- Числовой пример: `hoh_brand::branded_number!(pub SessionSeq, u64);` Затем `let seq: SessionSeq = hoh_brand::brand_number(7);`. Владелец выбирает u64/i64/f64 по смыслу; From снимает бренд. Числовой helper принимает примитив; повторное брендирование уже брендированного числа требует снятия бренда.
- expectTypeOf заменён Rust-проверкой типа присваивания; дополнительный runtime-тест проверяет прозрачный JSON строкового id.

## Не перенесено
Символ BRAND не переносится как runtime-сущность (раздел 8). Каталога locale нет.

## Не проверено
Нет отдельных compile-fail тестов несовместимости разных newtype.

## Вопросы ведущему
Нет.

