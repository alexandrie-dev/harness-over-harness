# hoh-util-values ← packages/util/values @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` (`JsonValue`) | `src/lib.rs`: `pub use serde_json::Value as JsonValue` | перенесён |
| `src/index.ts` (`isJsonValue`, `snapshotJsonValue`, `deepEqualJson`) | `src/lib.rs`: `is_json_value`, `snapshot_json_value`, `deep_equal_json` | перенесены в форме для `serde_json::Value` |
| `src/index.ts` (`assertNever`, `deepFreeze`, `WeakMapWithValues`) | нет | не перенесены, см. ниже |
| `src/partial-json.ts` | `src/partial_json.rs` (`PartialArguments`, `LengthReadOptions`) | перенесён полностью |
| `tests/partial-json.spec.ts` | `tests/partial_json.rs` (70 тестов) | перенесён |
| `tests/weak-map-with-values.spec.ts` | нет | не перенесён вместе с классом |
| (нет в образце) | `tests/json_helpers.rs` | три теста на Rust-форму `is_json_value`, `snapshot_json_value`, `deep_equal_json`: у образца для них спецификации нет |

`locale/` у пакета нет. Все `it.each` исходного файла собраны в один тест с циклом по тем же случаям
(`uses_final_text_after_...`, `keeps_a_split_escape_...`, `drops_an_escape_still_in_flight_...`,
`freezes_as_invalid_...`, `indexes_later_keys_...`, `keeps_the_outer_object_open_...`).

## Отличия от образца
- Значения JSON — `serde_json::Value` (с `preserve_order`). Поэтому `isJsonValue`/`snapshotJsonValue` проверяют только то, что
  в `Value` ещё возможно нарушить: отрицательный нуль (`-0.0`). Циклы, разреженные массивы, символы, функции, экземпляры
  классов, чужие области выполнения (`hasIntrinsicConstructor` и т. д.) в Rust невозможны. Обход итеративный, как в образце.
  `snapshot_json_value` возвращает копию (`Option<Value>`); обобщённого `<T>` нет.
- `deep_equal_json` сравнивает числа по значению (`1` равно `1.0`): в JavaScript один числовой тип. Порядок ключей не важен.
  Образец проверяет ключи через `key in right`, то есть и по цепочке прототипов; в `Value` прототипа нет.
- Числа в разобранном тексте: целое значение после `JSON.parse` в образце печатается как `1`, а `serde_json` разбирает `-1e3`
  как `f64` и печатает `-1000.0`. Разобранные значения с целой дробной частью и модулем меньше 2^53 приводятся к целому
  (PORTING 4). Значение `-0` разбирается как `0`.
- `PartialArguments`: методы `&mut self` (читатели сканируют текст и запоминают ответы); `invalid`, `closed`, `keys` — методы,
  а не свойства. `is_sealed(&self)` не может сканировать по типу.
- `PartialArguments.EMPTY` — функция `PartialArguments::empty()`, каждый вызов даёт свой запечатанный вид (общий `static` запрещён, раздел 6).
- `append` возвращает `Result<(), SealedViewError>` вместо исключения; текст ошибки: `PartialArguments: cannot append to a sealed view`.
  Новый публичный тип `SealedViewError` (thiserror).
- `settle(&mut self, text) -> Option<PartialArguments>`: `None` — тот же вид запечатан на месте (образец возвращает `this`),
  `Some(view)` — новый запечатанный вид, исходный не изменён.
- `fromObject(unknown)` → `from_object(&Value)`; не объект — вид без полей. Вызов с `undefined` невозможен.
- Тексты. Образец считает длины и смещения в единицах UTF-16, как и перенос; хранение — `Arc<[u16]>`. Ответ может содержать
  одинокий суррогат (префикс, оборванный посреди пары). Поэтому для `text` и `textPrefix` есть пары: `text_utf16`,
  `text_prefix_utf16` (точные единицы) и `text`, `text_prefix` (`String`, одинокий суррогат заменён на U+FFFD).
  Ключи полей хранятся как UTF-16; `keys()` отдаёт `String` с той же заменой (для объектного вида ключи точные).
- `append` принимает `&str`: фрагмент не может заканчиваться одиноким суррогатом (в JavaScript поток может оборвать пару).
- `LengthReadOptions { step: Option<f64>, offset: Option<f64> }` оставлен дробным, потому что образец делает `floor` и тестирует `2.9`.
  `string_exceeds` и `text_prefix` принимают `usize`; «не меньше нуля/единицы» сохранено (`text_prefix` берёт `max(1, n)`).
- `#[doc(hidden)] json_parse_count()` — счётчик разборов JSON на одном виде. Нужен только затем, чтобы перенести проверки
  образца «ничего не разбирается, пока не попросили», которые там сделаны шпионом на `JSON.parse`. Символа в образце нет.
- Идентичность разобранного значения (`Object.is` на объектах в `refresh`) заменена номером записи: контейнер считается
  изменившимся, только если поле перезаписано новой записью, как и в образце.
- Равенство видов (`PartialEq`) сравнивает только источник (фрагменты, объект, флаг запечатанности), как `toEqual` по перечислимым полям.

## Не перенесено
- `assertNever` — исчерпывающий `match` без ветки `_` (PORTING 4); по правилу проверки типов TypeScript.
- `deepFreeze` — в Rust значения неизменяемы по умолчанию. Пользователи — 27 вызовов в других пакетах dsh.
- `WeakMapWithValues` (и `tests/weak-map-with-values.spec.ts`) — опирается на слабые ссылки сборщика мусора. Пользователи в образце
  (все этапа 9, «их браузерный интерфейс и его транспорт», в `MAP.tsv` помечены `skip`): `client/ui-session`,
  `client/ui-agent-preset`, `client/ui-conversation` (`assembly.ts`, `historical-images.ts`), `client/ui-commands`,
  `client/ui-input-trigger`, `client/ui-model-selection`. Ключ везде — `SessionBinding`, значения — контроллеры окна.
  В переносимых пакетах пользователей нет.
- Проверки `v8 ignore`, шпионы на `String.prototype.indexOf` и `Array.prototype.join`: в Rust `is_sealed` не сканирует по типу,
  а `settle` не склеивает фрагменты. Шпион на `JSON.parse` заменён `json_parse_count`.

## Не проверено
- Отличия `serde_json` от `JSON.parse`, не покрытые тестами: порядок ключей с целочисленными именами (JavaScript ставит их
  первыми по возрастанию), отказ `serde_json` на числа вне диапазона `f64` (`1e999`; JavaScript даёт `Infinity`), предел вложенности 128
  у `serde_json`, отказ на одиночный суррогат в `\uXXXX` внутри вложенного значения (JavaScript принимает). Во всех этих случаях
  значение читается как неверное (`invalid`). Строки верхнего уровня с одиночным суррогатом разбираются своим декодером, без отказа.
- Целые больше 2^53 в `Value` сохраняют точность, в JavaScript они бы округлились.
- Платформенных веток нет.

## Вопросы ведущему
- Принять ли пару `text`/`text_utf16` (и `text_prefix`/`text_prefix_utf16`) как публичный вид? Потребителям, которым нужен `String`,
  достаточно `text`.
- Нужен ли потребителям `append` с фрагментами в UTF-16 (`&[u16]`)? Сейчас поток из `&str` не может оборвать суррогатную пару.
- В образце `README` называет `isJsonValue` проверкой «dense intrinsic arrays»; в Rust это свойство `Vec`. Если `serde_json::Value` не
  единственный носитель JSON в портируемых пакетах, нужна проверка для другого типа.
