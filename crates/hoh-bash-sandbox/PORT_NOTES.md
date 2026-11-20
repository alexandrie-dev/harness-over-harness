# hoh-bash-sandbox ← packages/shell/bash-sandbox @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | Плагин `SandboxBashExecutor` (NAME `SandboxBashExecutor`, `Config = hoh_bash_local::Config`, зависимости `subprocess`, `sandbox`, `sandboxPolicy`), регистрируется как `shell`. `resolve`/`execute`/`sandbox_mode`, оборачивание argv через `sandbox.confine`, факты `ShellSandboxInfo`, отказ «закрыто» (`SANDBOX_UNAVAILABLE`) для сбоя раннера. |
| `src/helpers.ts` | `src/helpers.rs` | Реэкспорт `classify_runner_failure`, `matches_signature`, `RunnerFailureMatch` из `hoh-sandbox`; `classify_denial(&ShellRunResult, &[String])`; собственная синхронная `is_runner_spawn_failure(&Value, Option<&str>, &str)` (`std::fs::metadata` + `rustix::fs::access(X_OK)` = `statSync`/`accessSync` образца; `rustix` — уже существующая зависимость workspace). |
| `tests/sandbox.spec.ts` | `tests/sandbox.rs` | 45 тестов (42 случая образца + 3 на ошибку помощника `Coded`) (все группы образца), на записывающем фальшивом `sandbox` и настоящих `hoh-sandbox-policy`, `hoh-bash-local`, `hoh-subprocess-local`. |
| `tests/foreground-timeout.spec.ts` | `tests/foreground_timeout.rs` | Все 6 тестов (7 с учётом `it.each` по `success`/`rejection`); виртуальное время `start_paused`. |
| `tests/partial-landlock.spec.ts` | `tests/partial_landlock.rs` | Все случаи: 19 тестов с настоящими `LocalSandboxProvider` и `LocalSubprocessRuntime`. |
| `tests/bwrap.e2e.ts` | `tests/bwrap_e2e.rs` | 4 теста, на этой машине выполняются (bwrap годен). |
| `tests/landlock.e2e.ts` | `tests/landlock_e2e.rs` | 4 теста; пропускают себя, если ядро не принуждает Landlock (на этой машине проба «unusable» — **не выполнялись**). `HOH_REQUIRE_NATIVE_SANDBOX=1` превращает пропуск в ошибку. |
| `tests/seatbelt.e2e.ts` | — | Не перенесён: macOS Seatbelt, на Linux неприменим. |
| `tests/fixtures/{async-confinement-failure,pending-confinement}.ts` | — | Не перенесены: сценарные фикстуры снимков (`snapshot-*`) для сценарного движка образца; ни один перенесённый пакет такого движка не имеет. Их смысл (ожидание подготовки, запрет спавна после таймаута подготовки, отказ асинхронной подготовки) покрыт `foreground_timeout.rs` и `sandbox.rs`. |
| `locale/*.json` | `locale/*.json` | Скопированы без изменений (8 файлов, проверено `cmp`). |
| `README*.md`, `README.i18n.yaml` | эти примечания и rustdoc | Прочитаны. |
| `tests/support/mod.rs` | — (вспомогательный) | Общие фикстуры: записывающий `FakeSandbox`, `ScriptedSubprocess` (настоящий локальный рантайм со сценарным `spawn`), сборка ядра. |

