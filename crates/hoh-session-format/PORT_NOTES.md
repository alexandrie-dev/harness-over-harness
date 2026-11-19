# hoh-session-format ← packages/session/session-format @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` (реэкспорт) | `src/lib.rs` (`pub use` всех модулей) | перенесён |
| `src/error.ts` | `src/error.rs`: `SessionFormatError` (enum, см. ниже) | перенесён |
| `src/types.ts` | `src/types.rs`: типы-синонимы, структуры, трейты | перенесён |
| `src/json.ts` | `src/json.rs` | перенесён |
| `src/filename.ts` | `src/filename.rs` | перенесён |
| `src/context.ts` | `src/context.rs` (`SessionFormatEventCollector`) | перенесён |
| `src/chain.ts` | `src/chain.rs` | перенесён |
| `src/catalog.ts` | `src/catalog.rs` | перенесён |
| `tests/json.spec.ts` | `tests/json.rs` (6 тестов) | перенесён, часть случаев невыразима (ниже) |
| `tests/filename.spec.ts` | `tests/filename.rs` (2) | перенесён |
| `tests/chain.spec.ts` | `tests/chain.rs` (8) | перенесён полностью |
| `tests/catalog.spec.ts` | `tests/catalog.rs` (11) | перенесён полностью (`it.each` собран в один тест с циклом) |
| c4 shared gap | `tests/error_cause.rs` (2) | точный message, класс, typed cause/source, вложенные причины и старые конструкторы |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `locale/` | нет | документация; `locale/` у пакета нет |

## Отличия от образца
- Форма API. Все операции синхронны, как в образце (асинхронных нет; `tokio` не нужен ни библиотеке, ни тестам, `Stream` не вводился).
  Интерфейсы-обратные вызовы образца стали трейтами: `SessionFormatMigration`, `SessionFormatMigrationStage`, `SessionFormatCodec`,
  `SessionFormatArtifactDecoder`, `SessionFormatCurrentEncoder`, `SessionFormatEventRun`, `SessionFormatMigrationContext`. Остальное — структуры.
  Трейты `Migration`, `Codec`, `CurrentEncoder` требуют `Send + Sync`, `Stage`, `Decoder`, `EventRun` — `Send`.
- JSON: `SessionFormatJsonValue/Object/Primitive` — синонимы `serde_json::Value` / `Map` (порядок ключей сохраняется, `preserve_order`).
  `SessionFormatHeader` и `SessionFormatEvent` — синонимы `Map<String, Value>`: заголовок хранит и неизвестные ключи, как в образце
  (`{ ...header, version: 1 }`). Типизированных полей у них нет; проверки образца (`snapshotSessionFormatHeader`) сохранены.
- Владение. Образец «передаёт принадлежащие вызывающему значения без копий и без заморозки». В Rust: строки и события передаются
  по значению (`decode_row(Value)`, `transform_event(Event)`, `emit_event(Event)`, `emit_run(Box<dyn EventRun>)`), `expand(self: Box<Self>)`
  расходует прогон. Артефакт восстанавливается значением: `Fn(SessionFormatArtifact) -> Result<SessionFormatArtifact>`; заголовок — `Fn(Header) -> Result<Header>`.
  Общего изменяемого состояния нет.
- `deepFreeze` не перенесён: значения неизменяемы, пока не заняты по `&mut` (так сказано в задании); `snapshotSessionFormatJson` /
  `snapshotSessionFormatHeader` возвращают копию после проверки (`hoh_util_values::snapshot_json_value` / `is_json_value`).
  Образец замораживает результат; в Rust этого свойства нет.
- Выходной контекст потока. В образце `createStream(header, cut, context)` хранит ссылку на общий контекст, а каталог читает `collector.values`.
  Общая изменяемая ссылка запрещена, поэтому `SessionFormatChain::create_stream(header, cut, output)` принимает `output` по значению
  (`SessionFormatMigrationStream<O>`), `output()` / `into_output()` возвращают его. Каталог так забирает `collector.values`. Порядок вызовов стадий и обработка ошибок те же.
  Контекст стадии — вложенный `&mut dyn SessionFormatMigrationContext` (остаток цепочки + выход), а не граф объектов.
- Ошибки. `SessionFormatError` — enum: `Format(String)` (`SessionFormatError`), `UnsupportedMigration { message, cause }`
  (`SessionFormatUnsupportedMigrationError`, `source()` отдаёт причину), `Other(Box<dyn Error + Send + Sync>)` — ошибка обратного вызова
  не из пакета (в образце проходит любое брошенное значение; `String(error)` соответствует `Display`). Тексты и порядок проверок совпадают.
  Добавлено, чего нет в образце: конструкторы `new`, `unsupported_migration`, `other` и `is_unsupported_migration()` (замена `instanceof`).
- Числа. `number` образца: версии, счётчики и сечения — `u64` (проверка `<= 2^53-1` сохранена там, где значение приходит как `u64`:
  `session_format_count_u64`); значения из JSON (`&Value`) проверяются как в образце: целое `f64` принимается, `-0` отклоняется (`session_format_count`,
  `session_format_safe_integer` → `i64`). Значение, которого нет в заголовке, считается `undefined`/`null` и даёт тот же отказ.
