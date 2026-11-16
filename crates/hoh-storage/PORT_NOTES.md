# hoh-storage ← packages/storage/storage @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | Сервис storage, плагин, формы, lifecycle key |
| src/backend.ts | src/backend.rs | Все интерфейсы и контрактные комментарии |
| src/error.ts | src/error.rs | Коды, имя, сообщение, cause; см. вопрос о HarnessError |
| src/registry.ts | src/registry.rs | Полностью, порядок вставки и сравнение экземпляров |
| tests/registry.spec.ts | tests/registry.rs | 5/5 исходных случаев и 1 дополнительный |
| tests/contract.ts | tests/contract.rs | Общий вызываемый набор для backend crates; см. ограничения ниже |
| locale/*.json | locale/*.json | Все 8 файлов скопированы без изменений |
| README.md, package.json | — | Прочитаны; внешнего кода/отдельной LICENSE нет |

## Отличия от образца

- StorageService — значение сервиса, Storage — тип ключа ServiceKey, StoragePlugin/apply — один плагин с именем storage, без config/inject. Это разделение класса Service по docs/kernel.md. Прямое свойство domain стало методом.
- StorageForms — открытая карта String → Arc<dyn Any + Send + Sync>, без перечня чужих форм. form/domain возвращают тот же Arc; владелец формы делает downcast. Map заменён IndexMap; удаление shift_remove сохраняет порядок.
- Возвращаемые register/mount функции остаются повторно вызываемыми Fn() с тем же сравнением идентичности Arc, включая повторную регистрацию того же экземпляра. В исходнике эти методы не вызывают ctx.effect: регистрация не снимается автоматически и не закрывает backend. Плагин-владелец может передать функцию в Undo::sync; сам сервис предоставляется kernel effect.
- BackendFuture/StorageResult — публичные псевдонимы для object-safe async API. KvLayout — закрытое объединение layout. KvSnapshot именует исходный inline result. JSON сохраняет порядок полей и различает отсутствие layout/compatibleVersions и null global. KvUnit::backup_record возвращает Option<future>: None означает отсутствие необязательного метода, а не успешную пустую операцию. kv() аналогично представляет необязательный facet.
- Только JSON Value на границе backend: нет исполняемых toJSON, циклов, undefined. Версии — u64 по PORTING §4. UNIT_NAME_RE — неизменяемый LazyLock<Regex>.
- Исходный StorageError наследует обычный Error, но PORTING §7 требует общий HarnessError для любых coded errors. Пока сохранён локальный StorageError с thiserror, поскольку зависимость hoh-error отсутствует в разрешённом manifest. Это незавершённое требование, не согласованное отклонение (см. вопросы).
- Дополнительный тест проверяет точные диагностические строки, insertion order после удаления/добавления и source cause.
- До реализации проверены импорты разрешённых packages/src: storage-json, storage-sqlite, storage-domain используют все runtime экспорты и типы, кроме формально открытого StorageForms (его расширяет storage-domain). Неиспользуемых экспортов нет.

## Не перенесено

- Из registry.spec.ts не пропущено ничего: 5 исходных случаев.
- contract.ts не является spec и не содержит собственного backend. Helper компилируется, но его сценарии здесь не исполняются: реальных backend в hub нет. Публичный run_kv_backend_contract и отдельные функции предназначены для импорта backend tests через #[path]. Из 18 развёрнутых случаев 13 перенесены целиком; 4 commits-the-current-stamp проверяют мутацию/штамп/сохранность других данных, но не предварительный throwing-toJSON; 1 restores-the-open-snapshot-after-serialization-failure не переносится, потому что исполняемый toJSON не представим в serde_json::Value. Это явный учёт отсутствующих assertions, а не ослабление ради прохождения.
- TypeScript declaration merging как механизм заменён открытой картой по PORTING §4/8. Других исключений нет.

## Не проверено

- Общий KV backend contract ожидает backend crates следующих этапов, здесь только компиляция helper.
- Платформенных веток нет. Контракты durability, versioning и batch atomicity реализуются backend, а не hub.

## Вопросы ведущему

1. Разрешить path-зависимость hoh-error для hoh-storage (или реэкспорт HarnessError в hoh-kernel) и заменить локальный StorageError конструктором общего HarnessError по PORTING §7. В текущем manifest зависимости нет; изменять чужие crates запрещено. Разрешение запрошено, ответа пока нет.
2. Для точного воспроизведения serialization failure KV contract нужна договорённость о Rust-входе до сериализации; Value уже исключает этот класс ошибок.

## Проверки

Все Cargo-команды: CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w4.

- cargo test -p hoh-storage: test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out. lib, contract helper и doc-tests: 0/0.
- cargo clippy -p hoh-storage --all-targets -- -D warnings: exit 0.
- cargo fmt -p hoh-storage -- --check: exit 0.
