# hoh-bash-local ← packages/shell/bash-local @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | Конфигурация, resolve, плагин `LocalBashExecutor`, `execute_argv`/`execute_spec`, `ExecuteHooks`. |
| (часть `index.ts`: класс `BashExecution`, settlement, `result()`, чтение вывода) | `src/execution.rs` | Выделено в отдельный файл ради размера; публичный тип `BashExecution` реализует `ShellProcess` + `ShellExecution` из `hoh-shell`. |
| `tests/executor.spec.ts` | `tests/executor.rs`, `tests/common/mod.rs` | Все 35 исходных случаев; тестовый subprocess-провайдер заменяет `hoh-subprocess-local` (см. «Не проверено»). |
| `tests/settings.spec.ts` | `tests/settings.rs` | Случай перенесён через `commit_volatile`; NaN-проверка вынесена в прямой тест `assert_serviceable_bash_config`, потому что JSON не представляет NaN. |
| `locale/*.json` | `locale/*.json` | Скопированы без изменений. |
| `README.md` | эти примечания и rustdoc | Прочитан полностью. |

В образце нет собственного `LICENSE`. Экспортируются `ENV_OVERRIDES`, `Config`, `assert_serviceable_bash_config`, `LocalBashExecutor`; default export — тот же публичный тип.

## Отличия от образца
- Исключения синхронных проверок и подготовки — `Result<_, BoxError>`; Promise — Future. `done()`/`result()` повторяемы (shared future, результат запоминается, `classify` вычисляется при первой проекции).
- `ENV_OVERRIDES` — массив из четырёх пар в исходном порядке; слияние окружения сохраняет порядок через `IndexMap`.
- Таймауты и byte budgets хранятся как `f64` (NaN и дроби доходят до проверки конфигурации). В `ShellExecRequest/Spec` и `SubprocessCollect` значения `u64`, поэтому дробные значения принимаются, как в образце (проверка «положительное конечное» идёт по исходному числу, `maxOutputBytes: 0.5` не вызывает панику), и приводятся к `u64` при построении spec: кап вывода (`maxOutputBytes`) — `ceil` (коллектор образца, `output.ts:162-180`, удерживает `ceil(cap)` байт, `0.5` → 1 байт; тест `a_fractional_output_cap_retains_ceil_of_it_like_the_ts_collector`), прочие — усечением; `stdoutMaxBytes` NaN/-1 непредставимы, проверяется только 0. Тест `accepts_a_fractional_output_cap_without_panicking`.
- `on_started` всегда раньше settlement (index.ts:387: microtask после синхронного хука): задача settlement ждёт закрытия `oneshot`-затвора после возврата хука (затвор открывает и паника хука). Поэтому хук не перезаписывает итоговые факты sandbox. Тест `on_started_runs_before_settlement_and_cannot_overwrite_final_sandbox_facts` (многопоточный runtime). Для сборки подклассов добавлен публичный `LocalBashExecutor::new(config, subprocess)`.
- `onProcessDone` (protected-метод) представлен хуком `ProcessDoneHook` в `ExecuteHooks`; `executeArgv` — публичный `execute_argv(spec, ArgvSource, ExecuteHooks)` с `PrepareArgv`.
- Факты sandbox (`mode`, `denied`, `runnerFailed`, `enforcement`) выставляются через `BashExecution::set_sandbox` и читаются из результата.
- Завершение процесса обрабатывает фоновая задача `tokio::spawn` (`settle_outcome`/`settle_rejection`): у источника нет жизненного цикла `ctx`, который она могла бы использовать, — это промис, не привязанный к плагину. Задача завершается вместе с процессом.
- `resolve` в трейте `ShellExecutor` не возвращает `Result`, а образец бросает исключение. Публичный `try_resolve` возвращает `Result`; `impl ShellExecutor::resolve` паникует с исходным текстом ошибки (SHIM(hoh-shell), см. вопросы). Потребители в `hoh-tool-bash` перехватывают панику.
- Тестовый провайдер в `tests/common` реализует только механику процессов, нужную исходным тестам, и не является переносом `hoh-subprocess-local`. Вместо JS-объекта с бросающимся `Symbol.toPrimitive` — ошибка, чей `Display` возвращает `fmt::Error`.

- Известное отличие (метаданные дробного капа): `maxOutputBytes: 0.5` и `printf x` в TS дают `truncated: true`, spill-файл и текст `[output truncated…]` (коллектор считает `bytes 1 > cap 0.5` и отбрасывает дробный остаток), у Rust (`ceil` → кап 1) — `truncated: false`: 1 байт помещается в кап. Удерживаемые байты совпадают, метаданные нет. Коллектор — `hoh-subprocess-local`, не этот крейт, поэтому не исправлено; закрывается там (кап проверяется как число, не как `u64`) либо передачей дробного капа через seam.

## Не перенесено
Нет исключённых исходных файлов и случаев (кроме непредставимых значений `stdoutMaxBytes`, см. выше). Npm/TypeScript-артефакты не переносятся (правило 8).

## Не проверено
- Реальный `hoh-subprocess-local` (ещё пишется w5): lifecycle и spill проверены только на тестовом провайдере.
- Только Linux; macOS не запускалась, Windows образцом не поддерживается.
- Дробные byte limits и значения выше `u64` не проверяются из-за типов `hoh-shell`/`hoh-subprocess`.

## Вопросы ведущему
- SHIM(hoh-shell): сделать `ShellExecutor::resolve` возвращающим `Result<ShellExecSpec, BoxError>`; тогда паника в `impl ShellExecutor` и `catch_unwind` в `hoh-tool-bash` удаляются.
- После появления реального `hoh-subprocess-local` заменить тестовый провайдер и повторить lifecycle/spill случаи.
- Для `bash-sandbox` согласовать замену protected `executeArgv`/`onProcessDone` (сейчас `ExecuteHooks`/`PrepareArgv`).
- Единое правило округления f64→u64 на границе spawn.
