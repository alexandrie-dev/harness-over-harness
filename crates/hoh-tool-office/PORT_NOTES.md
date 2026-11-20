# hoh-tool-office ← packages/document/tool-office @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/error.ts` | `src/error.rs` | 13 кодов `OfficeToolError`, полное отображение 11 ошибок Office Kit и цепочка причины |
| `src/formats.ts` | `src/formats.rs` | lowercase-расширения, семейства документов, матрица форматов и проверки аргументов |
| `src/content.ts` | `src/content.rs` | канонические значения и точный model-facing текст для изображений и временных файлов |
| `src/index.ts` | `src/lib.rs`, `src/operations.rs`, `src/shim.rs` | plugin, три условно активируемых инструмента, схемы, read authorization, render/convert/recalculate и lifetime owner |
| `tests/activation.spec.ts` | `src/lib.rs`, `tests/tool_office.rs` | Перенесён частично: конфигурация, identity-scoped transient marks, header admission всей группы и inactive/active step tool set; пропуски перечислены ниже |
| `tests/tool-office.spec.ts` | `src/{content,error,formats}.rs`, `tests/tool_office.rs` | Перенесён частично: таблицы аргументов/ошибок/контента и реальные kernel/tool pipeline вызовы; пропуски перечислены ниже |
| `tests/fixtures/fake-kit.ts` | `tests/tool_office.rs` | fake `OfficeKitApi`, deferred source read и захват запросов |
| `locale/*.json` | `locale/*.json` | скопированы без изменений |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json`, `tsconfig.json` | эти заметки и manifest | прочитаны; npm/build metadata не копируется |

Новых внешних зависимостей нет. Использованы workspace crates и уже закреплённые
`futures-util`, `schemars`, `serde`, `serde_json`, `tempfile` и `tokio`. JavaScript `trim()`
перенесён через общий `hoh_util_values::js_trim`, включая U+FEFF и полный ECMAScript whitespace.

## Отличия от образца

- `OfficeToolErrorCode` — закрытый Rust `enum`; экспортированные в TypeScript массив
  `OFFICE_TOOL_ERROR_CODES` и объект `OFFICE_KIT_FAILURE_CODES` выражены соответственно
  исчерпывающими `as_str` и `From<OfficeKitErrorCode>`. `OfficeToolError` остаётся constructor
  namespace, создающим единый `HarnessError`, согласно общему Rust error API workspace.
- `OfficeRenderedImage`, `OfficeRenderValue`, `OfficeFileValue`, `OfficeFileFormat`, структуры
  аргументов и семейства публичны вместо TypeScript interfaces/type aliases. Они сохраняют wire
  casing и отсутствие optional полей через `skip_serializing_if`.
- `hoh_tools::Agent` не открывает route options живого Agent. Для render plugin получает
  соответствующий `hoh_agent::Agent` из обязательного `Agents` service; уже снятая регистрация
  отклоняется как `ToolContextUnavailableError`. Это fixed-interface связка, а не копия Agent.
- Пакет `hoh-subagent` пока не предоставляет `rootOf`. `shim::RootSessionResolver`, kernel key
  `"subagents"`, выражает только нужную границу определения владельца output. При отсутствии
  сервиса и при `UNAUTHORIZED` владельцем остаётся вызывающая Session, как в образце.
- TypeScript допускает номера страниц до `Number.MAX_SAFE_INTEGER`, тогда как публичный
  `OfficeImagesRequest.pages` владельца `hoh-office-kit` имеет `Vec<u32>`. Этот crate поэтому
  отклоняет значения выше `u32::MAX` тем же `INVALID_ARGS`, чтобы не усекать их и не паниковать.
  Для точного расширения диапазона нужна правка чужого API `hoh-office-kit`, которую здесь не
  выполняли.
- Source callback хранится как Rust deferred bounded reader. Ключ остаётся JSON-массивом
  `[sessionId, processPath]`; stat-before/read/stat-after и преобразование `FS_TOO_LARGE` сохранены.
- После проверки output directory Rust дополнительно требует, чтобы конкретный путь результата
  Kit отображался в execution world, и возвращает тот же `OFFICE_UNSUPPORTED_REMOTE_WORKSPACE`,
  если дефектный provider нарушил этот инвариант. Образец полагается на provider contract и
  использует cast; для корректного Kit наблюдаемое поведение совпадает.
- Схема инструмента, как и JavaScript, принимает JSON-числа с целым значением независимо от
  записи (`144`, `144.0`, `1.44e2`). Перед typed deserialize такие `pages` и `dpi` теперь
  нормализуются в `i64`; небезопасные/слишком большие целые сохраняются sentinel-значением до
  исходной проверки `inputFamily` и последовательности pages/sheet/range/dpi, где получают
  исходный `ToolArgsError`, а не ошибку serde. Регрессии проходят через настоящий tool pipeline.
- `basename` для имён output использует разделители текущей платформы: на POSIX обратная косая
  черта остаётся частью имени, как у импортированного в образце `node:path.basename`; на Windows
  разделителями являются и `/`, и `\`.
- `extension_of` повторяет собственный `extensionOf` образца (`lastIndexOf` по `/` и `\\` на всех
  платформах): `/w/a\.docx` имеет пустое расширение. Это не `node:path.extname`.

## Не перенесено

- TypeScript-only compile-time проверки типов и npm build metadata отсутствуют по PORTING §8/10.
- PTC-выполнение не дублируется в этом crate: общий `hoh-tools` pipeline вызывает ту же
  `ToolDefinition`, сохраняет execution policy и проецирует её content. Пакет регистрирует
  concurrency flags, output renderers и presentation metadata, которые этот путь использует.
- Многочисленные `it.each` образца сведены в табличные Rust-тесты; отдельный Rust test обычно
  проверяет все строки исходной таблицы.
- Следующие сценарии `tests/tool-office.spec.ts` не перенесены отдельными end-to-end cases.
  Первоначальная сдача ошибочно считала unit-проверки ветвей достаточной заменой §10; эта запись
  исправляет состояние учёта, но не выдаёт непроверенные интеграции за покрытые:
  - отказы model route без `attachments`/`llm` и fallback на `agent.options` (отказ модели без
    image modality, отсутствующий `inputModalities` и неразрешённый маршрут перенесены тестом
    `render_refuses_models_without_image_input_and_unresolved_routes`);
  - WebP/JPEG/PNG normalization перенесены (`render_reports_the_media_type_…`); source без известного
    `size` не перенесён;
  - чужие Office Kit ошибки без классификации;
  - source read failure, замена во время `authorizeContent`, cached-result denial,
    outside-workspace read, permission denied и удалённая working directory;
  - повторный `convertTo` без `sheet`;
  - PTC dispatch, отсутствие Agent, `presentCall`, `isConcurrencySafe` и снятие registrations при
    disposal;
  - физическое удаление output при завершении root Session. Выбор owner для root, fallback на
    caller при `UNAUTHORIZED` и passthrough прочих ошибок проверены fake
    `shim::RootSessionResolver`; настоящее lifetime-удаление всё ещё требует публичный `rootOf`
    API `hoh-subagent`.
- Следующие сценарии `tests/activation.spec.ts` не перенесены через полный AgentLoop harness:
  четыре activation skill по отдельности, успешный/упавший nested `skill`, оба порядка плагинов,
  чужая Session, `UNKNOWN_TOOL`, три header name по отдельности, custom `activationSkills`, native
  и PTC resume/fork, сохранение header после history replacement, reload после `agent/disposed`
  и снятие marks при disposal. Причина учтённого пропуска — первоначальный перенос использовал
  только прямые kernel/session события вместо `hoh-agent-loop-testkit`; кодовые ветви существуют,
  но эквивалентность полному жизненному циклу здесь не заявляется.

## Не проверено

- Реальный LibreOffice/converter не запускается: граница проверена fake `OfficeKitApi`, а concrete
  converter принадлежит `hoh-office-kit`.
- Remote-workspace mapping и отказ filesystem authority реализованы, но в этом crate не
  fault-injected отдельным backend. Смена источника перед deferred read и превышение его bounded
  capacity проверены pipeline-тестом как `OFFICE_SOURCE_CHANGED`/`OFFICE_INPUT_TOO_LARGE`.
- Transient activation через успешный nested `tools/result` отдельно не исполнялась; прямой
  `skill-invocation`, игнорирование чужого skill, очистка `AgentDisposed`, header activation и
  group declarations проверены через прямой kernel/session harness.
- Per-image byte limit, typed attachment refusal, чужая storage error и GIF-defect проверены через
  pipeline; успешная normalization, Office Kit codes, unavailable/busy/render-only hints также
  проверены тестами.

## Вопросы ведущему

1. Для полного диапазона `pages` образца владелец `hoh-office-kit` должен расширить
   `OfficeImagesRequest.pages` с `Vec<u32>` до безопасного целого диапазона; сейчас согласованно
   используется более узкий API владельца.
2. После появления публичного `rootOf` в `hoh-subagent` нужно заменить `shim::RootSessionResolver`
   без изменения wire-поведения этого crate.

## Исправления по ревью 132939

- Добавлены pipeline-регрессии целых JSON-чисел: `pages: [2.0]`/`dpi: 144.0` успешно
  передаются Kit как `2`/`144`; `pages: [1e19]` возвращает `INVALID_ARGS` и исходный текст
  `pages must list distinct one-based page numbers`. Оба теста падали на прежнем serde-разборе.
- POSIX-регрессия `displayPath = "/w/a\\b.docx"` фиксирует basename `a\b.docx`; прежняя
  реализация ошибочно возвращала `b.docx`.
- Отдельный config regression фиксирует serde-отказ для `activationSkills: [1]`, `[null]` и
  не-массива при успешном typed `Config::validate` для `["office"]`.
- Таблицы файлов и раздел «Не перенесено» исправлены: все названные ревьюером отсутствующие
  исходные cases перечислены явно и больше не считаются покрытыми косвенными unit-тестами.

## Исправления по ревью 134441

- Восстановлен порядок TS-валидации для небезопасных целых: `inputFamily`, затем
  pages/sheet/range/dpi. Pipeline-таблица закрепляет три конфликтующих входа; отдельно проверены
  `dpi: 1e19` и дубликат `pages: [1.0, 1]` после нормализации.
- `extension_of` делит по обоим разделителям на всех платформах, как образец; регрессия
  `/w/a\.docx` (INVALID_ARGS «render accepts») проверена pipeline-тестом.
- Перенесены названные ревьюером исполнимые таблицы и fault cases: все 11 пар конвертации и
  recalculate; ранние argument refusals и префикс `invalid arguments:`; 11 кодов Kit через
  настоящий pipeline и сохранение cause; cancellation трёх операций; per-image limit, typed и
  untyped attachment failures, GIF; changed/grown deferred source; три ветви output owner через
  fake resolver.
- Интеграционный harness обновлён на единый `hoh_agent::Agent`, которым теперь пользуется
  `hoh-tools`; это адаптация теста к уже изменённому workspace API, production API crate не менялся.

Финальные команды выполнены offline с `CARGO_BUILD_JOBS=2` и фактической целью symlink
`CARGO_TARGET_DIR=/dev/shm/hoh/target-x6`:

- `timeout 300s cargo --offline test -p hoh-tool-office -- --test-threads=2`: exit 0,
  **33 passed; 0 failed** (24 unit + 9 integration; doc-tests 0).
- `cargo --offline clippy -p hoh-tool-office --all-targets --no-deps -- -D warnings`: exit 0,
  без warnings.
- `cargo fmt -p hoh-tool-office -- --check`: exit 0.
