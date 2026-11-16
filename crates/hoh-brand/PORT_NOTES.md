# hoh-brand ← packages/util/brand @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Перенесён полностью |
| повторные Branded-декларации в packages (список ниже) | src/ids.rs | Один Rust-тип на общий бренд по решению ведущего |
| tests/brand.spec.ts | tests/brand.rs | Все исходные runtime-сценарии перенесены |
| README.md | PORT_NOTES.md | Прочитан; контракт учтён, README не копируется |
| package.json | Cargo.toml | Cargo зависимости; дополнительных runtime entry points нет |

## Отличия от образца
- `Branded` и `BrandedNumber` стали traits-контрактами newtype, helpers `brand_string/brand_number` остаются без валидации. Сравнение/JSON сохраняют исходный примитив; арифметика требует явного снятия бренда.
- До реализации просмотрены `Branded<`, `BrandedNumber`, `brandString/brandNumber` во всём разрешённом packages; показательные владельцы: core/session/types (SessionId, SessionSeq, SessionLogOffset), llm/brand, jobs/brand, boot/plugin-market/detail.
- Дополнительные публичные макросы `branded_string!` и `branded_number!` объявляют доменные newtype; скрытый re-export `__serde` нужен derive в вызывающем crate. Макрос предполагает стандартное имя зависимости `hoh-brand`.
- Строковый пример в одну строку для собственного доменного id: `hoh_brand::branded_string!(pub ArchiveId);` Затем `let id: ArchiveId = hoh_brand::brand_string("archive-1");`. Макрос содержит #[serde(transparent)], conversions String/&str, as_str(), AsRef<str>, Borrow<str>, Display, PartialEq<str>/PartialEq<&str>, сравнение/сортировку/hash. Сериализация остаётся строкой; HashMap<Id,_>.get("x") и id == "x" работают без преобразования.
- Числовой пример: `hoh_brand::branded_number!(pub SessionSeq, u64);` Затем `let seq: SessionSeq = hoh_brand::brand_number(7);`. Макрос предназначен только для целочисленных представлений (например u64/i64), выводит Eq/Ord/Hash вместе с PartialEq/PartialOrd; From снимает бренд. Числовой helper принимает примитив; повторное брендирование уже брендированного числа требует снятия бренда.
- Дополнительный публичный модуль ids объявляет общие типы один раз; пакеты реэкспортируют их, вместо повторного вызова макроса. По полному поиску literal Branded-деклараций в src/ всех packages (без tests и сгенерированных api-catalog/guest-source) других повторяющихся имён не найдено. Для Source SessionTitleProviderId подтверждены две точки входа одного пакета, а не два разных пакета.

| бренд образца | тип в ids | пакеты/точки деклараций образца |
| --- | --- | --- |
| SessionId | SessionId | core/session/src/types.ts; llm/llm/src/types.ts |
| ApprovalRequestId | ApprovalRequestId | extensions/cordis-host-runner/src/types.ts; interaction/user-approval/src/types.ts |
| session-request-id | SessionRequestId | api/session-controller/src/types.ts; subagent/subagent/src/control-types.ts (имя SubagentPromptRequestId реэкспортирует тот же тип) |
| file-upload-receipt-id | FileUploadReceiptId | api/session-controller/src/types.ts (поле receiptId); client/file-upload/src/types.ts |
| SessionTitleProviderId | SessionTitleProviderId | session/session-title/src/index.ts и src/types.ts; потребители session-title-llm и session-title-first-prompt-llm импортируют этот id |

- Общие SessionId из двух модулей проверены присваиванием одного типа другому и прозрачным JSON; отдельные тесты проверяют borrowed lookup, оба строковых сравнения и integer sort/map keys.
- expectTypeOf заменён Rust-проверкой типа присваивания; дополнительный runtime-тест проверяет прозрачный JSON строкового id.

## Не перенесено
Символ BRAND не переносится как runtime-сущность (раздел 8). Каталога locale нет.

## Не проверено
Нет отдельных compile-fail тестов несовместимости разных newtype.

## Вопросы ведущему
Нет.

