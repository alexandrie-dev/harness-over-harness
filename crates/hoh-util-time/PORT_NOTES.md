# hoh-util-time ← packages/util/time @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` (реэкспорт; `canonicalClientTimeZone` → `canonical_client_time_zone`) | перенесён |
| `src/instant.ts` | `src/instant.rs` | перенесён |
| `src/duration.ts` | `src/duration.rs` | перенесён |
| `src/recurrence.ts` | `src/recurrence.rs` | перенесён |
| `src/cron.ts` | `src/cron.rs` | перенесён |
| `src/rrule.ts` | `src/rrule.rs` (включая обход вхождений, который образец берёт у npm `rrule`) | перенесён |
| `tests/time.spec.ts` | `tests/time.rs` | 4 теста из 4 |
| `tests/duration.spec.ts` | `tests/duration.rs` | 7 тестов из 7 |
| `tests/instant.spec.ts` | `tests/instant.rs` | 23 теста из образца (+1 проверка границ, +1 проверка отказа `resolve_local_instant`) |
| `tests/recurrence.spec.ts` | `tests/recurrence.rs` | 8 из 9 (см. «Не перенесено») |
| `tests/cron.spec.ts` | `tests/cron.rs` | все случаи; два теста с шпионами переносятся частично |
| `tests/rrule.spec.ts` | `tests/rrule.rs` | все случаи |
| `locale/` | — | в образце нет |
| вспомогательные модули | `src/calendar.rs`, `src/zone.rs`, `src/error.rs`, `src/zone_links.rs` | новые, внутренние (кроме `TimeError`) |

## Отличия от образца
- **Время и числа.** Эпоха в миллисекундах — `i64`; `LocalCalendarTime` хранит `i32`/`u32`. `Duration` остаётся на `f64`, потому что образец хранит дробные миллисекунды; все его части — `f64`, формулы те же. Тексты ошибок `Duration` повторяют `String(number)` JavaScript.
- **Ошибки-исключения** (`TypeError`, `RangeError`) — `Err(TimeError::Type | TimeError::Range)` с тем же текстом. `TimeError` — новый публичный тип, заменяющий исключения.
- **`resolve_local_instant`** принимает только `&LocalCalendarTime`: перегрузки на `Temporal.PlainDateTime` нет (нет такого типа). Возвращает `Result<Option<i64>, TimeError>`: `Err(Range)` для неизвестной зоны и несуществующей даты, как исключение образца. Годы вне ±9999 (предел `jiff`) тоже дают `Err(Range)`; образец принимает любой год `Temporal`.
- **Вместо `Temporal`** — `jiff` и собственная григорианская арифметика (`src/calendar.rs`) для всех дат; `jiff` берётся только для смещений зоны и разрешения местного времени. Политика `Temporal` сохранена: при перекрытии берётся более раннее, в «дыре» результата нет. Смещение для мгновения за пределом `Timestamp` у `jiff` (после 9999-12-30T22:00Z) берётся на ближайшей границе; влияет только на два последних дня 9999 года около перехода зоны, которых в данных нет.
- **Каноническое имя зоны** в образце даёт `Intl` (ICU). Здесь — таблица ссылок IANA `tzdata.zi` 2026e (`src/zone_links.rs`, 151 пара «псевдоним → цель»), потому что в системном каталоге зон нет файлов псевдонимов (например, `US/Eastern`). Различие с ICU: цель ссылки IANA не всегда совпадает с именем ICU (ICU отдаёт `Asia/Calcutta` для `Asia/Kolkata`, здесь остаётся `Asia/Kolkata`; то же для `Asia/Saigon`, `Europe/Kiev` и подобных). `Etc/UTC`, `Etc/GMT` и их синонимы дают `UTC`, как в V8. Имя зоны без учёта регистра ищется по таблице и по имёнам `jiff`, как это делает `Intl`. `cause` отказа `unsupported` — строка `Invalid time zone specified: <имя>` вместо объекта `RangeError`; `cause` отказа зоны в Cron/RRULE — строка с причиной.
- **Обход вхождений RRULE.** npm `rrule` заменён собственным обходом (порт итерации dateutil/rrule.js) только для профиля, который допускает `rrule.ts`; crate `rrule` не применим (INTERVAL до 2^53−1, обход по минутам и BYSETPOS по правилам rrule.js), зависимость удалена из манифеста. Повторяются поведения rrule.js, в том числе: первый период WEEKLY усечён по DTSTART; BYSETPOS считается по всему периоду, а вхождения раньше DTSTART отбрасываются после выбора; повторы BYSETPOS не склеиваются (склейку делает `count_window`); отрицательная позиция BYSETPOS дальше числа дней берёт первый день, как `slice(-k)[0]`; в сочетании `BYDAY=MO,2TU` остаются только пересечения (как фильтр rrule.js). Обход BYWEEKNO, BYYEARDAY, BYDAY с порядковыми номерами проверен вручную на нескольких правилах (ISO-неделя 1 и −1, `-1FR`, `2TU` в феврале), но тестами образца покрыт только разбор.
- **Новые публичные символы:** `TimeError`, `RRuleParseOptions` (параметры `parse`, у образца — безымянный объект), `RecurrenceResult::into_rule`, `as_str()` у перечислений причин (текст причины образца), `RecurrenceInputError::{new, with_cause}`, `parse_recurrence`, `recurrence_decision` (в образце внутренние, но доступны через `exports ./src/*`), `Cron::fields/expression/time_zone` и `RRule::expression/time_zone` как методы вместо свойств. `DurationParts` содержит `f64`.
- Заморозка (`Object.freeze`) не переносится: поля приватны, мутаторов нет.

