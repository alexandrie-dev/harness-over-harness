# hoh-fs-sandbox ← packages/fs/fs-sandbox @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs, src/shim.rs | SandboxedFileSystem, Config, plugin, пять ограждённых мутаций |
| src/containment.ts | src/containment.rs | лексическое сравнение и fallback по dev/ino |
| tests/containment.spec.ts | tests/containment.rs | все 6 объявлений |
| tests/fs-sandbox.spec.ts | tests/fs_sandbox.rs | все 24 объявления, таблица трёх режимов |
| locale/*.json | locale/*.json | восемь файлов скопированы без изменений |
| README.md, README.zh.md | — | прочитаны; собственного LICENSE нет |

Сохранены все 30 исходных тестовых объявлений, то есть 32 случая после разворачивания `it.each` по трём режимам. Дополнительный регрессионный тест проверяет требование вехи: абсолютный путь, `..` и symlink-выход отказывают с `FS_SANDBOX_DENIED` и не меняют существующие файлы.

## Отличия от образца
- Наследование `LocalFileSystem` заменено композицией: `SandboxedFileSystem` реализует объектобезопасный `FileSystem` и делегирует чтения, метаданные, watch и path helpers без изменений. Пять мутаций получают свежую цель после policy-проверки; move проверяет source и destination до делегирования.
- `sandboxMode` представлен `FileSystem::sandbox_mode() -> Some(default_mode)`. `Config` реэкспортирован из `hoh-fs-local`; plugin `FsSandboxPlugin` сохраняет исходное имя строки `SandboxedFileSystem` и предоставляет сервис `fs`.
- `isPathUnder` стал `is_path_under(path, root, Option<bool>) -> Result<bool, BoxError>`; optional bool сохраняет исходный host default. Linux dev/ino взяты из безопасного MetadataExt. На неподдержанных не-Unix целях identity fallback консервативно не разрешает иной spelling; сам `hoh-fs-local` сейчас Linux-only.
- Standalone-конструктор принимает `Arc<dyn SandboxPolicyApi>` для тестов и прямой композиции. Kernel plugin получает тот же интерфейс через service key.
- В исходном интеграционном тесте lifecycle session grant создаёт `sandbox-policy`; пока зависимость пишется параллельно, тот же observable fence проверен изменением policy SHIM (grant, mode override, revoke). Session-specific таблицу владеет будущий crate.

## Не перенесено
Нет исходных файлов, экспортов или тестовых объявлений, исключённых из переноса.

## Не проверено
Windows alias/case identity и macOS; конкретный локальный backend заявлен Linux-only. Остаточная TOCTOU между свежим resolve и syscall сохранена как в образце.

## Вопросы ведущему
- `src/shim.rs`: SHIM(hoh-sandbox-policy) — `SandboxPolicyApi::resolve(Option<&Session>)` и сервис `sandboxPolicy`. Заменить на принятый API `hoh-sandbox-policy`; одновременно `hoh-tool-fs` должен импортировать тот же service key вместо собственного shim, иначе одинаковое строковое имя не устраняет различие Rust `TypeId`.

## Проверка
Все команды: `CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x5`, cargo-вызовы ограничены внешним `timeout`.
- `cargo check -p hoh-fs-sandbox --tests --offline`: exit 0.
- `cargo test -p hoh-fs-sandbox --offline`: exit 0; 31 passed, 0 failed суммарно (containment 6, fs_sandbox 25; unit/doc 0).
- `cargo clippy -p hoh-fs-sandbox --all-targets --offline -- -D warnings`: exit 0.
- `cargo fmt -p hoh-fs-sandbox -- --check`: exit 0.
- Независимое read-only ревью исходника и текущего crate: material-замечаний после исправлений нет; проверки ревьюер не запускал.
