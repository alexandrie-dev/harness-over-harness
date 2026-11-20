# hoh-tool-skill ← packages/skill/tool-skill @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| `src/index.ts` | `src/lib.rs`, `src/catalog.rs`, `src/gesture.rs` | перенесён |
| `tests/tool-skill.spec.ts` | `tests/tool_skill.rs`, unit-тесты модулей | 33 из 34 сценариев перенесены; исключение ниже |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |

## Отличия от образца

- В Rust элемент публичного `SkillCatalogSource.entries` назван отдельным
  публичным типом `SkillCatalogEntry`; в TypeScript это анонимный вложенный тип.
- JavaScript `String.slice()` может оставить одиночную половину surrogate pair,
  тогда как обязательное общее правило порта запрещает резать символ посередине.
  Rust считает лимит в единицах UTF-16, но отступает до границы scalar value;
  случай закреплён тестом.
- Сценарии TypeScript сгруппированы в 27 integration и 5 unit-тестов по
  наблюдаемым контрактам: durable-каталог и compaction, шесть malformed-форм,
  incomplete/recovery с частичным результатом, оба proposed-message пути,
  layered scope/visibility, policy до и после загрузки, filesystem/resource
  base, пользовательские жесты и полный lifecycle с повторной регистрацией.
- После ревью `hoh-tool-skill.233808` восстановлены exact assertions для
  initial/update/tombstone/incomplete catalog text, полных error message/name/code,
  `value.resourceBase`, отсутствия скрытых metadata/body и user-only навыка в
  model-каталоге. Companion message проверяется по неизменному `id`.

## Не перенесено

- Прямая подмена `session.eventAt` из JavaScript-теста невозможна у закрытого
  Rust `Session`. Сам точный текст отказа сохранён в `catalog_history`; нарушение
  непрерывности журнала отдельно запрещает владелец `hoh-session`.
- ESM/package exports и README-артефакты npm не имеют runtime-аналога в Rust.

## Не проверено

- Нет платформенных веток.

## Вопросы ведущему

- Нет; новых внешних зависимостей и правок чужих API нет.

## Проверки

Все команды fix-round выполнены с `CARGO_BUILD_JOBS=2`,
`CARGO_NET_OFFLINE=true`, `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x4`.

- `timeout 300s cargo test --offline -p hoh-tool-skill`: unit —
  `5 passed; 0 failed`; integration — `27 passed; 0 failed`; doc-tests — 0/0.
  Всего 32/0.
- `timeout 300s cargo clippy --offline -p hoh-tool-skill --all-targets --no-deps -- -D warnings`:
  exit 0.
- `cargo fmt --package hoh-tool-skill -- --check`: exit 0.
- `cmp` всех 8 locale с образцом и `git diff --check`: exit 0.
- Независимый read-only fix-review подтвердил закрытие всех пунктов исходного
  ревью; после усиления lifecycle/partial/proposed тестов новых blocker/error нет.
