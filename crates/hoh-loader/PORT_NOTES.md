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
  `id: null`, `name: null`, `insert: null` означают отсутствие управления патчем; пустое name тоже falsy, как в образце.
- Тела предупреждений зависят от sink, как в образце: apply_patches использует bare `%C` logger без цвета,
  compose — JSON-quoted аргументы dump (`js_json_stringify`). Compose добавляет `[<слой>]`; binName и stderr
  принадлежат вызывающему приложению, поскольку в чистой сигнатуре их нет. Точные ссылки приведены ниже.
- L18 требует комментарий перед КАЖДОЙ корневой строкой: `# == <id> · <source> · patched by …`.
  Исходный dump объединял смежные строки. Источник base называется `base` (у compose нет пути base-файла),
  bundle — его имя, профиль — `profile`, дом — `home`, CommandLine — путь, Launcher — переданная метка.
  В origins учитываются также вложенные строки; skipped/no-op патч не добавляет provenance.
- Индекс патчей строится один раз на вызов. Вставки добавляют потомков в индекс; обычная замена config — нет.
  Старые удалённые из группы узлы остаются в индексе, как ссылки в образце. Для provenance используются
  prefix snapshots над исходным base, а не последовательные вызовы apply_patches над промежуточным деревом.
- ID генерируются детерминированным локальным счётчиком с seed от имени файла и вида документа; восемь hex-символов,
  с проверкой занятости в читаемом дереве. Общий patches::IdPool перед apply_patches/compose резервирует
  base и ВСЕ вставки ВСЕХ файлов, включая будущие id для стабильных prefix snapshots.
  Уникальные id выдаются при материализации результата в конце patches::run, в том числе детям,
  введённым заменой config. До этого исходный индекс патчей сохраняет порядок/last-wins поведение образца;
  исправление коллизии не перенаправляет патч на другого адресата. Глобального изменяемого состояния нет.
  Поскольку Entry не хранит происхождение id, неоднозначные явно повторённые id исправляются тем же правилом:
  первая строка сохраняет id, последующие получают свободные 8 hex; ссылки патчей на исходный id не меняются.
  Отличие от образца — только уникальные id в итоговых данных даже для неоднозначного explicit duplicate;
  во время самого алгоритма Map по-прежнему адресует последний duplicate. Уникальные explicit id не меняются.
- Числа YAML читаются с JS double-округлением, JSON.parse также округляет числа как JS; целые значения,
  помещающиеся в целочисленный JSON, не превращаются в `1.0`. Для текста используются js_string/js_number_string.
- Версии, npm resolution, проверки compatibility, исправление/создание manifest, удаление retired bundles,
  runtime module interception не перенесены (O6). BundleSource отвечает за получение Bundle и сообщение
  о недоступных/нечитаемых ресурсах; загрузчик получает готовые `(имя файла, текст)` в объявленном порядке.

## Отличия от спецификации

- `serde_json::Value` не представляет NaN/±Infinity. Скаляры `.inf`/`.nan` (с поддержанными source
  регистрами и знаками) намеренно отвергаются с положением. Числовое переполнение НЕ ошибка неявного чтения:
  js-yaml 4.3.2 отвергает его как int/float, поэтому оно остаётся строкой; explicit !!int/!!float — ошибка.
  `.`, `+.`, `-.`, `.e2`, `-.5`, `+.5` — строки; `.2e2` — число 20. Underscores в числах НЕ поддержаны.
  int `-0` становится положительным нулём (int.js:104), float `-0.0` и int `-0x0` сохраняют знак.
  Выражения отвергают не конечный числовой литерал/Number/сумму и одиночный UTF-16 surrogate в строке.
  JSON.parse теперь целиком использует js_json_parse, включая его общий Value-compatible выбор `null`
  для переполнения до Infinity (hoh-util-values/src/js_json.rs:402); это не переопределяется загрузчиком.
- В fix round прочитан точный read-only источник **js-yaml 4.3.2**,
  `/mnt/toshiba/alexandrie/hoh/refs/js-yaml/lib/schema/json.js`,
  `lib/type/{null,bool,int,float}.js`, `lib/loader.js`, `lib/dumper.js`.
  Старое утверждение, что правил из памяти достаточно, удалено: underscores, пограничные float и explicit
  теги исправлены. В tests/scalars.rs — таблица 79 plain-скаляров с видом, значением и source:line в каждой
  строке, плюс переполнение, округление radix integer с guard/sticky bits, signed zero и explicit теги.
- `parse_patches` требует абсолютный `file`, если вставленные plugin names нужно превратить в file URL.
  Иначе выдаётся Shape, поскольку в чистой сигнатуре нет cwd и настоящий process cwd не читается.
  Profile::load разрешает относительные входные пути через переданный EvalEnv::cwd.
- Встроенный Bundle с `dir: None` получает виртуальный каталог `<profile>/.hoh-bundles/<name>` для имён файлов
  и диагностики. Если нужны реальные относительные модули, BundleSource должен передать физический dir.
