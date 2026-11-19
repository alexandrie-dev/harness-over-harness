# hoh-loader ← vendor/{loader,include,group}, packages/boot/app-boot @ 39163dd3

Части A и B: данные YAML/профиля и живое дерево на принятом `hoh-kernel`.
Образец только читался; ничего в нём не запускалось и не собиралось.

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `vendor/include/src/index.ts`: диалект YAML | `src/yaml.rs`, `tests/entry_yaml.rs` | События saphyr-parser, JSON_SCHEMA, метки положения, собственный writer |
| `vendor/include/src/patch.ts`: applyPatches, ensureIds, ensureInsertIds | `src/patches.rs`, `src/yaml.rs`, `tests/entry_commit.rs` | Клонирование, рекурсивный индекс, вставки, замена целых полей |
| `vendor/loader/src/config/utils.ts`: evaluate/interpolate | `src/expr.rs`, `tests/expr.rs`, `tests/support/expr_eval.rs` | Закрытая грамматика §5.5, Expr::eval и kernel ConfigResolver для живых строк |
| `app-boot/src/index.ts`: parsePatchList, anchorInsertedPluginNames, renderConfigDump | `src/yaml.rs`, `src/patches.rs`, `tests/user_patches.rs`, `tests/config_dump.rs` | Чистые функции §2.9, единый список патчей, provenance |
| `app-boot/src/profile.ts`: loadProfileDirectory, composeEntries | `src/profile.rs`, `tests/profile.rs` | `BundleSource`, порядок файлов и слоёв, атомарное исключение сломанного bundle |
| `app-boot/src/profile-context.ts`: readProfilePatches, resolveTelemetryPatch | `src/profile.rs`, `tests/profile.rs` | Profile → home → overlays → telemetry; окружение только EvalEnv |
| `app-boot/src/bundle-platform.ts` | `src/profile.rs`, `tests/bundle_platform.rs` | Все Node-платформы; WSL по внедрённым переменным (ограничение ниже) |
| `app-boot/src/profile-sanitize.ts`, tests/profile-sanitize | `src/profile_sanitize.rs`, `tests/profile_sanitize.rs` | Все пять recovery случаев; timestamp передаёт вызывающий |
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
  Все falsy id/insert (null, false, 0, пустая строка) означают отсутствие управления патчем; пустое name тоже falsy.
  Truthy нестроковый id не преобразуется в string: Map не находит numeric 5 даже при наличии строкового id "5";
  предупреждение печатает исходное значение через js_string либо js_json_stringify для соответствующего sink.
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
- Явный непустой id НИКОГДА не меняется, включая дубликаты: чистые функции сохраняют обе строки,
  а индекс патчей адресует последнюю строку с этим id, как Map образца. Политика смонтированного дерева
  для duplicate explicit id: одна запись в store, последние options в группе побеждают (см. реализацию B ниже);
  origins остаётся заданной API картой по id, не по экземпляру строки.
- parse_entries/parse_patches сохраняют анонимность (`Entry.id == ""` либо отсутствующий id вложенной строки).
  Генерация перенесена в общую область apply_patches/compose: только так, не добавляя metadata в Entry и не
  читая глобальное состояние, можно гарантировать уникальность generated id между независимо прочитанными файлами.
  IdPool заранее резервирует explicit id из base, всех вставок и замен config, затем выдаёт восемь hex
  только анонимным строкам. Каждый generated id уникален относительно ВСЕХ explicit/generated id этой композиции.
- IdPool хранит выданный id по месту создания строки (base/индекс, patch/индекс/insert/индекс либо config/путь).
  Он живёт весь вызов compose: базовые строки получают id до первого prefix snapshot, вставки/замены — при
  первом появлении. При повторном построении префикса берётся тот же id из cache; обход изменённого документа
  не перенумеровывает строки. Входные base/layers не изменяются. Владелец живого дерева сохраняет полученные
  идентичности между отдельными вызовами/загрузками (src/identity.rs); cache чистого вызова не глобален.
- Числа YAML читаются с JS double-округлением, JSON.parse также округляет числа как JS; целые значения,
  помещающиеся в целочисленный JSON, не превращаются в `1.0`. Для текста используются js_string/js_number_string.
- Версии, npm resolution, проверки compatibility и runtime module interception не перенесены (O6).
  Profile metadata creation/normalization и удаление retired bundles перенесены в fix round B. BundleSource отвечает за получение Bundle и сообщение
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
- `Patch.id` теперь `Option<Value>` вместо `Option<String>` из §2.9: required closure behavior нельзя
  выразить исходной сигнатурой. Превращение numeric 5 в "5" ошибочно нашло бы строковой ключ Map, а Shape error
  вместо warning также расходился бы с источником. Raw Value сохраняет тип и JS-печать. Это единственная
  изменённая сигнатура; Entry и прочие поля Patch не расширялись, скрытых служебных ключей в set нет.
  Для malformed bundle element диагностическое `SkippedBundle.name: String` — js_string исходного значения;
  точный Node error находится в reason, сохранённый manifest не меняется.
- `LoadError::Expr` хранит текст в `expression`, а не `source`: thiserror трактует поле `source` как Error,
  а текст выражения не реализует Error. Исходная причина находится в `reason`; файл, строка и столбец сохранены.
- YAML имеет предел **128 развёрнутых уровней** и **100000 развёрнутых узлов** на документ.
  Anchors хранят Rc<Node>, aliases разделяют DAG до материализации Value; высота и размер известны до
  раскрытия alias. Превышение — LoadError::Yaml с файлом и позицией именно alias/узла, вызвавшего превышение.
  Это намеренное ограничение против экспоненциального расширения и стекового переполнения; js-yaml shares
  objects и не ограничивает их этим способом. Циклические aliases также отвергаются: Value ацикличен.
  Reader линейный по развёрнутому значению: проверка expressions
  смотрит структуру Node, validate_row пропускает config до отдельного прохода детей, материализация — один
  раз на корневую строку; anchoring изменяет config in place, назначение id происходит при композиции.
- Рекурсивные выражения по-прежнему ограничены 128 уровнями. Дополнительно принимается максимум
  **1024 лексических токенов** (EOF не считается); 1025-й токен — ExprError с его позицией. Ограничение действует
  до создания AST и ограничивает также left-deep binary chains, Clone и Drop. Проверена цепочка 200000 термов.
  Это намеренное отличие от JS и дополнительное ограничение §5.5. Дерево скрыто за Expr::Parsed.
- Telemetry opt-out в Profile::load определяет наличие строки по слоям над пустым profile root (как CLI образца).
  Проверяются только корневые строки, как profile-context.ts:82; вложенная telemetry строка Launcher-патча
  не порождает. Приложение с непустым base добавляет Launcher-патч после проверки своего полного Composition.
- В части A дополнительных публичных операций сверх §2.9 не было; дополнения части B перечислены ниже. Serialize/Deserialize для Isolate обеспечивают YAML
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
13. Размер выражения ограничен 1024 токенами до AST; большой chain возвращает позицию, а не overflow.
14. Win32 resolve сохраняет UNC device; cwd \\\\server\\share\\work и /x дают \\\\server\\share\\x.
    Win32 path helpers проверены с EvalEnv на Linux, **не запускались на Windows**.
15. Manifest читает js_json_parse вместо serde_json::from_str; source `app-boot/src/profile.ts:557`.
16. Отсутствующий/null dsh/profile/bundles дают пустой список, как `app-boot/src/profile.ts:685`.
    Не-object intermediate primitive даёт отсутствие свойства (JS boxing), а не отдельную shape error.
    Неподходящий корень manifest и bundles, который не является массивом, по-прежнему отвергаются.
    Нестроковый ЭЛЕМЕНТ массива теперь пропускается с per-bundle reason, а не отклоняет весь профиль (closure ниже).
