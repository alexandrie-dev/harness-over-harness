# hoh-package-manifest ← packages/util/package-manifest @ 39163dd3

## Файлы
| образец | перенос | состояние |
| --- | --- | --- |
| src/index.ts | src/lib.rs | все реэкспорты |
| src/types.ts | src/types.rs | serde-типы, точные JSON-имена |
| src/client-module.ts | src/client_module.rs | типы функций и интерфейсы |
| tests/ отсутствует | tests/json_shapes.rs | 8 собственных тестов чтения/записи JSON, включая 3 проверки порядка по тексту |

## Отличия от образца
Имена полей Rust — snake_case, JSON сохраняет исходные имена. Union patch получает
имя BundlePatch; литерал 1 — ManifestVersion. Это вспомогательные публичные типы.
IndexMap сохраняет порядок записей. Не вводятся проверки SemVer, путей и значения
по умолчанию listedByDefault: ими, как и в образце, владеет читатель.
LocalizedText::Translations хранит все локали, включая en, в одной IndexMap в
порядке входа. Метод LocalizedText::en читает английскую запись, возвращая None
при её отсутствии или для варианта Text. Карта без en допустима: требование en
есть в TypeScript-типе, но проверки во время выполнения у исходного пакета нет;
проверку и fallback определяют text-search и другие читатели.
DshEnginesManifest — прозрачная обёртка над одной IndexMap со всеми engines;
методы dsh(), node(), npm() читают именованные требования без перестановки ключей.
Эти методы — вспомогательный публичный API Rust вместо отдельных полей структуры.

Остальные типизированные структуры сериализуются в порядке объявления полей
Rust, как JSON.stringify объекта, построенного последовательным присваиванием
полей. Они не сохраняют исходный порядок произвольного прочитанного документа.
Три регрессионных теста сначала дали «5 passed; 3 failed» на прежнем коде:
локали {zh,en}, engines {node,pnpm,dsh,npm}, полный манифест с порядком
{dsh,engines,version,name}. Последний проверяет точный текст round trip через
serde_json::Map и отдельно текст сериализации структуры в порядке её полей,
включая сохранение порядка внутри engines. Сравнение Value по равенству объектов
не обнаруживает перестановку ключей, поэтому здесь сравниваются строки JSON.

Для переноса app-boot: ProfileManifest в packages/boot/app-boot/src/profile.ts:45 —
это Partial<DshPackageManifest>; локальному профилю name и version не нужны.
Здесь DshPackageManifest сохраняет обязательные name/version исходного полного
типа; не следует применять его напрямую как тип локального профиля.
readProfileManifest, withBundles и writeProfileManifest (profile.ts:548–617)
читают package.json, заменяют dsh.profile.bundles и пишут весь документ обратно,
сохраняя неизвестные поля и порядок всех ключей. Для такой операции нужен сам
JSON-объект, serde_json::Map с preserve_order, включённым в workspace, включая
вложенные объекты. Эти структуры предназначены для построения нового манифеста
и чтения уже проверенной части: они отбрасывают неизвестные поля, пишут известные
поля в порядке объявления, стирают явный null в Option-полях и отклоняют значения
неожиданного типа. У исходных читателей свои диагностические тексты, например
bundlePatchFiles (profile.ts:54–67): «dsh.bundle.patch must be a file path or a list
of file paths». Прямое serde-чтение структуры не заменяет эту проверку.

Функции client-module представлены Arc-замыканиями и трейтами; их невозможно
сериализовать в JSON, поэтому serde применяется только к данным types.ts.
StylesheetText — вспомогательный псевдоним типа замыкания. unknown представлен Value;
вызовы require/async не исполняют JavaScript, а описывают контракт для потребителя.

Экспорты client-module — DshClientModuleManifest, ClientBundleRequire,
ClientModuleFactory, ClientModuleMetaFactory, ClientModuleMeta,
ClientModuleStylesheetDescriptor и ClientModuleStyleContext — используются только
непереносимыми packages/client/* (modules, web и ui-renderer); они всё равно
представлены в crate. DshClientManifest напрямую импортирует client/modules и
входит в DshManifest.client. DshManifest, DshEnginesManifest и DshProfileManifest
также описывают вложенные части DshPackageManifest, который читает app-boot.
DshBundleManifest используют app-boot и official-bundles; LocalizedText — llm,
web, plugin-market, text-search и plugin-market-index; PluginLocalizedMeta —
app-boot, plugin-manager и official-bundles. Названные типы используются также
некоторыми исключёнными client/host/experimental-пакетами.

## Не перенесено
Нет. Исходных тестов нет (0 перенесено, 0 пропущено); locale и отдельной лицензии нет.

## Не проверено
Исполнение браузерных JavaScript-фабрик: пакет содержит только объявления.

## Вопросы ведущему
Нет. Зависимостей от других HoH crates нет.
