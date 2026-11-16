# hoh-loader ← vendor/{loader,include,group}, packages/boot/app-boot @ 39163dd3

Часть A: чтение данных, чистое наложение слоёв, вывод YAML, закрытый язык выражений и чтение профиля.
Зависимости на `hoh-kernel` нет. Образец не запускался и не собирался.

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `vendor/include/src/index.ts`: диалект YAML | `src/yaml.rs`, `tests/entry_yaml.rs` | События saphyr-parser, JSON_SCHEMA, метки положения, собственный writer |
| `vendor/include/src/patch.ts`: applyPatches, ensureIds, ensureInsertIds | `src/patches.rs`, `src/yaml.rs`, `tests/entry_commit.rs` | Клонирование, рекурсивный индекс, вставки, замена целых полей |
| `vendor/loader/src/config/utils.ts`: evaluate/interpolate | `src/expr.rs`, `tests/expr.rs`, `tests/support/expr_eval.rs` | Закрытая грамматика §5.5 вместо исполнения JS; интерполяция живых строк — часть B |
| `app-boot/src/index.ts`: parsePatchList, anchorInsertedPluginNames, renderConfigDump | `src/yaml.rs`, `src/patches.rs`, `tests/user_patches.rs`, `tests/config_dump.rs` | Чистые функции §2.9, единый список патчей, provenance |
| `app-boot/src/profile.ts`: loadProfileDirectory, composeEntries | `src/profile.rs`, `tests/profile.rs` | `BundleSource`, порядок файлов и слоёв, атомарное исключение сломанного bundle |
| `app-boot/src/profile-context.ts`: readProfilePatches, resolveTelemetryPatch | `src/profile.rs`, `tests/profile.rs` | Profile → home → overlays → telemetry; окружение только EvalEnv |
| `app-boot/src/bundle-platform.ts` | `src/profile.rs`, `tests/bundle_platform.rs` | Все Node-платформы; WSL по внедрённым переменным (ограничение ниже) |
| `app-boot/src/profile-sanitize.ts`, tests/profile-sanitize | `tests/profile_sanitize.rs` (учёт исключения) | Нет чистых случаев чтения/состава; восстановление файлов вне API §2.9 |
| `app-boot/tests/config-pattern.spec.ts` | `tests/config_pattern.rs` (учёт исключения) | Проверяет удалённый проектор Schemastery, не язык выражений |
| `packages/bundle/*/cordis.patch.yml`, `web-app/presets/*.patch.yml` | `tests/fixtures/dsh/` с исходными относительными путями | 10 неизменённых файлов; каждый читается |
| `snapshots/**/*.yml` | `tests/fixtures/dsh/snapshots/` | Все 611 файлов неизменены; выражения проверяются даже в YAML метаданных |
| §5.5: полный каталог | `tests/fixtures/expressions.txt` | 76 выражений: все разбираются и вычисляются с внедрёнными данными |

`tests/support/expr_eval.rs` подключён как unit-модуль `src/expr.rs`, чтобы проверять внутренний evaluator,
не добавляя временного публичного API. Таблица проверяет значения, ошибки, short-circuit, отсутствие сервиса,
`undefined` против `null`, строгие сравнения, JS-преобразование чисел, пути POSIX/Windows и package resources.
`tests/expr.rs` проверяет весь каталог грамматики и позиции отказов (UTF-16).

Для каждого из 244 `cordis.yml` / `cordis.snapshot.yml` проверяется форма строк либо патчей: некоторые replay-файлы
с этими именами содержат `insert`, поэтому имя файла не используется как утверждение о его формате.
Остальные snapshot YAML — метаданные; они проходят общий читатель и проверку всех expression nodes.

Реальный headless-шаблон объявлен в `packages/util/official-bundles/src/bundles.ts`:
`@deepseek-ai/dsh-base`, затем `@deepseek-ai/dsh-headless`. Сохранённого профиля с его `package.json`
и созданного runtime-файла `<profile>/cordis.yml` в дереве образца нет. `apps/cli/src/profile-boot.ts`,
`PROFILE_ROOT_CONFIG` и `prepareProfile`, создают этот файл со значением `[]` при запуске.
Тест создаёт manifest с точным порядком шаблона и накладывает настоящие bundle-файлы на пустой корень:
строки получаются из их вставок, предупреждений L10 нет, dump читается обратно без изменений.

## Отличия от образца

- `!!js` — только язык §5.5; `eval`, `new Function`, module imports и произвольный JS не исполняются.
- При чтении строки проверяются по L4, неизвестный ключ — Shape; исходный parsePatchList проверял только mapping.
  Передаваемые напрямую из Rust некорректные типы в `Patch::set` диагностируются отдельным предупреждением
  `patch: invalid fields for <id>: <serde error>, skipping`; строки никогда не исчезают молча.
