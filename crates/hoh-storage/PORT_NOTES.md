# hoh-storage ← packages/storage/storage @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Сервис storage, плагин, формы, lifecycle key |
| src/backend.ts | src/backend.rs | Все интерфейсы и контрактные комментарии |
| src/error.ts | src/error.rs | Все коды; StorageError — конструктор общего HarnessError с исходным именем/текстом/cause |
| src/registry.ts | src/registry.rs | Полностью, порядок вставки и сравнение экземпляров |
| tests/registry.spec.ts | tests/registry.rs | 5/5 исходных случаев и 2 дополнительных |
| tests/contract.ts | tests/contract.rs | Общий вызываемый набор для backend crates; 2 самостоятельных теста fixture/error checks; см. ограничения ниже |
| locale/*.json | locale/*.json | Все 8 файлов скопированы без изменений |
| README.md, package.json | — | Прочитаны; внешнего кода/отдельной LICENSE нет |

## Отличия от образца

- StorageService — значение сервиса, Storage — тип ключа ServiceKey, StoragePlugin/apply — один плагин с именем storage, без config/inject. Это разделение класса Service по docs/kernel.md. Прямое свойство domain стало методом.
- StorageForms — открытая карта String → Arc<dyn Any + Send + Sync>, без перечня чужих форм. form/domain возвращают тот же Arc; владелец формы делает downcast. Map заменён IndexMap; удаление shift_remove сохраняет порядок.
- Возвращаемые register/mount функции остаются повторно вызываемыми Fn() с тем же сравнением идентичности Arc, включая повторную регистрацию того же экземпляра. В исходнике эти методы не вызывают ctx.effect: регистрация не снимается автоматически и не закрывает backend. Плагин-владелец может передать функцию в Undo::sync; сам сервис предоставляется kernel effect.
- BackendFuture/StorageResult — публичные псевдонимы для object-safe async API: BackendFuture возвращает Result<T, hoh_error::BoxError>, StorageResult — Result<T, hoh_error::HarnessError>. KvLayout — закрытое объединение layout. KvSnapshot именует исходный inline result. JSON сохраняет порядок полей и различает отсутствие layout/compatibleVersions и null global. KvUnit::backup_record возвращает Option<future>: None означает отсутствие необязательного метода, а не успешную пустую операцию. kv() аналогично представляет необязательный facet.
- Только JSON Value на границе backend: нет исполняемых toJSON, циклов, undefined. Версии — u64 по PORTING §4. UNIT_NAME_RE — неизменяемый LazyLock<Regex>.
- По PORTING §7 StorageError — пустая структура с NAME и new(code, message), возвращающим именно hoh_error::HarnessError, без локального типа ошибки/обёртки. Сохраняются исходные порядок аргументов конструктора, имя StorageError, все коды и сообщения. Cause добавляется через HarnessError::with_cause; распознавание выполняется HarnessError::of и is_named(StorageError::NAME). Зависимость hoh-error добавлена по исправленному Dependencies bullet brief и прямому разрешению ведущего; прежний блокер закрыт.
- Два дополнительных registry-теста проверяют точные диагностические строки, insertion order после удаления/добавления, source cause и все четыре пути отказа hub как общий HarnessError внутри BoxError, без слоя обёртки.
- Два самостоятельных теста contract.rs исполняют доступные без backend части helper: исходный descriptor (имена, версия, таблицы, hasGlobal, отсутствие optional полей и порядок JSON), а также его error checks для version-mismatch/closed через BackendFuture, BoxError и SharedError с сохранением имени, кода, текста, cause и идентичности. Сценарии реального хранения не подменяются тестовым backend.
- До реализации проверены импорты разрешённых packages/src: storage-json, storage-sqlite, storage-domain используют все runtime экспорты и типы, кроме формально открытого StorageForms (его расширяет storage-domain). Неиспользуемых экспортов нет.

## Не перенесено

- Из registry.spec.ts не пропущено ничего: 5 исходных случаев.
- contract.ts не является spec и не содержит собственного backend. Публичный run_kv_backend_contract и отдельные функции предназначены для импорта backend tests через #[path]. Из 18 развёрнутых случаев 13 перенесены целиком; 4 commits-the-current-stamp проверяют мутацию/штамп/сохранность других данных, но не предварительный throwing-toJSON; 1 restores-the-open-snapshot-after-serialization-failure не переносится, потому что исполняемый toJSON не представим в serde_json::Value. Это явный учёт отсутствующих assertions, а не ослабление ради прохождения. Этот учёт не изменён заменой типа ошибки.
- TypeScript declaration merging как механизм заменён открытой картой по PORTING §4/8. Других исключений нет.

## Не проверено

- Сценарии общего KV backend contract, которым нужны open/load/write/reopen/close над средой, ожидают backend crates следующих этапов. Здесь компилируются все перенесённые функции и исполняются fixture/error checks helper; сохранность данных, version stamps, batch atomicity и восстановление после перезапуска не объявляются проверенными.
- Платформенных веток нет. Контракты durability, versioning и batch atomicity реализуются backend, а не hub.

## Вопросы ведущему

- Блокирующих недостающих API зависимостей нет; вопрос о hoh-error закрыт решением ведущего.
- Для точного воспроизведения serialization failure KV contract остаётся вопрос о Rust-входе до сериализации: Value уже исключает исполняемый toJSON и этот класс ошибок. Нужна договорённость, если эти assertions должны проверяться в будущих backend crates.

## Проверки

Все Cargo-команды: CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w4.

- cargo test -p hoh-storage: registry.rs — test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out. contract.rs — test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out. Всего 9 passed; 0 failed; lib/doc-tests 0/0.
- cargo fmt -p hoh-storage -- --check: exit 0.
- cargo clippy -p hoh-storage --all-targets -- -D warnings: exit 0. Первая попытка попала в временную ошибку параллельного редактирования hoh-kernel (fiber.rs, отсутствующая node_disposed); после ожидания повторная проверка чистая. Чужой crate не менялся.
