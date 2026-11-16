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
| `tests/codec.spec.ts` | `tests/codec.rs` | 9 переносимых групп и 1 дополнительная регрессия JSON/packed integer gaps; Proxy-случаи ниже |
| `tests/legacy.spec.ts` | `tests/legacy.rs` | 10 групп; `it.each` сгруппированы в циклы |
| `tests/migration.spec.ts` | `tests/migration.rs` | 10 тестов |
| `tests/relationships.spec.ts` | `tests/relationships.rs` | 24 группы |
| `tests/validation.spec.ts` | `tests/validation.rs`, `tests/common/mod.rs` | 19 групп; fixtures всех 51 типов и порча более 250 не-opaque полей |

`README.md` прочитан. `locale/` и собственного `LICENSE` в пакете нет; заимствований с отдельной лицензией исходный код и README не объявляют.

## Отличия от образца

- Соблюдены синхронные трейты и владение из `hoh-session-format`: `decode_row(Value)`, `transform_event(Map)`, `emit_run(Box<dyn SessionFormatEventRun>)`; `expand` расходует run и выдаёт ленивый итератор, без массива всех событий. Заголовок стадии клонируется, чтобы не хранить заимствование входа. Никаких изменений зависимостей не сделано.
- `releasedV0SessionFormatCodec`, `releasedV1SessionFormatCodec`, `sessionFormatV0ToV1` представлены константами `RELEASED_V0_SESSION_FORMAT_CODEC`, `RELEASED_V1_SESSION_FORMAT_CODEC`, `SESSION_FORMAT_V0_TO_V1`. Новые публичные типы `ReleasedSessionFormatCodec` и `SessionFormatV0ToV1` нужны для реализации трейтов базовой crate.
- `ReleasedAssistantChunkRun` — структура с полями исходного интерфейса; `runType` — метод базового трейта `run_type`. `is_released_assistant_chunk_run` проверяет тот же тег, но не выполняет narrowing/downcast (см. вопрос 1).
- `RELEASED_V0_EVENT_DISPOSITIONS` — неизменяемый lookup типа `ReleasedV0EventDispositions` с `get`/`iter`; возвращает принадлежащий вызывающему disposition. Дополнительная публичная функция `released_v0_event_dispositions()` материализует `IndexMap` для Rust-потребителей. `define_released_payload_disposition` принимает все три массива явно и копирует строки. `Object.freeze` не воспроизводится; данные не имеют разделяемого изменяемого владельца.
- Артефакты/события/заголовки используют JSON-типы базовой crate, сохраняющие порядок ключей. Удаление полей выполняется с сохранением порядка остальных. `undefined` представлен отсутствием ключа, явный `null` остаётся значением. Целые `f64` в safe-integer диапазоне после декодирования представлены целыми JSON-числами; `-0` сохраняется до проверяющего его валидатора. Числовые имена вложенных JSON-ключей перечисляются первыми по возрастанию, как в JavaScript. Исходные строки не меняются.
- Длины строк в этих проверках используются только для проверки непустоты; сравнения UTF-8/UTF-16 дают один результат. `localeCompare(..., 'en')` при формировании `RELEASED_V0_EVENT_TYPES` заменён фиксированным порядком замороженных 51 ASCII-имени; runtime-зависимости от локали нет.
- Все ошибки используют `hoh_session_format::SessionFormatError`, включая вариант `UnsupportedMigration`, как требует публичный контракт зависимости. Ошибки downstream-контекста возвращаются после того же изменения счётчика, что в образце. Recoverable issue хранит текст ошибки: в доступном JSON-представлении ошибки декодирования не имеют дополнительных cause-объектов.
- `ReleasedRelationshipExtensions` реэкспортирован из корня для объявления расширений в Rust; у образца интерфейс доступен через `./src/relationships`. Флаги — `bool`, `step_events` — `HashSet<String>`: исходные множества используются только для membership. Видимый порядок unresolved tool calls хранится в `IndexMap`.
- Опциональные аргументы TS заданы явно: recovery, version, extensions, политики координат и Assistant sources. `assert_released_v1_header` заимствует header; `restore_released_v1_artifact` принимает и возвращает артефакт по значению, а vocabulary заимствует как `HashSet<String>`.
- Проверка `KNOWN_SESSION_EVENT_TYPES` в тесте использует точный снимок `packages/core/session/src/known-event-types.ts` при указанном коммите: добавление зависимости на `hoh-session` запрещено заданием. Оба `tool/ptc-*` имени и разность множеств проверяются.
- `node:path.isAbsolute` повторён для текущей платформы; Windows root-relative пути (`\\foo`) принимаются, как в Node, через отдельную `cfg(windows)` ветку.

