# hoh-commander ← commander @ v15.0.0; deepseek-harness @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| commander/lib/command.js | src/command.rs | синхронный разбор, дерево команд, actions, вывод и перехват выхода в используемой части |
| commander/lib/option.js | src/option.rs | Opt, флаги, атрибуты, default/preset, choices, parser, mandatory, hideHelp |
| commander/lib/argument.js | src/argument.rs | Argument, обязательность, variadic, default, choices, parser |
| commander/lib/help.js | src/help.rs | стандартный formatter без терминала: usage, секции, выравнивание, переносы, аннотации |
| commander/lib/error.js | src/error.rs | CommanderError, InvalidArgumentError |
| commander/lib/suggestSimilar.js | src/suggest_similar.rs | UTF-16 edit distance, transposition, пороги, несколько подсказок |
| commander/LICENSE | LICENSE | MIT, скопирован без изменений |
| commander/tests/*.test.{js,cjs,mjs} | tests/*.rs, unit tests в src/command.rs и src/help.rs | полный построчный учёт ниже и в SOURCE_TESTS.md |
| deepseek-harness/apps/cli/src/args.ts | tests/support/programs.rs: dsh, plugin | декларации, тексты, parsers и проверки launcher; версия фикстуры 1.2.3 |
| deepseek-harness/packages/bundle/headless/src/startup.ts | tests/support/programs.rs: headless | декларация, тексты, choices ask/never, collector |
| deepseek-harness/packages/bundle/sdk-app/src/index.ts | tests/support/programs.rs: sdk | декларация и тексты стандартного профиля sdk |
| deepseek-harness/packages/bundle/acp-app/src/index.ts | tests/support/programs.rs: acp | декларация и тексты |
| deepseek-harness/packages/bundle/web-app/src/startup.ts | tests/support/programs.rs: web | декларация и тексты |
| deepseek-harness/packages/api/gateway/src/index.ts | отсутствует CLI | в указанной ревизии файл не импортирует commander и не объявляет программу |
| deepseek-harness/packages/boot/cmdline/src/index.ts, tests/cmdline.spec.ts | tests/command.rs, tests/repeated_parse.rs, tests/programs.rs | проверены перехват, writers, snapshot аргументов, action и отличие произвольной ошибки от commander exit; Cordis/Loader/EOF вне этого crate |
| plans/briefs/fix-stage1-n1-commander.md | tests/fix_round.rs, изменения реализации и исходных тестов | выполнены пункты 1–13; oracle ведущего не изменён |

До реализации насчитаны **23 различных метода commander**, вызываемых семью указанными
файлами: name, description, usage, version, option, requiredOption, addOption, choices,
argument, command, action, helpOption, helpCommand, addHelpText, allowUnknownOption,
passThroughOptions, enablePositionalOptions, exitOverride, configureOutput, parse, error,
opts, help. Дополнительно используются args, commands и _actionHandler; последний доступен
как has_action (только команда) и has_action_recursive (дерево). В headless также заменяется
сам error: этому соответствует error_handler, проверенный и для грамматической ошибки.
Вызовы default в gateway и sdk принадлежат схемам, а не commander; default всё равно
перенесён по прямому требованию задания.

Это область **переносимых сейчас потребителей**, а не всех импортов commander в dsh.
Ещё два потребителя пока вне переноса по MAP.tsv:
apps/plugin-market-generator (skip) использует new Command('plugin-market-generator') и
showHelpAfterError('(run with --help for usage)'). При его переносе нужен name(...) вместо
конструктора с именем и поддержка showHelpAfterError (включая наследование настройки
подкомандами при их регистрации). packages/experimental/cua-mcp (later) использует
new Command(commandName): требуется name(commandName) или отдельный именованный конструктор;
остальные его вызовы commander уже поддержаны. Сейчас оба пакета не портируются.

Шесть реальных деклараций — dsh, его plugin, headless, sdk, acp, web. Все builders доступны
из tests/support/programs.rs для повторного использования другим integration test.
У launcher передача профиля позиционным сокращением — отдельное преобразование перед parse
в args.ts; builder принимает уже расширенный --profile. Для каждого builder проверены
help, все объявленные флаги, успешный и неуспешный разбор. App lifecycle, публикация сервисов,
проверки сети/TLS URL и чтение stdin не являются поведением commander и фикстурой не запускаются.

## Отличия от образца

- Rust не поддерживает перегрузки/default arguments. Command setters возвращают &mut Self,
  Opt/Argument modifiers возвращают владение. Имена методов — snake_case.
  Getter name/description/usage называется get_name/get_description/get_usage.
  new Command() соответствует Command::new(); имя задаётся name.
- Перегрузки option выражены через option, option_with_default, option_with_parser,
  option_with_parser_and_default; requiredOption с parser — required_option_with_parser.
  Полная конфигурация также выражается add_option(Opt::new(...)).
  version с флагами и описанием — version_with_flags; helpOption(false/true) —
  help_option_enabled; строковая форма — help_option.
  helpCommand с сигнатурой — help_command_with_description, булева — help_command.
  Необязательное описание default задаётся default_description.
  error с настройками — error_with_options; help с error:true — help_with_error.
  Булевы настройки принимают явный bool.
- Публичные адаптеры Options = IndexMap<String, Value>, Output, ParseOptions::USER,
  ValueParser и ParseError добавлены для Rust. Output::new задаёт оба writer; stdout/stderr
  по умолчанию пишутся только этими writer. Парсер принимает последовательность пользовательских
  строк и явный ParseOptions::USER, эквивалент from:'user'; process.argv не читается.
- processed_args и аргументы action имеют тип Vec<Option<Value>>: None — undefined,
  Some(Value::Null) — null, variadic без значений — Some([]). opts() возвращает ссылку на
  локальный упорядоченный набор JSON-значений; отсутствующая опция не добавляется.
  callback parser получает raw &str и Option<&Value> предыдущего значения. Для булевого
  флага без operand raw равен строке "undefined": в используемом harness collector этот
  параметр отсутствующего значения не читает; optional без значения вообще не вызывает parser.
  Типизированные preset сохраняются при встроенных choices/variadic.
- Action — Fn(processed_args, options, &mut Command) -> Result<(), ParseError>.
  Снимки первых двух параметров позволяют вызвать error/help у команды без конфликта заимствований.
  Все сохраняемые closures — Send + Sync и хранятся в Arc; захваченное изменяемое состояние
  можно хранить через Arc/Mutex. Command, Opt, Argument, Output, ParseError и CommanderError
  проверены на Send. ParseError::Commander хранит CommanderError,
  ParseError::Other сохраняет исходный Box<dyn Error + Send + Sync>; доступны other и commander.
  Типы ошибок используют thiserror::Error.
  JS this заменён явным параметром команды. Поля ошибок exit_code, code, message публичны.
- error/help возвращают CommanderError; вызывающий action передаёт его через Err(...into()).
  parse возвращает Result<&mut Command, ParseError>. error/error_with_options/help/help_with_error
  помечены #[must_use] с требованием вернуть ошибку из action.
  Для exit_override и exit_callback действует последний вызов: callback после override
  снова включает callback, override после callback его подавляет.
  Без callback библиотека только возвращает ошибку. Она никогда не завершает процесс.
  error_handler заменяет стандартный error, включая его вывод и код, как headless в JSON-режиме.
- Ошибки определения builder (плохие флаги, конфликт регистрации, variadic не последним,
  неиспользуемый default обязательного argument, некорректная позиция help text) возвращаются
  как ParseError::Other при parse, а не бросаются при построении. Тексты сохраняются.
  Каждая команда сохраняет первую ошибку. Перед любым разбором проверяется всё дерево:
  собственная ошибка, затем подкоманды в порядке регистрации с обходом в глубину.
  Если ошибочны и родитель, и ребёнок, JS сообщает ошибку первого по времени builder call,
  Rust — родителя. Это отличие необходимо для chaining без panic и Result у каждого setter.
  pass_through_options проверяет настройку родителя в момент вызова; включение positional
  у родителя уведомляет уже созданные дочерние команды, поздние дети получают текущий снимок.
- Output::out_help_width/err_help_width задают независимые ширины stdout/stderr.
  Output::default на Unix читает число колонок каждого терминального потока через безопасный
  rustix::termios::tcgetwinsize; у нетерминальных потоков и на остальных платформах ширины
  остаются unset. Fallback — **80**, minWidthToWrap — **40**. Цвет не определяется.
  Длины и edit distance считаются в UTF-16; пробелы help используют точное множество JS \\s
  (U+FEFF включён, U+0085 исключён). JSON-аннотации и преобразование значений используют
  hoh_util_values::js_json_stringify/js_string; ручная сериализация удалена.
  В fix brief разрешены hoh-util-values, thiserror и Unix-зависимость rustix с feature termios.
- suggestSimilar сортирует равные кандидаты в порядке, соответствующем используемым
  ASCII-именам. Общий ICU localeCompare для произвольных Unicode-команд не воспроизведён:
  в commander это единственное используемое место localeCompare. Настраиваемые сортировки
  Help, также использующие localeCompare, исключены как неиспользуемые.

## Не перенесено

Ни один из следующих API не вызывается переносимыми сейчас программами; исключения тестов помечены
индивидуально в [SOURCE_TESTS.md](SOURCE_TESTS.md).

- Исполняемые подкоманды, поиск executable, executableDir, inspector-port, сигналы дочерним
  процессам, executableFile, platform-specific запуск Node.
- Default subcommands (isDefault), wildcard/* команды, aliases/alias, summary, скрытые команды.
- addCommand для внешнего Command; createCommand/createOption/createArgument/createHelp,
  расширение классов и замена фабрик; singleton program и глобальное состояние npm-модуля.
- EventEmitter, option/command/help события, command:* listeners и lifecycle hooks.
- env для опций, conflicts, implies и внутренний DualOptions для этих правил.
- storeOptionsAsProperties; optsWithGlobals; публичные get/setOptionValue, source tracking,
  saveStateBeforeParse/restoreStateBeforeParse как точки расширения. Внутренний reset состояния
  и ленивый reset подкоманд при повторном parse перенесены.
- parseAsync и асинхронные actions; соглашения argv node/electron/eval и их autodetect,
  nameFromFilename и чтение настоящего process.argv.
- exitOverride(fn) с пользовательским обработчиком; process.exitCode как глобальное состояние.
  Сам процесс завершает только код приложения через явный callback.
- addHelpOption/addHelpCommand с пользовательским объектом; deprecated addHelpCommand overloads,
  arguments(), legacy descriptions, noHelp, RegExp вместо parser.
- configureHelp, пользовательские Help subclasses/методы/стили, sortOptions/sortSubcommands,
  showGlobalOptions, option/command/help groups и заголовки групп.
  Встроенные алгоритмы Help перенесены как закрытые функции и проверены unit tests.
- Пользовательские callback getOut/ErrHelpWidth (вместо них ширины задаются Output builder),
  getOut/ErrHasColors, FORCE_COLOR/NO_COLOR,
  stripColor replacement, outputError и getters configureOutput.
- showHelpAfterError и наследование этой настройки пока не нужны переносимым потребителям;
  они понадобятся для исключённого plugin-market-generator, как описано выше.
  showSuggestionAfterError как настройка не используется; стандартные suggestions включены.
- Функциональный addHelpText с JS context; deprecated callback help/outputHelp.
  Все четыре строковые позиции before/beforeAll/after/afterAll перенесены.
- Возможность бросить произвольное значение из value parser: по контракту задания parser
  возвращает только Value или InvalidArgumentError. Для actions произвольная типизированная
  ошибка поддержана; нет смысла имитировать бросание JS примитива или проверку instanceof.
- npm imports, CommonJS/ESM/package exports, TS type checks, Node-specific fixtures/test mocks.

Учёт тестов: **612 перенесённых деклараций test(), 505 исключённых** из
1117 в 109 исходных файлах. Это не число развёрнутых JS forEach:
например, отрицательные числа проверяются всей матрицей из 119 сочетаний.
Одна табличная Rust-проверка может покрывать несколько исходных деклараций.
Динамические варианты deprecated arguments() исключены, остальные варианты сохранены.
Полные имена случаев в snake_case, строки источника и причины каждого исключения перечислены
в SOURCE_TESTS.md. Сводка по каждому файлу:

| исходный файл commander/tests/ | перенесено | исключено |
| --- | ---: | ---: |
| args.literal.test.js | 2 | 0 |
| args.variadic.test.js | 7 | 0 |
| argument.chain.test.js | 0 | 5 |
| argument.choices.test.js | 5 | 0 |
| argument.custom-processing.test.js | 16 | 1 |
| argument.required.test.js | 5 | 0 |
| argument.variadic.test.js | 11 | 0 |
| command.action.test.js | 9 | 2 |
| command.addCommand.test.js | 0 | 4 |
| command.addHelpOption.test.js | 0 | 3 |
| command.addHelpText.test.js | 12 | 7 |
| command.alias.test.js | 0 | 13 |
| command.allowExcessArguments.test.js | 10 | 0 |
| command.allowUnknownOption.test.js | 6 | 0 |
| command.argumentVariations.test.js | 7 | 1 |
| command.asterisk.test.js | 0 | 11 |
| command.chain.test.js | 22 | 22 |
| command.commandHelp.test.js | 2 | 0 |
| command.configureHelp.test.js | 0 | 4 |
| command.configureOutput.test.js | 11 | 10 |
| command.copySettings.test.js | 10 | 3 |
| command.createArgument.test.js | 0 | 3 |
| command.createHelp.test.js | 0 | 1 |
| command.createOption.test.js | 0 | 4 |
| command.default.test.js | 0 | 9 |
| command.description.test.js | 1 | 0 |
| command.error.test.js | 4 | 0 |
| command.executableSubcommand.inspect.test.js | 0 | 8 |
| command.executableSubcommand.lookup.test.js | 0 | 11 |
| command.executableSubcommand.mock.test.cjs | 0 | 2 |
| command.executableSubcommand.search.test.cjs | 0 | 18 |
| command.executableSubcommand.signals.test.js | 0 | 4 |
| command.executableSubcommand.test.js | 0 | 1 |
| command.exitOverride.test.js | 16 | 5 |
| command.help.test.js | 22 | 8 |
| command.helpCommand.test.js | 13 | 1 |
| command.helpOption.test.js | 13 | 0 |
| command.hook.test.js | 0 | 27 |
| command.name.test.js | 4 | 4 |
| command.nested.test.js | 1 | 0 |
| command.onCommand.test.js | 0 | 3 |
| command.option-misuse.test.js | 0 | 2 |
| command.parse.test.js | 17 | 17 |
| command.parseOptions.test.js | 32 | 1 |
| command.positionalOptions.test.js | 35 | 10 |
| command.registerClash.test.js | 1 | 5 |
| command.showHelpAfterError.test.js | 0 | 8 |
| command.showSuggestionAfterError.test.js | 0 | 4 |
| command.summary.test.js | 0 | 1 |
| command.unknownCommand.test.js | 4 | 1 |
| command.unknownOption.test.js | 6 | 0 |
| command.usage.test.js | 10 | 0 |
| commander.storeOptionsAsProperties.test.js | 0 | 9 |
| createCommand.test.js | 0 | 3 |
| deprecated.test.js | 0 | 3 |
| help.argumentDescription.test.js | 5 | 0 |
| help.boxWrap.test.js | 14 | 1 |
| help.commandTerm.test.js | 3 | 2 |
| help.commandUsage.test.js | 4 | 1 |
| help.longestArgumentTermLength.test.js | 3 | 0 |
| help.longestCommandTermLength.test.js | 5 | 0 |
| help.longestOptionTermLength.test.js | 3 | 0 |
| help.minWidthToWrap.test.js | 0 | 3 |
| help.optionDescription.test.js | 9 | 1 |
| help.optionTerm.test.js | 4 | 0 |
| help.padWidth.test.js | 3 | 1 |
| help.preformatted.test.js | 9 | 0 |
| help.showGlobalOptions.test.js | 0 | 5 |
| help.sortCommands.test.js | 0 | 2 |
| help.sortOptions.test.js | 0 | 6 |
| help.style.test.js | 0 | 17 |
| help.subcommandDescription.test.js | 2 | 2 |
| help.suggestion.test.js | 14 | 5 |
| help.visibleArguments.test.js | 4 | 2 |
| help.visibleCommands.test.js | 2 | 1 |
| help.visibleGlobalOptions.test.js | 0 | 4 |
| help.visibleOptions.test.js | 8 | 0 |
| helpGroup.test.js | 0 | 27 |
| imports.test.cjs | 0 | 22 |
| imports.test.mjs | 0 | 11 |
| incrementNodeInspectorPort.test.cjs | 0 | 12 |
| negatives.test.js | 10 | 2 |
| option.bad-flags.test.js | 2 | 0 |
| option.chain.test.js | 0 | 8 |
| options.bool.combo.test.js | 21 | 0 |
| options.bool.small.combined.test.js | 1 | 0 |
| options.bool.test.js | 13 | 0 |
| options.camelcase.test.js | 6 | 0 |
| options.choices.test.js | 5 | 0 |
| options.conflicts.test.js | 0 | 21 |
| options.custom-processing.test.js | 13 | 0 |
| options.default.test.js | 26 | 0 |
| options.dual-options.test.js | 0 | 13 |
| options.env.test.js | 0 | 31 |
| options.flags.test.js | 10 | 0 |
| options.getset.test.js | 0 | 7 |
| options.implies.test.js | 0 | 21 |
| options.mandatory.test.js | 19 | 1 |
| options.optional.test.js | 8 | 0 |
| options.opts.test.js | 6 | 1 |
| options.optsWithGlobals.test.js | 0 | 8 |
| options.preset.test.js | 8 | 0 |
| options.registerClash.test.js | 6 | 0 |
| options.required.test.js | 6 | 0 |
| options.twice.test.js | 6 | 0 |
| options.values.test.js | 9 | 0 |
| options.variadic.test.js | 19 | 0 |
| options.version.test.js | 12 | 1 |
| useColor.test.js | 0 | 7 |

## Не проверено

Проверки выполнены с CARGO_NET_OFFLINE=true и
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-n1:
cargo test -p hoh-commander — суммарно 191 passed, 0 failed, включая oracle;
cargo clippy -p hoh-commander --all-targets -- -D warnings — exit 101:
защищённый tests/oracle.rs:135 содержит .map(|op| list(op)), clippy::redundant_closure.
Файл принадлежит ведущему и запрещён к редактированию; lint не подавлен.
Других диагностик этот запуск не сообщил.
cargo fmt -p hoh-commander -- --check — успешно.
Независимое read-only ревью проверило три исправленных краевых случая
(-0, тип preset, optional parser в короткой группе), не запуская проверок.

Oracle ведущего проверяет 996 запусков 13 программ и 9 отклоняемых определений; все совпали.
tests/oracle.rs и tests/fixtures не изменены. Node и генератор fixture не запускались.
Для пункта 7 новый тест сначала упал на старом поведении; тесты пунктов 10–12 сначала усилены,
затем подтвердили отказы на временных поломках dispatch help, выбора root action и накопления
variadic preset. Поломки удалены; исходные аргументы preset восстановлены до байта.
На не-Unix платформах автоматическое определение ширины не реализовано; ширины unset.
Терминальные Unix-потоки и другие платформы не проверены в этом нетерминальном запуске.
Произвольные Unicode localeCompare-кандидаты, изменение
публичного дерева команд изнутри action и необычные JS-объекты не являются входами harness.
Ветвления Windows/macOS для запуска subprocess относятся к исключённым executable-командам.
Тестовые builders не запускают настоящие сервисы, браузер, транспорт и lifecycle приложения.

## Вопросы ведущему

Вопросов, блокирующих используемую часть commander, нет. Расхождение задания с gateway
зафиксировано выше: вместо несуществующей CLI gateway шестой builder соответствует plugin.

