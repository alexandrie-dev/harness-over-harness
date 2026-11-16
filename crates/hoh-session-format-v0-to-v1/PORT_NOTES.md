# hoh-session-format-v0-to-v1 ← packages/session/session-format-v0-to-v1 @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | реэкспорты перенесены |
| `src/codec.ts` | `src/codec.rs` | оба физических кодека, compact runs, ленивое раскрытие, диапазоны ссылок, атомарное recoverable-декодирование |
| `src/dispositions.ts` | `src/dispositions.rs` | все 51 событие, required/optional/opaque, фиксированный список типов |
| `src/migration.ts` | `src/migration.rs` | синхронная стадия и все ограниченные legacy-нормализации |
| `src/payload-validation.ts` | `src/payload_validation.rs` | все ветки вложенной проверки, включая owner-opaque варианты |
| `src/relationships.ts` | `src/relationships.rs` | связи turns/steps/tools/PTC/retries/commands/titles/compactions, расширения следующих форматов |
| `src/validation-helpers.ts` | `src/validation_helpers.rs` | object/key проверки и тексты ошибок |
| `src/validation.ts` | `src/validation.rs` | логические заголовки, координаты, surface metadata, payload admission, vocabulary-neutral v1 restore |
| `src/testing/restore.ts` | `src/testing/restore.rs` | фасад на production-каталоге; неизменяемый глобальный каталог заменён локальным |
| `src/testing/validation.ts` | `src/testing/validation.rs` | все пять политик проверки |
| `tests/codec.spec.ts` | `tests/codec.rs` | 9 переносимых групп и 4 дополнительных теста JSON/packed integer gaps/downcast; Proxy-случаи ниже |
| `tests/legacy.spec.ts` | `tests/legacy.rs` | 10 исходных групп и 1 регрессия некорректных replacement starts; `it.each` сгруппированы в циклы |
| `tests/migration.spec.ts` | `tests/migration.rs` | 10 тестов |
| `tests/relationships.spec.ts` | `tests/relationships.rs` | 24 исходные группы и 1 регрессия прямой проверки неверных индексов |
| `tests/validation.spec.ts` | `tests/validation.rs`, `tests/common/mod.rs` | 19 исходных групп и 3 регрессии labels/порядка ключей/имён прототипа; fixtures всех 51 типов и порча более 250 не-opaque полей |

`README.md` прочитан. `locale/` и собственного `LICENSE` в пакете нет; заимствований с отдельной лицензией исходный код и README не объявляют.

## Отличия от образца

