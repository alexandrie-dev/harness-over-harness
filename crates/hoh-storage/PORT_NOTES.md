# hoh-storage ← packages/storage/storage @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Сервис storage, плагин, формы, lifecycle key |
| src/backend.ts | src/backend.rs | Все интерфейсы и контрактные комментарии |
| src/error.ts | src/error.rs | Собственный StorageError с thiserror: имя, коды, сообщение и cause |
| src/registry.ts | src/registry.rs | Полностью, порядок вставки и сравнение экземпляров |
| tests/registry.spec.ts | tests/registry.rs | 5/5 исходных случаев и 2 дополнительных |
| tests/contract.ts | tests/contract.rs | Все 18 развёрнутых сценариев, 5 с согласованным заменителем отказа; макрос отдельных тестов |
| — | tests/contract_support.rs | Два самостоятельных теста descriptor/error checks, не попадающих в backend crates при #[path] |
| — | tests/contract_runner.rs, tests/support/contract_backend.rs | Проверка регистрации всех 18 сценариев на test-only memory fixture; 2 проверки чувствительности сценариев к нарушениям |
| locale/*.json | locale/*.json | Все 8 файлов скопированы без изменений |
| README.md, package.json | — | Прочитаны; внешнего кода/отдельной LICENSE нет |

## Отличия от образца

- StorageService — значение сервиса, Storage — тип ключа ServiceKey, StoragePlugin/apply — один плагин с именем storage, без config/inject. Это разделение класса Service по docs/kernel.md. Прямое свойство domain стало методом.
- StorageForms — открытая карта String → Arc<dyn Any + Send + Sync>, без перечня чужих форм. form/domain возвращают тот же Arc; владелец формы делает downcast. Map заменён IndexMap; удаление shift_remove сохраняет порядок.
- Возвращаемые register/mount функции остаются повторно вызываемыми Fn() с тем же сравнением идентичности Arc, включая повторную регистрацию того же экземпляра. В исходнике эти методы не вызывают ctx.effect: регистрация не снимается автоматически и не закрывает backend. Плагин-владелец может передать функцию в Undo::sync; сам сервис предоставляется kernel effect.
- BackendFuture/StorageResult — публичные псевдонимы для object-safe async API: BackendFuture возвращает Result<T, hoh_error::BoxError>, StorageResult — Result<T, StorageError>. KvLayout — закрытое объединение layout. KvSnapshot именует исходный inline result. JSON сохраняет порядок полей и различает отсутствие layout/compatibleVersions и null global. KvUnit::backup_record возвращает Option<future>: None означает отсутствие необязательного метода, а не успешную пустую операцию. kv() аналогично представляет необязательный facet.
- Только JSON Value на границе backend: нет исполняемых toJSON, циклов, undefined. Версии — u64 по PORTING §4. UNIT_NAME_RE — неизменяемый LazyLock<Regex>.
- По уточнённому PORTING §7 исходный базовый класс определяет Rust-тип: StorageError extends Error, поэтому восстановлен собственный StorageError с приватными code/message/cause и thiserror, а не HarnessError. new(code, message) возвращает Self; сохраняются имя StorageError, все коды/сообщения, стандартные Display/Error, clone и cause через Arc. Проверка типа — downcast_ref::<StorageError>(); HarnessError::of его не распознаёт. Предыдущее противоположное указание ведущего отменено fix-stage1-w4-storage.md. hoh-error остаётся нужен для общего BoxError в BackendFuture, apply и with_cause, а также для проверки разграничения типов в тестах.
- Два дополнительных registry-теста проверяют точные диагностические строки, insertion order после удаления/добавления, source cause и все четыре пути отказа hub: boxed StorageError успешно downcast-ится в собственный тип и не распознаётся HarnessError::of.
- Два self-tests перенесены из includable contract.rs в отдельный integration target contract_support.rs: исходный descriptor (поля, optional defaults, порядок JSON) и error checks для version-mismatch/closed через BackendFuture/BoxError с сохранением имени, кода, текста и cause. Backend crates их больше не наследуют.
- Все функции сценариев остаются публичными; прежняя агрегирующая async-функция run_kv_backend_contract заменена макросом того же имени. Он регистрирует 18 отдельных #[tokio::test], по одному на каждую развёрнутую ветвь source it/it.each, с узнаваемыми snake_case именами. Отказ одного сценария не прячет остальные.
- В пяти сценариях ошибку throwing toJSON заменяет put_record("undeclared_table", ..., Value): таблица отсутствует в descriptor.tables, её отвергают все исходные backend (storage-json single-unit.records/per-record-unit.tableDir; storage-sqlite statementsFor). Четыре commits-the-current-stamp теперь сначала отвергают запись на v4, проверяют прежний snapshot, закрывают writer, снова открывают v3 и проверяют старые данные; затем исполняют исходную мутацию и проверки штампа/остальных данных. Восстановленный restores-the-open-snapshot проверяет неизменность load_all после отказа, успешный retry на том же unit и сохранение результата после reopen. Это доказывает отсутствие побочных изменений/рестампинга от отвергнутой записи и возможность продолжить работу. Отказ в середине сериализации и откат после частичного изменения serialization/publish этим не проверяются: Value не содержит исполняемого toJSON. Согласованный brief заменитель меняет триггер, но не проверки состояния.
- contract_runner.rs действительно включает helper через #[path] и инстанцирует макрос: все 18 случаев запускаются отдельно на изолированной test-only memory fixture. Её reopen делит память, а не диск: это проверка кода runner и assertions, не утверждение о долговечности production backend. Дополнительные проверки намеренно ломают fixture: рестампинг при rejected write обнаруживается каждым из четырёх commits-сценариев, изменение readable snapshot — восстановленным retry-сценарием.
- До реализации проверены импорты разрешённых packages/src: storage-json, storage-sqlite, storage-domain используют все runtime экспорты и типы, кроме формально открытого StorageForms (его расширяет storage-domain). Неиспользуемых экспортов нет.

## Не перенесено

- Из registry.spec.ts не пропущено ничего: 5 исходных случаев.
- contract.ts: перенесено 18/18 развёрнутых сценариев, ни один не пропущен. Из них 13 с исходным триггером и 5 с undeclared-table отказом вместо throwing toJSON по fix brief. Не воспроизводится только сам исполняемый JS toJSON/отказ посреди сериализации; это явно ограничивает смысл пяти адаптированных тестов.
- TypeScript declaration merging как механизм заменён открытой картой по PORTING §4/8. Других исключений нет.

## Не проверено

- Сценарии общего KV backend contract исполняются на test-only fixture для проверки runner; production JSON/SQLite backend и реальная среда хранения ожидают своих crates следующих этапов. Дисковая durability, batch atomicity и восстановление после настоящего перезапуска здесь не объявляются проверенными.
- Платформенных веток нет. Контракты durability, versioning и batch atomicity реализуются backend, а не hub.

## Вопросы ведущему

Нет: тип StorageError и заменитель throwing-toJSON определены fix-stage1-w4-storage.md; недостающих API зависимостей нет.

## Для потребителей и авторов backend

Для подключения тестов backend определяет async fn create_harness() -> contract::KvBackendContractHarness (каждый вызов создаёт новую пустую изолированную среду) и пишет три строки в своём tests/*.rs; модуль contract.rs сам тестов не регистрирует:

```rust
#[path = "../../hoh-storage/tests/contract.rs"]
mod contract;
contract::run_kv_backend_contract!(contract, create_harness);
```

Для включаемого helper нужны hoh-storage и workspace-зависимости serde, serde_json, tokio (с test macros/runtime, уже включёнными в workspace).

KvSnapshot хранит таблицы и записи в IndexMap в порядке вставки, тогда как исходный loadAll возвращает обычные объекты JavaScript: канонические integer-like ключи (array indices 0..2^32-2) перечисляются сначала по возрастанию, например "2", "10", затем остальные ключи вроде "b" в порядке вставки. Backend, чей порядок файла или listing виден снаружи, обязан выдавать этот порядок, а не слепо повторять raw insertion order IndexMap; hoh_util_values::js_json_stringify уже соблюдает это правило при записи JSON-объектов. Для прочих строковых ключей сохраняется порядок вставки, а не лексикографическая сортировка.

## Проверки

Все Cargo-команды: CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w4.

- cargo test -p hoh-storage: contract_runner.rs — test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out. contract_support.rs — test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out. registry.rs — test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out. Всего 29 passed; 0 failed; lib, includable contract.rs и doc-tests — 0/0.
- cargo clippy -p hoh-storage --all-targets -- -D warnings: exit 0.
- cargo fmt -p hoh-storage -- --check: exit 0.