## Не перенесено
- Проверки замороженности и отсутствия приведений типа (`Object.isFrozen`, `Object.hasOwn(Duration.prototype, 'valueOf')`) в `duration.spec.ts` — понятий языка Rust нет (правило раздела 10).
- `recurrence.spec.ts`: «preserves unexpected parser failures» — Rust не имеет неожиданного исключения из разбора; замыкание `parse_recurrence` возвращает отказ значением. Проверка отсутствия свойства `code` у отказа — у `RecurrenceResult::Rejected` такого поля нет.
- Значения `NaN` и нецелые эпохи в `instant.spec.ts` и `recurrence.spec.ts` (`isFourDigitYearInstant(0.5)`, `next(NaN)`): тип `i64` их не выражает. Проверки границ (±1 мс) сохранены.
- `cron.spec.ts`: два теста считают вызовы `Temporal.PlainDate.prototype.add/subtract/toPlainDateTime` шпионами полифила (границы 400·(366+12)·2 и 1500). Счёта вызовов в Rust нет; сохранены наблюдаемые результаты тех же сценариев и значение `CRON_SEARCH_HORIZON_YEARS`.
- Не перенесено намеренно: README, проверки сборки npm.

## Не проверено
- Ветки платформ: код платформо-независим; зоны читает `jiff` из системного `/usr/share/zoneinfo` (на Windows и macOS без системных данных нужен пакет данных `jiff`, не проверялось).
- Тесты зависят от данных зон на машине (версия tzdata 2026e); переходы New_York, Kiritimati, Los_Angeles и Singapore проверены только на ней.
- Порт обхода rrule.js не сверялся со сторонним npm-прогоном: сверка шла по тестам образца и по ручным примерам выше.
- Годы вблизи 0001 в зонах с LMT-смещениями (секунды): проверены только `Etc/GMT±1`.

## Вопросы ведущему
- Каноническое имя зоны: принять таблицу IANA вместо ICU (см. отличия) или добавить таблицу имён ICU для расхождений (`Asia/Calcutta`, `Europe/Kiev`, …)? Источник её составления не перенесён.
- В образце `dateutil.sort(poslist)` при BYSETPOS вне числа дней сортирует массив с недействительными датами и компаратором, дающим `NaN`: порядок остальных элементов в этом случае не определён. Перенос просто пропускает такие позиции и сортирует остальные. Подозрение на ошибку образца, не исправлялось.
- `BYDAY=MO,2TU` в образце (как в rrule.js) даёт пустой набор; перенесено как есть.
- Нужен ли `serde` для `RecurrenceOccurrence` и результатов разборщиков: в образце сериализации в этом пакете нет, добавлено не было.
