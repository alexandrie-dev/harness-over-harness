# hoh-sandbox ← packages/sandbox/sandbox @ 39163dd3

## Файлы
| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | типы, ошибки, сервис `sandbox` |
| src/roots.ts | src/roots.rs | канонизация, повторная проверка дополнительных корней, дедупликация |
| src/diagnostics.ts | src/diagnostics.rs | атрибуция spawn, правила runner failure, denial signatures |
| src/escalation.ts | src/escalation.rs | лестница разрешений, маркеры, проверка аргументов, approval |
| src/locales.ts | src/locales.rs | все восемь исходных строк |
| tests/vocabulary.spec.ts | tests/vocabulary.rs | 4 исходных объявления |
| tests/escalation.spec.ts | tests/escalation.rs | 15 объявлений, все строки таблиц |
| tests/diagnostics.spec.ts | tests/diagnostics.rs | 11 объявлений, все строки таблиц |
| tests/roots.spec.ts | tests/roots.rs; unit test src/roots.rs | 10 из 11 объявлений; исключён Windows |
| README.md | — | прочитан; собственного LICENSE и каталога locale нет |

Всего 40 из 41 исходного объявления; при развёртывании it.each — 53 из 54 исходных случаев. Единственное исключение — Windows roots. Все строки сохранённых таблиц исполнены циклами Rust-тестов.

## Отличия от образца
- Перенос Linux, по заданию ведущего. Абстрактный `SandboxProvider` — trait, ключ `Sandbox` регистрирует `dyn SandboxProvider` под исходным именем `sandbox`. Конкретного поставщика и конфигурации в этом пакете нет.
- Синхронные функции с файловым вводом-выводом (`canonical_path`, `workspace_writable_roots`, `writable_roots`, `is_runner_spawn_failure`) стали async, чтобы не блокировать исполнитель. Проверки доступа X_OK идут через безопасный rustix на blocking worker. Порядок дополнительной проверки корней перед канонизацией workspace сохранён. Временный каталог повторяет Linux Node TMPDIR/TMP/TEMP и удаление конечного `/`.
- `is_runner_spawn_failure` принимает `&serde_json::Value`: это представление исходных неизвестных полей code/path/syscall. Отсутствующее поле соответствует undefined; неструктурированный null не совпадает. Сам Error вызывающий не сериализует автоматически: передаёт его факты. Синтетическое значение undefined целиком не представимо отдельно от null.
- Закрытые строки — enums; ошибки-наследники HarnessError возвращают непосредственно общий `HarnessError` с исходными name/code/message. У preparation error причина задаётся `with_cause`.
- Дополнительные публичные Rust-имена: `Sandbox` (ключ), `SandboxFuture`, `RunnerFailureMatch` (именованный результат), `EscalationAsk` (анонимный аргумент request), методы as_str/Display/преобразования enum. `EscalationAsk` всегда содержит display_reason, поскольку approveEscalation всегда его передаёт; generic A/C сохраняют структурную независимость approval. Модули открыты как пары исходных src-путей.
- Некорректный EscalationOutcome невозможно вернуть из trait: защитный исходный тест `bogus as never` проверяет отказ serde при разборе неизвестного варианта, а не исполняет недостижимый match.
- Таблицы it.each представлены циклами. Подмена realpathSync в одном исходном roots-тесте заменена unit-инъекцией EACCES в тот же обработчик inspection_error. Все остальные файловые проверки используют временные каталоги; системный `/tmp` только канонизируется.
- Проверено употребление экспортов другими переносимыми пакетами: sandbox-local, sandbox-policy, bash/pwsh-sandbox, fs-sandbox, shell/tool-*, PTC/SSH/terminal/permission-presets. Внешних production-импортов нет у SANDBOX_UNAVAILABLE, SANDBOX_PREPARATION_ERROR_CODES, SandboxPreparationErrorCode, SANDBOX_ESCALATION_ERROR_CODES, SandboxEscalationError, SandboxEscalationErrorCode, WIDER_MODES, EscalationApprover, EscalationApproval, EscalationOutcome, EscalationRequest; они сохранены. `locales::{en,zh,ja,es,fr,de,ru,pt}` и escalation_display_reason также не импортируются извне. Исходные объекты словарей en.escalation и т. п. представлены одноимёнными функциями-рендерерами, поскольку каждый объект имеет только эту функцию.

## Не перенесено
- Windows-сравнение регистра и разделителей в roots: один исходный тест, по явному Linux-only заданию. Раннеры Linux/macOS/Windows находятся в других пакетах, этот пакет их не создаёт.

## Не проверено
- Windows/macOS; некорректные UTF-16-суррогаты непредставимы в Rust String. Дифференциальный запуск JavaScript не выполнялся.

## Вопросы ведущему
Нет швов отсутствующих зависимостей. Потребителям синхронных roots/diagnostics потребуется await по указанным сигнатурам.

## Проверка
Все cargo-команды: CARGO_BUILD_JOBS=2, CARGO_NET_OFFLINE=true, CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w4.
- `cargo check -p hoh-sandbox --tests`: exit 0.
- `cargo test -p hoh-sandbox`: exit 0; 40 passed, 0 failed суммарно. Отдельные targets: unit 1; diagnostics 11; escalation 15; roots 9; vocabulary 4; doc 0 — везде 0 failed.
- После переполнения диска проверены целостность файлов, JSON/TOML и окончания файлов; повреждений не обнаружено. `cargo fmt -p hoh-sandbox -p hoh-fs -p hoh-fs-local -- --check`: exit 0.
- `cargo clippy -p hoh-sandbox -p hoh-fs -p hoh-fs-local --all-targets -- -D warnings`: exit 0; `Finished dev profile [unoptimized + debuginfo] target(s) in 8m 41s`. После ENOSPC запуск повторён, замечание items_after_test_module исправлено переносом модуля в конец roots.rs без изменения поведения.
- Заключительный `cargo fmt -p hoh-sandbox -p hoh-fs -p hoh-fs-local -- --check` после lint-правок: exit 0.
