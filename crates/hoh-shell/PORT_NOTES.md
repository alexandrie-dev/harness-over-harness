# hoh-shell ← packages/shell/shell @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | `ShellExecutor`, ServiceKey `shell`, регистрация provider и исходные root re-export. |
| `src/types.ts` | `src/types.rs` | Все request/spec/result/process-типы, readers, sandbox facts и managed environment. |
| `src/render.ts` | `src/render.rs` | Приоритет signal-marker, ASCII digits, требуемые newline/end и удаление marker из body. |
| `tests/service.spec.ts` | `tests/service.rs` | Все 3 случая с исходными значениями. |
| `tests/render.spec.ts` | `tests/render.rs` | Все 4 случая со всеми исходными проверками. |
| `README.md`, `README.zh.md` | rustdoc и эти примечания | Прочитаны; lifecycle и first-cause контракты сохранены в типах provider. |

`locale/` и собственного `LICENSE` в пакете нет. Все исходные exports перенесены; список exports,
не используемых другими переносимыми пакетами, пуст. `ShellObservedStreams`, `SubprocessOutputRead` и
`SubprocessOutputReader`, доступные в исходном `src/types.ts`, находятся в публичном модуле `types`.

## Отличия от образца

1. Абстрактный класс — объектно-безопасный `ShellExecutor`; `Shell` — ServiceKey со строкой `shell`.
   Provider вызывает `provide_shell(&ctx, Arc<dyn ShellExecutor>)`. Получение sandboxMode — `sandbox_mode()`,
   умолчание остаётся `None`. `resolve` принимает borrowed request, `execute` владеет resolved spec и
   возвращает boxed Send Future с `BoxError`.
2. JS-объекты process/execution — traits `ShellProcess` и `ShellExecution`. Свойства стали методами;
   `done` — cloneable Shared Future `ShellDone`, которая никогда не возвращает ошибку. `shell_done` —
   Rust-конструктор для provider. `observed()` клонирует два `Arc<dyn SubprocessOutputReader>`;
   `kill()` и `read_output()` используют внутреннюю изменяемость provider.
3. `AbortSignal` — `hoh_timeout::AbortSignal`; `DSH_ENV_PREFIX`, `DshEnvironment*`, `CollectedOutput`
   и output readers не копируются, а реэкспортируются из `hoh-subprocess`. Sandbox-типы используются
   из текущего `hoh-sandbox` без shim. `env` — IndexMap с порядком вставки; camelCase и отсутствие
   optional-полей заданы serde. Runtime signal в JSON не входит. Таймауты — f64, байты — u64,
   exit code — Option<i64>; NodeJS.Signals представлен исходным строковым именем `Option<String>`.
4. `ParsedExitStatus` — enum с вариантами `ExitCode { body, exit_code }` и `Signal { body, signal }`.
   Разбор написан строковыми операциями вместо RegExp; `\d` сохранён как ASCII-only. `exitCode` — u64:
   фактические коды выхода покрыты, а decimal-строки больше u64::MAX, которые JS Number принял бы
   с округлением/Infinity, отдельно не представлены и при разборе остаются обычным body. JSON-имя поля — `exitCode`
   (`rename_all_fields = "camelCase"`), порядок ключей `body`, затем `exitCode`/`signal`.

## Не перенесено

Нет: 3 исходных source-файла и 7 из 7 исходных тестовых случаев перенесены. В исходном пакете нет
npm/type-only тестов. Реализации запуска процессов принадлежат providers и сюда не входят.

## Не проверено

Настоящие process providers, spawn failure, deadlines и spill files не запускаются этим abstract seam,
как и в исходных unit-тестах. Платформенные ветки в пакете отсутствуют. Сверка использует текущий API
`hoh-sandbox`, чей `PORT_NOTES.md` на момент переноса ещё отсутствовал.

## Вопросы ведущему

README образца описывает `SHELL_SETTINGS_NAMESPACE`, но такого export нет в `src/index.ts` и нет
использований в TypeScript. В перенос он не добавлен; подтвердить, что это устаревшая документация.
