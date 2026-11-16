# hoh-kernel-macros ← Rust API docs/kernel.md §2.8

## Файлы

`src/lib.rs`: `#[derive(Inject)]`, `#[plugin]`. Тринадцать compile-fail фикстур и две pass-фикстуры (полный, минимальный и переставленный набор параметров; гигиена) в `tests/ui` и `tests/pass`.

## Отличия от спецификации

Trybuild запускается обычным `#[test]`: проверяется компилятор, не асинхронное поведение. В исходном разрешённом manifest macros crate нет зависимости tokio; добавлена только явно разрешённая dev-зависимость `hoh-kernel`.

## Не перенесено

Нет. Ошибки полей Inject, имени параметра, типа Cfg и отсутствия async проверены; обязательные проверки режима события, повторного next/dispose и закрытого конструктора Service также проверены. Проверки `reflect.spec` «access check» и «access error» перенесены в UI.

## Не проверено

Схемы с переименованным в Cargo именем зависимости hoh-kernel: макросы используют контрактное имя `::hoh_kernel`.

## Вопросы ведущему

Нет.

## Fix round D1, D2 и решения A/F

- Сначала добавлены hygiene pass и view_private compile-fail, запущены на принятой базе: hygiene не компилировался (Result alias, vec macro, shadowing config/ctx/deps); View::default неожиданно компилировался. Затем исправлена реализация.
- D1: пути генерируемого кода квалифицированы через ::core / ::std / ::hoh_kernel, включая Result, primitive str, TypeId и vec!. Идентификаторы аргументов и локальной ссылки на функцию получают mixed_site. Pass case включает Result<T> alias, конфликтующий vec!, функции config/ctx/deps и __hoh_ctx. Имя зависимости остаётся контрактным ::hoh_kernel (переименование зависимости не проверено).
- D2: View::default удалён в kernel, пустой View создаётся только pub(crate) методом; снимок новой ошибки E0599 сохранён в tests/ui/view_private.stderr. Прежние 11 compile-fail cases не ослаблены.
- A: процессный Realm counter и local jiff timezone реализованы ядром; dispose Output=(), infallible PluginDef::of, R40/R42/R26, расширения Listen::on_update/retain_raw_config/ConsoleConfig/Volatile Default и typed error boundary сохранены по §10.1. Макросы публичных сигнатур не меняют.
- F: Kernel живёт до явного shutdown(); Drop всех внешних handles actor не останавливает. Пределы конфигурации (4096, стек приложения 64 МиБ), continuations и тестов описаны в ../hoh-kernel/PORT_NOTES.md; макросы не добавляют численных пределов.
- Нет новых неперенесённых macro cases. Trybuild остаётся synchronous compile-only; сеть не используется.

## Проверки

Четвёртый раунд ext: реализация макросов не менялась; Mode=Contained поддерживается существующим event!. Добавлена compile-fail fixture contained_modes: запрещены emit/bail/parallel/serial/waterfall/on для Contained и dispatch для Emit. Всего 13 compile-fail и 2 pass fixtures; старые cases не менялись. Публичные additions и переносимые правила R63–R70 описаны в ../hoh-kernel/PORT_NOTES.md, секция «Четвёртый круг: добавленный API».

Третий раунд fix2: реализация макросов не менялась; новые проверки касаются kernel. В ../hoh-kernel/PORT_NOTES.md записаны точный WORDS ledger, partly disputed legacy R48 constraints, исправления уведомлений/replies/optional integers/inspection/settled и lifetime шагов chain: запущенная task продолжает работу после Drop caller future. Устоявшийся fiber не выдаёт SelfWait из settled(). Прежние 12 UI и 2 pass fixtures сохранены.

`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-k1`:

- `cargo test -p hoh-kernel-macros`: 1 passed; 0 failed, включая 13 compile-fail и 2 pass fixtures; unit/doc tests 0/0.
- `cargo clippy -p hoh-kernel -p hoh-kernel-macros --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-kernel -p hoh-kernel-macros -- --check`: exit 0.
