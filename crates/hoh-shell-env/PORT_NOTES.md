# hoh-shell-env ← packages/shell/shell-env @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | Сервис `shellEnv` (`ShellEnvRegistry`), `BashEnvContributor`, `collect`, `list`, плагин `ShellEnvPlugin` (NAME `shell-env`). |
| `src/error.ts` | `src/error.rs` | `SHELL_ENV_ERROR_CODES`, `ShellEnvErrorCode`, `ShellEnvError::new` → `HarnessError` с именем `ShellEnvError`. |
| `tests/shell-env.spec.ts` | `tests/shell_env.rs`, `tests/common/mod.rs`, unit-тесты в `src/lib.rs` | См. «Не перенесено». |
| `locale/*.json` | `locale/*.json` | Скопированы без изменений. |
| `README.md` | эти примечания и rustdoc | Прочитан полностью. |

В образце нет `LICENSE`.

## Отличия от образца
- `register` возвращает `Result<Disposer, BoxError>` (исходное исключение); привязка к области — `ctx.try_effect("bashEnv.register()")`.
- Значения вклада — типизированный `IndexMap<String, String>`: проверка «значение не строка» непредставима и в Rust невозможна (типовая гарантия).
- `localeCompare` заменён ASCII-приближением `locale_compare` (сортировка списка и перечня ключей). Для не-ASCII ключей порядок может отличаться от ICU.
- `DSH_NODE_EXECUTABLE` не выставляется (в Rust всегда один исполняемый файл, случая «Node при отсутствии SEA» нет); ключ остаётся зарезервированным, вклад с ним отклоняется.
- `DSH_INSTALL_DIR` = `dirname(installAnchor)` (как `dirname(profile.installAnchor)` образца). SHIM(hoh-loader): в `ProfileContext` нет `installAnchor`, поэтому якорь приходит сервисом `installAnchor` (`InstallAnchorKey`, значение `InstallAnchor(PathBuf)`). Его предоставляет лаунчер `apps/hoh` рядом с `profileContext` (`profile_boot::run_profile`, `RunProfileOptions::install_root`; корень — `install_root_of(exe)`: префикс над `bin/`, иначе каталог бинарника; якорь — `<корень>/package.json`, как в образце). Тест лаунчера: `apps/hoh/tests/install_anchor.rs` (корень установки ≠ каталог бинарника → `collect()` даёт корень). Если сервиса нет (лаунчер без `install_root`, сторонняя композиция), подставляется каталог текущего исполняемого файла.
- `DSH_HOME` из конфигурации `dsh_home`; тест читает значение из конфигурации, а не из переменной окружения процесса (правило: тесты не меняют env).

## Не перенесено
- «keeps the Node executable where runtime answers no SEA module» — относится к Node-режиму, не существует в Rust.
- «rejects a non-string value» — невозможен по типам (см. выше).
- Тест чтения `DSH_HOME` из env процесса перенесён как unit-тест через карту окружения.

## Не проверено
- Не-ASCII порядок `locale_compare`.
- Другие платформы, кроме Linux.

## Вопросы ведущему
- `hoh-loader` (его пишет l1): `ProfileContext` по-прежнему без `installAnchor`; когда поле появится, `collect` должен читать его вместо сервиса `installAnchor`, а лаунчер — перестать предоставлять сервис отдельно.
- В `hoh-tools` нет публичного конструктора `ToolExecution` для тестов: тесты строят его через `ToolExecutionPipeline` и фиктивный инструмент `bash`.
- Единый тип `Agent` (сейчас по одному shim в hoh-tools и hoh-working-directory).
