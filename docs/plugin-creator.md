# Plugin Creator

Автор и внешний агент используют один CLI для создания и проверки Rust-плагинов HoH.
Для каждого плагина откройте отдельный worktree, чтобы его Cargo workspace и индекс
оставались независимыми от других авторов.

```sh
git worktree add -b plugin/hello ../hoh-hello HEAD
cd ../hoh-hello
cargo run --offline -p hoh -- plugin templates --json
```

Нужны закреплённый в `rust-toolchain.toml` Rust, офлайн-кеш Cargo и Python 3.11 или новее.
Если `hoh` ещё не доступен в PATH, используйте префикс
`cargo run --offline -p hoh -- plugin` для команд ниже.

```sh
hoh plugin new hello --template tool --json
hoh plugin test hello --json
```

`new` выводит каталог, два основных файла в `editFiles` и следующую команду в
`testCommand`. Шаблон `tool` регистрирует `hello_echo` и включает проверки вызова,
неверного ввода и выгрузки. Шаблон `basic` проверяет загрузку. Для события сессии
и панели выполните `hoh plugin guide session-panel` и откройте `plugins/turn-note`.

- `hoh plugin check hello --json` проверяет идентификатор, JSON-манифест и имя
  пакета в Cargo. `linkedManifest` сравнивает метаданные со встроенной копией.
- `hoh plugin test hello --json` сначала делает ту же проверку, затем запускает
  офлайн-тесты выбранного пакета. При ошибке сборки показывает до трёх
  диагностик. Полный вывод Cargo и тестов доступен по пути `log`.
- Отчёты создания, проверки и тестирования имеют `formatVersion: 1`.
  Успех помечается `ok: true`. При ошибке код выхода ненулевой, а в `issues`
  есть стабильный `code`, сообщение и, где применимо, файл, строка и подсказка.

## Подключение к движку

Для предварительного просмотра индекса выполните
`python3 scripts/generate-plugin-index.py --candidate hello --dry-run --json`.
Для локального запуска с окном используйте `scripts/plugin-dev.sh hello` в
worktree автора с настроенными `CARGO_HOME` и `CARGO_TARGET_DIR`. Скрипт
пересобирает движок с выпущенными плагинами и выбранным кандидатом, затем
печатает путь к бинарнику для уже собранного окна.

После ревью Rust-кода внесите ID в `plugins/index.json` и выполните
`python3 scripts/generate-plugin-index.py` для сборки поставки. Rust-плагин
работает внутри процесса HoH, поэтому его исходники ревьюятся как код ядра.
