# hoh-deepseek-llm-api-extensions ← packages/llm/deepseek-llm-api-extensions @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | Реестр, регистрация, подготовка и публикация сервиса |
| src/types.ts | src/types.rs, реэкспорт из lib.rs | Все 6 публичных типов, публичный модуль types |
| tests/registry.spec.ts | tests/registry.rs | Все 8 исходных случаев представлены; 7 полностью, в одном ждёт kernel подпроверка logger args; ещё 6 проверок границ |
| locale/*.json | locale/*.json | Все 8 файлов скопированы до байта, diff -r без различий |
| README.md | — | Прочитан; контракт учтён |
| package.json | Cargo.toml | Дополнительная точка входа ./types сохранена модулем types |

Собственного LICENSE и указаний на заимствования из других проектов в пакете нет.

## Отличия от образца

- Класс `DeepSeekLlmApiExtensionRegistry` реализует Plugin явно (допустимо по kernel §2.8), чтобы сохранить один тип и для сервиса, и для точки загрузки. Plugin::NAME = `DeepSeekLlmApiExtensionRegistry` — имя исходного JS-класса; сервис — `deepseekLlmApiExtensions`. Config = NoConfig, Deps = (). `new(ctx)` создаёт значение; Plugin::apply публикует его через provide, вместо побочного действия super(ctx, name).
- Добавлен типизированный ключ `DeepSeekLlmApiExtensions`; register получает `&Context` вызывающего по kernel §7. Результат — Disposer/Result вместо функции-отмены/исключения. Тип ошибок `ExtensionError` (InvalidField, DuplicateField, Aggregate) — адаптация Error/AggregateError; список errors имеет исходный порядок. Для одиночного отказа и отмены сохраняется идентичность SharedError.
- DeepSeekLlmApiJson = serde_json::Value; открытая DeepSeekLlmApiExtensionMap = serde_json::Map, без общего enum чужих полей. PreparedDeepSeekLlmApiExtension<T> и trait DeepSeekLlmApiExtensionProvider<T> сохраняют параметр значения; T: Serialize + Send. Замыкания с Future автоматически реализуют trait. Result/Option заменяют throw/undefined. Подготовка сериализует полученное T в отдельный Value; ошибка сериализации обрабатывается как невозможность structuredClone.
- Запрос передаётся всем провайдерам через один Arc с body, session_id, purpose, signal. Ничего из этих фактов не пересериализуется в протокол данным пакетом. Отмена — общий hoh_timeout::AbortSignal по PORTING §6. Подготовка запускается eagerly и фиксирует providers при вызове, а не при первом polling.
- Поля готового запроса приватны, доступ через fields(): &Map; это гарантирует глубокую неизменяемость без Object.freeze. Никаких изменяемых alias на провайдерский Value нет. Clone подготовленной транзакции разделяет те же поля и acceptance.
- Добавлен AcceptanceCallback = Arc<dyn Fn() -> BoxFuture<Result<(), BoxError>>>. Состояние результата захватывается замыканием через Arc, заменяя JS bind(result). accept запускает одну общую Shared-future с отдельным драйвером Tokio, чтобы Drop ожидающего не отменял callbacks и повторный вызов никогда их не запускал заново. Подготовленная acceptance остаётся действительной после удаления регистраций и выгрузки сервиса, как в образце.
- Работа подготовки после отмены запроса продолжает выполняться, как проигравший Promise.race, до завершения или выгрузки самого реестра; она принадлежит ctx.spawn. Выгрузка реестра прекращает эту работу, в отличие от независимых JS promises: правило владения задачами ядра не допускает работу плагина после выгрузки. Уже захваченная acceptance независима от регистрации, согласно контракту источника.
- Проверка trim воспроизводит именно ECMAScript whitespace (включая FEFF, исключая 0085); тексты валидации и duplicate, включая JSON-кавычки имени, сохранены. JSON-цитирование — hoh_util_values::js_json_stringify.
- Два оставшихся ограничения диагностик ядра вынесены в «Вопросы ведущему» с точными местами, текущим состоянием и минимальными необходимыми дополнениями. Их побайтная эквивалентность не заявляется; своих экспортёров, интерпретатора `%o`, дополнительных строк журнала и утечек строк ради `'static` не добавлено.
- Проверены внешние потребители: llm-deepseek (adapter, types, request-extensions), session-log-deepseek, plugin-package-inventory-deepseek, test-support/llm-replay. Именные импорты DeepSeekLlmApiExtensionProvider и PreparedDeepSeekLlmApiExtension извне не найдены (контекстно выводятся при register); все экспорты сохранены. Declaration merging как механизм заменён открытой Map по PORTING §4/§8.

## Не перенесено

Целиком исключённых исходных случаев нет: представлены все 8 (7 полностью, 1 частично). В `rejects_invalid_field_names_and_omits_a_failing_field_warning_once` перенесены invalid field, omission, два запроса, количество accept и warn-once; **не перенесена подпроверка исходного двухэлементного args с идентичностью ошибки**, поскольку Record его не хранит. Текущая проверка одной строки не считается выполнением этой исходной подпроверки. Тестов с ignore нет.

Runtime Object.isFrozen заменён невозможностью получить mutable reference к выходным полям. JS this заменён захваченным состоянием. Динамическая effect label и полная доставка logger args ждут дополнения kernel, см. вопросы. Своей заменой текстов/протокола эти ограничения не исправляются.

## Не проверено

Платформенных веток нет. Побайтная эквивалентность lifecycle-диагностик с динамической меткой и исходной пары аргументов warning не проверена и невозможна на текущем API ядра; конкретные препятствия ниже. Изменение контракта kernel и его экспортёров не входит в назначенные crates.

## Вопросы ведущему

### 1. Динамическая метка регистрации

- **Источник:** `packages/llm/deepseek-llm-api-extensions/src/index.ts:91–99`, в особенности строка 99.
- **Что создаёт источник:** эффект с точной меткой `deepseekLlmApiExtensions.register(${JSON.stringify(fieldName)})`; например, `deepseekLlmApiExtensions.register("test_alpha")`. Имя поля является частью метки, а не отдельной строкой журнала. Диагностика освобождения/утраты эффекта может показать всю метку.
- **Что позволяет ядро сейчас:** `crates/hoh-kernel/src/effects.rs:403–408` — `Context::try_effect(label: &'static str, ...)`; само `Effect.label` на строке 97 тоже `&'static str`. По этому API нельзя передать строку, построенную из произвольного имени поля, сохранив обычное владение памятью. Вызов текущего порта использует статическую метку `deepseekLlmApiExtensions.register`; это остаётся неполнотой lifecycle-диагностик, а не объявленным эквивалентом источника.
- **Минимальное дополнение ядра для этого потребителя:** принимать `impl Into<Arc<str>>` в try_effect и внутренних reserve_effect/Effect::new, хранить `Arc<str>` вместо `&'static str`. Существующие литералы продолжат работать; в порте можно будет передать `format!("deepseekLlmApiExtensions.register({})", quote_field(field))` без утечки памяти. Расширять публичные effect_async/spawn для этого потребителя не требуется. Отдельной строки журнала с полем добавлять не требуется.

### 2. Структурированное предупреждение о подготовке поля

- **Источник:** `packages/llm/deepseek-llm-api-extensions/src/index.ts:118–121`, в особенности строка 121. Исходная проверка `tests/registry.spec.ts` сравнивает именно массив аргументов экспортёра, включая исходную ошибку.
- **Что создаёт источник:** один WARN от `deepseekLlmApiExtensions`, один раз на поле за жизнь реестра. `args[0]` = `deepseek-llm-api-extensions: omitting field "test_alpha" from this request because its preparation failed: %o`; `args[1]` — та же пойманная ошибка, без String/JSON-преобразования. Источник `vendor/cordis/src/logger.ts` передаёт args экспортёру; `vendor/logger-console/src/index.ts:9–17` задаёт для `%o` node:util.inspect. Значит, текст для консоли выбирает экспортёр; другой читатель видит исходную пару, включая идентичность ошибки.
- **Что позволяет ядро сейчас:** `crates/hoh-kernel/src/logger.rs:82–83` — только `Logger::warn(impl Display)`; `Record` на строках 15–22 хранит лишь `message: String`, без args/ошибки. Сохранить пару и дать экспортёру выбрать её представление невозможно. Текущий нативный вызов порта передаёт префикс и Display ошибки одной строкой; тест проверяет warn-once и эту Rust-строку, **но исходная подпроверка точного args и идентичности ошибки в logger остаётся неперенесённой**. Это оставшийся разрыв, не побайтное воспроизведение. Публичный протокол запросов не меняется.
- **Минимальное дополнение ядра для этого потребителя:** `Logger::warn_with_error(format: impl Into<Arc<str>>, error: hoh_error::SharedError)` и хранение исходного format + SharedError в Record, доступных exporter/buffer. Новый метод не должен заранее подставлять Display/Debug вместо `%o`; разрешение формата принадлежит экспортёру. Обобщённый `warn_args(Vec<LogArg>)` также подходит, но для единственной пары format/error не обязателен. Тогда порт передаст исходный template и ту же SharedError, а source-test сможет сравнить обе части, не ослабляя исходную проверку.

Разрешение path-зависимостей получено; по исправленному brief добавлены hoh-timeout, hoh-error и hoh-util-values. Их API достаточен. Обе описанные неполноты требуют решения ведущего в hoh-kernel; чужие crates не менялись.

## Проверки

Все команды с `CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w5`.

- `cargo build -p hoh-deepseek-llm-api-extensions`: exit 0.
- Первый `cargo test -p hoh-deepseek-llm-api-extensions` выявил недоступный при workspace-features макрос futures_util::poll!. Проверка pending заменена std::future::poll_fn; новых features или библиотек не добавлено.
- `cargo test -p hoh-deepseek-llm-api-extensions`: `test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (unit/doc-tests 0).
- `cargo clippy -p hoh-deepseek-llm-api-extensions --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-deepseek-llm-api-extensions -- --check`: exit 0.
- locale: `diff -r` с исходным каталогом, exit 0, различий нет.