- Соблюдены синхронные трейты и владение из `hoh-session-format`: `decode_row(Value)`, `transform_event(Map)`, `emit_run(Box<dyn SessionFormatEventRun>)`; `expand` расходует run и выдаёт ленивый итератор, без массива всех событий. Заголовок стадии клонируется, чтобы не хранить заимствование входа. В fix-round по решению ведущего у `hoh-session-format::SessionFormatEventRun` добавлен supertrait `Any`; другие зависимости не менялись.
- `releasedV0SessionFormatCodec`, `releasedV1SessionFormatCodec`, `sessionFormatV0ToV1` представлены константами `RELEASED_V0_SESSION_FORMAT_CODEC`, `RELEASED_V1_SESSION_FORMAT_CODEC`, `SESSION_FORMAT_V0_TO_V1`. Новые публичные типы `ReleasedSessionFormatCodec` и `SessionFormatV0ToV1` нужны для реализации трейтов базовой crate.
- `ReleasedAssistantChunkRun` — структура с полями исходного интерфейса; `runType` — метод базового трейта `run_type`. `is_released_assistant_chunk_run` по-прежнему проверяет только тег. Добавлены публичные `as_released_assistant_chunk_run(&dyn SessionFormatEventRun) -> Option<&ReleasedAssistantChunkRun>` и `into_released_assistant_chunk_run(Box<dyn SessionFormatEventRun>) -> Result<Box<ReleasedAssistantChunkRun>, Box<dyn SessionFormatEventRun>>`: безопасное narrowing через `Any` без раскрытия run. Чужой тип (в том числе с совпадающим тегом) возвращается с тем же адресом и содержимым. Проверено заимствование всех compact-полей и сохранение allocation при owning downcast.
- `RELEASED_V0_EVENT_DISPOSITIONS` — неизменяемый lookup типа `ReleasedV0EventDispositions` с `get`/`iter`; возвращает принадлежащий вызывающему disposition. Дополнительная публичная функция `released_v0_event_dispositions()` материализует `IndexMap` для Rust-потребителей. `define_released_payload_disposition` принимает все три массива явно и копирует строки. `Object.freeze` не воспроизводится; данные не имеют разделяемого изменяемого владельца.
- Артефакты/события/заголовки используют JSON-типы базовой crate, сохраняющие порядок ключей. Удаление полей выполняется с сохранением порядка остальных. `undefined` представлен отсутствием ключа, явный `null` остаётся значением. Crate обрабатывает значения, а текстовая граница принадлежит общим `hoh_util_values::{js_json_parse, js_json_stringify, js_json_stringify_pretty}`: их правила определяют числа, суррогаты и порядок ключей в JSON-тексте. Все вызовы `JSON.stringify` образца, включая framed goal/title text и ошибки, используют `js_json_stringify`. Частный сериализатор и `normalize_json_layout` удалены: writer сам упорядочивает ключи, reader сразу выдаёт их в JS-порядке. Локальная `js_keys` оставлена **только** в `assert_released_v0_keys`: у программно собранного `Map` порядок первого неожиданного ключа определяет текст ошибки. Никакой иной нормализации значений при декодировании нет; `-0` доходит до проверяющего его валидатора без изменений. Исходные строки не меняются.
- Длины строк в этих проверках используются только для проверки непустоты; сравнения UTF-8/UTF-16 дают один результат. `localeCompare(..., 'en')` при формировании `RELEASED_V0_EVENT_TYPES` заменён фиксированным порядком замороженных 51 ASCII-имени; runtime-зависимости от локали нет.
- Все ошибки используют `hoh_session_format::SessionFormatError`, включая вариант `UnsupportedMigration`, как требует публичный контракт зависимости. Ошибки downstream-контекста возвращаются после того же изменения счётчика, что в образце. Recoverable issue хранит текст ошибки: в доступном JSON-представлении ошибки декодирования не имеют дополнительных cause-объектов.
- `ReleasedRelationshipExtensions` реэкспортирован из корня для объявления расширений в Rust; у образца интерфейс доступен через `./src/relationships`. Флаги — `bool`, `step_events` — `HashSet<String>`: исходные множества используются только для membership. Видимый порядок unresolved tool calls хранится в `IndexMap`.
- Опциональные аргументы TS заданы явно: recovery, version, extensions, политики координат и Assistant sources. `assert_released_v1_header` заимствует header; `restore_released_v1_artifact` принимает и возвращает артефакт по значению, а vocabulary заимствует как `HashSet<String>`.
- Проверка `KNOWN_SESSION_EVENT_TYPES` в тесте использует точный снимок `packages/core/session/src/known-event-types.ts` при указанном коммите: добавление зависимости на `hoh-session` запрещено заданием. Оба `tool/ptc-*` имени и разность множеств проверяются.
- `node:path.isAbsolute` повторён для текущей платформы; Windows root-relative пути (`\\foo`) принимаются, как в Node, через отдельную `cfg(windows)` ветку.