- Параметры по умолчанию. `label = '...'` в образце: у каждой функции две формы — короткая с меткой образца (`snapshot_session_format_json`,
  `snapshot_session_format_header`, `session_format_version`) и `_labeled` с явной меткой. `session_format_count`/`session_format_safe_integer` метку требуют.
  Новые публичные символы: `session_format_count_u64`, `SessionFormatSourceKind`, `SessionFormatValidation`, `SessionFormatHeaderRestorer`, `SessionFormatArtifactRestorer`, `SessionFormatHeaderReadResult::status()`.
- `sessionFormatLogFilename(version: number)` → `session_format_log_filename(version: u64)`: отрицательные, дробные значения и `-0` непредставимы;
  проверка `> 2^53-1` сохранена.
- `parseSessionFormatLogFilename` разобран вручную по выражению `^session(?:\.v([1-9][0-9]*))?\.jsonl$`, без `regex` (только ASCII-цифры; `$` не совпадает до `\n`).
- `SessionFormatHeaderReadResult` — enum (`Current`, `MigrationRequired`, `Unsupported`, `Malformed`) вместо размеченного по `status` объединения;
  `Malformed.stored_version` — `Option` (поле в образце отсутствует, когда версию прочитать нельзя).
- `SessionFormatRestore` — структура (внутри два варианта), `finish(self)` расходует restore (образец: «вызвать `finish()` один раз»).
- Обёртка `Object.freeze({...codec})` / `defineSessionFormatMigration(...)` возвращает ту же `Arc`, замораживания нет.
- `SessionFormatRestoreOptions.validation` — enum `SessionFormatValidation { Transformed, Current }`.

- Заголовок текущего восстановления. Образец кладёт в готовый артефакт сам объект заголовка декодера (`catalog.ts:180`), поэтому изменение,
  сделанное декодером при разборе строк, видно в результате. Перенос берёт `decoder.header().clone()` при `finish()`, а `SessionFormatRestore::header()`
  для текущей версии отдаёт заголовок декодера в его нынешнем состоянии. Просмотр настоящих декодеров (`session-format-v0-to-v1` … `v3-to-v4`
  `src/codec.ts`): ни один не меняет заголовок после выдачи (заголовок строится один раз через `snapshotSessionFormatJson`, обёртки v2-to-v3 и v3-to-v4
  копируют `{ ...decoder.header, version }` при создании). Для миграции заголовок берётся у потока, как в образце. Тест: `current_restore_takes_the_decoder_header_...`.
- Описание миграции читается один раз при построении цепочки (`DeclaredMigration`: имя, `from_version`, `to_version`); образец копирует описание
  (`chain.ts:33`). Позднее изменение того, что вернёт `to_version()`, цепочку не затрагивает. Тест: `reads_the_migration_declaration_once_at_construction`.
- Значение в тексте ошибки печатается как в JavaScript (`hoh_util_values::js_string`): `current Session restorer returned v[object Object]; expected v1`;
  отсутствующая версия — `undefined`. Тест: `restorer_version_diagnostics_print_the_value_like_javascript`.
- Целые больше 2^53. Вход `9007199254740993` после снимка: здесь `9007199254740993`, в JavaScript `9007199254740992` (число округляется при разборе).
- Число с плавающей записью. Вход `1e0` или `1.0`: здесь после снимка сериализуется как `1.0`, в JavaScript как `1`. Проверки координат (`session_format_count` и др.)
  принимают такие значения как целые, как в образце; нормализации произвольного JSON нет.
- Ключи-индексы объекта. Вход `{"2":"b","1":"a","x":0}`: здесь порядок ключей файла (`2,1,x`, сериализация `{"2":"b","1":"a","x":0}`),
  в JavaScript целочисленные ключи идут по возрастанию первыми (`1,2,x`, `{"1":"a","2":"b","x":0}`), в том числе во вложенных объектах.
- Версия `-0` от восстановителя. Текущая версия 0, восстановитель артефакта вернул версию `-0`: в JavaScript `-0 === 0`, проверка проходит;
  здесь отказ `current Session restorer returned v0; expected v0` (`session_format_version_labeled` отклоняет `-0`). Тест:
  `restorer_returning_negative_zero_version_is_rejected_unlike_javascript`.

- Fix-round c4, пункты 1 и 8: `SessionFormatEventRun: Any + Send` позволяет стабильный upcast `&dyn SessionFormatEventRun -> &dyn Any` и `Box<dyn SessionFormatEventRun> -> Box<dyn Any + Send>`, без дополнительных методов и без раскрытия run. Конкретный тип должен быть `'static` (требование `Any`); владение строками/событиями и синхронные операции сохранены. Тест `event_runs_support_borrowed_and_owned_any_upcasts_preserving_send` проверяет borrowed/owned downcast, сохранение allocation после отказа по чужому типу и последующее раскрытие.
- Fix-round c4, пункт 8: JSON-кавычки имени миграции в `chain::json_string` пишет `hoh_util_values::js_json_stringify`; имеющаяся path-зависимость уже достаточна. Тест `duplicate_migration_names_use_shared_js_json_quoting` проверяет кавычку, backslash, перенос строки и управляющие символы в имени. Все 31 прежний тест и 2 новых проходят (33 passed, 0 failed); clippy с `--all-targets -- -D warnings` и fmt `-- --check` чисты при `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c4`.