- `null` удаляет поле патча (L10); исходный applyPatches присваивал `null`.
  Пустая строка `Patch::name`, если задана, тоже является утверждением имени (L10).
- Предупреждения L10 содержат имена/id без кавычек, как написано в спецификации; printf `%C` в исходном dump
  представлял аргументы через JSON.stringify и добавлял кавычки. `compose` добавляет `[<слой>]` и вызывает
  переданный sink, сам не пишет в stderr.
- L18 требует комментарий перед КАЖДОЙ корневой строкой: `# == <id> · <source> · patched by …`.
  Исходный dump объединял смежные строки. Источник base называется `base` (у compose нет пути base-файла),
  bundle — его имя, профиль — `profile`, дом — `home`, CommandLine — путь, Launcher — переданная метка.
  В origins учитываются также вложенные строки; skipped/no-op патч не добавляет provenance.
- Индекс патчей строится один раз на вызов. Вставки добавляют потомков в индекс; обычная замена config — нет.
  Старые удалённые из группы узлы остаются в индексе, как ссылки в образце. Для provenance используются
  prefix snapshots над исходным base, а не последовательные вызовы apply_patches над промежуточным деревом.
- ID генерируются детерминированным локальным счётчиком с seed от имени файла и вида документа; восемь hex-символов,
  с проверкой занятости во всём читаемом дереве. Глобального генератора/изменяемого состояния нет.
- Числа YAML читаются с JS double-округлением, JSON.parse также округляет числа как JS; целые значения,
  помещающиеся в целочисленный JSON, не превращаются в `1.0`. Для текста используются js_string/js_number_string.
- Версии, npm resolution, проверки compatibility, исправление/создание manifest, удаление retired bundles,
  runtime module interception не перенесены (O6). BundleSource отвечает за получение Bundle и сообщение
  о недоступных/нечитаемых ресурсах; загрузчик получает готовые `(имя файла, текст)` в объявленном порядке.

## Отличия от спецификации

- `serde_json::Value` не представляет NaN/±Infinity. Корректные для js-yaml не конечные скаляры (`.nan`,
  `.inf`, переполнение экспоненты и пограничные формы вроде `.`) отвергаются с положением, вместо тихого `null`.
  Выражения также отвергают не конечные числовые литералы/результат Number/JSON.parse и одиночный UTF-16 surrogate в строке,
  который невозможно сохранить в Rust String без потери данных. В поставленных файлах таких значений нет.
- Исходных `node_modules/js-yaml/lib/type/int.js` и `float.js` в образце НЕТ. Использованы известные правила
  js-yaml 4 JSON_SCHEMA/core resolvers, не запуск Node и не сверка установленного пакета: регистры null/bool,
  ведущие нули, знаки, нижние `0x`/`0o`/`0b`, внутренние underscores, точка/экспонента; нет дат и sexagesimal.
  Краевые числовые формы закреплены unit-тестом `scalar_resolver_boundaries`.
- `parse_patches` требует абсолютный `file`, если вставленные plugin names нужно превратить в file URL.
  Иначе выдаётся Shape, поскольку в чистой сигнатуре нет cwd и настоящий process cwd не читается.
  Profile::load разрешает относительные входные пути через переданный EvalEnv::cwd.
- Встроенный Bundle с `dir: None` получает виртуальный каталог `<profile>/.hoh-bundles/<name>` для имён файлов
  и диагностики. Если нужны реальные относительные модули, BundleSource должен передать физический dir.
- Для WSL EvalEnv позволяет внедрить WSL_DISTRO_NAME/WSL_INTEROP, но не kernel release. Проверка строки
  osRelease на `microsoft` не делается; скрытого чтения ОС нет. Для полного случая образца нужно расширение EvalEnv.
- `LoadError::Expr` хранит текст в `expression`, а не `source`: thiserror трактует поле `source` как Error,
  а текст выражения не реализует Error. Исходная причина находится в `reason`; файл, строка и столбец сохранены.
- YAML и рекурсивные выражения имеют явный предел вложенности 128 с позиционной ошибкой; это ограничивает
  расход стека. Дерево термов скрыто за вариантом Expr::Parsed; ParsedExpr непрозрачен и не реэкспортируется.
- Telemetry opt-out в Profile::load определяет наличие строки по слоям над пустым profile root (как CLI образца).
  Приложение с непустым base добавляет Launcher-патч после проверки своего полного Composition.
- Дополнительных публичных операций сверх §2.9 нет. Serialize/Deserialize для Isolate обеспечивают YAML
  `true | string`; дополнительные Clone/Debug/PartialEq и поля ExprError нужны для тестов и диагностики.

## Часть B

