# hoh-deepseek-llm-api-extensions ← packages/llm/deepseek-llm-api-extensions @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | Реестр, регистрация, подготовка и публикация сервиса |
| src/types.ts | src/types.rs, реэкспорт из lib.rs | Все 6 публичных типов, публичный модуль types |
| tests/registry.spec.ts | tests/registry.rs | Все 8 исходных случаев перенесены с согласованной адаптацией logger к одной строке; ещё 8 проверок границ, включая реальную cleanup-диагностику регистрации |
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
- Проверка trim использует общий `hoh_util_values::js_trim` (PORTING.md §4): локальный js_whitespace удалён, даже несмотря на совпадавший набор символов. Общий помощник проверен по Node 22.23.1 и сохраняет FEFF/0085 семантику. Тексты валидации и duplicate, включая JSON-кавычки имени, сохранены. JSON-цитирование — hoh_util_values::js_json_stringify.
- **Согласованное отличие диагностического журнала:** источник `packages/llm/deepseek-llm-api-extensions/src/index.ts:121` передаёт format с `%o` и пойманную ошибку раздельно. По решению ведущего журнал kernel — диагностический поток, не сравниваемый вывод; docs/kernel.md §7 переводит printf-вызовы ctx.logger в одну строку, форматируемую на месте вызова. Поэтому `%o` ошибки представлен её Display и всей цепочкой Error::source(), итеративно через `: `, в **одной** WARN-записи с исходным префиксом и warn-once на поле. Дополнительного API logger не требуется; вопрос закрыт. Это сознательное отличие от JS args/node:util.inspect, не потерянная подпроверка. В соответствии с решением и правилами диагностирования ошибок ядра (§3.7/R57 и logger tests) сохраняются одно сообщение и управление количеством записей, а не JS-идентичность аргумента. Тесты проверяют точную строку, один warn при двух запросах и полную цепочку причин; простая ошибка без cause оставляет прежний текст.
- Прежнее ограничение метки закрыто: `Context::try_effect` принимает owned label через `impl Into<Arc<str>>`. Регистрация передаёт точную source-метку `deepseekLlmApiExtensions.register(${JSON.stringify(fieldName)})` (`src/index.ts:99`): в Rust `format!("deepseekLlmApiExtensions.register({})", quote_field(&field))`, где quote_field использует общий js_json_stringify. Это восстановление source-поведения, а не намеренное отличие. Собственных экспортёров, интерпретатора `%o`, дополнительных строк журнала и утечек строк ради `'static` не добавлено.
- Проверены внешние потребители: llm-deepseek (adapter, types, request-extensions), session-log-deepseek, plugin-package-inventory-deepseek, test-support/llm-replay. Именные импорты DeepSeekLlmApiExtensionProvider и PreparedDeepSeekLlmApiExtension извне не найдены (контекстно выводятся при register); все экспорты сохранены. Declaration merging как механизм заменён открытой Map по PORTING §4/§8.

## Не перенесено

Исключённых исходных runtime-случаев нет: перенесены все 8, 0 пропущено. В `rejects_invalid_field_names_and_omits_a_failing_field_warning_once` проверка JS args/идентичности ошибки согласованно заменена проверкой одной диагностической строки и warn-once (решение ведущего, см. «Отличия от образца»); это не блокировка на kernel. Тестов с ignore нет.

Runtime Object.isFrozen заменён невозможностью получить mutable reference к выходным полям. JS this заменён захваченным состоянием. Незавершённых частей из-за kernel больше нет; динамическая effect label перенесена и наблюдается в cleanup diagnostic.

## Проверка обновлённых правил PORTING.md §4 и §7

- Из нового списка общих помощников применим trim: собственный предикат заменён js_trim. В этом пакете нет path/fs/readdir/process-операций и прямых системных ошибок, требующих node_path или node_fs_error_message/node_spawn_error_message. JSON-текст имени поля уже формируется js_json_stringify; serde_json::to_value используется только для detached JSON-значения, не вместо записи протокольного текста.
- Тип ошибки определяется исходным базовым классом: валидация и дубликат бросают native Error, совместная acceptance — native AggregateError. Поэтому ExtensionError остаётся собственным enum с thiserror, без превращения в HarnessError. Тесты invalid/duplicate и aggregate дополнены проверками границы BoxError/SharedError: concrete ExtensionError узнаётся, HarnessError::of возвращает None. Отдельная ошибка провайдера и причина отмены по-прежнему передаются с исходной SharedError identity.

## Не проверено

Платформенных веток нет. Полная метка lifecycle-диагностик проверена после возврата register: тестовый provider паникует в Drop при удалении из настоящего реестра, а проверка сравнивает точную ERROR-запись cleanup с литералом ожидаемой метки. Имя содержит кавычку, обратную косую, внутренний newline и Unicode; проверяется JSON-экранирование, а не только наличие общего префикса. Паника находится только в test fixture; production cleanup не менялся. Побайтная эквивалентность JS-пары args и Rust-строки диагностического журнала не требуется по решению ведущего; пользовательские сообщения/ошибки и протоколы запросов этим решением не меняются.

## Вопросы ведущему

Нет. Owned labels доступны в kernel и применены к регистрации; вопрос закрыт. Вопрос о формате warning ранее закрыт решением ведущего и отражён в «Отличия от образца». API hoh-kernel, hoh-timeout, hoh-error и hoh-util-values достаточен. Чужие crates не менялись.

## Проверки

Все команды с `CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w5`.

- До изменения label новый `registration_cleanup_diagnostics_preserve_the_json_quoted_field_label` воспроизвёл ошибку: `test result: FAILED. 15 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out`. Реальная cleanup-запись содержала только статическое имя вместо source-метки с JSON-quoted field; test-only panic был пойман ядром, отказ теста — именно сравнение diagnostic label.
- После исправления `cargo test -p hoh-deepseek-llm-api-extensions`: `test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (unit/doc-tests 0).
- `cargo clippy -p hoh-deepseek-llm-api-extensions --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-deepseek-llm-api-extensions -- --check`: exit 0.
- locale: `diff -r` с исходным каталогом, exit 0, различий нет.
