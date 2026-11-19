# hoh-time-context ← packages/context/time-context @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | плагин, проекция `timeContext` v3, prepended pre-step listener, throttling и durable snapshot-context перенесены |
| `src/request-zone.ts` | `src/request_zone.rs` | извлечение, проверка, классификация и model-visible policy перенесены |
| `src/timestamp.ts` | `src/timestamp.rs` | ISO-shaped zoned timestamp перенесён через `jiff` |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `tests/request-zone.spec.ts` | `tests/request_zone.rs` | 5 / 0 случаев образца; дополнительная проверка принятой modern-IANA разницы |
| `tests/time-context.spec.ts` | `tests/time_context.rs`, unit-тесты `src/lib.rs` | основные ветви покрыты интеграционными и чистыми unit-проверками; literal testkit-ограничения перечислены ниже |
| `tests/initialization.spec.ts` | `tests/time_context.rs` | 0 / 4 literal race cases; ready/cancelled/disposed/config-failed ветви проверены без задержки регистрации |
| `tests/time-context.e2e.ts` | — | проверка собранного npm-пакета исключена правилом §10 |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json`, `tsconfig.json`, `tsdown.config.ts` | `PORT_NOTES.md`, `Cargo.toml` | прочитаны; отдельной лицензии в пакете нет |

## Отличия от образца

1. Публичные Rust-типы `TimeContext`, `Deps`, `TimeContextState`, `TimestampFormatter` и `TimeContextError`
   нужны typed kernel,
   projection handle и модульной границе Rust; в TypeScript их роли выполняют function-plugin, declaration
   merging, встроенные `Error`/`TypeError` и `Intl.DateTimeFormat`. `TimeContextError` остаётся обычной ошибкой,
   не `HarnessError`, и сохраняет JavaScript class name и `cause`.
2. `Intl.DateTimeFormat` заменён `jiff` из уже зафиксированного workspace. Config fallback, как и `Intl`,
   принимает известные базе aliases и single-segment zones (`CET`, `GMT`) и использует ECMAScript-oriented
   `primary_iana_name()` как resolved label. Строгий durable browser input по-прежнему проходит общий
   `hoh-util-time`: `Asia/Kolkata` каноничен, а legacy `Asia/Calcutta` там отклоняется. Node/ICU образца на
   этой машине сообщает обратный canonical label. Это принятое modern-IANA отличие закреплено тестами;
   ошибки lookup нормализованы в plain `RangeError`, который остаётся причиной plugin-load `Error`.
3. Promise готовности регистрации разделяется между перекрывающимися pre-step запросами через
   `tokio::sync::watch`; значение результата клонирует `SharedError`, сохраняя одну причину отказа. Это нативный
   эквивалент совместно ожидаемого JavaScript Promise.
4. `TimeContextState` хранит nullable числа как `serde_json::Number`, а не как один Rust integer: исходная zod
   schema допускает любое JSON-число, и checkpoint обязан отклоняться или восстанавливаться с тем же shape.
5. Config-zone в форме фиксированного offset (`+08:00`) принимается некоторыми версиями `Intl.DateTimeFormat`,
   но `jiff` требует named IANA zone либо `UTC`. Поэтому config offset отклоняется как `invalid IANA timeZone`;
   browser-reported offset также сохраняется durable, но отклоняется только на первом eligible request. Это
   принятое отличие Rust time-zone backend, закреплённое listener-level тестом timing отказа.

## Не перенесено

- `time-context.e2e.ts`: это built-package/Loader-проверка npm, прямо исключённая PORTING §10. Plugin contract,
  namespace и загрузка через Rust kernel проверены интеграционным тестом.
- Четыре literal initialization race cases требуют отсутствующего hook задержки/отказа регистрации проекции и
  полного agent-loop testkit. Сама production-последовательность сохранена: prepended listener ставится до await,
  все запросы ждут один результат, после ожидания проверяется active owner.
- В `time-context.spec.ts` недоступны буквальные spies на чтение prefix журнала и process-global fake Date/TZ.
  Durable compaction/resume throttling, old-version replay/freeze и scoped waterfall reject/error/cancel покрыты
  буквально. `hoh-agent-loop-testkit` теперь присутствует, но не предоставляет аналога source `ScriptedAdapter`
  с управляемыми двумя ответами и fake-clock tool; поэтому полное adapter/tool ordering остаётся неперенесённым.
  Production listener ordering проверяется через настоящий kernel waterfall, а длительности/backward clock —
  чистыми unit-тестами без process-global часов.

## Не проверено

- Искусственное перекрытие первого запроса с вручную задержанной регистрацией проекции: публичный Rust API
  `SessionProjectionRegistry` не предоставляет hook задержки. Listener ставится до `registration.await`, общий
  readiness-result ожидается каждым запросом; ready, already-cancelled, disposed и registration/config failure
  ветви проверяются без такого hook.
- Отказ определения системной зоны нельзя детерминированно подменить без process-global вмешательства;
  проверен успешный native resolver и точный внешний текст обёртки для неверной явной зоны. В минимальном
  контейнере без IANA `/etc/localtime` Rust может сообщить load error, тогда как ICU часто возвращает `UTC`.
- Windows и macOS отдельно не запускались; платформенных ветвей в коде крейта нет.

## Вопросы ведущему

- Если `SessionProjectionRegistry` получит test hook для отложенного результата регистрации, исходный четырёхходовый
  initialization race следует повторить буквально; production API этого крейта менять для тестового hook не требуется.