Оставлены за пределами crate до появления рабочего kernel:
`PluginRegistry` (включая alias/carrier/resource_dir), `BootOptions`, `boot`, `LoaderKey`, `Loader`,
`EntryInfo`, `EntryStatus`, `EntryChange`, `UpdateAction`, `Audit`, `Group`, `Expr::eval` и kernel-вариант LoadError.
Также: монтирование cordis:include/group, carriers, resolver/interpolation, settled, recompose, audit,
cmdline/config-editor и runtime create/remove. Рекомпозиция данных уже реализована функцией compose.

Адаптер Expr::eval вызывает единственный `expr::evaluate(expr, env, base_dir, service_lookup, resource_lookup)`.
`service_lookup` возвращает None ТОЛЬКО для отсутствующего сервиса. При `ctx.has(name)` и отсутствии view
адаптер возвращает `Some({})`; данные берёт из Context::view. Resource lookup учитывает aliases registry.
Сам evaluator не обращается к процессному окружению или registry/kernel.

От kernel нужны: Context::{has,view,plugin_dyn,isolate,intercept,with_tag}, PluginDef/Plugin,
PluginOptions + ConfigResolver, Fiber::{raw_config,effective_config,resolve_config,state,settled,update,
commit_volatile,restart,dispose}, события жизненного цикла, Logger и Kernel::snapshot.
Resolver вызывается перед validate, не выдаётся carriers; undefined в объекте удаляет ключ, в массиве
становится null. Part A уже возвращает None как undefined, но обход живой config относится к части B.

## Не перенесено

### Случаи, ожидающие часть B

`cordis-entry-commit.spec.ts`:
- `reports assigned ids and applied options for create, update, move and remove` — часть B для create/update/remove;
  move не переносится по O4/O5.
- `reports explicit Fiber saves and self-disposal but ignores child disposal` — часть B для lifecycle;
  обратная запись не переносится по O4.
- `keeps adopted entries and does not report file-driven removal or owner teardown` — часть B, Group/Fiber.
- `mounts explicit ids containing the separator without nested-path resolution` — часть B, mounting;
  чистое адресование патчами id с `:` уже проверено.

`user-patches.spec.ts` (чистые части чтения/якорения уже перенесены):
- `loads absolute plugin paths from patch files as file URLs ($label)` — часть B только для boot и сервиса;
  преобразование всех имён/вложенных строк проверено сейчас.
- `keeps Include's config literal — a nested row's !!js belongs to that row's fiber` — часть B.
- `waits for row injections before resolving !!js and resolves again after provider replacement` — часть B.
- `evaluates a !!js disabled expression against the loader context` — часть B; чистый evaluator покрыт таблицей.
- `keeps the raw expression in the options so write-back preserves the !!js form` — часть B для options;
  round trip expression nodes проверен; write-back не переносится по O4.
- `re-evaluates when update() replaces the expression, mounting and unmounting` — часть B.
- `retains unchanged import diagnostics across profile reconciliation` — часть B.
- `rejects a context without the launcher root Include` — часть B.
- `removes a previously failed entry without reporting its old activation error` — часть B.
- `reports an unchanged failed entry as a warning but rejects a changed configuration with the same failure` — часть B.
- `logs a superseded activation failure without failing removal` — часть B.
- `waits for a removed plugin to release its resources` — часть B.
- `applies id-targeted overrides, inserts, and interpolates !!js from the environment` — часть B для boot;
  patches/evaluation отдельно проверены.
- `mounts no patch layer for an absent or empty user layer` — часть B для mounting; чтение уже проверено.

Loader runtime, volatile-update, включения и файлы app-boot/config-reload/startup-diagnostics/cmdline,
не входящие в задание части A, — часть B по §8.2–8.4.
`config-dump: fails loud on a missing, unparsable, or non-array base config` — часть B только для missing base
(IO boot); чистый parse проверяет остальные два отказа.

### Случаи, не имеющие чистого API в §2.9

- `cordis-entry-yaml: folds long scalars at the requested line width` — исключён явно §8.2, оформление js-yaml.
- `config-dump: groups contiguous rows with the same origin and patches under one separator` — заменён
  проверкой per-row комментариев, поскольку обязательный L18 задаёт другой формат.
- `config-dump: defaults its warn sink to one stderr line per skipped patch` — у compose sink обязателен,
  stderr принадлежит вызывающему приложению; в чистой библиотеке нет default stderr.
- `profile: joins the home and rejects traversal-shaped names` — CLI discovery, Profile::load принимает dir.
- `profile: creates manifest, user patch layer, and pnpm workspace once, never overwriting`;
  `auto-initializes only shipped templates and fails loud otherwise`;
  `normalizes only the exact installation-owned headless bundle tuple`;
  `removes retired %s and keeps the rest of an application-owned manifest` — запись/миграции профиля,
  отсутствуют в §2.9; чтение manifest не меняет сохранённый выбор.
