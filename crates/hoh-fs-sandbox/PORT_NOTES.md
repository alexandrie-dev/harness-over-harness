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
- Наследование `LocalFileSystem` заменено композицией: `SandboxedFileSystem` реализует объектобезопасный `FileSystem`. Оконное расширение `protectedRoots` закрывает чтение, метаданные, listing, watch и все мутации независимо от режима; в window это весь канонический `DSH_HOME`, включая ещё не созданные descendants под symlink-home. Чтение выполняется через `/proc/self/fd/<O_PATH>` уже проверенного файла/каталога; мутации используют повторно разрешённый physical path, потому что атомарная sibling-write/rename семантика `hoh-fs-local` не работает через procfs magic-link. Общий deployment-lock сериализует model-facing fs/bash effects: другой инструмент модели не может подменить suffix после проверки.
- `sandboxMode` представлен `FileSystem::sandbox_mode() -> Some(default_mode)`. `Config` реэкспортирован из `hoh-fs-local`; plugin `FsSandboxPlugin` сохраняет исходное имя строки `SandboxedFileSystem` и предоставляет сервис `fs`.
- `isPathUnder` стал `is_path_under(path, root, Option<bool>) -> Result<bool, BoxError>`; optional bool сохраняет исходный host default. Linux dev/ino взяты из безопасного MetadataExt. На неподдержанных не-Unix целях identity fallback консервативно не разрешает иной spelling; сам `hoh-fs-local` сейчас Linux-only.
- Для каждого существующего protected root provider при создании запоминает `(dev, ino)`. Target
  открывается относительно заранее открытого parent fd, а вся цепочка предков проверяется через
  `openat(fd, "..")`/`fstat`; поэтому переименование самого корня либо его предка не снимает fence.
  Отсутствующий configured root сохраняет прежнюю lexical-защиту будущего пути.
- Standalone-конструктор принимает `SandboxPolicyService` (клон настоящего сервиса `hoh-sandbox-policy`; `resolve(SandboxPolicyRequest)` возвращает `Result`, ошибка политики — ошибка мутации). Kernel plugin получает `Service<SandboxPolicy>` (ключ `sandboxPolicy` крейта `hoh-sandbox-policy`).
- Тесты идут на настоящем `hoh-sandbox-policy` (плагин над `SessionProjectionRegistry`); session-override и корни пишет сам сервис; тест сессионных разрешений регистрирует их через `register_writable_root`/`set_sandbox_mode` и проверяет изоляцию двух сессий и снятие при выгрузке плагина (`fs-sandbox.spec.ts:96-128`).

## Не перенесено
Нет исходных файлов, экспортов или тестовых объявлений, исключённых из переноса.

## Не проверено
Windows alias/case identity и macOS; конкретный локальный backend и descriptor-bound защита заявлены Linux-only. Произвольный внешний same-UID процесс вне harness не входит в модель угроз этого оконного guard и не берёт общий effect-lock.

Заранее созданная вне модели жёсткая ссылка на защищённый файл остаётся известным ограничением:
проверка model-facing fs идёт по пути, а не по полному реестру `(dev, ino)` всех protected файлов.
Сама модель создать такую ссылку не может, поскольку `link(2)` требует доступа к исходному пути.

## Вопросы ведущему
- SHIM(hoh-sandbox-policy) снят: `src/shim.rs` удалён, собственного ключа `sandboxPolicy` в крейте нет (раньше ядро отказывало смонтировать его вместе с `hoh-sandbox-policy`: другой тип значения под одним именем).

## Проверка
Все команды: `CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x5`, cargo-вызовы ограничены внешним `timeout`.
- `cargo check -p hoh-fs-sandbox --tests --offline`: exit 0.
- `cargo test -p hoh-fs-sandbox --offline`: exit 0; 31 passed, 0 failed суммарно (containment 6, fs_sandbox 25; unit/doc 0).
- `cargo clippy -p hoh-fs-sandbox --all-targets --offline -- -D warnings`: exit 0.
- `cargo fmt -p hoh-fs-sandbox -- --check`: exit 0.
- Независимое read-only ревью исходника и текущего crate: material-замечаний после исправлений нет; проверки ревьюер не запускал.

## Корень файловой системы (круг исправлений credguard)

- У `/` нет родителя, который можно закрепить. Без `protectedRoots` запись в `/` теперь отвечает по режиму, как в
  образце: `FS_NOT_REGULAR_FILE` в `danger-full-access`, текст read-only в `read-only`. При непустых
  `protectedRoots` `/` по-прежнему считается защищённым хранилищем (он содержит их).

## Порядок проверок, чтения без защищённых корней, watch (credguard, ревью 225600)

- Мутации: сначала режим, как в `fs-sandbox/src/index.ts:156-161`: `read-only` отказывает с
  `FS_SANDBOX_DENIED` до любого открытия пути, `danger-full-access` без защищённых корней отдаёт цель делегату
  как есть; `resolve`/якорь появляются только после режима. Каталог `chmod 000` и цикл симлинков в `read-only`
  дают отказ режима, в `danger-full-access` — ответ делегата.
- Без `protectedRoots` чтения, `lstat`, `watch` и мутации идут прежним путём источника: ни `/proc/self/fd`,
  ни якорей, ни замка. Проверка идентичности и якоря — расширение только для непустых корней.
- `watch` отдаёт делегату реальный путь цели, а не `/proc/self/fd/N`: корень наблюдения — каталог файла, номер
  дескриптора после возврата не переиспользуется.
- Замок `protected_effect` в этих крейтах возвращён как расширение окна защищённого хранилища (образца в TS нет):
  объявлен в `hoh-sandbox-policy` (`lock_protected_effect*`), используется здесь и в `hoh-bash-sandbox`.