17. Telemetry opt-out смотрит только root .some, как `app-boot/src/profile-context.ts:82`.
18. **Два sink:** `vendor/cordis/src/logger.ts:53` передаёт %C в Logger.color, `:79`/80 без цвета
    возвращает bare строку. `vendor/logger-console/src/index.ts:15` меняет только o/O, не C.
    В dump `app-boot/src/index.ts:682` заменяет %C через JSON.stringify — с кавычками и JSON escaping.
    apply_patches и compose используют соответственно эти два формата; tests закрепляют оба.
19. Единственный IdPool резервирует explicit id из base/всех inserts/config replacements до prefix snapshots;
    выдаёт id только анонимным строкам и кеширует по месту создания. Explicit duplicate не переписывается.
    Нет mutable global, входные массивы не изменяются.
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

## Closure check: пять уточнений

1. В `packages/boot/app-boot/src/profile.ts:690`/691 разрешение каждого элемента выполняется внутри try;
   `:704`/705 записывает String(error) и продолжает. Нестроковое имя отвергает вызов
   `createRequire(anchor).resolve.paths(packageName)` в `profile.ts:632`, с Node ERR_INVALID_ARG_TYPE для request.
   Profile::load теперь пропускает только такой элемент. Для ["a",5,"b"] сохранены a/b и ровно одна причина:
   `TypeError [ERR_INVALID_ARG_TYPE]: The "request" argument must be of type string. Received type number (5)`.
   Также сохранена печать отрицательного нуля в Node reason. Это не добавляет npm resolution.
2. `vendor/include/src/patch.ts:29`/31 и `:37`/39 генерируют id только при его отсутствии; explicit duplicate
   не исправляется. Прежнее ожидание assert_ne удалено, обе строки сохраняют id "duplicate", поздний патч
   по-прежнему меняет последнюю строку. Generated id проверяются отдельно на уникальность против всех явных
   и ранее выданных id. В части B store переиспользует одну запись с последними options; чистое дерево сохраняет обе строки.
3. СНАЧАЛА добавлен regression с двумя anonymous base rows (root/внутри группы) и двумя вставками в группы,
   стоящие перед ними в документе. На старом коде он упал: base_root сменил 00000001 на 00000002.
   После cache по creation site сохраняются оба базовых id и created_by=base; новые строки сохраняют
   created_by=insert и свои id в следующем prefix snapshot. Дополнительный regression покрывает config replacement.
4. `vendor/include/src/patch.ts:65` проверяет truthiness insert, `:67` и `:86` — id; `:91` использует Map.get
   без приведения типа, `:93` печатает not-found warning. Проверены id: 0, id: false, insert: false,
   id: 5, а также empty string/нулевой insert: первые дают required-id warning, numeric 5 —
   `patch: entry 5 not found` даже при наличии explicit string id "5". String "5" по-прежнему адресует строку.
5. EXPR_MAX_TOKENS=1024: 1025-й токен возвращает positional error. Легальный includes со списком из 85 строк
   читается, а chain из 200000 термов всё ещё отвергается до AST. Clone/Drop проверены на 512 термах.

Все source paths этого раздела относительно read-only корня
`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`. Образец не исполнялся.

## Реализация части B

Все семь пунктов brief выполнены в пределах нижеописанных исключений и двух kernel gaps.

| пункт | реализация и проверка |
|---|---|
| 1, PluginRegistry | src/registry.rs: exact-name перед alias, цепочки/cycles, carrier и resource_dir; tests/registry.rs |
| 2, строки/fibers | src/runtime.rs: disabled, inject, isolate, intercept, raw/effective config; Expr::eval вызывает единственный evaluator части A; tests/runtime.rs, user_patches.rs, volatile_update.rs |
| 3, Group/Include | Group, cordis:group/include, aliases @deepseek-ai/cordis-plugin-group/include; carrier не получает resolver; обновление на месте, чтение YAML/JSON, initial; tests/config_reload.rs |
| 4, Loader | LoaderKey="loader", boot, entries, resolve, create/remove, settled, recompose, audit, dump; §2.9 сигнатуры сохранены |
| 5, app-boot | config-reload, startup audit/diagnostics, profile-sanitize, boot failure cleanup; tests/app_boot.rs, startup_diagnostics.rs, profile_sanitize.rs |
| 6, ConfigEditor | src/config_editor.rs, ConfigEditorKey="configEditor"; явные patch-file edits, validate → atomic write → reconcile → rollback; tests/config_editor.rs |
| 7, duplicate explicit id | EntryGroup.update строит last-id map: один mounted Node с последними options; raw Composition хранит обе строки, patch index указывает на последнюю |