- Для WSL EvalEnv позволяет внедрить WSL_DISTRO_NAME/WSL_INTEROP, но не kernel release. Проверка строки
  osRelease на `microsoft` не делается; скрытого чтения ОС нет. Для полного случая образца нужно расширение EvalEnv.
- `LoadError::Expr` хранит текст в `expression`, а не `source`: thiserror трактует поле `source` как Error,
  а текст выражения не реализует Error. Исходная причина находится в `reason`; файл, строка и столбец сохранены.
- YAML имеет предел **128 развёрнутых уровней** и **100000 развёрнутых узлов** на документ.
  Anchors хранят Rc<Node>, aliases разделяют DAG до материализации Value; высота и размер известны до
  раскрытия alias. Превышение — LoadError::Yaml с файлом и позицией именно alias/узла, вызвавшего превышение.
  Это намеренное ограничение против экспоненциального расширения и стекового переполнения; js-yaml shares
  objects и не ограничивает их этим способом. Циклические aliases также отвергаются: Value ацикличен.
  Reader линейный по развёрнутому значению: проверка expressions
  смотрит структуру Node, validate_row пропускает config до отдельного прохода детей, материализация — один
  раз на корневую строку; назначение id и anchoring изменяют config in place.
- Рекурсивные выражения по-прежнему ограничены 128 уровнями. Дополнительно принимается максимум
  **256 лексических токенов** (EOF не считается); 257-й токен — ExprError с его позицией. Ограничение действует
  до создания AST и ограничивает также left-deep binary chains, Clone и Drop. Проверена цепочка 200000 термов.
  Это намеренное отличие от JS и дополнительное ограничение §5.5. Дерево скрыто за Expr::Parsed.
- Telemetry opt-out в Profile::load определяет наличие строки по слоям над пустым profile root (как CLI образца).
  Проверяются только корневые строки, как profile-context.ts:82; вложенная telemetry строка Launcher-патча
  не порождает. Приложение с непустым base добавляет Launcher-патч после проверки своего полного Composition.
- Дополнительных публичных операций сверх §2.9 нет. Serialize/Deserialize для Isolate обеспечивают YAML
  `true | string`; дополнительные Clone/Debug/PartialEq и поля ExprError нужны для тестов и диагностики.
  Для Bundle добавлен Deserialize как граница приёма сырой metadata: отсутствующие platforms — None,
  явно заданные null/bool/string/non-string array отвергаются тем же текстом, что assertBundlePlatform.
  Его поля и BundleSource API не менялись.

## Fix round: пункты 1–23 и точные ссылки

Ни один исходник образца не исполнялся. Новые уточнения §5.5 и три правила JSON/рекурсии/целых из
PORTING.md §4 прочитаны. Ни 76 выражений каталога, ни 621 YAML не потребовали смешивать ?? с ||/&& без скобок;
полный корпус прошёл строгий parser, расширения грамматики не делались.

1. Scalar resolver/constructors перенесены с источника: `refs/js-yaml/lib/type/int.js:20` (resolve),
   `:92` (construct), `float.js:6` (pattern), `:24` (finite guard), `:38` (construct),
   `null.js:5`, `bool.js:5`; `lib/schema/json.js:12` задаёт порядок. 79 строк таблицы имеют собственные source:line.
2. Aliases разделяют Rc DAG до Value; два бюджета проверяются до expansion; doubling/depth-chain тесты.
3. Устранены node.value() на каждом уровне validation/entry, и повторные сериализации групп при id/anchor.
4. `refs/js-yaml/lib/loader.js:1794` возвращает undefined для потока без документов.
   В `vendor/include/src/index.ts:299`/301 undefined и null отвергаются через Array.isArray;
   `packages/boot/app-boot/src/index.ts:594` (patch list) и `:663` (dump) делают то же самое.
   Поэтому правильный публичный результат parse_entries/parse_patches остаётся Shape error для пустого
   текста, комментариев и null. Это проверено для обоих readers, а не заменено принятием пустого массива.
5. Null id/name/insert сводятся к None, algebra выдаёт source warning о требуемом id.
6. YAML quote портирует escapes и printable check из `refs/js-yaml/lib/dumper.js:38`, `:188`, `:543`.
   Проверены U+007F/U+0085/U+2028/U+2029/U+FEFF одновременно в ключах и значениях.
7. Два исходных writer-случая сравнивают полные строки dump, включая комментарии и завершающую LF.
8. JSON.parse использует js_json_parse; закреплены numeric key order 1/2/b, 129 уровней и V8 error text.
9. Includes сначала вычисляет все элементы, затем аргумент; lookup-order и первая ошибка проверены.
10. String operand — JS concatenation, два number — сумма; прочие сочетания, включая 1+true+null, — ошибка
    по новой таблице §5.5 (хотя полный JS даёт 2). Неверное ожидание строки удалено.
