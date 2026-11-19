# hoh-plugin-package-inventory-deepseek ← packages/llm/plugin-package-inventory-deepseek @ 39163dd3

Исходник прочитан из read-only дерева
`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/llm/plugin-package-inventory-deepseek`.
Ничего в TypeScript-дереве не запускалось.

## Файлы

| образец | Rust | состояние |
|---|---|---|
| `src/types.ts` | `src/lib.rs`: `DeepSeekPluginPackageIdentity`, `DeepSeekPluginPackageInventoryExtension` | wire-форма и optional version сохранены |
| `src/index.ts`: config/apply | `Config`, `PluginPackageInventoryDeepSeek`, `Deps` | default-on, регистрация `dsh_plugin_packages`, effect-owned withdrawal |
| manifest resolution | `PackageIdentityResolver` | bare/file/absolute/URL/Cordis, ближайший manifest, immutable successful cache |
| per-request collection | `Inventory::collect` | active Loader rows, live agent/preset inspection, dedup, сортировка, warning once |
| `tests/inventory.spec.ts` | `tests/inventory.rs`, `src/tests.rs` | plugin/Loader wiring и таблицы resolver/metadata |
| `tests/private-package-composition.spec.ts` | `tests/private_package_composition.rs` | file URL, private true/false, name-only рядом с versioned |
| `locale/*.json` | `locale/*.json` | восемь файлов скопированы побайтно |

Новых внешних зависимостей нет: используются только workspace crates и уже закреплённые
`serde`, `serde_json`, `schemars`, `thiserror`, `url`, а в тестах `tempfile` и `tokio`.

## Отличия от образца

- Cordis context proxy заменён явными `Service<...>` из `Deps`; plugin называется
  `PluginPackageInventoryDeepSeek`, константа `NAME` сохраняет исходное имя.
- Declaration merging заменён экспортируемыми Rust wire-структурами. `version: 1` представлен `u8`.
- В образце внутренние `PackageIdentityResolver`, `ActiveEntry` и helpers непубличны. В Rust они также
  непубличны; дополнительные публичные `PluginPackages*` и `InventoryRuntime*` — временные fixed-interface
  shims, перечисленные ниже, потому что владельцы нужных source facts ещё не публикуют их.
- Node `createRequire(...).resolve.paths` заменён локальным подъёмом по каталогам `node_modules`.
  Он сохраняет ближайший package lookup и directory/file anchor, но не добавляет глобальные Node paths
  (`NODE_PATH`, home libraries, prefix), а также не повторяет special-case, пропускающий промежуточный
  `node_modules/node_modules`. Эти пути не публикуются Loader facts и не используются штатными bases.
  Четвёртый anchor `import.meta.url` представлен file URL `src/lib.rs` этого crate.
- Non-file anchor без `PluginPackages` в Node заставляет `createRequire` бросить и прекращает resolution
  этого entry; Rust пропускает такой anchor и пробует следующий file/absolute anchor. Штатные Loader,
  host и module anchors являются file URL. Относительный `tree_base` отклоняется как `InvalidBase`.
- Manifest читается через lossy UTF-8, как Node `readFileSync(..., 'utf8')`. `serde_json::Value` не может
  представить принятую `JSON.parse` строку с lone UTF-16 surrogate и сохраняет recursion limit 128;
  такие manifest считаются unreadable. Снятие лимита создало бы риск stack overflow при drop глубокой
  `Value`, а lossless lone-surrogate потребовал бы отдельного UTF-16 JSON/wire-типа вне текущего API.
- Как и синхронный TS resolver, Rust resolver использует blocking metadata/read вызовы. В Rust они идут
  на runtime thread; перенос в `spawn_blocking` потребовал бы отдельного lifecycle-owned blocking actor.
  Текущие manifests малы и успешные результаты кэшируются, поэтому это оставлено явной границей.
- Формат `%o` Cordis logger не имеет публичного аналога; warning сохраняет prefix, module name и цепочку
  ошибки через Rust `Display`, но platform-specific хвост parse/I/O error отличается.

## Совпадающее поведение

- `enabled` по умолчанию true; false ничего не регистрирует.
- Обычный Schemastery object сохраняет неизвестные config keys; Rust `Config` также их принимает.
- В inventory попадают только негрупповые, enabled и `FiberState::Active` строки Loader.
- Relative/absolute/file URL ищут ближайший `package.json`; `cordis:` и не-file URL не дают identity.
- Bare package выделяет обычное или scoped имя без subpath и ищется вверх по `node_modules` от каждого anchor.
- Снимок optional package service авторитетен: его miss не обходит профиль нативным поиском.
- Пустой/нестроковый name исключает пакет. Version включается только для непустой строки; исходная строка
  не trim-ится. Blank-проверка использует `hoh_util_values::js_trim`, включая BOM.
