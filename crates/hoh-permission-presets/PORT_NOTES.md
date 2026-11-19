# hoh-permission-presets ← packages/interaction/permission-presets @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs`, `src/shim.rs` | Таблица presets, Auto, fold/projection, catalog, pinning сессий и write path |
| `src/types.ts` | `src/types.rs` | `PresetOption`, `PermissionCatalog`, `PermissionSelection`, событие catalog-changed |
| `src/client.ts` | `src/client.rs` | Чистый re-export типов client namespace |
| `tests/initialization.spec.ts` | `tests/initialization.rs` | 2 / 0 исходных случая |
| `tests/permission-presets.spec.ts` | `tests/permission_presets.rs` | 31 / 0 исходных случай |
| `tests/projection.spec.ts` | `tests/projection.rs` | 11 / 0 исходных случаев |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `README.md`, `README.zh.md`, `package.json` | эти заметки, `Cargo.toml` | Прочитаны; контракты и exports учтены |

Собственного `LICENSE` в пакете нет. Все 44 объявления `it`/`it.each`
представлены 44 Rust-тестами; `it.each(['fresh', 'auto'])` сохраняет оба
варианта в одном тесте по правилу PORTING §10. 37 сценариев проверяют
настоящие session/projection/kernel-границы; оставшиеся 7 представлены через
fixed interfaces неготовых dependencies, как описано ниже.

## Отличия от образца

- `PermissionPresets` — kernel service key с исходным именем `permissionPresets`;
  `PermissionPresetDeps` — Rust-имя исходного `static inject`. `CatalogChanged` — тип
  declaration-merged события `permission-presets/catalog-changed`.
- Методы, создающие эффект от имени вызывающего, получают `&Context`:
  `register_auto(&Context, ...) -> Disposer`. Это прямое соответствие kernel.md §7.
- Typert `Remote('catalog')` не переносится по PORTING §8; `catalog()` остаётся обычным
  публичным методом сервиса.
- Ручной zod schema состояния заменён строгим serde-разбором с обязательными
  `preset`, `sandbox`, `approval`, `seeded` и запретом лишних ключей.
- `Contained` и `Callbacks::invoke` заменяют ручной JS-dispatch и сохраняют изоляцию
  sync/async failures и текст warning.
- Публичных named imports из этого пакета в переносимых production `src/`
  (исключая client/api/host/experimental/apps) нет; потребители используют сервис
  через declaration merging. Все exports образца тем не менее сохранены.

## Не перенесено

- Typert host/remote entry points — PORTING §8.
- Регистрация `/permission` в process command registry ждёт `hoh-commands`, которого
  ещё нет. Точный handler/result лежит в приватном `SHIM(hoh-commands)` и покрыт
  четырьмя исходными command-сценариями; после приёмки зависимости нужна
  только привязка registry и `ApprovalService::set_policy`.
- Optional `settings.configure({ auto: false }, fiber)` ждёт рабочего API `hoh-settings`:
  сейчас его `src/lib.rs` пуст и сам крейт остановлен на недостающем API loader.

## Не проверено

Платформенных ветвей, сети и файлового I/O в пакете нет. Полное окно и будущая
командная и settings-интеграции не запускались: зависимые крейты ещё не готовы.
Два initialization-сценария проверяют pending admission и отсутствие записей на
приватной fixed interface, но не повторяют транзакцию factory из `hoh-agent-loop`.
Четыре command-сценария и settings-visibility проверяют shim/schema без ещё
отсутствующих command/settings registry API. Это не выдаётся за полную integration
coverage; после приёмки dependencies требуется замена shim и повтор этих границ.

## Вопросы ведущему

- `SHIM(hoh-sandbox-policy)` в `src/shim.rs`: `set_sandbox_mode` точно добавляет
  `sandbox/mode` с `{ mode }`. Заменить импортом из `hoh-sandbox-policy`, когда следующий
  пакет будет принят.
- `SHIM(hoh-commands)` в `src/shim.rs` и private handler в `src/lib.rs`: заменить
  на настоящий `CommandDefinition` с id `@deepseek-ai/dsh-permission-presets`, когда `hoh-commands`
  появится; тексты и write path уже зафиксированы тестами.
- Для optional settings child нужен публичный API `configure({ auto: false }, fiber)`
  или его Rust-эквивалент. Чужой `hoh-settings` не изменялся.