11. Parentheses сохраняются в AST; unparenthesized ?? с ||/&& — positional parse error.
12. Все цифры hex-escape проверяются is_ascii_hexdigit до from_str_radix, включая URI и surrogate suffix.
13. Размер выражения ограничен 256 токенами до AST; большой chain возвращает позицию, а не overflow.
14. Win32 resolve сохраняет UNC device; cwd \\\\server\\share\\work и /x дают \\\\server\\share\\x.
    Win32 path helpers проверены с EvalEnv на Linux, **не запускались на Windows**.
15. Manifest читает js_json_parse вместо serde_json::from_str; source `app-boot/src/profile.ts:557`.
16. Отсутствующий/null dsh/profile/bundles дают пустой список, как `app-boot/src/profile.ts:685`.
    Не-object intermediate primitive даёт отсутствие свойства (JS boxing), а не отдельную shape error.
    Неподходящий корень manifest и bundles неправильного типа по-прежнему отвергаются.
17. Telemetry opt-out смотрит только root .some, как `app-boot/src/profile-context.ts:82`.
18. **Два sink:** `vendor/cordis/src/logger.ts:53` передаёт %C в Logger.color, `:79`/80 без цвета
    возвращает bare строку. `vendor/logger-console/src/index.ts:15` меняет только o/O, не C.
    В dump `app-boot/src/index.ts:682` заменяет %C через JSON.stringify — с кавычками и JSON escaping.
    apply_patches и compose используют соответственно эти два формата; tests закрепляют оба.
19. Единственный IdPool резервирует base+все inserts до prefix snapshots; завершающий контроль run
    охватывает config replacements. Нет mutable global, входные массивы не изменяются.
20. `app-boot/src/profile.ts:702` склеивает файлы одного bundle, `:703` добавляет ОДИН ProfileLayer.
    `app-boot/src/index.ts:701` добавляет patchedBy один раз за layer, не за файл. Теперь Profile::load
    делает один Layer на bundle. Это не глобальный dedup строк: если caller намеренно повторит слои с
    одинаковой меткой, source тоже добавит метку повторно. Для multi-file Layer.file=None; пути уже
    использованы при parse/anchoring и сохранены в ошибках, API отдельного списка путей не содержит.
21. `app-boot/src/profile.ts:70` и `:633` используют Node join, не resolve. Абсолютный второй сегмент
    принимается, но НЕ сбрасывает первый сегмент, в отличие от PathBuf::join. Применён Node-compatible
    join для bundle file и виртуального bundle name; absolute-сегменты не запрещены. Npm resolution
    всё ещё не добавлен. Тесты сохраняют anchor под каталогом bundle.
22. user_patches проверяет полные message/file/line/column; добавлены re-read после правок profile/home/overlay
    при unset telemetry, nested row и сырой metadata boundary для нестроковых platforms.
23. Старые описания scalar rules, источников и non-finite поведения исправлены; intentional limits перечислены.

В этом разделе `refs/js-yaml` — абсолютный read-only корень
`/mnt/toshiba/alexandrie/hoh/refs/js-yaml`, остальные source paths — относительно
`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`. Пункт 4 не требует менять уже правильный отказ
на undefined/null: он требует сверить callers и явно закрепить их поведение. Неверных пунктов задания нет.

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
- `bundle-platform: rejects malformed platform metadata` для null/bool/string/non-string array теперь
  проверяется на границе Bundle::deserialize; пустые списки и неизвестные имена проверяются Profile::load.
  WSL-тест использует внедрённые env markers вместо отсутствующего в EvalEnv osRelease; ограничение не скрыто.

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
cargo clippy -p hoh-loader --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo]
cargo fmt -p hoh-loader -- --check
exit 0
cargo test -p hoh-loader
```

Итог fix round по всем suites: **87 passed; 0 failed; 0 ignored** (11 unit + 76 integration).
Строки cargo test: unit `11 passed`; bundle_platform `5 passed`; config_dump `7 passed`;
entry_commit `10 passed`; entry_yaml `18 passed`; expr `6 passed`; profile `15 passed`;
scalars `4 passed`; shipped_yaml `3 passed`; user_patches `8 passed`; в каждой строке `0 failed`.
Оба обязательных остальных результата: clippy `Finished dev profile`, fmt --check — exit 0.
Все 23 пункта fix brief обработаны, незавершённой работы fix round нет.
Модули учёта config_pattern/profile_sanitize и doctests содержат 0 исполняемых случаев.
Все 621 YAML и три LICENSE дополнительно сравнены с образцом через read-only `cmp`: различий нет.

Cargo автоматически обновляет свой workspace lockfile при добавлении разрешённых зависимостей
hoh-util-values/url; исходники и manifest других crates вручную не менялись.

## Вопросы ведущему

1. Нужен ли injectable kernel-release в EvalEnv для WSL, если нет WSL_DISTRO_NAME/WSL_INTEROP?
2. Для NaN/Infinity оставить явный отказ (текущее поведение) или вводить отдельное представление вне Value?
3. Если восстановление profile-sanitize требуется уже на этапе 0, где должен жить его отдельный файловый API?
   В обязательном API §2.9 его нет, а в чистые операции части A он не входит.