## Отличия от образца
- **Обёртка над процессом.** Образец отдаёт исполнение `LocalBashExecutor` и хранит факты каждого процесса в `Map processFacts` (ключ — объект процесса), удаляя запись на settle. В Rust ключа-объекта нет: факты замыкаются в `Decorate`, который создаётся на вызов (`ArgvSource::Prepare` + хуки `on_started`/`on_process_done`), поэтому «утечка» невозможна по построению, а тест «processFacts.size == 0» не переносится (записи нет как класса).
- **Идентичность дескриптора.** `ShellExecution`, который возвращает `execute`, — обёртка `SandboxExecution`, делегирующая `ShellProcess` внутреннему процессу и мемоизирующая проекцию `result()` (`Shared`). Образец штампует факты прямо на объекте процесса и возвращает тот же объект; сравнений идентичности в тестах нет.
- **Идентичность `AbortSignal` и ошибок.** В `hoh-timeout` у `AbortSignal` нет сравнения идентичности: «тот же сигнал» проверяется поведением (одинаковая причина `ptr_eq` у `SharedError`, одинаковый `TimeoutReason`); «ошибка та же» — `SharedError::ptr_eq`/`downcast_ref` по типу-маркеру.
- **Классификация раннера синхронна.** `ProcessDoneHook` в `hoh-bash-local` синхронный и вызывается внутри задачи settle до `done`, поэтому проверка раннера — собственная синхронная `helpers::is_runner_spawn_failure` на `std::fs`, как `statSync`/`accessSync` образца; асинхронная `hoh_sandbox::is_runner_spawn_failure` здесь не используется, блокировки потока (`block_on`) нет.
- **Факты ошибки spawn.** Образец читает `code`/`syscall`/`path` из объекта ошибки Node. В Rust их несёт только текст «spawn <file> <CODE>» (`SubprocessSpawnRejection`) — и точно так же `SubprocessForeignError::Coded`, которым родитель восстанавливает ошибку помощника `hoh-subprocess-runner` (код и сообщение, без отдельных `syscall`/`path`). `spawn_error_facts` собирает `{code, syscall: "spawn <file>", path: <file>}` из обоих видов, проходя по цепочке `source()`/`SharedError`; сообщение другой формы (например `ENOENT: …, execve '…'`) даёт только `code` и раннеру не приписывается. Для чужих ошибок без структуры — `null`, как у образца без `code`.
- **Код выхода.** `ShellRunResult.exit_code` — `i64`, а `matches_signature` из `hoh-sandbox` берёт `i32`; значение вне `i32` считается «нет кода» (`exit_code_of`). Достижимо только для заведомо недопустимого кода выхода.
- **Политика вызова.** `resolve` ставит политику вызова так же, как образец: явная политика запроса выигрывает, иначе запрашивается `hoh_sandbox_policy::resolve(SandboxPolicyRequest)`; корень `workspace-write` без настройки — рабочий каталог процесса (закреплено тестом `workspace_write_rides_the_policy_workspace_root_…`).
- **Оконное расширение относительно dsh.** Непустые `protectedRoots` делают confinement
  обязательным и для `danger-full-access`: провайдер получает `unrestrictedWrites` и маску,
  поэтому все остальные host paths сохраняют полный доступ, а DSH_HOME нельзя открыть даже после
  одобрения. Перед подготовкой следующей команды сверяется сохранённая при старте identity каждого
  существующего корня; перенос с восстановлением старого pathname другим inode завершается
  fail-closed. Requested mode в публичных foreground/background facts остаётся danger-full-access.
- **Тест «rejection with undefined».** Образец отклоняет подпроцесс значением `undefined`; в Rust отклонение — всегда `SharedError`, поэтому тест воспроизводит «ошибку без структуры» (тип без кода/пути) и ожидает тот же текст «no structured reason» в выводе и те же факты.
- **Таймеры.** Счётчик таймеров `vi.getTimerCount()` не переносится; «дедлайн снят» проверяется тем, что после продвижения виртуального времени спавн/сигнал не происходят.
- **Раннер «скрипт Node».** Тест порядка запуска (`non-bash runner … BASH_ENV`) использует `sh -c` вместо скрипта на Node: проверка той же очерёдности «раннер раньше хука `BASH_ENV`».

## SHIM
- SHIM(hoh-shell): `ShellExecutor::resolve` не возвращает `Result` (образец бросает при недопустимом бюджете); исключение `LocalBashExecutor` остаётся паникой, как в `hoh-bash-local`.

## Не перенесено
- `seatbelt.e2e.ts` (macOS) и две сценарные фикстуры — см. таблицу.
- В `sandbox.spec.ts` не перенесено только то, что опирается на `processFacts` (см. «Отличия»); `it.each` по `kill`/`none` и по формам раннера (`absolute`/`bare`/`relative`) развёрнуты в отдельные тесты или цикл.
- В `partial-landlock.spec.ts`: проверка `processFacts.size`; `Error:`-префикс текста проверяется как у образца (`subprocess failed before reporting an outcome: Error: spawn …`).

## Не проверено
- `landlock_e2e.rs` не выполнялся на этой машине (проба Landlock «unusable»; тесты сами пропускаются). Путь `partial_landlock.rs` покрывает Landlock-ветку поддельным лаунчером, но не настоящим ядром.
- Только Linux; macOS-ветка Seatbelt в этом крейте не нужна (провайдер — `hoh-sandbox-local`).
- Ветка «malformed executable» в `partial_landlock.rs` пишется под обе возможные реакции ОС (прямой `ENOEXEC` или повтор через `/bin/sh`); на этой машине фактически выполняется одна.
- `cargo check --workspace` проверяет сборку, но `apps/hoh` (замена `window_bash_sandbox.rs`) — работа sg1, не проверялось.

## Стабилизация полного workspace-прогона 2026-11-19