Новые модули соответствуют vendor/loader/src/config/{entry,group,tree,isolate}.ts, vendor/include/src/index.ts,
vendor/group/src/index.ts, app-boot/src/{index,startup-diagnostics,profile-sanitize,config-schema/*}.ts и
packages/boot/config-editor/src/index.ts. Восемь config-editor locale JSON скопированы без изменений.

L12 обрабатывает независимые строки одновременно, сохраняя ошибки каждой. O3 пересоздаёт контекст при
name/inject/isolate/intercept changes. L13 использует raw сравнение, retain_raw_config + commit_volatile,
fallback на update, явный retry Failed. Ошибка volatile-кандидата сохраняет raw и старый действующий Cfg.
R53/§10.1: Cfg handle читает новый Arc snapshot целиком; ранее полученный Arc не изменяется; kernel может
послать VolatileUpdate с пустыми paths. Тесты явно учитывают эти Rust правила вместо JS mutable references.
L14 disabled не реактивен. L15 undefined удаляет ключ объекта и становится null в массиве.
L17 ждёт kernel transitions, Pending — покой. ConfigReload имеет точное имя app-boot/config-reload.

Include хранит file-local Identities между rereads/recompose; неизменённые anonymous rows сохраняют id
при вставках перед ними (включая вложенную группу). Сопоставление — raw row + parent + base/insert scope,
а не новый номер документа. Новые id резервируются против явных/ранее выданных. Внутренний ключ экранирует
% и : по компонентам дерева; public id остаётся исходным. Literal a:b и путь Include a → b могут одновременно
монтироваться, но resolve не адресует literal id с separator, как tree.ts:resolve (V27). Обычная Group не
создаёт нового tree namespace; Include создаёт. Ownership/versions защищают заменённую группу и запоздавшие
IO/update completions. В tests/runtime.rs и user_patches.rs есть bounded multi_thread(2) варианты загрузки,
удаления и 32 одновременных create; обычные async случаи используют start_paused=true.

Shipped YAML: каждый из пяти official-bundles templates (acp, web, headless, sdk, sdk-minimal) берётся в
исходном порядке, включая четыре web presets. Registry содержит пустые stand-in plugins для каждого имени
и внедрённые service views/resources; compose не выдаёт L10 warnings, settled завершается, audit пуст.
Checked-in runtime profile/base в source нет: CLI создаёт cordis.yml со значением []; тест повторяет это.

### Atomic write

Используется принятый hoh-atomic-write = { path = "../hoh-atomic-write" }: with_file_lock на package.json
и write_file_atomic с mode 0o600 для cordis.patch.yml и rollback. Loader-specific причины дублировать
packages/util/atomic-write нет. Прежний src/atomic_write.rs удалён вместе с прямыми getrandom/rustix/sha2
зависимостями. Ошибка underlying lock сохраняется в цепочке LoadError::AtomicWrite, включая HarnessError code;
ошибки самой edit операции downcast возвращаются как исходный LoadError. Контекст владеет edit task,
поэтому бросивший ожидание caller не обрывает начатую транзакцию; unload отменяет её, а RAII общего crate
освобождает lock/temp. In-process edits также сериализованы service mutex. HMR exclusive queue отсутствует O10.

## Отличия части B от образца и §2.9

Обязательные API §2.9, ранее отсутствовавшие в A, присутствуют с заданными сигнатурами.
Принятое отличие A Patch.id: Option<Value> остаётся единственным изменением обязательной сигнатуры.

- Public additions сверх §2.9: модули config_editor, config_schema, startup_diagnostics, profile_sanitize;
  ConfigEditor/ConfigEditorKey/Configuration и mount/document_path/entries/configuration/edit;
  ConfigSchemaEntry/ConfigSchemaDiagnostic, generate_config_schema/collect_config_schemas/build_config_schema_document;
  StartupError/StartupEntryDiagnostic/StartupOutcome/ActivationErrors и типы Startup*State/CompositionDiagnostic,
  capture/capture_startup_composition/format_activation_error/audit_startup_entries;
  resolve_profile_dir/init_profile/read_profile_manifest/write_profile_manifest/load_profile;
  Loader::refresh и event ConfigReload. Они нужны для прямых экспортов app-boot/config-editor из brief.
  Entry/Isolate получили JsonSchema; EntryInfo — Clone, enums — Debug/PartialEq/Copy по смыслу.
  LoadError расширен Kernel, Runtime, Startup, AtomicWrite, Boot и ConfigFile (с cause).
- Registry заменяет imports/module-resolution; его default регистрирует builtins и две dsh aliases.
  PluginDef всегда имеет typed Config, даже NoConfig. Dynamic exports/отсутствующий Config и JS getter не существуют.
- ConfigSchema использует schemars вместо проектора Schemastery (§8.3): стандартные $ref/#/definitions,
  defaults/annotations, вложенные expression alternatives и отдельные literal carrier containers.
  Обход include-chain итеративный; циклы проверяются canonical path, malformed siblings не теряются.
  inject-object из source schema исключён: L4 и Entry §2.9 принимают только Vec<String>.
  Node interception, разные resolutions одного имени, native JS lazy/UID/validator/prototype hooks исключены.
  Детерминированные Rust validation/parser errors заменяют тексты JS engine; source-owned фиксированные
  сообщения схемы/патчей/startup policy сохранены. IO хранит исходную std::io::Error в LoadError::Io.
- boot §2.9 получает готовый Context: host preparation и .env выполняет вызывающий. Имя в его diagnostics —
  "hoh", потому что BootOptions не имеет binName; audit_startup_entries принимает явное имя.
  При заранее/посреди boot закрытом root получить Service невозможно: Kernel Closed/Inactive сохраняется
  в LoadError::Boot с source-prefix `hoh: plugin tree failed to load: `, тогда как source возвращал disposed Context.
- Startup diagnostics имеют typed Rust cause chain/aggregate вместо JS stack/non-Error objects.
  ordinary Debug не раскрывает composition/messages/entries. Bounded composition сохраняется до teardown;
  warning/error messages копируются после cleanup из instance-owned kernel logger buffer, а не через временный
  JS exporter. Buffer принадлежит ядру и продолжает работать после успешного boot. Версия пакета — unknown (O6).
  Source metadata — filesystem base directory, а не JS module baseUrl; она immutable, поэтому source-трюк
  удаления baseUrl не переносится. capture/capture_startup_composition теперь async и сами читают
  Context::reflect().store().await; public capture_with_snapshot workaround удалён. Kernel sixth round принят.
- ConfigEditor::mount получает профиль/BundleSource/home/overlays/EvalEnv явно вместо динамического
  profileContext service; configuration async из-за filesystem IO. edit callback Send + 'static: задача
  принадлежит Context. src/yaml_edit.rs использует saphyr-parser spans (character → byte offsets),
  меняет только config/row/insert/delete spans и собственные expression-marker spans. Остальные комментарии,
  anchors, aliases, flow maps, порядок и relative plugin names побайтно сохраняются. Новые узлы — block style;
  только корневая flow sequence принудительно становится block, как document.contents.flow=false.
  Журнал включений не используется (O4); отказ rollback-recompose заменяет исходную ошибку, как source.
- sanitize_profile — синхронный filesystem API образца; caller обязан остановить профиль и исключить записи.
  Timestamp i64 передаёт caller вместо Date.now (детерминированные тесты), возвращается Option<PathBuf>.
  Broken manifest проверяется до backup; patch никогда не разбирается, dependencies/другие поля сохраняются.

## Не перенесено

O4/O5: inclusion journal, implicit Fiber save/write-back, moves/adoption между деревьями, runtime module
interception и JS export manipulation. O6: npm resolution/installation/compatibility и link projections;
metadata profile cases #3/#5/#6/#12–14 не исключаются O6 и теперь перенесены.
O10: file watchers/HMR; refresh вызывается явно и тестирует reload без изменения кода плагинов.
cmdline и apps/hoh — отдельное задание, прямо исключённое brief. Не осталось открытого списка API «Часть B».

Ниже единица учёта — исходная группа it/it.each, а не число Rust fn или комбинаций параметров.
В loader-volatile-update в текущем source **23 группы / 26 развёрнутых случаев**, а не 25 из старого плана:
19 групп (22 развёрнутых случая) проходят, четыре исключены §8.2; блокировок R54 больше нет.
«Перенесено» означает применимую Rust часть; специальные адаптации/исключённые assertions перечислены здесь:
- volatile #9/#10/#11/#21 и entry-commit #1: file persistence/move assertions убраны только по O4/L16;
  чтение/живое обновление/create/remove/raw/noSave semantics проверяются.
- volatile #5: даты представлены ISO strings, массивы JSON; сами Date/class instances исключены §8.2.
- source expressions process.version, dshPath(..), get('x').a вне закрытой §5.5 в тестовых fixtures заменены
  разрешёнными platform/resource_dir/dshHomePath/get('x')?.a формами; grammar не расширялась.
- config-reload #7: HMR заменён явным refresh (O10); те же четыре вида отказа, успешные siblings сохранены.
- config-dump #2: комментарии перед каждой строкой по L18 вместо исходной группировки separator.
- startup-diagnostics #3/#5/#7: capture получает store из Context, без Kernel handle. Для #5/#7
  typed Arc<()> зарегистрирован Loading-поставщиком (R26), второй сервис отсутствует; проверяются owner,
  same-scope/other-scope и реальное состояние loading вместо JS active/undefined. После разрешения gate
  потребитель всё ещё Pending из-за второго сервиса. #6: проверен disabled после self-dispose; удаление immutable source metadata невозможно.
  #8: Rust compile-time употребление exported types вместо expectTypeOf.
- config-schema #3/#8: typed schemars descriptions/defaults/shared/recursive refs вместо namespace/class/lazy.
  JS failing-lazy restoration assertion #8 не имеет Rust аналога; не скрывается как проверенная.
  #12/#13: одна explicit definition/alias вместо import counts; сборщик не вызывает plugin apply.
- bundle-platform #1: env markers вместо kernel release; ограничение EvalEnv части A сохранено.

| source test file | перенесено | блокировано | не перенесено |
|---|---:|---:|---:|
| loader-volatile-update.spec.ts | 19 | 0 | 4 |
| cordis-entry-commit.spec.ts | 4 | 0 | 2 |
| cordis-entry-yaml.spec.ts | 4 | 0 | 1 |
| app-boot.spec.ts | 22 | 0 | 40 |
| config-reload.spec.ts | 7 | 0 | 0 |
| startup-diagnostics.spec.ts | 8 | 0 | 0 |
| profile-sanitize.spec.ts | 5 | 0 | 0 |
| config-dump.spec.ts | 5 | 0 | 1 |
| user-patches.spec.ts | 20 | 0 | 0 |
| profile.spec.ts | 14 | 0 | 17 |
| config-schema.spec.ts | 33 | 0 | 8 |
| schemastery-json-schema.spec.ts | 1 | 0 | 46 |
| loader-shape.compat.spec.ts | 0 | 0 | 1 |
| config-pattern.spec.ts | 0 | 0 | 3 |
| bundle-platform.spec.ts | 4 | 0 | 0 |

### Поимённый учёт исходных случаев

Нумерация внутри файла — порядок групп it/it.each в read-only source. Имена Rust — snake_case;
несколько параметров могут быть одним циклом, доступны дополнительные regressions.

#### scripts/loader-volatile-update.spec.ts

Перенос: tests/volatile_update.rs.

Перенесено:

- #1 `updates a whole-config reference and reports the root path`.
- #2 `commits all references before one local event without remounting`.
- #3 `keeps references on invalid updates and replaces them on mixed updates`.
- #4 `bypasses custom update hooks for volatile changes and retains them for mixed changes`.
- #5 `compares ordinary dates and arrays and retains the existing nonvolatile restart behavior`.
- #6 `resolves pending updates and creates fresh references after dependency replacement`.
- #9 `reloads real YAML and persists plain values independently of custom update hooks`.
- #10 `updates volatile children through Group and Include custom updates and saves ordinary config`.
- #11 `commits volatile edits in place and retains direct Fiber updates`.
- #12 `parses a volatile-only update through the config hook without patching the entry context`.
- #13 `keeps old references untouched when isolation replaces a dependency (available: %s)`.
- #14 `applies profile patches to a %s subtree without HMR and keeps mixed updates on the normal lifecycle`.
- #15 `logs an invalid candidate and keeps its references while other entries commit`.
- #16 `retains raw expression edits for dependency restoration (volatile: %s)`.
- #17 `remounts when changed expressions resolve to ordinary values different from the live config`.
- #20 `keeps an ordinary plugin mounted when an absent object becomes its explicit default`.
- #21 `keeps a noSave volatile value as the pending raw config across a no-op entry update`.
- #22 `isolates a failing notification listener and dispatches with a Context receiver`.

- #19 `remounts when an ordinary parent expression removes a live reference path`.

Не перенесено / блокировано:

- #7 `remounts for changed class instances and cyclic ordinary config` — Не перенесено: JS class/URL/cyclic values либо undefined в массиве не представимы JSON; исключения §8.2.
- #8 `distinguishes undefined from null inside volatile arrays` — Не перенесено: JS class/URL/cyclic values либо undefined в массиве не представимы JSON; исключения §8.2.
- #18 `compares ordinary URL transforms by value and remounts when an expression result changes` — Не перенесено: JS class/URL/cyclic values либо undefined в массиве не представимы JSON; исключения §8.2.
- #23 `remounts instead of committing when an ordinary field parses into a fresh class instance` — Не перенесено: JS class/URL/cyclic values либо undefined в массиве не представимы JSON; исключения §8.2.

#### scripts/cordis-entry-commit.spec.ts

Перенос: tests/entry_commit.rs.

Перенесено:

- #1 `reports assigned ids and applied options for create, update, move and remove`.
- #4 `mounts explicit ids containing the separator without nested-path resolution`.
- #5 `detaches file entries even with no patches`.
- #6 `clones inserted groups and lets later patches address their descendants`.

Не перенесено / блокировано:

- #2 `reports explicit Fiber saves and self-disposal but ignores child disposal` — Не перенесено: O4/O5: журнал/explicit Fiber save/adoption/move не переносится; self-disposal отдельно проверен startup_diagnostics.rs.
- #3 `keeps adopted entries and does not report file-driven removal or owner teardown` — Не перенесено: O4/O5: журнал/explicit Fiber save/adoption/move не переносится; self-disposal отдельно проверен startup_diagnostics.rs.

#### scripts/cordis-entry-yaml.spec.ts

Перенос: tests/entry_yaml.rs.

Перенесено:

- #1 `reads and writes !!js scalars as expression nodes`.
- #2 `reads binary integers as numbers and dates as strings`.
- #3 `writes repeated references in full`.
- #5 `locates parse errors`.

Не перенесено / блокировано:

- #4 `folds long scalars at the requested line width` — Не перенесено: §8.2: оформление long scalar с заданной line width исключено.

#### packages/boot/app-boot/tests/app-boot.spec.ts

Перенос: tests/app_boot.rs.

Перенесено:

- #34 `ignores active, disabled, and absent required entries`.
- #35 `warns once for optional import, apply, and dependency failures`.
- #36 `reports a throwing disabled expression on $id (required: $required)`.
- #37 `preserves nested activation causes and aggregate member failures`.
- #39 `combines required %s and optional failures without a separate warning`.
- #40 `omits an error cause when required plugins are only waiting for services`.
- #41 `keeps diagnostic metadata available without expanding it in ordinary error inspection`.
- #42 `retains nested and shared errors in a fatal diagnostic without duplicating them`.
- #43 `groups original failure stacks and pending services in one startup diagnostic`.
- #44 `rejects a required entry pending on an injected service`.
- #45 `loads expressions and rejects missing, malformed, non-array, and non-mapping overlays`.
- #46 `retains import errors and inactive-entry metadata after disposing the startup tree`.
- #47 `retains warnings and errors from asynchronous failed-startup cleanup`.
- #49 `boots a leaf config through the real Loader and settles the tree`.
- #51 `runs host preparation before the Loader tree mounts`.
- #52 `exposes dshHomePath and dshPath to Loader config expressions`.
- #53 `returns instead of asserting over a tree a surface disposed mid-startup`.
- #54 `returns when disposal completes before root entry creation returns`.
- #55 `keeps successful entries and warns about optional import, config, disabled, sync apply, async apply, and dependency failures`.
- #56 `rejects a %s root configuration`.
- #57 `disposes startup after a required %s failure`.
- #58 `disposes successful entries and rejects when a required entry fails`.

Не перенесено / блокировано:

- #1 `resolves relative to the given cwd outside replay mode` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #2 `swaps a cordis.yml/.yaml basename for cordis.snapshot.yml in replay mode` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #3 `leaves a non-cordis basename alone in replay mode and defaults cwd to the process cwd` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #4 `loads variables from .env in the given dir` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #5 `stays silent when no .env exists (ambient environment wins)` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #6 `warns (labelled, single line) when .env exists but cannot be loaded` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #7 `defaults dir to the process cwd and warn to a stderr write` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #8 `layers user under project under the inherited environment` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #9 `refuses to launch when a .env sets %s, before applying anything` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #10 `accepts the proxy names from the Harness-home .env, below an exported one` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #11 `accepts the DeepSeek endpoints from the Harness-home .env, below an exported one` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #12 `refuses %s from the Harness-home .env and names the exact spelling` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #13 `still refuses every other bootstrap name in the Harness-home .env: %s` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #14 `names the Harness-home file as the way out when a project .env sets %s` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #15 `treats the invoking directory as the Harness home when they are the same directory` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #16 `reports each file value with its absolute path` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #17 `resolves the harness home from the inherited environment, never from a file` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #18 `warns and continues when a layer exists but cannot be read` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #19 `reports to stderr when the caller supplies no reporter` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #20 `passes over an absent layer without reporting it` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #21 `carries only the inherited environment when neither file exists` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #22 `reads a harness home that is also the invocation directory exactly once` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #23 `writes one labelled line with the stack and exits 1 on an Error rejection` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #24 `stringifies a non-Error rejection and an Error without a stack falls back to its message` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #25 `returns an uninstaller that removes the handler (and defaults to the real process)` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #26 `does not report an activation rejection shared by entries in the boot audit` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #27 `awaits the release hook before exiting so the terminal owner can restore it` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #28 `still exits when the release hook rejects` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #29 `exits without waiting when a release hook never settles` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #30 `reports only the first rejection and keeps handling later ones during the release` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #31 `turns an uncaught exception into a labelled diagnostic with its enumerable properties and exits 1` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #32 `includes the cause chain in the diagnostic` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #33 `awaits the release hook for an uncaught exception as it does for a rejection` — Владелец: следующий task cmdline + apps/hoh (resolveConfigPath, loadEnv/loadLayeredEnv, installFailLoud и запреты bootstrap .env); прямое решение ведущего fix round B.
- #38 `describes nested, stackless, non-error, pending, and unexpected failures` — Не перенесено целиком: JS stack/non-Error/синтетические fiber состояния; Rust nested/aggregate причины проверены отдельными случаями.
- #48 `stops collecting startup diagnostics after a successful boot` — Не перенесено: подсчёт приватных JS logger exporters. Rust использует instance-owned buffer ядра, отдельного startup exporter нет.
- #50 `can resolve bare plugins from the harness when the config project shadows their package name` — Не перенесено: Node bare module resolution исключён O6.
- #59 `falls back to the deepest cause message when its stack was erased` — Не перенесено: JS host-preparation/stackless error adapter; §2.9 принимает готовый Context.
- #60 `reports a non-Error rejection from host preparation` — Не перенесено: non-Error rejection host preparation; typed Rust Error и готовый Context (§2.9).
- #61 `rejects and disposes when an error cause is cyclic (indirect: %s)` — Не перенесено: циклическая JS cause в host preparation; у boot §2.9 нет такого callback.
- #62 `expands a stackless aggregate at the deepest activation cause` — Не перенесено: stackless JS aggregate в host preparation; Rust агрегаты activation покрыты #37/#42.

#### packages/boot/app-boot/tests/config-reload.spec.ts

Перенос: tests/config_reload.rs.

Перенесено:

- #1 `writes and activates initial entries when an included file is missing`.
- #2 `keeps the last good tree instead of throwing, then applies the next valid edit`.
- #3 `re-applies entry patches and inserted entries on every re-read (parity with initial load)`.
- #4 `lets a later patch configure or disable a row an earlier patch inserted`.
- #5 `keeps siblings after a required-id %s failure during HMR`.
- #6 `keeps the previous fiber config after schema rejection and retries a valid edit`.
- #7 `lets a booted composition share one isolate realm across a group of rows`.

#### packages/boot/app-boot/tests/startup-diagnostics.spec.ts

Перенос: tests/startup_diagnostics.rs.

Перенесено:

- #1 `retains disabled and waiting entries after the real Loader has been disposed`.
- #2 `preserves the startup failure when the Loader is already unavailable to diagnostic capture`.
- #3 `identifies a registered provider that is isolated from the waiting consumer`.
- #4 `reports omitted plugin and missing-service counts instead of silently dropping rows`.
- #6 `retains the Loader disablement after disposal and reports missing source metadata`.
- #8 `exports the named types used by startup composition diagnostics`.

- #5 `reports a registered same-scope provider whose value is unavailable`.
- #7 `attributes a nested %s provider to its Loader entry`.

#### packages/boot/app-boot/tests/profile-sanitize.spec.ts

Перенос: tests/profile_sanitize.rs; #5 — unit src/profile_sanitize.rs.

Перенесено:

- #1 `recovers a profile without loading its broken plugins or patch`.
- #2 `preserves previous backups across retries and later recovery actions`.
- #3 `does not create an absent profile and backs up a patch even without a manifest`.
- #4 `leaves the patch untouched when profile JSON is invalid`.
- #5 `propagates backup failures before changing activation`.

#### packages/boot/app-boot/tests/config-dump.spec.ts

Перенос: tests/config_dump.rs.

Перенесено:

- #1 `composes overlay layers in order, prints !!js verbatim, and labels each section with its source and patches`.
- #2 `groups contiguous rows with the same origin and patches under one separator`.
- #3 `composes all layers as one flattened patch list, exactly like boot()`.
- #4 `reports a patch whose target row is absent through warn with its layer label and keeps composing`.
- #6 `fails loud on a missing, unparsable, or non-array base config`.

Не перенесено / блокировано:

- #5 `defaults its warn sink to one stderr line per skipped patch` — Не перенесено: Default stderr sink — код приложения; §2.9 требует переданный warn.

#### packages/boot/app-boot/tests/user-patches.spec.ts

Перенос: tests/user_patches.rs.

Перенесено:

- #1 `returns undefined when no user patch file exists`.
- #2 `parses a patch list and preserves !!js expressions as loader expression nodes`.
- #3 `loads absolute plugin paths from patch files as file URLs ($label)`.
- #4 `anchors inserted relative plugins to the patch file and keeps assertion names literal`.
- #5 `fails loud on an unreadable file (a present user patch layer is never skipped)`.
- #6 `fails loud on unparsable YAML and on a !!js tag with no expression body`.
- #7 `fails loud when the file is not a top-level array or an entry is not an object`.
- #8 `keeps Include's config literal — a nested row's !!js belongs to that row's fiber`.
- #9 `waits for row injections before resolving !!js and resolves again after provider replacement`.
- #10 `evaluates a !!js disabled expression against the loader context`.
- #11 `keeps the raw expression in the options so write-back preserves the !!js form`.
- #12 `re-evaluates when update() replaces the expression, mounting and unmounting`.
- #13 `retains unchanged import diagnostics across profile reconciliation`.
- #14 `rejects a context without the launcher root Include`.
- #15 `removes a previously failed entry without reporting its old activation error`.
- #16 `reports an unchanged failed entry as a warning but rejects a changed configuration with the same failure`.
- #17 `logs a superseded activation failure without failing removal`.
- #18 `waits for a removed plugin to release its resources`.
- #19 `applies id-targeted overrides, inserts, and interpolates !!js from the environment`.
- #20 `mounts no patch layer for an absent or empty user layer`.

#### packages/boot/app-boot/tests/profile.spec.ts

Перенос: tests/profile.rs.

Перенесено:

- #1 `skips an unsupported saved bundle before composing any runtime overrides and retains the selection`.
- #4 `composes current files from profile data and retains launch overlay and telemetry precedence`.
- #9 `loads an explicitly owned profile directory outside CLI discovery`.
- #10 `resolves each dsh.profile.bundles entry to its patch layer in order, plus the user layer`.
- #11 `applies a dsh.bundle.patch list in order, anchoring inserted paths beside each file`.
- #15 `skips a bundle with %s, retains selections, and retries it on reread`.
- #17 `still rejects invalid profile manifests and user patches`.
- #18 `applies layers over an empty root and reports skipped patches`.

- #3 `joins the home and rejects traversal-shaped names`.
- #5 `creates manifest, user patch layer, and pnpm workspace once, never overwriting`.
- #6 `writes and reads back, and fails loud on a broken manifest`.
- #12 `auto-initializes only shipped templates and fails loud otherwise`.
- #13 `normalizes only the exact installation-owned headless bundle tuple`.
- #14 `removes retired %s and keeps the rest of an application-owned manifest`.

Не перенесено / блокировано:

- #2 `resolves peers from each installation without sharing profile state` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #7 `prefers the installation anchor, falls back to the profile, and fails loud` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #8 `resolves a package whose exports map omits ./package.json` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #16 `skips a bundle whose own dsh peers are incompatible until the profile exempts that exact pair` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #19 `collects the app and bundle dependencies without writing profile packages` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #20 `keeps selected bundle closures profile-local without overriding installation packages` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #21 `discovers dependencies beside a symlinked bundle real path (packaged: %s)` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #22 `traverses every explicit bundle root even when a nested package has the same name` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #23 `resolves import-only exports from each package installation` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #24 `resolves conditional subpath exports through Node` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #25 `uses the legacy index fallback when a package has no exports or main` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #26 `uses Node legacy resolution for an extensionless main entry` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #27 `fails loud on a missing legacy main entry` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #28 `preserves native ESM export errors and null-map legacy resolution` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #29 `removes only the symlinks that point into .dsh-module-fallback and the directory itself` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #30 `removes the directory when the profile has no node_modules and keeps links whose target parent is gone` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.
- #31 `leaves a profile without the directory untouched` — Не перенесено: O6: npm installation/resolution/peer compatibility и link projection cleanup; profile metadata операции #3/#5/#6/#12–14 теперь перенесены.

#### packages/boot/app-boot/tests/config-schema.spec.ts

Перенос: tests/config_schema.rs.

Перенесено:

- #1 `owns ordered composition, skipped-bundle diagnostics, and runtime resolution without mutating layers`.
- #3 `serializes namespace and class Config exports, retaining descriptions, defaults, and shared refs`.
- #4 `includes anonymous, repeated, disabled, and conditional rows without reading their config values`.
- #5 `reports failed disabled declarations rather than equating discovery with bootability`.
- #6 `records dormant carriers without config as childless trees and names a missing config otherwise`.
- #8 `retains recursive references and restores serialization state after a failing lazy schema`.
- #11 `walks native groups but not ordinary plugin config arrays`.
- #12 `recognizes external canonical Include aliases and caches them per tree base`.
- #13 `recognizes an external canonical Group even when Include cannot resolve`.
- #14 `reports unknown tree-carrier identities instead of silently omitting their children`.
- #15 `reads literal includes using their directory, applies patches, and keeps shared includes independent`.
- #16 `expands initial entries in memory without writing missing include files`.
- #17 `reports cycles, missing files, and malformed documents without losing subsequent entries`.
- #18 `omits configuration snippets from invalid %s include diagnostics`.
- #19 `preserves non-missing include %s failures instead of using initial`.
- #20 `detects include cycles through directory symlinks`.
- #21 `marks a nonliteral or unsupported include incomplete: %j`.
- #22 `rejects dynamic include patch structure without printing its expression: %j`.
- #23 `preserves null group overrides and disabled/config expressions in include patches`.
- #24 `rejects malformed and dynamic child groups`.
- #25 `keeps valid siblings and original positions when composed rows are malformed`.
- #26 `keeps healthy group and include siblings after malformed child rows`.
- #27 `reports unavailable include children when composition fails`.
- #28 `does not validate contents of an insertion that Loader skips`.
- #29 `allows disabled config omission without disabling group:true or supplied-config validation`.
- #31 `does not retain a stale patch constraint after an invalid duplicate-id row`.
- #32 `validates known entry Config and whole-config patches while keeping metadata-only patches valid`.
- #33 `describes Loader metadata and anonymous insertion separately from config replacement`.
- #34 `allows config and disabled expressions but keeps Include and Group containers literal`.
- #35 `uses last root-tree ids without importing include-local ids into overlay targeting`.
- #36 `does not keep stale target constraints when an unmounted group-metadata child replaces an indexed id`.
- #40 `includes caller composition diagnostics without mutating them`.

- #39 `uses an unknown schema for an indexed target without a collected declaration`.

Не перенесено / блокировано:

- #2 `propagates resolution setup failure without starting collection` — Не перенесено: Node resolution setup, O6.
- #7 `distinguishes absent Config, unsupported validators, import failures, and Config getter failures` — Не перенесено: отсутствующий Config/JS getter/unsupported native validators; typed Plugin имеет Config, экспортирует schemars.
- #9 `does not invoke native validation or UID serialization hooks` — Не перенесено: native JS validation/UID serialization hooks; нет Schemastery, typed schema не исполняет config callbacks.
- #10 `reports non-Error lazy builder failures without losing other schemas` — Не перенесено: non-Error JS lazy builders; заменены schemars.
- #30 `repeats projection limitations for every entry sharing a partial Config` — Не перенесено: partial limitations проектора Schemastery; заменён schemars.
- #37 `keeps different resolutions of one plugin name explicit instead of selecting one arbitrarily` — Не перенесено: разные Node resolution одного specifier; явный PluginRegistry даёт одну definition, O6.
- #38 `keeps an %s alternate resolution open without weakening a known patch target` — Не перенесено: альтернативный Node resolution/unsupported native Config, O6/schemars.
- #41 `releases interception when the Node module loader is unavailable` — Не перенесено: освобождение Node loader interception, O6.

#### packages/boot/app-boot/tests/schemastery-json-schema.spec.ts

Перенос: tests/config_schema.rs.

Перенесено:

- #25 `accepts expressions at every config value but not dictionary keys`.

Не перенесено / блокировано:

- #1 `%s preserves omission and null acceptance` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #2 `derives required parent fields from fallback validation, not only meta.required` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #3 `preserves overlapping union acceptance instead of emitting oneOf` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #4 `keeps native container behavior for arrays, dictionaries, and tuples` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #5 `emits ordinary number, string, and const constraints` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #6 `marks unprojected behavior explicitly` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #7 `does not run transform callbacks and marks their validation partial` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #8 `does not emit unsafe flagless regex constraints: %s` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #9 `keeps transform-strict dictionaries separate from ordinary dictionaries` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #10 `permits expressions inside structured constants while rejecting non-null extra members` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #11 `matches native nullable constant object members: %s` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #12 `retains nested nullish constants and exact array length` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #13 `keeps inherited-key object constant behavior partial without rejecting valid defaults` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #14 `marks inherited intersect fallback acceptance unknown instead of optional` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #15 `widens ordered unions when failed branches can adapt the input` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #16 `does not mistake ordinary defaults for input writeback` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #17 `allows original dictionary values affected by %s key collisions` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #18 `keeps strict dictionary value assertions when every string key is accepted unchanged` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #19 `preserves cached and fresh lazy loose fallback, including missing input` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #20 `does not turn a failed shared traversal into a dangling recursive reference` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #21 `does not simulate shared lazy metadata changes or reject values they may enable` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #22 `keeps UTF-16 maximum length as a sound code-point upper bound` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #23 `keeps descendant limitations at their owning field` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #24 `preserves roles, units, presentation metadata, and localized descriptions` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #26 `exports finite recursive references without mutating native lazy schemas` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #27 `rejects lazy cycles without a concrete schema and reports builder failures` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #28 `reuses recursive targets reached more than once and keeps required null rejection` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #29 `preserves effective cached lazy metadata instead of applying later outer requiredness` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #30 `retains volatile metadata without changing JSON input types` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #31 `rejects volatile fields without a fixed, nonvolatile parent path` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #32 `uses an already resolved lazy inner schema without invoking its builder again` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #33 `reports an incomplete native %s declaration` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #34 `supports empty and optional tuples and unrestricted object declarations` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #35 `preserves descriptions without an English entry: %j` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #36 `omits non-finite %s %d without losing type constraints` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #37 `treats native no-op infinite bounds as absent unless a step origin depends on them` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #38 `keeps anyOf when an earlier primitive transform branch cannot write back into the input` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #39 `does not duplicate null in the type list of a wrapped null-only projection` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #40 `invokes a shared lazy builder once across chains within one document` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #41 `omits cyclic default annotations while retaining the schema` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #42 `has deterministic output independent of native allocation ids` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #43 `omits non-JSON defaults with an explicit partial annotation` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #44 `keeps valid defaults and input precision when presentation metadata is omitted` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #45 `reports non-Error %s annotation failures without losing the document` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #46 `omits unrepresentable localized descriptions without executing their callbacks` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.
- #47 `retains sibling constraints when a nested default or constant is not JSON-compatible` — Не перенесено: §8.3: проектор Schemastery заменён schemars; специфические native/lazy/metadata/prototype/transform/regex правила не переносились. Recursive refs/defaults/volatile input проверяются на Rust schema в config_schema.rs и в ядре.

#### packages/boot/app-boot/tests/loader-shape.compat.spec.ts

Перенос: tests/config_schema.rs (учёт исключения здесь).

Не перенесено / блокировано:

- #1 `tags the running Node loader with the resolver signature that runtime accepts` — Не перенесено: Node internal loader version/signature и HMR; O6/PORTING §8, нет Node module loader в HoH.

#### packages/boot/app-boot/tests/config-pattern.spec.ts

Перенос: tests/config_pattern.rs.

Не перенесено / блокировано:

- #1 `%s with flags %s has portability %s` — Не перенесено: Schemastery RegExp portability projector заменён schemars (§8.3); 25 таблиц параметризованного случая не про Expr.
- #2 `uses native flagless semantics when flags are absent` — Не перенесено: Schemastery RegExp portability projector заменён schemars (§8.3); 25 таблиц параметризованного случая не про Expr.
- #3 `does not hide unexpected parser failures` — Не перенесено: Schemastery RegExp portability projector заменён schemars (§8.3); 25 таблиц параметризованного случая не про Expr.

#### packages/boot/app-boot/tests/bundle-platform.spec.ts

Перенос: tests/bundle_platform.rs.

Перенесено:

- #1 `admits WSL independently of ordinary Linux and native Windows`.
- #2 `allows undeclared platforms and an explicit %s Host`.
- #3 `rejects macOS-only bundles on %s Hosts`.
- #4 `rejects malformed platform metadata $platforms`.

## Не проверено

Нативные macOS/Windows filesystem/file-URL ветки не запускались; все проверки выполнены на Linux.
Platform choices/Win32 expression paths проверены с внедрённым EvalEnv. WSL только по release без env markers
не поддержан текущим EvalEnv. Windows lock probing общего hoh-atomic-write ждёт этапа 5, как записано в его
PORT_NOTES.md; loader не содержит собственной платформенной копии. Kernel правила R63–R70 пятого круга
прочитаны: literal labels совместимы с impl Into<Arc<str>>, новые contained/child API loader не требует.

## Выражения вне грамматики

Во всех 621 shipped YAML (10 bundles/presets + 611 snapshots) и 76 выражениях каталога §5.5 новых форм нет.
Внешние JS test-fixtures не объявлены частью shipped grammar; их адаптации перечислены выше.

## Лицензии

В licenses/ неизменённые MIT LICENSE vendor/{loader,include,group}. У packages/boot/config-editor нет отдельного
LICENSE в каталоге, locale/*.json скопированы побайтно (8 языков). 621 YAML тоже сравнены побайтно с source.
Новые сторонние зависимости не устанавливались; private atomic-write copy удалена в пользу принятого crate.

## Fix round B: двенадцать замечаний и четыре исключения

Сначала добавлены tests/review_fixes.rs (16 первоначальных regressions) и пять ConfigEditor regressions.
Первый cargo test --no-fail-fast дал **16/16 FAILED + 5/5 FAILED**, а Context-only gap дал providers=0 вместо 1;
оба R54 воспроизведения уже прошли на kernel sixth round. Затем исправления и повторные проверки.
Три дополнительных span cases сначала упали: block scalar склеивал следующую строку, empty config давал
обратный range, marker map не становился !!js; соответствующие regressions сохранены.
Ещё два span regressions сначала упали: flow-root conversion терял комментарии между элементами,
а выбор строки по alias вместо anchor declaration мешал точечному изменению config; оба исправлены.

1. Ancestor disabled: runtime::Group::disabled проходит owner chain и вычисляет условия; group сама enabled.
   Toggle value/expression проверен вместе с сохранением group UID и dispose/recreate child.
   Source vendor/loader/src/config/entry.ts:75–85.
2. ConfigEditor: отдельный span editor; полные expected files для set/add/remove config и delete row,
   комментарий, UTF-8, anchor+alias, flow map, relative insert name не теряются. Rollback error приоритетен.
   Source packages/boot/config-editor/src/index.ts:109–146; DEPS.md §5.5 №2 (variant a).
3. Root failures идут через bootstrap Include → StartupError. ConfigFile сохраняет read/parse/validate stage
   и cause, missing пишет config file not found, non-array — config file must be a top-level array.
   Прочие boot failures имеют prefix hoh: plugin tree failed to load. app_boot #56 проверяет три исходных
   сообщения, review_fixes дополнительно сравнивает полные missing/non-array diagnostics.
4. Initial получает id до записи; JSON без trailing LF, YAML с id; сравниваются полные файлы.
5. Include internal/update применяет patches к cache.data; recompose берёт тот же сохранённый base.
   Только refresh ставит explicit read request. Файл на диске меняется, но виден лишь после refresh.
6. Все introduced failures собираются в один source activationDiagnostic header; неизменённые — warnings,
   Pending остаётся quiescent и не introduced, как index.ts:412–419.
7. Failed import diagnostic имеет phase failed to import; underlying registry reason сохранён.
8. Invalid YAML include diagnostics содержат line/column без snippets; source collect.ts:148–156.
9. entry.description, document.description и $comment скопированы из document.ts:130–160.
10. sanitize использует общий внутри loader read_profile_manifest: exact bin/path/read texts, parse error
    без bin prefix; with_bundles воспроизводит object spread строк/массивов/primitive metadata.
11. Volatile shortcut разрешён только при единственном изменённом поле config; disabled false→null
    вместе с config идёт в ordinary update.
12. Start reconciliation сортирует canonical uint32 array indices (кроме 4294967295) численно перед строками;
    document entries остаются в порядке документа. Проверен apply order 2,10,01,last.

Исключения:
- startup-diagnostics #5/#7 перенесены, owner/visibility assertions сохранены с typed Arc<()> и отдельным
  отсутствующим сервисом; unavailable typed provision держится Loading (R26), а не JS active/undefined.
- app-boot #1–33: следующий task cmdline/apps/hoh, по прямому решению ведущего, не необъяснённые exclusions.
- profile #3/#5/#6/#12–14 и config-schema #39 перенесены. Добавлены profile_files и standalone document
  builder; O6 остаётся только для npm/module resolution/installation/compatibility/link projections.
- Duplicate id между разными Group одного tree: source EntryGroup.create переиспользует entry/fiber и меняет
  parent (vendor/loader/src/config/group.ts:20–27, особенно :22/:25); port пересоздаёт fiber при смене owner,
  потому что Context имеет immutable parent (O3/L13). Это намеренное отличие. Перенесённые source cases
  не наблюдают этот move; явные move/adoption cases исключены O4/O5 и не заявляются как пройденные.

Дополнительные API и ограничения:
- resolve_profile_dir/init_profile/read_profile_manifest/write_profile_manifest/load_profile получают home
  явно, load_profile — BundleSource вместо installAnchor. Все пять shipped templates и точный старый
  headless tuple перенесены; прочие tuples не переписываются. Profile::load удаляет только два RETIRED_BUNDLES.
  Новый regression relative home сначала FAILED; load_profile теперь разрешает его через EvalEnv.cwd,
  как Profile::load, и не читает действительный process cwd.
- build_config_schema_document принимает ConfigSchemaEntry, root targets и готовую карту schemars
  definitions вместо native Schemastery projection; несуществующая собранная декларация получает unknownConfig.
- В read_profile_manifest локальный read-error adapter воспроизводит Node ENOENT/EACCES/ENOTDIR/EISDIR;
  hoh-error не разрешён как новая прямая зависимость этим заданием. Неизвестный OS error сохраняет свой Display.
- String spread требует представимых UTF-16 unit values: surrogate property values невозможно выразить
  serde_json::Value, поэтому такой metadata input явно отвергается, как прочие unrepresentable JS values.
- После R54 fixture Nested явно inline (#[schemars(schema_with = "inline_nested_schema")]), как source
  scripts/volatile-config.fixture.ts:12 z.object(...).default({}). Default остаётся на непосредственном
  object schema, assertions четырёх прежних тестов сохранены. Nonrecursive $ref sibling default — отдельный
  kernel gap, воспроизведён и оставлен ignored, без обхода в production loader.
  Ordinary-default source case теперь проверяет ordinary nested.fixed, его default и реальный remount,
  вместо прежнего более слабого volatile nested.count. Kernel не изменялся.

## Проверка

Все команды: CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-l1.

- cargo test -p hoh-loader — exit 0; **270 passed, 0 failed, 1 ignored** (19 unit + 251 integration;
  doctests: 0). Единственный ignore — новый минимальный R54 $ref/default gap ниже; три прежних включены.
- cargo clippy -p hoh-loader --all-targets -- -D warnings — exit 0, без diagnostics.
- cargo fmt -p hoh-loader -- --check — exit 0, чисто.

## Вопросы ведущему

Исходные R54 и L19 закрыты шестым кругом kernel: два первоначальных tests/kernel_gaps.rs больше не ignored,
полный volatile regression тоже включён; capture_with_snapshot удалён, Context читает reflect.store.
Новый **R54** gap: default у нерекурсивного $ref теряется при переходе к definition. Минимум — object
properties.nested={$ref:'#/$defs/Nested',default:{}}, $defs.Nested={type:'object',properties:{count:{type:'number','x-volatile':true}}};
old={}, next={nested:{count:1}}. Ожидается true, получается false. Test
r54_preserves_a_default_on_a_nonrecursive_object_reference сначала FAILED, теперь ignored по правилу brief.
Kernel не редактировался. Source fixtures проверены inline, как непосредственно объявленные source z.object.
Остались прежние вопросы A о WSL kernel release в EvalEnv и NaN/Infinity вне serde_json::Value.
# Stage0 launcher API supplement

The latest task's API/test accounting is appended at the end of this document.

## Stage0 launcher и API для hoh-settings (18.11.2026)

cmdline сохранён модулем hoh-loader (MAP row3); hoh-commander не зависит от
loader, поэтому отдельный hoh-cmdline не нужен. apps/hoh владеет native process
адаптером. Весь список app-boot #1–33 выше теперь перенесён в
`tests/support/launcher_helpers.rs` (подмодуль tests/app_boot.rs): 3 path,
4 loadEnv, 15 layeredEnv/bootstrap prohibitions, 11 failLoud groups. Предыдущая
надпись «следующий task» для этих 33 cases заменяется этой записью.
Явные cwd/home/map/warn/event hooks заменяют ambient process defaults; source
case default paths используют те же переданные host facts. Ни один тест не
читает реальный home или environment. Тексты bootstrap guards до байта;
read failures используют hoh-error Node fs formatter. Proxy/endpoint provenance
через принятый hoh-launch-environment, без std::env::set_var (unsafe в edition2024).

### Публичные API для w5

| Потребность | API |
|---|---|
| Отказ после reload, внутри file/serial transaction, до write | `ConfigEditor::try_edit(&EntryInfo, FnOnce(Value, Value) -> Future<Output=Result<Value, hoh_error::BoxError>>) -> Result<(), BoxError>` |
| Schema реального definition, проверка stale uid | `ConfigEditor::schema(&EntryInfo)`, `Loader::entry_schema(&EntryInfo) -> Result<Option<Value>, LoadError>` |
| Interpolate в actual entry config context | `Loader::interpolate_entry(&EntryInfo, &Value) -> Result<Value, LoadError>` |
| Inherited defaults/validation без candidate hooks | `ConfigEditor::resolve_inherited` / `Loader::resolve_inherited(&EntryInfo, &Value) -> async Result<Value, LoadError>` |
| Active profile facts | `ConfigEditor::home() -> &Path`, `profile_name() -> &str`; `ProfileContextKey` (`profileContext`) / immutable service `ProfileContext {name, dir, home}` |

Source: config-editor/src/index.ts:86–146; settings/settings/src/index.ts:213–219,
224,241–257,296–324,391–429; vendor/loader/src/config/entry.ts interpolation in
fiber context. try_edit forwards the original boxed callback error unchanged,
including downcast type and pointer; old infallible edit remains compatible.
NoConfig represents a native plugin without Config export (schema returns None).
The registry exposes its actual native Config schema, no reconstructed settings
validator. Interpolation uses the actual context captured by ConfigResolver,
including injected service visibility and entry realms.

Kernel codec is public through Fiber::resolve_config, which also runs a fiber's
candidate hooks; PluginDef does not expose its codec separately. Therefore the
loader uses the actual registered definition in a private kernel with one
permanently pending fiber (unprovided extra required injection), then calls that
codec after interpolation in the real context. No apply, no service effects,
no candidate hooks; the active kernel is unchanged. Awaited shutdown on success,
synchronously enqueued shutdown on cancellation. Invalid/incomplete inherited
config falls back to interpolated raw per source inheritedConfig; interpolation
errors still propagate. Async API is the native adaptation; no kernel edits or
second schema validator. Tests cover codec defaults, missing required fallback,
original error identity, no apply/hooks, active-tree isolation and private cleanup.
ProfileContext is the source subset needed by settings, not npm install/runtime
fields excluded by O6. Launcher supplies the exact profile name and explicit home.
ProfileContextKey exposes its name/dir/home as JSON for the shipped ctx.get(...)
expressions. The entry owns its configuration-context capture; Resolver retains
only a Weak handle, preventing a fiber/resolver/context ownership cycle. A
dedicated unit case checks that context tags release after entry removal.

### cmdline tests

20 source groups from packages/boot/cmdline/tests/cmdline.spec.ts in tests/cmdline.rs.
The source's object-form inject group is excluded by binding L4 (lists only);
a native rejection test explicitly records that decision. Other groups preserve
immutable argv, multiple independent parsers, child actions/help, typed errors,
owned EOF/readiness subscriptions, cancellation/microtasks and byte-preserving
transport notification without consuming stdin. Whole CLI source-case/file
accounting is in apps/hoh/PORT_NOTES.md and SOURCE_TEST_CASES.md.

### Отличия

Native process defaults are explicit injected host facts. FailLoudReason uses
native Error Debug/source with an injectable host inspection string rather than
JS util.inspect properties. Native host event bridge replaces JS global hooks;
assembled rejection identity is scoped by an owned guard. No grammar widening:
L1 reserved expression markers, L4 array injections, L2/L18 YAML writer remain
binding. CLI golden schema differences are precisely listed in apps/hoh notes.

## Проверка ведущего 19.11: исправление девяти падений

- `config_schema`: относительные имена строк из Include разрешаются относительно
  каталога читаемого файла, включая `initial`; исходное имя в опубликованной
  декларации сохраняется. Источник: app-boot/src/config-schema/collect.ts:125–168,
  особенно передача нового baseUrl при walk(children). Исходный acceptance-тест
  canonical Include/re-export alias сохранён без ослабления.
- `format_activation_error`: отдельные visited и rendered множества. Разные
  vtable при upcast одного error больше не дублируют его сообщение; обход
  aggregate/source продолжается, одинаковые тексты разных объектов сохраняются.
- По запросу ведущего о memory-mcp-configs добавлен **один фиксированный терм**,
  отсутствующий в исходном §5.5:
  `process.env.MEMORY_FILE_PATH?.trim() || process.getBuiltinModule('node:path').join(process.getBuiltinModule('node:os').homedir(), '.dsh-mcp-reference-memory.jsonl')`.
  Источник: apps/cli/config/examples/mcp-memory/mcp-reference-memory.cordis.yml:12–13.
  Это явное дополнение к спецификации для данного примера, а не общий `.trim()`,
  OS import или исполнение JS. Исходные YAML сохранены без изменений. Trim — JS;
  OS home берётся только из EvalEnv::var HOME / USERPROFILE, отдельно от DSH home.
  При отсутствии обоих нужного override и OS home возвращается positioned error;
  системная passwd/Windows lookup не делается. Есть проверки Linux/Windows,
  whitespace override, отличия OS home от DSH home и отказа прочим JS-методам.
- Cancellation-тест исправляет ошибочный setup по **R15**: uid 0 всегда Active.
  Проверяются реальный disposable child, завершение его эффекта, Closed ядра и
  неизменность Active корня; проверка отмены не заменена ожиданием явного close.
  Guard по-прежнему синхронно ставит shutdown в очередь в Drop.