## Не перенесено

- `Proxy` и перехватывающие getters в `codec.spec.ts`: тест `contains non-SessionFormatError row failures during recoverable scans` и Proxy-часть `streams the same row-atomic recoverable prefix`. По разделу 8 `Proxy` не переносится; `serde_json::Value` не содержит исполняемых getters и не может бросить постороннее исключение при чтении. Ветка catch-перевода такого исключения в `released Session row N is malformed` непредставима. Проверки на `null`, gaps и последующий committing `turn/end` перенесены.
- `undefined` внутри `tool/result.meta` непредставим в `Value`; тот же отказ `opaque meta is not lossless JSON` проверяется на вложенном `-0.0`, который базовый `is_json_value` считает нелоссless. `maxRetries: undefined` заменён удалением свойства (на JSON-границе это один результат).
- JavaScript Object.freeze/идентичность объектов не имеют эквивалента в этих принадлежащих вызывающему Rust-значениях.
- Доступ к конкретным полям compact run через `Box<dyn SessionFormatEventRun>` после type guard не перенесён: базовый трейт не предоставляет downcast или доступ к stream. Сам компактный объект хранится и передаётся без раскрытия; вопрос 1 описывает требуемое решение зависимости.
- Общая точная сериализация произвольных IEEE-754 чисел за пределами safe-integer диапазона не может быть обеспечена JSON-синонимами базовой crate. Например, `serde_json::Value` может хранить `9007199254740993` без JS-округления; `1e20` при сериализации обычным serde остаётся экспоненциальным. Невалидные счётчики отклоняются, но такие числа в owner-opaque данных требуют общего решения (вопрос 2). Аналогично не представлены одиночные UTF-16 суррогаты; это ограничение JSON-носителя зависимости.

## Не проверено

- Windows-ветка absolute cwd и macOS не запускались; проверки выполнялись на Linux.
- Реальный внешний conformance corpus и полная цепочка v0→v4 здесь не запускались: задание разрешает только проверки своей crate. Покрыты перенесённые 5 спецификаций и дополнительная регрессия чисел/порядка ключей/packed gaps.
- Свойства JavaScript, непредставимые базовыми JSON-типами (getters, циклы, symbols, undefined, lone surrogates), см. выше.

Проверки с `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c4`:

- `cargo test -p hoh-session-format-v0-to-v1`: 73 passed, 0 failed (codec 10, legacy 10, migration 10, relationships 24, validation 19; unit/doc tests 0).
- `cargo clippy -p hoh-session-format-v0-to-v1 --all-targets -- -D warnings`: без предупреждений.
- `cargo fmt -p hoh-session-format-v0-to-v1 -- --check`: чисто.

## Вопросы ведущему

1. Для v1→v2 необходим доступ к полям `ReleasedAssistantChunkRun` после передачи через базовый `SessionFormatEventRun`. Нужен одобренный общий механизм безопасного downcast (например, `Any`/`into_any`) либо структурированный доступ к compact run в `hoh-session-format`. В своей crate нельзя изменить этот контракт; неизвестные runs по-прежнему раскрываются, известные передаются компактно. `unsafe` не используется.
2. Где должен жить JS-совместимый сериализатор произвольного JSON (округление больших чисел, экспоненты, суррогаты), учитывая зафиксированные `Map<String, Value>` на границе `hoh-session-format`? Без общего носителя/сериализатора полное побайтовое совпадение за пределами представимого профиля не завершено.
3. Экспорты без внешнего переносимого потребителя (проверены импорты TS, исключены client/api/typert/host/experimental/apps web/desktop): `RELEASED_V0_EVENT_TYPES`, `restoreReleasedV1Artifact`; через `./src/*` также `SUBAGENT_DESCRIPTOR_VERSIONS`, `assertReleasedArtifactCoordinates`, `assertReleasedSessionFormatHeader`, `releasedV0Record`, `assertReleasedV0Keys` и все функции `src/testing/{restore,validation}`. Они перенесены. Остальные корневые экспорты используют session-format-v1-to-v2, session-format-v2-to-v3, session-format-catalog и тесты session-persistence-jsonl; `ReleasedAssistantChunkRun` и `ReleasedRelationshipExtensions` также используются неявно через сигнатуры guard/validator.
