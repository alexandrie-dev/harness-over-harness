# hoh-fs-sandbox ← packages/fs/fs-sandbox @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | SandboxedFileSystem, Config, plugin, пять ограждённых мутаций |
| src/containment.ts | src/containment.rs | лексическое сравнение и fallback по dev/ino |
| tests/containment.spec.ts | tests/containment.rs | все 6 объявлений |
| tests/fs-sandbox.spec.ts | tests/fs_sandbox.rs | все 24 объявления, таблица трёх режимов; тест сессионных разрешений идёт через настоящие сессии (`register_writable_root`, `set_sandbox_mode`, выгрузка плагина) |
| locale/*.json | locale/*.json | восемь файлов скопированы без изменений |
| README.md, README.zh.md | — | прочитаны; собственного LICENSE нет |

Сохранены все 30 исходных тестовых объявлений, то есть 32 случая после разворачивания `it.each` по трём режимам. Дополнительный регрессионный тест проверяет требование вехи: абсолютный путь, `..` и symlink-выход отказывают с `FS_SANDBOX_DENIED` и не меняют существующие файлы.

## Отличия от образца
- Наследование `LocalFileSystem` заменено композицией: `SandboxedFileSystem` реализует объектобезопасный `FileSystem` и делегирует чтения, метаданные, watch и path helpers без изменений. Пять мутаций получают свежую цель после policy-проверки; move проверяет source и destination до делегирования.
- `sandboxMode` представлен `FileSystem::sandbox_mode() -> Some(default_mode)`. `Config` реэкспортирован из `hoh-fs-local`; plugin `FsSandboxPlugin` сохраняет исходное имя строки `SandboxedFileSystem` и предоставляет сервис `fs`.
- `isPathUnder` стал `is_path_under(path, root, Option<bool>) -> Result<bool, BoxError>`; optional bool сохраняет исходный host default. Linux dev/ino взяты из безопасного MetadataExt. На неподдержанных не-Unix целях identity fallback консервативно не разрешает иной spelling; сам `hoh-fs-local` сейчас Linux-only.
- Standalone-конструктор принимает `SandboxPolicyService` (клон настоящего сервиса `hoh-sandbox-policy`; `resolve(SandboxPolicyRequest)` возвращает `Result`, ошибка политики — ошибка мутации). Kernel plugin получает `Service<SandboxPolicy>` (ключ `sandboxPolicy` крейта `hoh-sandbox-policy`).
- Тесты идут на настоящем `hoh-sandbox-policy` (плагин над `SessionProjectionRegistry`); session-override и корни пишет сам сервис; тест сессионных разрешений регистрирует их через `register_writable_root`/`set_sandbox_mode` и проверяет изоляцию двух сессий и снятие при выгрузке плагина (`fs-sandbox.spec.ts:96-128`).

## Не перенесено
Нет исходных файлов, экспортов или тестовых объявлений, исключённых из переноса.

## Не проверено
Windows alias/case identity и macOS; конкретный локальный backend заявлен Linux-only. Остаточная TOCTOU между свежим resolve и syscall сохранена как в образце.

## Вопросы ведущему
- SHIM(hoh-sandbox-policy) снят: `src/shim.rs` удалён, собственного ключа `sandboxPolicy` в крейте нет (раньше ядро отказывало смонтировать его вместе с `hoh-sandbox-policy`: другой тип значения под одним именем).

## Проверка
Все команды: `CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x5`, cargo-вызовы ограничены внешним `timeout`.
- `cargo check -p hoh-fs-sandbox --tests --offline`: exit 0.
- `cargo test -p hoh-fs-sandbox --offline`: exit 0; 31 passed, 0 failed суммарно (containment 6, fs_sandbox 25; unit/doc 0).
- `cargo clippy -p hoh-fs-sandbox --all-targets --offline -- -D warnings`: exit 0.
- `cargo fmt -p hoh-fs-sandbox -- --check`: exit 0.
- Независимое read-only ревью исходника и текущего crate: material-замечаний после исправлений нет; проверки ревьюер не запускал.