### Проверка cause для c4 (w7)

Используется SessionFormatError::new_with_cause(message, cause) и FormatWithCause
{ message, cause: Box<dyn Error + Send + Sync> }. Это тот же класс SessionFormatError,
не UnsupportedMigration и не Other; Display message не дополняется причиной.
Error::source() возвращает исходную причину с downcast до исходного типа.
Прежние new, Format(String), unsupported_migration, other и UnsupportedMigration
{ message, cause } сохранены. error_cause.rs проверяет причину, вложенную цепочку,
класс и старые конструкторы.

После замечания ведущего убраны избыточные cause() и path-зависимость hoh-error.
Отдельный getter не нужен: достаточно стандартного Error::source().
Конструктор/вариант сохранены: в проверенном git HEAD их нет, а актуальный c4 уже
вызывает new_with_cause и сопоставляет FormatWithCause. Откат всей разницы error.rs
удалил бы используемый API. Нового API причин сверх этого существующего seam нет.
Код c4 writer w7 не менял.

## Не перенесено

- `deepFreeze` (раздел 8 / задание): нет аналога.
- Случаи `tests/json.spec.ts`, не выразимые в `serde_json::Value`: `Infinity`, `undefined`-член, разреженный массив, символьный ключ, неперечислимый член,
  свойство массива, цикл, экземпляры классов, `null` как заголовок. Покрыт только `-0` (остальные нарушения невозможны по построению типа).
  Отрицательные/дробные/`-0` версии для `sessionFormatLogFilename` — см. выше.
- `/* v8 ignore */` ветки «кодека нет» оставлены кодом, но недостижимы тестом (построение каталога доказывает полноту).
- `Object.freeze` результатов (`readHeader` и др.).

## Не проверено
- Платформенных веток нет. Кодеки и миграции реальных версий (v0–v4) здесь не проверялись: они живут в `session-format-v*` и `session-format-catalog`.
- Замена `localeCompare`/сортировок не требовалась (в пакете их нет). Длины строк не считаются.

## Вопросы ведущему
1. Использование экспортов вне неперенесённых пакетов. Все экспорты, кроме трёх, используются пакетами `session-format-v0..v4`,
   `session-format-catalog`, `session-persistence-jsonl`, `test-support/session-snapshot`. Только внутри самого пакета (и его тестов) используются:
   `createSessionFormatChain`, `inspectSessionFormatVersion`, `snapshotSessionFormatHeader` (последние два нужны каталогу и цепочке). Перенесены все.
   `isSessionFormatJsonObject`, `parseSessionFormatLogFilename`, `sessionFormatLogFilename` ещё используются неперенесённым
   `experimental/webworker-packer`, но у них есть и перенесённые потребители (`session-format-v*`, `session-persistence-jsonl`, `session-snapshot`,
   `session-query/session-log-export`); решение по ним не требуется.
2. Форма `SessionFormatHeader`/`SessionFormatEvent` как `Map<String, Value>` без типизированных полей: дальним crates придётся читать
   `version`, `seq`, `type` через `get`. Устраивает ли это, или нужна типизированная обёртка с `Deref<Target = Map>`?
3. Подозрение на образец: `finishDecoder` в каталоге сверяет только предварительно объявленный срез с `finish()` декодера, а при `undefined` — нет;
   перенесено как есть. Тест `bypasses ... current input` и др. заданы так, как в образце.
4. `createSessionFormatChain` и подобные принимают `Arc<dyn Trait>`; если дальним crates нужны не-`Send` стадии, ограничения `Send` придётся снять.

## Проверки исправления c4 (w7)

Первый прогон до удаления лишнего getter: CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w7.

- cargo test -p hoh-session-format --test error_cause: exit 0, **2 passed; 0 failed**.
- cargo test -p hoh-session-format -- --quiet: exit 0, **35 passed; 0 failed**.
- cargo fmt -p hoh-llm -p hoh-session-format -- --check: exit 0.
- cargo clippy -p hoh-llm -p hoh-session-format --all-targets -- -D warnings: exit 0.

Продолжение после указания ведущего выполнено с `CARGO_BUILD_JOBS=2`, offline и target
в tmpfs. Удалены ненужные cause() и dependency hoh-error; тест использует
Error::source(). Совместный прогон `cargo test -p hoh-llm -p hoh-session-format`
после этой уборки прошёл: **272 passed; 0 failed**, из них hoh-session-format 35/0.
Набор из 31 собирающегося потребителя прошёл 2406/0, 12 ignored. Clippy
`--all-targets -- -D warnings` чист для shared crates и всех проверяемых потребителей,
чьи графы не блокируют чужие warnings; fmt `--check` чист для всех 43 целей.
Внешние compile/test/clippy-блокеры перечислены в hoh-llm/PORT_NOTES.md и не требуют
изменений этого crate.
