# hoh-skill-office ← packages/skill/skill-office @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | `Config`, `PythonEnvironment`, plugin `SkillOfficePlugin`, provider `dsh-office`, YAML-frontmatter, progressive discovery/get |
| `assets/*/SKILL.md` | `assets/*/SKILL.md` | 4 файла скопированы без изменений |
| `locale/*.json` | `locale/*.json` | 8 файлов скопированы без изменений |
| `tests/skill-office.spec.ts` | `tests/skill_office.rs`, unit test `src/lib.rs` | все ветви provider-контракта: packaged/relocated assets, config, CRLF/folded YAML, persistent/transient failures, abort, invalidation, dedupe/recovery, disposal и asset guidance |
| `tests/progressive-loading.spec.ts` | `tests/skill_office.rs` | детерминированные входы progressive loading проверены как exact bodies/router/authoring split; agent-loop wiring принадлежит принятым `hoh-tool-skill` и `hoh-agent-loop` |
| `tests/render-context.spec.ts`, `tests/fixtures/*.ts` | — | test-only валидаторы чужих Session/tool events; у пакета нет соответствующего production export |
| `tests/*.e2e.ts` | — | условные системные тесты реального LLM/runtime/Office tools, требуют `DEEPSEEK_API_KEY` и `DSH_PRIMARY_RUNTIME`; не являются hermetic проверками этого provider-крейта |
| `README*.md`, `README.i18n.yaml`, `package.json`, `tsconfig.json` | rustdoc и эти заметки | прочитаны; npm/build/i18n metadata не переносится |

Собственного `LICENSE` и отдельно лицензированных фрагментов нет.

## Точное поведение

- Provider перечисляет `office`, `office-docx`, `office-pptx`, `office-xlsx` в исходном порядке с provider
  `dsh-office`, source `bundled`, rank `BUNDLED_SKILL_RANK`, обеими invocation flags, directory resource base
  и locator `SKILL.md`.
- `list` читает четыре файла параллельно. `ENOENT`/`ENOTDIR` и ошибки YAML — постоянные complete-omissions;
  остальные ошибки чтения делают snapshot incomplete, повторяются при следующем lookup и возвращаются из `get`.
  Постоянный отказ `get` инвалидирует кеш каталога и возвращает `None`.
- Одинаковое строковое представление ошибки предупреждается один раз для данного skill и очищается успешным
  чтением. Abort возвращается вызывающему без предупреждения.
- `assetRoot` и все производные пути проходят через общий эквивалент Node `path.join`: сегменты `.`/`..`
  нормализуются до чтения и до публикации `resourceBase`/`locator`/filesystem diagnostics.
- Три authoring skills получают точный раздел `## Python environment`; router `office` его не получает.
  Значение по умолчанию — `workspace-dependencies`, альтернативное — `user`.
- Как Node `readFile(path, "utf8")`, чтение заменяет невалидные UTF-8 последовательности на U+FFFD. Это
  намеренно не считается временной filesystem-ошибкой; отдельная регрессия добавлена по независимому review.

## Отличия Rust

- TypeScript `yaml.parse` выражен через уже зафиксированный `saphyr-parser` и bounded JSON-materializer:
  поддержаны anchors/aliases, стандартные `!!map`/`!!seq` и core scalar tags, quoted/folded scalars,
  scalar object keys с JS-приведением к строке, non-finite значения в игнорируемых metadata, custom tags,
  duplicate-key rejection, предел глубины 128 и предел развёртывания 100000 узлов. После materialization
  извлекается корневое строковое поле `description`. Ошибка синтаксиса маркируется `YAMLParseError`.
  Неизвестный тег принимается, как в `yaml.parse`, но parser warning не дублируется: provider-контракт имеет
  только warnings о недоступном skill, а TS-тесты не наблюдают внутренний warning YAML-библиотеки.
- Тело обрезается общим `js_trim`, включая U+FEFF, который входит в JavaScript WhiteSpace, но не в Rust
  `str::trim`.
- Packaged asset root — абсолютный `CARGO_MANIFEST_DIR/assets`, Rust-эквивалент package-relative
  `new URL('../assets/', import.meta.url)`. Явный `assetRoot` обязан быть абсолютным.
- JavaScript object spreads выражены `serde_json::Value`: definition сохраняет summary candidate и удаляет
  только `rank` и `locator`, как образец.

## Дополнительные проверки

- Проверены Node-compatible invalid UTF-8 и снятие всех кандидатов при dispose — два edge-case, найденные
  независимым plan review и не ослабляющие исходные проверки.
- Подменяемая только внутри provider-реализации граница чтения проверяет TS-ветку ошибки без Node code:
  incomplete discovery, успешный retry и повторный exact error из `get`. Production reader остаётся
  `tokio::fs::read`.
- Проверены точные 8 locale JSON, context budgets и запрещённые/обязательные Office guidance fragments.
- Отдельная гонка concurrent `get` с `list` после dispose не добавлена: её нет в TS-спеке, а lifecycle и
  удаление provider уже проверяются публичным `list` после dispose; конкурентная семантика принадлежит
  `hoh-skill`, не этому stateless filesystem provider.

## Финальный независимый review

- Временная filesystem-ошибка `get` теперь возвращается публичным `HarnessError` с Node code (`EISDIR`,
  `EACCES` и т. п.) и исходной `io::Error` как cause. Для `EISDIR` сохранён Node syscall `read`; регрессия
  проверяет typed `name/code/message`, а не подстроку.
- YAML materializer заменил прежний direct-scalar extractor; регрессии с aliased description и tagged
  `!!map` падают на прежнем коде и проходят на текущем.

## Не проверено

- Credentialed live LLM/Office runtime suites не запускались: сеть и внешние runtime запрещены условиями
  переноса. Визуальный DOCX/PPTX/XLSX вывод и encrypted workbook принадлежат системному Office tool profile.
- Windows separator нормализуется в `/`, как `directory.split(sep).join('/')`; прогон выполнен на Linux.

## Проверки

Команды выполняются offline с `CARGO_BUILD_JOBS=2`, target через назначенную ссылку
`/mnt/toshiba/alexandrie/hoh/target-x6` на `/dev/shm/hoh/target-x6`; tests — только под `timeout`.

- `cargo test --offline -p hoh-skill-office --no-fail-fast` — 17 passed, 0 failed; doc-tests 0/0.
- `cargo clippy --offline -p hoh-skill-office --all-targets --no-deps -- -D warnings` — чисто.
- `cargo fmt -p hoh-skill-office -- --check` — чисто.
