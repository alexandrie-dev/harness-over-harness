# hoh-session-format-v2-to-v3 ← packages/session/session-format-v2-to-v3 @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | все реэкспорты, включая frozen v2 codec |
| `src/codec.ts` | `src/codec.rs` | v3 framing поверх v2; admission до suppression; semantic recovery и accepted inherited cut |
| `src/migration.ts` | `src/migration.rs` | потоковая стадия, protected head, SHA-256 identities, cut/delivery guards, preset/PTC renames |
| `src/payload.ts` | `src/payload.rs` | весь v2 audit, рекурсивный owned content, native v3 admission/canonicalization |
| `src/references.ts` | `src/references.rs` | все явно перечисленные локальные координаты; opaque captures не меняются |
| `src/validation.ts` | `src/validation.rs` | v3 header, ownership/head checks, private relationship projection |
| `tests/catalog.ts` | `tests/common/mod.rs` | production catalog v0→v1→v2→v3, снимок known vocabulary |
| `tests/admission.spec.ts` | `tests/admission.rs` | 16 групп; таблицы выполняются циклами |
| `tests/canonical-envelopes.spec.ts` | `tests/canonical_envelopes.rs` | 42 группы с учётом describe.each двух режимов |
| `tests/canonical-regressions.spec.ts` | `tests/canonical_regressions.rs` | 6 групп |
| `tests/combined-migration.spec.ts` | `tests/combined_migration.rs` | 9 групп |
| `tests/content-admission.spec.ts` | `tests/content_admission.rs` | 64 группы с учётом 9 carriers и 2 stream owners; Rust test modules/macros сохраняют отдельные имена |
| `tests/migration.spec.ts` | `tests/migration.rs` | 40 групп, включая реальные v0/v1/v2 physical inputs |
| `tests/preset-migration.spec.ts` | `tests/preset_migration.rs` | 6 групп |
| `tests/structural-regressions.spec.ts` | `tests/structural_regressions.rs` | 8 групп |

Прочитаны все 815 строк src, все tests, README и package.json. В пакете нет `locale/` или собственного `LICENSE`; заимствования с отдельной лицензией не объявлены.

## Отличия от образца

