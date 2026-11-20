# hoh-office-kit ← packages/document/office-kit @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/errors.ts` | `src/errors.rs` | `OfficeKitError`, 11 wire-кодов и исходная причина |
| `src/identity.ts` | `src/identity.rs` | три отдельных branded string identity |
| `src/types.ts` | `src/types.rs` | запросы, расширения, форматы и результаты; deferred bounded reader |
| `src/queue.ts` | `src/queue.rs` | общий admission queue, source/digest sharing, LRU и cancellation |
| `src/output.ts` | `src/output.rs` | `NOFOLLOW`, opened-file metadata, byte bounds, PDF/PNG signatures |
| `src/outputs.ts` | `src/outputs.rs` | private owner store, retention, eviction, release и safe startup sweep |
| `src/locate.ts` | `src/locate.rs` | installation anchor и первая `.asar` → `.asar.unpacked`; см. отличие ниже |
| `src/index.ts` | `src/lib.rs`, `src/converter.rs` | service/plugin, config, slots, scratch files, четыре операции и converter seam |
| `tests/*.spec.ts` | `tests/*.rs` | 48 Rust runtime tests; покрытие и исключения ниже |
| `locale/*.json` | `locale/*.json` | скопированы побайтно |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json` | эти заметки и manifest | прочитаны; package/build metadata не копируется |

Новых внешних зависимостей нет. Использованы только workspace crates и уже закреплённые
`getrandom`, `indexmap`, `rustix`, `schemars`, `serde`, `serde_json`, `sha2`, `tempfile`,
`thiserror`, `tokio`.

## Отличия от образца

- Независимого `@deepseek-ai/libreoffice-kit@0.2.3` и его исходника в образце нет. По решению
  `docs/porting/DEPS.md` §5.5(11) граница перенесена как публичные `Converter` и
  `ConverterFactory`: `UnavailableConverterFactory` сохраняет lazy registration и исходный
  install hint, `ExternalSofficeFactory` запускает явно переданный `soffice`. Конкретный
  `OfficeKitService` можно собрать с тестовым/прикладным factory; kernel plugin по умолчанию
  предоставляет fail-loud unavailable backend и не ищет/не устанавливает программу.
- Внешний `soffice` умеет PDF/format/recalculate через CLI, но не предоставляет API исходного
  Kit для PNG selection, missing fonts, page count, archive/font byte limits и native/WASM
  package selection. Его `render_images` возвращает `unavailable`; это утверждённый fixed
  interface, не подмена поведения основного сервиса. Полный converter легко подключается без
  изменения admission/output API.
- `OfficeKit` — kernel service key `"officeKit"`, значение — object-safe `OfficeKitApi`;
  реализация называется `OfficeKitService`, plugin — `OfficeKitPlugin`. Методы, создающие
  эффекты, получают ownership от plugin; Session disposal вызывает `release`.
- Пути boundary представлены `PathBuf`; wire-строки форматов и error codes сохранены. Внутренние
  ключи операции сериализуются JS-compatible writer, cache key остаётся
  `<generation>:<sha256>`.
- `createEnginePackageResolver` не воспроизводит Node `createRequire` и поиск по всем
  `node_modules`: совместимый helper ищет private engine под выбранным Kit anchor и сохраняет
  ASAR/WASM mapping. Runtime Rust использует `ConverterFactory`, а не npm resolver.
- Дополнительные публичные Rust-типы (`OfficeKitApi`, `OfficeKitService`, converter request/result,
  `SourceReader`, `QueueResult`, `ConfigError`) выражают классы, callback interfaces и исключения
  TypeScript без параллельного wire-протокола.

## Не перенесено

- Typert decorators/generated `remote`/`typert` entry points исключены PORTING §8. Обычные
  service methods `convert`, `render_images`, `convert_to`, `recalculate`, `generation` и
  `output_directory` сохранены. `render(workspaceFileScope, path, ...)` ждёт владельца Rust API
  `workspaceFiles`: соответствующего crate/API в workspace нет; копия authorization logic сюда
  не вносилась. Поэтому 18 declarations / 32 expanded cases `remote.spec.ts` представлены только
  object-safety/wire-code тестом и не объявляются перенесёнными.
- Node-only dynamic `import()`, `createRequire`, `MODULE_NOT_FOUND` identity, invalid npm manifest
  и Vitest module/fs monkey-patching исключены по PORTING §8. Из `anchors`, `engine-package`,
  `engine-resolver`, `kit-lookup` сохранены anchor isolation, ASAR/WASM mapping, missing/incomplete
  package и lookup union; 6 Node-error/prototype cases не перенесены.
- Инъекционные тесты, которым Vitest подменяет уже открытый `FileHandle` или `rm/writeFile/lstat`
  внутри `node:fs/promises`, не имеют production hook в Rust. Реальные regular/link/limit/signature,
  cleanup, retention, eviction, release, sweep и concurrency boundaries проверены; 10 mock-only
  failure/race cases не перенесены.

Итого: 48 Rust tests покрывают 57 из 104 исходных `it`/`it.each` declarations (развёрнутые
таблицы выполняются внутри тестов). 47 declarations не перенесены: 18 Remote, 6 Node module,
10 monkey-patch I/O и 13 вариантов queue/provider. Утверждение «13 дублирующих» неверно: после
круга исправлений добавлены реальные unix-проверки (chmod/umask, неудаляемый каталог, зависший
`soffice`), eviction фонового job, `max_cached_entries` и stress-тест digest-гонки. Не перенесены
по-прежнему: приоритеты/понижение, LRU алиасов, повторное чтение после отмены, `max_source_entries`,
а также provider-слоты и dispose с ошибками.

## Не проверено

- Реальный LibreOffice/`soffice` не запускался и в окружении не искался. Windows/macOS branches,
  Windows junction/ACL и ASAR packaging не исполнялись.
- Ошибки удаления, которые ОС возвращает только при EBUSY/permission failure, покрыты сохранением
  state в коде, но без платформенной fault injection. Engine RSS и внешний CLI timeout принадлежат
  concrete converter.

## Вопросы ведущему

1. Нужен принятый Rust API владельца `workspaceFiles` для обычного (не Typert) метода `render`:
   authorized probe/stat/read и точные RemoteError mapping сейчас невозможно связать без правки
   или копирования чужой границы.
2. Для production image rendering нужен concrete `Converter`, эквивалентный API независимого
   Kit; внешний `soffice` CLI не возвращает его selection metadata.

## Проверки

- Регрессия retention после продвижения виртуального времени ожидает завершения асинхронного
  `remove_dir_all` ограниченным числом scheduler yields, как исходный Vitest `waitFor`; прежняя единственная
  уступка планировщику могла проверить файл до завершения удаления при параллельном прогоне.

Все проверки выполнены с `CARGO_BUILD_JOBS=2`, offline и выделенным target:

- `timeout 900s cargo test --offline -p hoh-office-kit`: 48 passed, 0 failed;
- `timeout 900s cargo clippy --offline -p hoh-office-kit --all-targets --no-deps -- -D warnings`:
  без предупреждений;
- `timeout 120s cargo fmt -p hoh-office-kit -- --check`: чисто.
