# hoh-base ← packages/bundle/base @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs` | пустой runtime API сохранён; добавлен Rust-экспорт `CORDIS_PATCH` |
| `cordis.patch.yml` | `cordis.patch.yml` | перенесён как профильные данные; две Windows/Pwsh строки отложены |
| `tests/base.spec.ts` | `tests/base.rs` | 2/2 случая: структура core patch и платформенный shell-гейтинг |
| `package.json` | `Cargo.toml`, `CORDIS_PATCH` | bundle patch доступен напрямую; npm metadata не переносится |

Перенесено **2 из 2** исходных тестовых случаев, не перенесено 0.

## Отличия от образца

- В исходном `src/index.ts` runtime API отсутствует. Rust экспортирует дополнительную константу
  `CORDIS_PATCH`, чтобы статический компоновщик профиля мог получить содержимое package export
  `./cordis.patch.yml` без npm-разрешения.
- Cargo-крейт не перечисляет десятки crates строк профиля как зависимости: patch остаётся данными,
  а наличие зарегистрированных плагинов проверяет владелец итоговой композиции. Это соответствует
  роли bundle и не притягивает runtime-код в библиотеку данных.

## Не перенесено

- Строки `pwsh-sandbox` и `tool-pwsh` ждут этапа 5 согласно назначению `MAP.tsv` и
  правилу PORTING.md §8. Вместе с ними до этапа 5 отложена симметричная проверка win32-ветки;
  POSIX `bash-sandbox`/`tool-bash` и их точное выражение отключения сохранены.
- npm metadata, installation/export-map и проверки наличия npm dependencies не имеют runtime-аналога
  в Rust. Соответствующие plugin identity закреплены по настоящим строкам patch.

## Не проверено

- Фактическая оценка `process.platform` на Windows ждёт этапа 5. YAML разбирается настоящим
  `hoh-loader`, а выражения сохраняются как loader expression nodes.

## Вопросы ведущему

- Нет; правки чужих API не требуются.