- `profile: writes and reads back, and fails loud on a broken manifest` — запись fixture принадлежит тесту;
  чтение/отказ на сломанном manifest покрыты в `profile.rs`; публичного writer manifest нет.
- `profile: prefers the installation anchor, falls back to the profile, and fails loud`;
  `resolves a package whose exports map omits ./package.json`;
  `skips a bundle whose own dsh peers are incompatible until the profile exempts that exact pair`;
  все случаи `isolated profile resolution`, `createRuntimeResolution`, `removeLinkProjections` — npm/module
  resolution, compatibility и управление установкой исключены O6. Сбой BundleSource и retry проверяются сейчас.
- `bundle-platform: rejects malformed platform metadata` для null/bool/string/non-string array — исключено
  типом Bundle::platforms; пустые списки и неизвестные имена проверяются в Rust. WSL-тест использует внедрённые
  env markers вместо отсутствующего в EvalEnv osRelease; ограничение не скрыто.

`profile-sanitize.spec.ts`: все пять случаев меняют файлы, не являются чистыми text→data операциями и не
требуют kernel; они НЕ обозначены как часть B и не подменены ослабленными тестами:
- `recovers a profile without loading its broken plugins or patch`;
- `preserves previous backups across retries and later recovery actions`;
- `does not create an absent profile and backs up a patch even without a manifest`;
- `leaves the patch untouched when profile JSON is invalid`;
- `propagates backup failures before changing activation`.
Операция sanitizeProfile отсутствует в §2.9; восстановление/backup/writeProfileBundles принадлежит редактору
или launcher, а не чистому loader. `tests/profile_sanitize.rs` содержит этот учёт, исполняемых случаев нет.

`config-pattern.spec.ts`: параметризованный `%s with flags %s has portability %s` (25 строк таблицы),
`uses native flagless semantics when flags are absent`, `does not hide unexpected parser failures` — это
createPatternCheck и RegExpParser из проектора Schemastery→JSON Schema. §8.3 заменяет сам проектор Schemars;
у hoh-loader нет regex-projector API. `tests/config_pattern.rs` содержит учёт, исполняемых случаев нет.
Они не относятся к парсеру выражений §5.5 и не отложены под видом необходимости live kernel.

## Не проверено

Нативные macOS/Windows filesystem/file-URL ветки не запускались. Выбор платформ и вычисления путей Win32
проверены с заданным EvalEnv на Linux; это не проверка Win32 filesystem. WSL без env markers, определяемый
только через kernel release, не поддержан текущей сигнатурой EvalEnv (см. выше).
Обратная запись, загрузка модулей JS, npm resolution, реальные домашний каталог/окружение и kernel не использовались.

## Выражения вне грамматики

Не обнаружены. Проверены выражения во всех 621 скопированных YAML (10 bundle/preset + 611 snapshot).
Грамматика не расширялась под конфигурацию. Скриптовые JS-модули в name остаются строками; их дальнейший
Unresolved — задача части B, а не повод исполнять образец.

## Лицензии

Без изменений скопированы `vendor/loader/LICENSE`, `vendor/include/LICENSE`, `vendor/group/LICENSE`
в `licenses/{loader,include,group}-LICENSE` (MIT). Ведущему: учесть их в общем NOTICE.

## Проверка

Только разрешённые Cargo-команды с `CARGO_NET_OFFLINE=true`
и `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-l1`.
Финальные проверки прошли:

```text
cargo build -p hoh-loader
Finished `dev` profile [unoptimized + debuginfo]
cargo clippy -p hoh-loader --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo]
cargo fmt -p hoh-loader -- --check
exit 0
cargo test -p hoh-loader
```

Итог по всем suites: **62 passed; 0 failed; 0 ignored** (7 unit + 55 integration).
Строки cargo test: unit `7 passed`; bundle_platform `4 passed`; config_dump `7 passed`;
entry_commit `7 passed`; entry_yaml `14 passed`; expr `3 passed`; profile `9 passed`;
shipped_yaml `3 passed`; user_patches `8 passed`; в каждой строке `0 failed`.
Модули учёта config_pattern/profile_sanitize и doctests содержат 0 исполняемых случаев.
Все 621 YAML и три LICENSE дополнительно сравнены с образцом через read-only `cmp`: различий нет.

Cargo автоматически обновляет свой workspace lockfile при добавлении разрешённых зависимостей
hoh-util-values/url; исходники и manifest других crates вручную не менялись.

## Вопросы ведущему

1. Нужен ли injectable kernel-release в EvalEnv для WSL, если нет WSL_DISTRO_NAME/WSL_INTEROP?
2. Для NaN/Infinity оставить явный отказ (текущее поведение) или вводить отдельное представление вне Value?
3. Если восстановление profile-sanitize требуется уже на этапе 0, где должен жить его отдельный файловый API?
   В обязательном API §2.9 его нет, а в чистые операции части A он не входит.
