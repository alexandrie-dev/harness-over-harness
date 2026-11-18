# hoh-subprocess ← packages/subprocess/subprocess @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/control.ts` | `src/control.rs` | Константы fd 7 / `DSH_SUBPROCESS_CONTROL`, однократное потребление marker, точный отказ, readable/writable/allowHalfOpen. Явные snapshot и opener описаны ниже. |
| `src/error.ts` | `src/error.rs` | Все 7 кодов, оба подкласса HarnessError, классификация foreign errno и сохранение cause/identity. |
| `src/index.ts` | `src/lib.rs` | Абстрактный runtime, сервис `subprocess`, scrub окружения и применение proxy overlay. |
| `src/types.ts` | `src/types.rs` | Все исходные типы, поля, варианты stdio/сигналов/состояний, readers и методы handles. |
| `tests/control.spec.ts` | `tests/control.rs` | Все 3 исходных случая с развёрнутым it.each. Дополнительно: отказ opener, сырые bytes/half-close на memory duplex и Unix socket. |
| `tests/error.spec.ts` | `tests/error.rs` | Все 5 исходных случаев. Дополнительно: JS-кавычки имени программы и реальные ошибки старта ENOENT/EACCES/ENOTDIR. |
| `tests/service.spec.ts` | `tests/service.rs` | Все 3 исходных случая: регистрация провайдера-плагина, duplicate-service, scrub. Дополнительно: весь terminal API и причина отмены. |
| `tests/egress.spec.ts` | `tests/egress.rs` | Все 5 случаев; два сетевых случая запускают настоящий дочерний Rust fixture вместо Node. Дополнительно direct-слой поверх proxy. |
| — | `tests/types.rs` | Дополнительные проверки JSON формы, порядка ключей, null/undefined и всех terminal signals. |
| `README.md` | эти примечания и rustdoc | Прочитан полностью; контракты provider lifetime, private target identity и control bytes учтены. |

В образце нет locale/ и собственного LICENSE; указаний на заимствованный код нет.
Внутренние зависимости: исходные hoh-error, hoh-http-proxy, hoh-kernel; добавлены разрешённые брифом
hoh-timeout и hoh-util-values. Остальные зависимости только workspace; корневые файлы не менялись.

## Отличия от образца

1. Абстрактный класс `SubprocessRuntime` — объектно-безопасный trait с теми же четырьмя методами.
   `Subprocess` — ServiceKey с точной строкой `subprocess`, Value = dyn SubprocessRuntime.
   Провайдер реализует свой Plugin, а в apply вызывает
   `provide_subprocess(&ctx, Arc::new(provider)).await?` (либо ctx.provide::<Subprocess>).
   Никаких нового plugin name/config/defaults нет. Kernel обеспечивает duplicate-service и владение
   provision; реальные процессы и их teardown, как в исходном абстрактном классе, принадлежат провайдеру.
   Context не добавляется к spawn: исходный seam не создаёт эффектов вызывающего; провайдеры владеют
   managed range своим lifetime. Событий исходный пакет не объявляет и не рассылает: Contained/dispatch
   из четвёртого/пятого круга kernel здесь не используется.
2. Async методы — SubprocessFuture (boxed Send Future с BoxError); spawn синхронно возвращает
   Result<SubprocessHandle, BoxError>. Streams — owned AsyncRead/AsyncWrite, control — оба интерфейса.
   Дополнительные SubprocessLifecycle и SubprocessTerminalControl делегируют методы handle конкретному
   провайдеру; lifecycle — дополнительное Rust-поле вместо JavaScript методов объекта.
   SubprocessDone — cloneable Shared future с Result<SubprocessOutcome, SharedError>;
   subprocess_done создаёт его, а провайдер обязан начать процесс/producer до выдачи handle.
   Обычный handle не содержит pid; Object.hasOwn(handle, 'pid') исходного теста выражено формой Rust-типа.
   Terminal handle сохраняет pid. Отдельные await/clone done не потребляют output/readers и не меняют cause.
3. AbortSignal во всех spec/методах — hoh_timeout::AbortSignal, причина — SharedError.
   В этом пакете нет child_process.spawn, abort listener, TERM/KILL, ожидания pipes или таймеров:
   Node-правила «TERM один раз и немедленный abort report» этому коду не требуются.
   Они относятся к реализации провайдера или к стороннему спавнеру; здесь не добавлена скрытая
   эскалация/ожидание смерти потомка. Grace, контроль живого managed range и quiescence остаются их контрактом.
