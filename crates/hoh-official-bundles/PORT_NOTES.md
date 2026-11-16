# hoh-official-bundles ← packages/util/official-bundles @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | все реэкспорты и OfficialBundleCatalogEntry |
| src/bundles.ts | src/bundles.rs | ProfileTemplate и все шесть списков/карт |
| src/plugin-manager.ts | src/plugin_manager.rs | все пять списков/множеств |
| src/catalog.generated.ts | src/catalog_generated.rs | обе записи, все локали и полные SVG data URL |
| tests/official-bundles.spec.ts | tests/official_bundles.rs | оба исходных случая; ещё два сравнения полных данных и порядка |
| литералы src/bundles.ts и src/plugin-manager.ts | tests/fixtures/bundle_lists.json | снимок списков, ключей объектов и порядка итерации Set |
| литерал src/catalog.generated.ts | tests/fixtures/catalog.json | снимок каталога с точным текстом и порядком всех локалей |
| README.md | — | прочитан полностью; контракт учтён |
| package.json | Cargo.toml и публичные модули | root и исходные модули представлены |

В исходном пакете нет отдельного LICENSE и каталога locale/. Метаданные восьми языков находятся в самом каталоге. Атрибуций заимствованного стороннего кода нет.

## Отличия от образца

- Это библиотека данных, не плагин: в образце нет name/inject/Config/apply, сервисов, событий и собственных ошибок. Правила обновлённого PORTING §7 не требуют создавать здесь тип ошибки. Ресурсов, фоновых задач, файлового ввода-вывода и обращений к сети нет.
- Массивы строк представлены неизменяемыми срезами `&[&str]`; объекты PROFILE_TEMPLATES и INSTALLATION_OWNED_PROFILE_TUPLES — IndexMap, множества RETIRED_BUNDLES, PROTECTED_PLUGINS, HIDDEN_BUNDLES и EXPERIMENTAL_BUNDLES — IndexSet. Порядок объявления/вставки сохранён, сортировки нет. Все исходные имена пакетов сохранены, включая записи client/host/experimental: здесь это пользовательские данные, а не зависимости от исключённых пакетов.
- Неизменяемые карты, множества и каталог публикуются через std::sync::LazyLock: строки и коллекции выделяются один раз из литералов. Доступ даёт только общую ссылку к фиксированным данным поставки; изменяемого состояния плагина или динамического реестра нет. Операции get/contains/iter идут по IndexMap/IndexSet; обычные списки используют contains/iter среза. Никакой установки или обнаружения модулей при инициализации не происходит.
- ProfileTemplate.bundles — Vec<String>; package_name/platforms — String/Option<Vec<String>> с исходными JSON-именами и отсутствием None-поля. Тип platforms повторяет DshBundleManifest.platforms. Дополнительный публичный OfficialBundleCatalogMeta выражает исходный `Required<Pick<PluginLocalizedMeta, 'title' | 'description' | 'icon'>>`: все три поля обязательны, поля error нет. title и description используют существующий hoh_package_manifest::LocalizedText, без копирования его типа.
- LocalizedText::Translations хранит все восемь локалей в одной IndexMap в исходном порядке `de, en, es, fr, ja, pt, ru, zh`. Английская запись не выделяется в отдельное поле. JSON-ключи записей идут packageName/meta, внутри meta — title/description/icon; platforms отсутствует в обеих исходных записях, version не добавляется. Unicode escape-последовательности TypeScript записаны в Rust как соответствующие UTF-8 символы: значения строк, включая апостроф U+2019, совпадают. SVG base64 сохранён целиком без перекодирования.
- Публичные модули повторяют исходные файлы: bundles, plugin_manager, catalog_generated; все исходные root-экспорты сохранены. Имена констант сохранены, имена Rust-полей — snake_case, JSON — исходный camelCase. serde реализует представление данных; новых функций записи JSON или загрузки каталога нет.
- Перед переносом проверены импорты разрешённых packages/*/src: app-boot/profile использует PROFILE_TEMPLATES, INSTALLATION_OWNED_PROFILE_TUPLES и RETIRED_BUNDLES; plugin-manager/operations — PROFILE_TEMPLATES и DEFAULT_PROFILE_BUNDLES; plugin-manager/index — OPTIONAL_BUNDLES, OFFICIAL_ON_DEMAND_CATALOG и PROTECTED_PLUGINS. Прямых потребителей ON_DEMAND_BUNDLES, ProfileTemplate и OfficialBundleCatalogEntry вне владельца среди переносимых пакетов не найдено. EXPERIMENTAL_BUNDLES, EXTENSION_BUNDLES, HIDDEN_BUNDLES и MORE_BUNDLES используют исключённые client/ui-plugin-manager; все они всё равно перенесены. Скрипты и приложения также читают исходные данные, но в эту инвентаризацию переносимых пакетов не входят.
- Исходные два теста сохранены с узнаваемыми snake_case-именами. Проверка `/\S/` использует общий hoh_util_values::js_trim по PORTING §4: Rust trim и собственный набор пробелов не вводятся. Два дополнительных теста сравнивают строки сериализации, а не только равенство Value: перестановка ключей/локалей или элементов обнаруживается. Фикстуры извлечены из литералов read-only образца как данные, без исполнения TypeScript, Node или генератора npm; множества в фикстуре представлены массивами их элементов по порядку вставки, не результатом JSON.stringify(Set).

## Не перенесено

Нет исключённых исходных файлов, экспортов или runtime-тестов: 2 из 2 исходных случаев перенесены, 0 пропущено. Каталог locale/ отсутствует. Генератор npm каталога не входит в src/ данного пакета и не переносится по PORTING §8; потребляется уже зафиксированный каталог образца, как и в исходной сборке.

## Не проверено

Платформенных веток нет. Установка пакетов, registry, инициализация профилей и применение списков принадлежат потребителям и здесь не выполняются. Порядок и содержимое всех объявленных списков, полей каталога, локалей, текстов и иконок проверены полностью. Значения платформ не объявлены ни у одной из двух записей.

## Вопросы ведущему

Нет. Обновлённые LocalizedText и остальные API hoh-package-manifest достаточны; недостающих зависимостей нет. Незавершённой работы нет.

## Проверки

Все команды используют `CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w3`.

- `cargo test -p hoh-official-bundles`: `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (2 исходных + 2 дополнительных; unit/doc-tests 0/0).
- `cargo build -p hoh-official-bundles`: exit 0.
- `cargo clippy -p hoh-official-bundles --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-official-bundles -- --check`: exit 0.