- Соблюдён синхронный API и владение `hoh-session-format`: стадии получают owned `Map`/`Box<dyn SessionFormatEventRun>` и синхронный context, `finish` возвращает `u64`. Компактный generic run расходуется ленивым итератором. Исходный header и известный source cut копируются при создании стадии; состояния разных стадий независимы. Заголовок unseeded стадии заранее сообщает `Some(0)`, seeded — `None`.
- Экспортируемые объекты имеют Rust-константы `RELEASED_V2_SESSION_FORMAT_CODEC`, `RELEASED_V3_SESSION_FORMAT_CODEC`, `SESSION_FORMAT_V2_TO_V3`; новые публичные unit-типы `ReleasedV3SessionFormatCodec` и `SessionFormatV2ToV3` реализуют базовые трейты. Опциональное множество в `assert_v3_event` задаётся как `Option<&HashSet<String>>`; version в `assert_event` — `u64` (исходный union 2|3); `is_repair_identity` принимает два `Option<&Value>`, чтобы отсутствие не подменялось null. Все subpath exports представлены публичными модулями.
- `SURFACE_TYPES` — immutable slice в исходном порядке вместо JS Set. Множества ids/known types используются только для membership, поэтому HashSet не меняет наблюдаемого порядка. Локальное отображение координат — плотный `Vec<u64>`; JSON reference преобразуется только через точный `session_format_count` и проверенный `usize::try_from`, без усечения дроби и алиасов отрицательного индекса.
- JSON-текст и интерполяция значений используют `hoh_util_values::{js_json_stringify, js_string}`. Текстовая граница входа принадлежит `js_json_parse`. Предусловие: входные values уже имеют JavaScript property order; ручной `Value` или `serde_json::from_str` сохраняет insertion order и не нормализуется этим пакетом. Удаление `system` и пустых optional fields сохраняет порядок остальных свойств; обновление существующего ключа сохраняет его позицию. Отсутствие свойства и явный null различаются, в том числе в content-kind diagnostics и native retired-header admission.
- SHA-256 вычисляет разрешённый `sha2` над UTF-8 **общего JS JSON текста** исходного identity array; hex — 64 lowercase цифры, prefix исходный. Тексты prompt, пробельные символы, timestamps, исторические ids, serialized arguments/replay/title input и captures не переписываются. Отдельной locale-зависимой сортировки здесь нет. Проверки длины строк — только на пустоту; ASCII regexp repair suffix разобран без Unicode `\d` и округления чисел.
- Header/key/object helpers строятся на публичных API предыдущей crate; frozen payload/surface/relationship semantics импортируются из владельцев. Собственные алгоритмы этих зависимостей не копировались. Private relationship view имеет owned копии, потому что базовый API принимает целый artifact. Оно восстанавливает старые имена PTC/replacement и только внутренний repair id; наружу возвращается исходный owned v3 artifact с прежними сообщениями и именами.
- Ошибки остаются `hoh_session_format::SessionFormatError` / `UnsupportedMigration` (наследники plain Error в TS). В контекстном content diagnostic исходный `String(error)` включает имя класса: перенос явно добавляет `SessionFormatError:` или `SessionFormatUnsupportedMigrationError:` перед исходным текстом. Recoverable semantic issue хранит текст первого frozen format error; hard structural/obsolete-PTC admission выполняется до любой suppression и не попадает в этот catch.
- Frozen known-event vocabulary тестового каталога взят точным снимком `packages/core/session/src/known-event-types.ts` @39163dd3. Зависимость на `hoh-session` не разрешена manifest; production-код принимает vocabulary вызывающего кода, как образец.
- `Object.freeze`, JS reference identity и sharing вложенных объектов заменены владением Rust; source `toBe` проверки представлены проверками равенства и сохранения входного JSON. Намеренно сохранён ранее принятый lookup без JS prototype members: `toString`/`constructor` не становятся известными event types.
- По решению ведущего v1 продукта нацелен на Linux. Проверка absolute cwd наследует POSIX helper предыдущей crate; Windows-поведение остаётся примечанием, не локальной реализацией Node path. Предыдущая crate во время review не меняется без доказанного тестом дефекта.
- Рекурсивная проверка content ограничена общим `JS_JSON_MAX_DEPTH` (4096), приложение предоставляет стек 64 МиБ согласно PORTING. Программно собранные values также не должны выходить за предел; последовательности событий и runs обходятся циклами.

## Не перенесено

- Ни одна source test group или строка таблицы не исключена. `it.each` варианты выполняются циклами; `describe.each` и генерируемые source for-loops представлены отдельными Rust modules/test functions. Проверки неизменности входных данных сохранены; runtime freeze и JS object identity заменены Rust ownership, см. выше.
- В исходнике нет Typert, динамического import, Proxy-тестов, сетевых e2e или npm build tests. Эффектов ядра, таймеров, файлового ввода-вывода и плагина тоже нет.

## Не проверено

- Реальный внешний conformance corpus и последующий edge v3→v4 не запускались. Исторические fixtures v0 и v1 проходят все три production adjacent stages до v3, включая collapse chunks, неизвестный заранее cut, owned content admission и native reopen.
- Windows/macOS не запускались. JS getters, циклы, symbols и не-JSON значения не представимы в используемом базовом API. Ручные входы, нарушающие precondition уже проверенного mapper event/типов, не составляют новую схему валидации.

Проверки выполнялись с `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c4`:

- `cargo build -p hoh-session-format-v2-to-v3`: exit 0.
- `cargo test -p hoh-session-format-v2-to-v3`: exit 0, **191 passed; 0 failed; 0 ignored**; unit/doc-tests — 0/0. Все 191 source group (с учётом генерации for/describe.each) перенесены; варианты it.each исполняются внутри групп. Исключённых групп или строк таблиц нет.
- `cargo fmt -p hoh-session-format-v2-to-v3 -- --check`: exit 0.
- `cargo clippy -p hoh-session-format-v2-to-v3 --all-targets -- -D warnings`: exit 0, без предупреждений.