- Два теста отсутствующего `workdir` зависели от выбранного `LocalSubprocessRuntime` пути запуска.
  Прямой путь возвращает `SubprocessSpawnRejection`, а helper-путь под параллельной нагрузкой
  возвращает тот же типизированный errno как `SubprocessForeignError::Coded`; прежний тестовый
  `error_code` распознавал только первый вариант. Извлечение теперь проходит типизированную цепочку
  `SharedError`/`source` и принимает обе контрактные формы, не разбирая текст и по-прежнему требуя
  точный `ENOENT`. Отдельная регрессия фиксирует helper-форму, которая падала на старом helper.
- Native e2e-тесты допускают отдельный `HOH_NATIVE_SANDBOX_TEST_ROOT` для окружений, где настоящий
  `$HOME` намеренно read-only; по умолчанию, как и раньше, используется `$HOME`. Это меняет только
  место временного workspace, не режим sandbox и не проверяемую границу записи.

## Вопросы ведущему
- `hoh-shell`: `ShellExecutor::resolve` с `Result` (как и для `hoh-tool-bash`).
- `hoh-subprocess`: чтобы `SubprocessForeignError::Coded` нёс `syscall`/`path`, как ошибка Node, — тогда `spawn_error_facts` не придётся разбирать текст сообщения (сейчас разбирается в этом крейте; `hoh-subprocess` не менялся).
- Экспорты образца, которыми не пользуется ни один перенесённый пакет: `helpers.*` (используются только самим пакетом и тестами) — сохранены как публичные ради паритета с `index.ts`.
- Замена `apps/hoh/src/window_bash_sandbox.rs` на этот крейт — за sg1; он совместим по `Config` (`hoh_bash_local::Config`) и по сервису `shell`.

## Замок protected_effect (круг исправлений credguard)

- Расширение сверх образца: при непустых `protectedRoots` процессы оболочки и операции файловых инструментов
  сериализуются замком `protected_effect` (`hoh-sandbox-policy`). В образце аналога нет.
- Ожидание замка теперь часть самого вызова: оно прерывается по `spec.signal` и по `spec.timeout_ms`
  (`lock_protected_effect_within`), а уже отменённый сигнал замок не ждёт. Раньше второй вызов стоял за
  фоновым процессом до его конца, не глядя ни на сигнал, ни на срок.
- Замок держится не всю жизнь процесса. Живая модель показала зависание: команда, пережившая свой вызов
  (сервер, наблюдатель, долгая сборка), держала замок до конца, и каждый следующий вызов вставал за ней.
  Теперь замок отпускается точным маркером готовности, а не сроком: исполнитель создаёт для вызова каталог
  `ready_dir` (новое поле `SandboxPolicy.ready_dir`, `hoh-sandbox`), `hoh-sandbox-local` монтирует его в bwrap
  на запись (`--bind d d`), а команда оборачивается в `: > '<d>/ready' 2>/dev/null; <команда>`. Файл
  появляется уже внутри песочницы, когда bwrap закончил монтирование и запустил оболочку, то есть путевая
  изоляция применена; хост опрашивает файл и в этот момент отпускает замок. Окна подмены проверенного пути
  после маркера нет: корни уже смонтированы. Срока-таймера нет: если маркер не появился (раннер упал,
  команда не стартовала), замок держится до конца процесса, то есть закрыт по умолчанию. Каталог маркера
  удаляется при завершении вызова.
- Каталог маркера не предсказуем и не лежит в `/tmp`: база — `$XDG_RUNTIME_DIR/hoh-ready` (иначе
  `$XDG_STATE_HOME/hoh/ready`, `~/.local/state/hoh/ready`, в последнюю очередь `<tmp>/hoh-ready-<uid>`); база и
  подкаталог создаются с правами 0700 и проверяются (настоящий каталог, не ссылка, владелец — текущий
  пользователь, прав группы и прочих нет), имя подкаталога — 128 случайных бит. Сигналом считается только
  обычный файл `ready` того же владельца: ссылка или каталог на его месте замок не снимают. Путь в обёртке
  экранируется для одинарных кавычек. Остаточное ограничение: процесс с тем же UID (например, одобренная
  фоновая команда с записью на хосте) может перечислить каталог и создать `ready` раньше времени; граница
  угрозы здесь «чужой пользователь/предсказуемое имя», а не «тот же UID».
- Почему не `--info-fd` у bwrap: он тоже пишется после настройки монтирований, но требует передать
  дополнительный fd через `hoh-subprocess`/`hoh-subprocess-local`, что инвазивно; маркер-файл обходится без
  чужих крейтов. Landlock-раннер к делу не относится: защищённые корни его профиль отвергает, они работают
  только на bwrap.
- Тесты: раннер-заглушка без маркера держит замок (второй вызов ждёт); команда, пережившая вызов, замок не
  держит (в том числе на настоящем bwrap, `bwrap_e2e`); подмена пути при настройке раннера невозможна, пока
  замок не отпущен (`protected_effect_runner_setup`).