4. Оба исходных класса ошибок наследуют HarnessError и представлены конструкторами, возвращающими
   именно hoh_error::HarnessError. Cause добавляется стандартным with_cause. SubprocessError::is
   узнаёт также SubprocessExecutableNotFoundError (исходный instanceof базового подкласса), но не
   ошибку другого класса с совпавшим кодом. Все семь строк кодов, имена и сообщения сохранены.
   SubprocessForeignError — обычная thiserror Error с message и произвольным JSON code, не HarnessError.
   Это явное представление обычного JS Error с errno-полем; числовой/null code не классифицируется.
   SubprocessFailure различает SharedError, не-Error JSON value (Arc для identity) и Undefined;
   функция сохраняет их без преобразования, если исходник возвращает вход как есть.
   Для std::io::Error, переданного как ошибка старта, errno/message получают через общие
   node_error_code/node_spawn_error_message. Исходный io::Error сохраняется в cause через SharedError.
   Уже подготовленное сообщение платформы передаётся через SubprocessForeignError без переформатирования.
   JSON.stringify(program) выполняет hoh_util_values::js_json_stringify, не serde_json::to_string.
5. Глобальное окружение и глобальная proxy installation не изменяются (PORTING §6).
   `scrubbed_parent_env(Some(&installation))` читает vars_os lossily и возвращает новый IndexMap.
   `scrubbed_parent_env_from(snapshot, Some(&installation))` работает над явным snapshot:
   сначала имитирует publication installation.environment(), затем scrub, затем child overlay.
   None — отсутствие установки. Имена KEY/PASSWORD/SECRET/TOKEN матчятся ASCII case-insensitively,
   как JS regexp /i без /u; DSH_-prefix использует полное Unicode upper-case, как исходный toUpperCase.
   Экспорт SENSITIVE_ENV_PATTERN содержит исходный текст regexp как &str; RegExp-объекта Rust нет.
   Явные env/None tombstones передаются провайдеру отдельно и не проходят повторный scrub.
6. `open_inherited_control_channel(&mut environment, opener)` потребляет marker из принадлежащего
   launcher snapshot, а затем ровно один раз вызывает opener с
   ControlChannelOptions { fd: 7, readable: true, writable: true, allow_half_open: true }.
   Это явная замена process.env и конструктора node:net.Socket: launcher должен передать opener,
   который отдаёт владение своим уже безопасно полученным duplex endpoint для fd 7.
   Здесь нет unsafe adoption произвольного сырого дескриптора или unsafe set_var/remove_var.
   На Windows lookup marker case-insensitive. Неверный/отсутствующий marker удаляется до отказа,
   а ошибка opener проходит как есть. ControlChannelError::NotInherited — обычная Error с точным
   текстом `subprocess control channel was not inherited`.
   Пакет не содержит framing, JSON сообщений или парсера control protocol: bytes, backpressure,
   half-close и ownership остаются у возвращённого stream. Нет новых заголовков/разделителей/перекодирования.
7. Типы данных сохраняют camelCase и порядок полей через serde. Счётчики/байты/dimensions/pid — u64,
   exit code — Option<i64>, nullable outcome.signal — Option<String>, graceMs — f64, чтобы не терять
   дробное значение до проверки провайдера. DshEnvironmentKey — String: template literal TypeScript
   не добавлял runtime validation, а явный env по исходному контракту не валидирует namespace.
   Подтипы SubprocessSpill, SubprocessControlMode, SubprocessPlatform, SubprocessTerminalActivityState
   именуют вложенные объекты/строковые объединения. Сигналы terminal имеют ровно 5 исходных вариантов.
   Runtime-only signal не сериализуется: транспорт провайдера передаёт отмену отдельно.
   При JSON-сериализации spawn spec env-значения None отсутствуют, как undefined в JavaScript;
   сам runtime map сохраняет эти tombstones для удаления переменных при запуске.
   Serialization вспомогательных spec/data — дополнительный Rust API, не готовый runner-протокол.