Точные строки итогового test run:

| binary | результат |
|---|---|
| `admission` | `test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| `canonical_envelopes` | `test result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s` |
| `canonical_regressions` | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s` |
| `combined_migration` | `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s` |
| `content_admission` | `test result: ok. 64 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s` |
| `migration` | `test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s` |
| `preset_migration` | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `structural_regressions` | `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s` |

В первом запуске исправлена только Rust fixture: helper для canonical tool log добавлял sourceEventSeqs в admission repair test, где исходник их не содержит. Source assertions не ослаблялись; production-код и зависимости из-за этого не менялись.

## Вопросы ведущему

1. До записи кода проверены внешние импорты с исключениями каталогов из задания: `session-format-v3-to-v4` использует v3 header/codec/restorer и реэкспорт v2 codec; `session-format-catalog` — оба codec/migration/header/restorer. У корневых `assertV3RowAdmission`, `assertV3EventAdmission` не найден внешний переносимый потребитель. Через `./src/*` также нет внешних потребителей `SURFACE_TYPES`, `record`, `keys`, `assertEvent`, `isRepairIdentity`, `assertV3StructuralRow`, `assertV3Event`, `canonicalizeTransformedEvent`, `remapEvent`; всё перенесено.
2. Решения ведущего по предыдущей crate приняты: cause/push_value precision принадлежат отдельным задачам владельцев; POSIX helper сохранён. Здесь не вводится новый обход этих зависимостей. Полная цепочка прошла на source fixtures, дефект предыдущей crate этими тестами не обнаружен; `hoh-session-format-v1-to-v2` не изменялась. Новых недостающих API зависимостей и незаконченных собственных веток нет.

## Дополнение: фиксированные system-message identities

Добавлены два regression tests в `tests/migration.rs`, production-код не менялся.
`v2-identity` закреплён точным literal из source session-format-catalog/tests/catalog.spec.ts:137:
`v2-to-v3-system-9673c4ed630de6c21ea6bd6b573094ea8e5e216843a1b572a68657499ad9667b`.
Для `сессия-猫-🦀` ожидается
`v2-to-v3-system-839148ff4c9e493a1990f9e5468f22c5b7e290b412e7df52843c7ff443e7c807`.
Literal вычислен разрешённой ведущим командой node -e с crypto SHA-256 над
`JSON.stringify(["session-format-v2-to-v3","сессия-猫-🦀",1,"step/start"])`;
полная команда сохранена комментарием в тесте. Два новых случая дополняют прежние
191 source groups, а не заменяют существующие проверки. Прежние result lines выше — история;
актуальный отдельный прогон завершён ниже.

Текущая проверка с `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2`
и тем же `CARGO_TARGET_DIR`:

- `cargo clippy -p hoh-session-format-v2-to-v3 -p hoh-session-persistence-jsonl --all-targets -- -D warnings`:
  exit 0, без предупреждений.
- `cargo fmt -p hoh-session-format-v2-to-v3 -p hoh-session-persistence-jsonl -- --check`:
  exit 0, без вывода.
- `RUST_TEST_THREADS=2 cargo test -p hoh-session-format-v2-to-v3`:
  exit 0, **193 passed; 0 failed; 0 ignored**, unit/doc-tests — 0/0.
  `Finished test profile [unoptimized + debuginfo] target(s) in 24m 45s`, включая очередь регулятора.

| binary | актуальная result line |
|---|---|
| `admission` | `test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `canonical_envelopes` | `test result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s` |
| `canonical_regressions` | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s` |
| `combined_migration` | `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s` |
| `content_admission` | `test result: ok. 64 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s` |
| `migration` | `test result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s` |
| `preset_migration` | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `structural_regressions` | `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s` |

Оба новых identity cases прошли. Производственные файлы не менялись, новых
расхождений с source и незавершённой работы по этому дополнению нет.
