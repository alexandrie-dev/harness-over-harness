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
- Fallback читает окружение только при отсутствии сервиса, через vars_os с lossy UTF-8 (Node также предоставляет строки). По замечанию независимого ревью выделена публичная seam-функция `create_inherited_environment_layer(impl IntoIterator<Item = (OsString, OsString)>)`: она строит process-слой из переданных пар, без чтения/изменения окружения. Публичный `launch_environment_of` по-прежнему передаёт ей `std::env::vars_os()`.
- Fallback-тест передаёт ровно известную пару `DSH_ENV_SPEC_FALLBACK=ambient` через эту seam и проверяет значение, process-provenance и недоступность из project/user. Отдельно проверены пустой iterator, невалидные UTF-8 имя и значение через Unix OsStringExt::from_vec с байтом 0xFF, и коллизия двух разных невалидных имён (0xFF/0xFE), дающих одинаковый to_string_lossy. Тесты не читают окружение процесса, не меняют его и не предполагают, что оно непустое.
- При коллизии имён после to_string_lossy побеждает **последняя пара в переданном iterator**: IndexMap сохраняет одно строковое имя и последнее значение (позицию первого появления). В настоящем fallback это порядок vars_os(), который отдельно не нормализуется. Тест проверяет оба порядка пар. **Поведение Node для такой коллизии не проверено**; совпадение с Node в этом случае не заявляется.
- Просмотрены потребители packages (без client/api/typert/host/experimental): app-boot, credentials-local, llm-deepseek*, llm-pi-ai, web-search-deepseek, web-deepseek-api, bundle/web-app. Все экспорты сохранены. В переносимых внешних пакетах не найдены явные импорты `LaunchEnvironmentSource`, `LaunchEnvironmentLayerInput`, `DSH_LAUNCH_ENVIRONMENT_KEY` (первые два используются также через типы аргументов).

## Не перенесено

Нет. Все 15 исходных runtime-случаев перенесены, 0 пропущено. Два параметризованных шаблона развёрнуты в 6 и 2 самостоятельных теста. Ещё 7 проверок: повтор источника, регистр имён, JSON-форма записи, разделение Arc-хранилища, пустой iterator окружения, невалидный UTF-8, коллизия lossy-имён.

## Не проверено

Ветка Windows (`to_uppercase` для имён окружения) реализована через cfg(windows), но на Linux не исполнялась; одинаковые имена с разным регистром покрыты платформенным тестом для текущей ОС. Особые различия Unicode-версий Rust и V8 при uppercase отдельно не сверялись. macOS использует общую POSIX-ветку и отдельно не запускалась. Невалидный UTF-8 и коллизии проверены на Unix через iterator seam; реальное окружение тестового процесса не изменялось. Непарные UTF-16 суррогаты OsString Windows не проверены. Поведение Node при коллизии невалидных имён не сверялось.

## Вопросы ведущему

Нет. Недостающего API hoh-kernel нет.

## Проверки

Команды выполняются с `CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w5`.

- `cargo build -p hoh-launch-environment`: exit 0.
- `cargo test -p hoh-launch-environment`: 22 passed; 0 failed (unit `1 passed; 0 failed`, integration `21 passed; 0 failed`; doc-tests 0).
- `cargo clippy -p hoh-launch-environment --all-targets -- -D warnings`: exit 0.
- `cargo fmt -p hoh-launch-environment -- --check`: exit 0.