- Невалидные UTF-8 bytes декодируются с U+FFFD, как Node UTF-8 decoder.
- `JSON.parse(null)`-случай сохранён отдельно: JSON `null` вызывает unreadable error, как обращение
  `manifest.name` в JavaScript. Invalid JSON и null не кэшируются; успешные identity и отсутствие identity
  кэшируются до конца жизни plugin.
- Ошибка одного entry не отклоняет запрос и логируется не более одного раза на module name.
- Dedup различает name-only и строковую version `"undefined"`. Сортировка сравнивает UTF-16 code units,
  а не locale/UTF-8, затем version; name-only идёт раньше versioned.
- Для найденного live agent добавляется inventory standing preset; неизвестный session и agent без mounted
  preset получают только host inventory.

### Fixed-interface shims и границы чужих API

- `PluginPackagesKey`/`PluginPackages` — `// SHIM(hoh-loader)` для отсутствующего в Rust публичного
  `Context['pluginPackages']`. Snapshot берётся один раз при apply, как в исходнике. Без сервиса действует
  нативный `node_modules` walk. Нужен публичный package lookup владельца app-boot/Loader, после чего этот shim
  следует заменить адаптером или типом владельца.
- `InventoryRuntimeKey`/`InventoryRuntime` — `// SHIM(hoh-loader)`: текущий публичный `EntryInfo` не раскрывает
  declaring tree base URL. Если bridge опубликован, resolver получает точный host base и per-entry base.
  Без bridge host base берётся из `ProfileContext.dir/cordis.yml`, а direct embedding — из crate-local
  аналога `import.meta.url`; обычные Loader entries используют host base.
- `hoh-agent-preset-registry::AgentPresetModule` уже содержит `base_url/use_host_base`, но его владелец сейчас
  возвращает `base_url=None,use_host_base=true`, поскольку тот же Loader API отсутствует. Этот крейт корректно
  потребляет поля; nested preset base станет точным без его изменения после исправления владельца.

Следствие двух последних пунктов: конфликтующие bare-версии в nested Loader include и nested standing-preset
можно разрешить точно через `InventoryRuntime`/готовые composition facts, но стандартная связка текущих чужих
крейтов этих facts пока не публикует. Чужие крейты в этой работе не менялись. Это единственный известный
production wiring gap; чистый resolver и обе anchor-семантики покрыты тестами.

## Не перенесено

- TypeScript-only declaration-merging check не нужен: extension registry хранит открытую JSON map по
  PORTING §4/§8.
- Dynamic replacement Loader import implementation из source-теста не переносится по PORTING §8; те же
  absolute/file/http/Cordis ветви проверяются через Rust Loader registry и resolver.
- Других source runtime-файлов нет; локали перенесены полностью.

## Не проверено

- Текущие чужие crates не публикуют declaring base вложенного Loader include и nested standing-preset.
  Ordinary nested Loader покрыт end-to-end через `InventoryRuntime`; resolver покрыт двумя точными preset
  bases и `use_host_base`, но production end-to-end именно nested standing-preset нельзя проверить до
  появления owner API. Root standing-preset проверен с live agent, реальным mount и сервисом,
  опубликованным после активации inventory.
- Windows absolute paths не запускались на Windows. `Path::is_absolute` следует host platform; file URL
  покрыт на Linux.
- Lone-surrogate JSON names и manifests глубже 128 уровней намеренно остаются unreadable по описанной
  выше границе `String`/`serde_json`; invalid UTF-8 replacement и обычные parse errors покрыты тестами.
- Глобальные Node module paths и anchor внутри уже названного `node_modules` не проверялись: штатный Loader
  передаёт file URL активного дерева, а production inventory не должен зависеть от process-global installs.

## Вопросы ведущему

1. Нужен read-only Loader API `EntryInfo.declaring_base_url` (включая nested include) и тот же fact в
   `AgentPresetModule`; после этого удалить `InventoryRuntimeKey` и fallback к host base.
2. Нужен owned app-boot/Loader `PluginPackages` service с `package_of(name, anchor)`; после этого заменить
   `PluginPackagesKey` типом владельца, сохранив authoritative-miss семантику.
3. После появления этих API следует добавить production end-to-end для конфликтующих nested bare versions
   и live mounted preset; сейчас точная resolver-семантика проверена только через fixed-interface вход.

## Проверки

Команды выполняются офлайн с `CARGO_BUILD_JOBS=2`, target
`/mnt/toshiba/alexandrie/hoh/target-x1`; тесты только под `timeout`.
Итоговые числа заполняются отчётом после финального прогона.