- JSON-индексы преобразуются через точный `as_u64` либо после проверки целочисленности, неотрицательности и диапазона `f64` до 2^53; затем индекс массива проверяется через `usize::try_from`. Неверные значения дают `None`, а не 0 или усечённую дробь. Это относится к replacement starts, event seq, command/title references и координатам relationship state. При прямом вызове relationship validator `command/done` с `kind: "success"` допускает отсутствующий target (`sourceEventSeq: -1`, `2.5` или индекс за концом массива), как `source?.type` в образце: отказ возникает только при `kind !== "success"` либо существующей ссылке на `command/run` или `command/done`. Точный `exact_index` не позволяет неверной ссылке попасть в другое событие. Для title `messageSeqs` отсутствующая ссылка по-прежнему даёт отказ; в полном пути восстановления ссылки также проверяет payload validator.
- Публичные поля `ReleasedAssistantChunkRun` позволяют вручную собрать run, который не мог бы выйти из декодера. Если время члена не является допустимым целым (`time0` или соответствующий `dt` неверны), `ReleasedAssistantChunkRun::expand` пропускает этот член через `filter_map`; образец продолжает выдавать событие, в частности с `NaN` при отсутствующем/нечисловом времени. Это отличие недостижимо для run, прошедшего атомарную проверку декодера.
- Предусловие порядка свойств после удаления `normalize_json_layout`: входные значения уже должны иметь порядок свойств JavaScript, который гарантирует `js_json_parse`. `Value`, построенный через `serde_json::from_str` или вручную, итерируется в порядке вставки; crate не нормализует такой вход. Исключение — локальная `js_keys` при выборе первого неожиданного ключа в `assert_released_v0_keys`, описанная выше.
- Диагностические labels и legacy ids берут исходные `event.type` / `event.seq` через `hoh_util_values::js_string`; отсутствие даёт `undefined`. Преобразование координат не подменяет текст значения (например, `abc`, `1.5`). Число при сравнении earlier-reference не округляется.
- По решению ведущего сохранено отличие plain-object inventory: в TS `RELEASED_V0_EVENT_DISPOSITIONS[type]` для `toString`, `constructor`, `hasOwnProperty` возвращает унаследованную функцию. Здесь настоящий lookup считает эти имена неизвестными типами и возвращает `UnsupportedMigration`. Это намеренное отличие, закреплённое регрессией.
- Предусловие рекурсивных payload-проверок и вызовов `js_string`: входные значения приходят из `js_json_parse`, который отказывает при вложенности глубже `JS_JSON_MAX_DEPTH` (4096). Программно собранные значения также не должны превышать этот предел; для потоков приложения PORTING задаёт стек 64 МиБ. Неограниченные последовательности событий обрабатываются циклами.

## Не перенесено

- `Proxy` и перехватывающие getters в `codec.spec.ts`: тест `contains non-SessionFormatError row failures during recoverable scans` и Proxy-часть `streams the same row-atomic recoverable prefix`. По разделу 8 `Proxy` не переносится; `serde_json::Value` не содержит исполняемых getters и не может бросить постороннее исключение при чтении. Ветка catch-перевода такого исключения в `released Session row N is malformed` непредставима. Проверки на `null`, gaps и последующий committing `turn/end` перенесены.
- `undefined` внутри `tool/result.meta` непредставим в `Value`; тот же отказ `opaque meta is not lossless JSON` проверяется на вложенном `-0.0`, который базовый `is_json_value` считает нелоссless. `maxRetries: undefined` заменён удалением свойства (на JSON-границе это один результат).
- JavaScript Object.freeze/идентичность объектов не имеют эквивалента в этих принадлежащих вызывающему Rust-значениях.

## Не проверено

- Windows-ветка absolute cwd и macOS не запускались; проверки выполнялись на Linux.
- Реальный внешний conformance corpus и полная цепочка v0→v4 здесь не запускались: fix-round разрешает проверки двух crate задания. Покрыты перенесённые 5 спецификаций и регрессии downcast, точных индексов, labels, JSON-границы и имён прототипа; все прежние тесты `hoh-session-format` также прошли.
- Свойства JavaScript, непредставимые базовыми JSON-типами (getters, циклы, symbols, undefined), см. выше.

Проверки с `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c4`:

- `cargo test -p hoh-session-format-v0-to-v1`: 81 passed, 0 failed (codec 13, legacy 11, migration 10, relationships 25, validation 22; unit/doc tests 0).
- `cargo clippy -p hoh-session-format-v0-to-v1 --all-targets -- -D warnings`: без предупреждений.
- `cargo fmt -p hoh-session-format-v0-to-v1 -- --check`: чисто.

## Вопросы ведущему

1. Решения fix-round по narrowing и JSON-тексту выполнены; незавершённых пунктов этого brief нет.
2. Экспорты без внешнего переносимого потребителя (проверены импорты TS, исключены client/api/typert/host/experimental/apps web/desktop): `RELEASED_V0_EVENT_TYPES`, `restoreReleasedV1Artifact`; через `./src/*` также `SUBAGENT_DESCRIPTOR_VERSIONS`, `assertReleasedArtifactCoordinates`, `assertReleasedSessionFormatHeader`, `releasedV0Record`, `assertReleasedV0Keys` и все функции `src/testing/{restore,validation}`. Они перенесены. Остальные корневые экспорты используют session-format-v1-to-v2, session-format-v2-to-v3, session-format-catalog и тесты session-persistence-jsonl; `ReleasedAssistantChunkRun` и `ReleasedRelationshipExtensions` также используются неявно через сигнатуры guard/validator.
