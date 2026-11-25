# Plugin Creator

Plugin Creator входит в движок HoH как `hoh:core-plugin-creator`. Автор
или внешний агент может работать через CLI из репозитория HoH.

```sh
hoh plugin templates --json
hoh plugin guide tool --json
hoh plugin new hello --template tool --json
hoh plugin check hello --json
cargo test --offline -p hoh-plugin-hello
```

Шаблон `tool` регистрирует `hello_echo` через сервис Tools ядра. Готовый
smoke-тест вызывает инструмент, проверяет ошибку при неверном входе и
проверяет снятие регистрации при выгрузке. Измените обработчик и его схемы
в `plugins/hello/src/lib.rs`. Имя инструмента укажите также в
`plugins/hello/hoh-plugin.json`.

Команда `hoh plugin new hello` создаёт минимальный Rust-плагин с тестом
загрузки. Для обработки события сессии и вывода в панели выполните
`hoh plugin guide session-panel`. Рецепт указывает на пример `plugins/turn-note`.

Команда `check` проверяет наличие Cargo-манифеста и поля
`hoh-plugin.json`. Ошибки попадают в `issues`, код выхода равен 1.
Поле `linkedManifest` сравнивает файл со встроенным в движок манифестом.
Возможные значения — `missing`, `matches` и `differs`. У свежего шаблона
это `missing`. Для проверки поведения Rust запустите smoke-тест.

После ревью запустите из корня `python3 scripts/generate-plugin-index.py`
и пересоберите движок. Через этот индекс Rust-плагины подключаются к
профилю окна. Команда `scripts/plugin-dev.sh hello` выполняет оба шага
с настроенным офлайн-кешем Cargo.

Сгенерированный Rust исполняется внутри HoH. Ревью исходников перед
добавлением в индекс проводится по тем же правилам, что и для кода ядра.
