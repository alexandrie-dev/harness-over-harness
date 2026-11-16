# hoh-launch-environment ← packages/util/launch-environment @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs | Все типы, функции и ключ контекста перенесены |
| tests/launch-environment.spec.ts | tests/launch_environment.rs | 15/15 развёрнутых случаев (9 шаблонов it/it.each) |
| README.md | — | Прочитан; контракт учтён |
| package.json | Cargo.toml | Дополнительных runtime entry points нет |

Каталогов locale и собственного LICENSE в пакете нет; заимствований из других проектов не указано.

## Отличия от образца

- Типизированный ключ `LaunchEnvironment` добавлен для `Context::get/provide`; строка остаётся `launchEnvironment`, `DSH_LAUNCH_ENVIRONMENT_KEY` сохранён. Это библиотека без Plugin, как в образце.
- `LaunchEnvironmentSnapshot` — непрозрачный Clone со структурно неизменяемым Arc-хранилищем. Копия из контекста разделяет исходное хранилище (проверено внутренним тестом); JS Object identity наружу не выставляется. Входные слои копируются; возвращённая запись принадлежит вызывающему.
- Входные values — IndexMap с порядком вставки, источник — закрытый enum с serde-именами `process`, `project-env`, `user-env`. `None` означает отсутствие path; порядок полей записи value/source/path сохранён. Serialize/Deserialize и стандартные Clone/Debug/PartialEq/Eq/Hash у типов данных — Rust-адаптация публичного API.
- Fallback читает окружение только при отсутствии сервиса, через vars_os с lossy UTF-8 (Node также предоставляет строки). В fallback-тесте вместо глобального vi.stubEnv проверяется каждое реально унаследованное значение и отсутствие project/user provenance: изменение окружения в edition 2024 требует запрещённого unsafe.
- Просмотрены потребители packages (без client/api/typert/host/experimental): app-boot, credentials-local, llm-deepseek*, llm-pi-ai, web-search-deepseek, web-deepseek-api, bundle/web-app. Все экспорты сохранены. В переносимых внешних пакетах не найдены явные импорты `LaunchEnvironmentSource`, `LaunchEnvironmentLayerInput`, `DSH_LAUNCH_ENVIRONMENT_KEY` (первые два используются также через типы аргументов).

## Не перенесено

Нет. Все 15 исходных runtime-случаев перенесены, 0 пропущено. Два параметризованных шаблона развёрнуты в 6 и 2 самостоятельных теста. Ещё 4 проверки: повтор источника, регистр имён, JSON-форма записи, разделение Arc-хранилища.

## Не проверено

Ветка Windows (`to_uppercase` для имён окружения) реализована через cfg(windows), но на Linux не исполнялась; одинаковые имена с разным регистром покрыты платформенным тестом для текущей ОС. Особые различия Unicode-версий Rust и V8 при uppercase отдельно не сверялись. macOS использует общую POSIX-ветку и отдельно не запускалась. Невалидный UTF-8 системного окружения отдельно не создавался.

## Вопросы ведущему

Нет. Недостающего API hoh-kernel нет.

## Проверки

Команды выполняются с `CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w5`.

- `cargo test -p hoh-launch-environment`: 19 passed; 0 failed (unit 1, integration 18; doc-tests 0).
- `cargo clippy -p hoh-launch-environment --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-launch-environment -- --check`: exit 0.