8. Новые публичные символы для Rust-представления: provide_subprocess, Subprocess (ServiceKey),
   scrubbed_parent_env_from, SubprocessFuture, SubprocessReadable/Writable/Duplex/Control,
   SubprocessDone/subprocess_done, SubprocessLifecycle, SubprocessTerminalControl,
   SubprocessEnvironment, SubprocessSpill/ControlMode/Platform/TerminalActivityState,
   SubprocessForeignError, SubprocessFailure, SubprocessError::is,
   ControlChannelOptions и ControlChannelError. Их назначение и вызовы описаны выше.

Прочитаны production imports переносимых пакетов. Не используются снаружи самого исходного пакета:
SENSITIVE_ENV_PATTERN, SUBPROCESS_ERROR_CODES, SubprocessErrorCode, SubprocessStdinMode и SubprocessStdio.
Остальные ключевые exports используются subprocess-local/subprocess-ssh, shell/bash/pwsh,
terminal-bash, ptc-runtime-node, subagent backends, MCP и boot consumers; они все перенесены.
Дополнительная точка входа ./control — публичный модуль control; ./src/* — публичные error/types/control.

## Не перенесено

Нет полностью исключённых исходных случаев: 16 из 16 представлены (control 3, error 5, service 3,
egress 5). Npm/TypeScript генераторов в исходных tests нет. Source-specific свойства JavaScript
заменены явными Rust представлениями, как описано выше. В двух egress случаях запрещённый брифом Node
заменён реальным дочерним Rust test executable: fixture получает ровно scrubbed env и явно строит
клиент по переданному proxy policy. Это проверяет среду и реальную доставку через proxy; автоматическая
реакция именно Node 22 на NODE_USE_ENV_PROXY не запускается. Fixture отмечен ignored для обычного
test harness и вызывается только двумя egress tests через --ignored --exact; это не пропущенный
исходный test case. Произвольные JS Function/Symbol как thrown values не представлены (PORTING §8).

## Не проверено

Проверка 2026-11-18, каждая cargo-команда с
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w6`:

- `cargo test -p hoh-subprocess --no-fail-fast`: 23 passed, 3 failed, 1 ignored.
  control: 5 passed / 1 failed; error: 7 / 0; service: 4 / 0; egress: 4 / 2 / 1 ignored;
  types: 3 / 0; unit и doc: 0 / 0. Все test binaries скомпилированы.
  Три отказа — `Os { code: 1, kind: PermissionDenied, message: "Operation not permitted" }`
  при создании socket, до проверки сетевого обмена. Для ведущего вне sandbox:
  `tests/control.rs::preserves_every_control_byte_and_independent_half_close_on_unix_socket`,
  `tests/egress.rs::a_child_node_honors_the_proxy_the_user_exported`,
  `tests/egress.rs::a_child_node_reaches_a_proxy_the_user_gave_only_as_all_proxy`.
  `egress_child_fetch_fixture` ignored только как entry point дочернего процесса;
  два egress tests запускают его сами, отдельного пропущенного сценария нет.
- `cargo clippy -p hoh-subprocess --all-targets -- -D warnings`: exit 0, без предупреждений.
- `cargo build -p hoh-subprocess`: exit 0, `Finished dev profile`.
- `cargo fmt -p hoh-subprocess -- --check`: exit 0.

Windows native environment folding и Windows duplex handles не запускались; public stream/opener
граница не требует Unix типов. macOS не запускалась. Никаких новых внешних сетевых адресов нет:
HTTP fixture слушает только 127.0.0.1:0, Unix fixture использует локальную socket pair.

## Вопросы ведущему

Отсутствующих API нет. Есть расхождение общего `hoh_error::node_spawn_error_message` с уточнением
задания: для EAGAIN/EMFILE/ENFILE helper сейчас пишет `spawn <command> <CODE>`, тогда как задание
требует для всех кодов, кроме ENOENT/EACCES, `spawn <CODE>`. В subprocess сохранён обязательный вызов
общего helper без локальной копии его логики; чужой crate не менялся. Это касается только входа
`std::io::Error` с этими тремя errno; явный `SubprocessForeignError` сохраняет исходный message до байта.
Нужна сверка/правка общего helper ведущим. Проверены реальные ENOENT/EACCES/ENOTDIR; редкие отказы
ресурсов не провоцировались на общей машине.

Raw descriptor acquisition выполняется launcher/opener; из абстрактного subprocess seam не вводится
собственная небезопасная platform binding.
