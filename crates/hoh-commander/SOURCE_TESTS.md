# Реестр тестов commander v15.0.0

Разобраны все 109 файлов *.test.js, *.test.cjs, *.test.mjs из tests/.
Единица учёта — декларация test(), а не количество её запусков внутри JS forEach.
Из 1117 деклараций: 612 перенесены, 505 исключены с причиной.
Матрицы параметров (в частности 17 отрицательных значений × 7 режимов) сохранены в Rust-таблицах;
один Rust test может проверять несколько исходных деклараций. Для динамических вариантов
arguments()/argument()/addArgument()/command() перенесены поддержанные формы, а arguments()
исключён как deprecated. Node argv-конвенция в остальных случаях заменена from:user.
Оригинальные имена ниже нормализованы в snake_case; номер строки однозначно различает повторения.
Точные ожидаемые тексты находятся в Rust-проверках. Служебные testHelpers.js, fixtures/** и
fixtures-extensions/** обслуживают исключённые Node/import/subprocess проверки и не исполнялись.

| Исходный файл tests/ | Перенесено | Исключено |
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

## args.literal.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 11 | `when_arguments_includes_then_stop_processing_options` | Перенесён: tests/arguments.rs |
| 25 | `when_arguments_include_then_more_are_passed_through_as_args` | Перенесён: tests/arguments.rs |

## args.variadic.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_no_extra_arguments_specified_for_program_then_variadic_arg_is_empty_array` | Перенесён: tests/arguments.rs |
| 20 | `when_extra_arguments_specified_for_program_then_variadic_arg_is_array_of_values` | Перенесён: tests/arguments.rs |
| 33 | `when_no_extra_arguments_specified_for_command_then_variadic_arg_is_empty_array` | Перенесён: tests/arguments.rs |
| 44 | `when_extra_arguments_specified_for_command_then_variadic_arg_is_array_of_values` | Перенесён: tests/arguments.rs |
| 56 | `when_program_variadic_argument_not_last_then_error` | Перенесён: tests/arguments.rs |
| 64 | `when_command_variadic_argument_not_last_then_error` | Перенесён: tests/arguments.rs |
| 72 | `when_variadic_argument_then_usage_shows_variadic` | Перенесён: tests/arguments.rs |

## argument.chain.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_call_default_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 12 | `when_call_argparser_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 18 | `when_call_choices_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 24 | `when_call_argrequired_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 30 | `when_call_argoptional_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |

## argument.choices.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 7 | `when_command_argument_in_choices_then_argument_set` | Перенесён: tests/arguments.rs |
| 19 | `when_command_argument_is_not_in_choices_then_error` | Перенесён: tests/arguments.rs |
| 32 | `when_choices_called_then_parameter_does_not_change` | Перенесён: tests/arguments.rs |
| 40 | `when_choices_called_and_argchoices_later_changed_then_parameter_does_not_change` | Перенесён: tests/arguments.rs |
| 48 | `when_choices_called_and_parameter_changed_the_choices_does_not_change` | Перенесён: tests/arguments.rs |

## argument.custom-processing.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 10 | `when_argument_not_specified_then_callback_not_called` | Перенесён: tests/arguments.rs |
| 18 | `when_argument_not_specified_then_action_argument_undefined` | Перенесён: tests/arguments.rs |
| 28 | `when_custom_with_starting_value_and_argument_not_specified_then_callback_not_called` | Перенесён: tests/arguments.rs |
| 36 | `when_custom_with_starting_value_and_argument_not_specified_with_action_handler_then_action_argument_is_starting_value` | Перенесён: tests/arguments.rs |
| 50 | `when_custom_with_starting_value_and_argument_not_specified_without_action_handler_then_processedargs_has_starting_value` | Перенесён: tests/arguments.rs |
| 58 | `when_default_value_is_defined_without_custom_processing_and_argument_not_specified_with_action_handler_then_action_argument_is_default_value` | Перенесён: tests/arguments.rs |
| 70 | `when_default_value_is_defined_without_custom_processing_and_argument_not_specified_without_action_handler_then_processedargs_is_default_value` | Перенесён: tests/arguments.rs |
| 78 | `when_argument_specified_then_callback_called_with_value` | Перенесён: tests/arguments.rs |
| 87 | `when_argument_specified_with_action_handler_then_action_value_is_as_returned_from_callback` | Перенесён: tests/arguments.rs |
| 103 | `when_argument_specified_without_action_handler_then_processedargs_is_as_returned_from_callback` | Перенесён: tests/arguments.rs |
| 113 | `when_argument_specified_then_program_args_has_original_rather_than_custom` | Перенесён: tests/arguments.rs |
| 126 | `when_custom_with_starting_value_and_argument_specified_then_callback_called_with_value_and_starting_value` | Перенесён: tests/arguments.rs |
| 140 | `when_variadic_argument_specified_multiple_times_then_callback_called_with_value_and_previousvalue` | Перенесён: tests/arguments.rs |
| 152 | `when_variadic_argument_without_action_handler_then_processedarg_has_array` | Перенесён: tests/arguments.rs |
| 159 | `when_parsefloat_1e2_then_action_argument_is_100` | Перенесён: tests/arguments.rs |
| 170 | `when_defined_default_value_for_required_argument_then_throw` | Перенесён: tests/arguments.rs |
| 177 | `when_custom_processing_for_argument_throws_plain_error_then_not_commandererror_caught` | По контракту задания parser возвращает InvalidArgumentError; произвольная ошибка сохраняется для action и проверена отдельно. |

## argument.required.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_name_with_surrounding_then_argument_required` | Перенесён: tests/arguments.rs |
| 14 | `when_name_with_surrounding_then_argument_optional` | Перенесён: tests/arguments.rs |
| 19 | `when_name_without_surrounding_brackets_then_argument_required` | Перенесён: tests/arguments.rs |
| 25 | `when_call_argrequired_then_argument_required` | Перенесён: tests/arguments.rs |
| 32 | `when_call_argoptional_then_argument_optional` | Перенесён: tests/arguments.rs |

## argument.variadic.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_no_extra_arguments_specified_for_program_then_variadic_arg_is_empty_array` | Перенесён: tests/arguments.rs |
| 20 | `when_extra_arguments_specified_for_program_then_variadic_arg_is_array_of_values` | Перенесён: tests/arguments.rs |
| 36 | `when_no_extra_arguments_specified_for_command_then_variadic_arg_is_empty_array` | Перенесён: tests/arguments.rs |
| 47 | `when_extra_arguments_specified_for_command_then_variadic_arg_is_array_of_values` | Перенесён: tests/arguments.rs |
| 59 | `when_program_variadic_argument_not_last_then_error` | Перенесён: tests/arguments.rs |
| 67 | `when_command_variadic_argument_not_last_then_error` | Перенесён: tests/arguments.rs |
| 75 | `when_variadic_argument_then_usage_shows_variadic` | Перенесён: tests/arguments.rs |
| 82 | `when_variadic_used_with_choices_and_one_value_then_set_in_array` | Перенесён: tests/arguments.rs |
| 95 | `when_variadic_used_with_choices_and_two_values_then_set_in_array` | Перенесён: tests/arguments.rs |
| 108 | `when_variadic_has_default_array_then_specified_value_is_used_instead_of_default_not_appended` | Перенесён: tests/arguments.rs |
| 121 | `when_variadic_has_default_array_then_specified_value_is_used_instead_of_default_not_appended` | Перенесён: tests/arguments.rs |

## command.action.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_action_called_then_command_passed_to_action` | Перенесён: tests/command.rs / tests/arguments.rs |
| 19 | `when_action_called_then_this_is_set_to_command` | JS this заменён явным &mut Command в аргументах action; идентичность JS-контекста не переносится. |
| 29 | `when_action_called_then_program_args_only_contains_args` | Перенесён: tests/command.rs / tests/arguments.rs |
| 40 | `via_methodname` | Перенесён: tests/command.rs / tests/arguments.rs |
| 54 | `via_methodname` | Перенесён: tests/command.rs / tests/arguments.rs |
| 66 | `when_action_on_program_and_no_arguments_then_action_called` | Перенесён: tests/command.rs / tests/arguments.rs |
| 78 | `via_methodname` | Перенесён: tests/command.rs / tests/arguments.rs |
| 92 | `via_methodname` | Перенесён: tests/command.rs / tests/arguments.rs |
| 106 | `via_methodname` | Перенесён: tests/command.rs / tests/arguments.rs |
| 124 | `via_methodname` | Перенесён: tests/command.rs / tests/arguments.rs |
| 139 | `when_action_is_async_then_can_await_parseasync` | parseAsync не используется; перенесён синхронный parse. |

## command.addCommand.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 7 | `when_addcommand_and_specify_subcommand_then_called` | addCommand и отдельно созданные подкоманды не используются; используется command. |
| 18 | `when_commands_added_using_addcommand_and_command_then_internals_similar` | addCommand и отдельно созданные подкоманды не используются; используется command. |
| 45 | `when_command_without_name_passed_to_addcommand_then_throw` | addCommand и отдельно созданные подкоманды не используются; используется command. |
| 53 | `when_executable_command_with_custom_executablefile_passed_to_addcommand_then_ok` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |

## command.addHelpOption.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_addhelpoption_has_custom_flags_then_custom_short_flag_invokes_help` | addHelpOption не используется; helpOption перенесён. |
| 21 | `when_addhelpoption_has_custom_flags_then_custom_long_flag_invokes_help` | addHelpOption не используется; helpOption перенесён. |
| 33 | `when_addhelpoption_with_hidden_help_option_then_help_does_not_include_help_option` | addHelpOption не используется; helpOption перенесён. |

## command.addHelpText.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 14 | `when_before_string_then_string_before_built_in_help` | Перенесён: tests/command.rs |
| 25 | `when_before_function_then_function_result_before_built_in_help` | Функциональный addHelpText и JS context не используются; все строковые позиции перенесены. |
| 36 | `when_before_function_returns_nothing_then_no_effect` | Функциональный addHelpText и JS context не используются; все строковые позиции перенесены. |
| 46 | `when_beforeall_string_then_string_before_built_in_help` | Перенесён: tests/command.rs |
| 57 | `when_after_string_then_string_after_built_in_help` | Перенесён: tests/command.rs |
| 68 | `when_afterall_string_then_string_after_built_in_help` | Перенесён: tests/command.rs |
| 79 | `when_all_the_simple_positions_then_strings_in_order` | Перенесён: tests/command.rs |
| 96 | `when_silly_position_then_throw` | Перенесён: tests/command.rs |
| 111 | `when_before_on_program_then_not_called_on_subcommand` | Перенесён: tests/command.rs |
| 120 | `when_beforeall_on_program_then_is_called_on_subcommand` | Перенесён: tests/command.rs |
| 129 | `when_after_on_program_then_not_called_on_subcommand` | Перенесён: tests/command.rs |
| 138 | `when_afterall_on_program_then_is_called_on_subcommand` | Перенесён: tests/command.rs |
| 157 | `when_help_requested_then_text_is_on_stdout` | Перенесён: tests/command.rs |
| 166 | `when_help_for_error_then_text_is_on_stderr` | Перенесён: tests/command.rs |
| 175 | `when_help_requested_then_context_error_is_false` | Функциональный addHelpText и JS context не используются; все строковые позиции перенесены. |
| 187 | `when_help_for_error_then_context_error_is_true` | Функциональный addHelpText и JS context не используются; все строковые позиции перенесены. |
| 202 | `when_help_on_program_then_context_command_is_program` | Функциональный addHelpText и JS context не используются; все строковые позиции перенесены. |
| 214 | `when_help_on_subcommand_and_before_subcommand_then_context_command_is_subcommand` | Функциональный addHelpText и JS context не используются; все строковые позиции перенесены. |
| 227 | `when_help_on_subcommand_and_beforeall_on_program_then_context_command_is_subcommand` | Функциональный addHelpText и JS context не используются; все строковые позиции перенесены. |

## command.alias.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_command_has_alias_then_appears_in_help` | Псевдонимы команд не используются. |
| 16 | `when_command_has_aliases_added_separately_then_only_first_appears_in_help` | Псевдонимы команд не используются. |
| 23 | `when_command_has_aliases_then_only_first_appears_in_help` | Псевдонимы команд не используются. |
| 30 | `when_command_name_alias_then_error` | Псевдонимы команд не используются. |
| 40 | `when_use_alias_then_action_handler_called` | Псевдонимы команд не используются. |
| 48 | `when_use_second_alias_added_separately_then_action_handler_called` | Псевдонимы команд не используются. |
| 56 | `when_use_second_of_aliases_then_action_handler_called` | Псевдонимы команд не используются. |
| 64 | `when_set_alias_then_can_get_alias` | Псевдонимы команд не используются. |
| 71 | `when_set_aliases_then_can_get_aliases` | Псевдонимы команд не используются. |
| 78 | `when_set_alias_on_executable_then_can_get_alias` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 87 | `when_aliases_called_then_parameter_does_not_change` | Псевдонимы команд не используются. |
| 95 | `when_aliases_called_and_aliases_later_changed_then_parameter_does_not_change` | Псевдонимы команд не используются. |
| 103 | `when_aliases_called_and_parameter_later_changed_then_aliases_does_not_change` | Псевдонимы команд не используются. |

## command.allowExcessArguments.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 15 | `when_specify_excess_program_argument_then_error_by_default` | Перенесён: tests/repeated_parse.rs |
| 24 | `when_specify_excess_program_argument_and_allowexcessarguments_false_then_error` | Перенесён: tests/repeated_parse.rs |
| 34 | `when_specify_excess_program_argument_and_allowexcessarguments_then_no_error` | Перенесён: tests/repeated_parse.rs |
| 44 | `when_specify_excess_program_argument_and_allowexcessarguments_true_then_no_error` | Перенесён: tests/repeated_parse.rs |
| 54 | `when_specify_excess_command_argument_then_error_by_default` | Перенесён: tests/repeated_parse.rs |
| 64 | `when_specify_excess_command_argument_and_allowexcessarguments_false_then_error` | Перенесён: tests/repeated_parse.rs |
| 74 | `when_specify_expected_arg_and_allowexcessarguments_false_then_no_error` | Перенесён: tests/repeated_parse.rs |
| 84 | `when_specify_excess_after_arg_and_allowexcessarguments_false_then_error` | Перенесён: tests/repeated_parse.rs |
| 94 | `when_specify_excess_after_arg_and_allowexcessarguments_false_then_error` | Перенесён: tests/repeated_parse.rs |
| 104 | `when_specify_args_for_args_and_allowexcessarguments_false_then_no_error` | Перенесён: tests/repeated_parse.rs |

## command.allowUnknownOption.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_specify_unknown_program_option_then_error` | Перенесён: tests/command.rs |
| 17 | `when_specify_unknown_program_option_and_allowunknownoption_false_then_error` | Перенесён: tests/command.rs |
| 26 | `when_specify_unknown_program_option_and_allowunknownoption_then_no_error` | Перенесён: tests/command.rs |
| 39 | `when_specify_unknown_program_option_and_allowunknownoption_true_then_no_error` | Перенесён: tests/command.rs |
| 52 | `when_specify_unknown_command_option_then_error` | Перенесён: tests/command.rs |
| 65 | `when_specify_unknown_command_option_and_allowunknownoption_then_no_error` | Перенесён: tests/command.rs |

## command.argumentVariations.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 11 | `using_methodname` | Перенесён: tests/repeated_parse.rs |
| 23 | `using_methodname` | Перенесён: tests/repeated_parse.rs |
| 35 | `using_methodname` | Перенесён: tests/repeated_parse.rs |
| 47 | `using_methodname` | Перенесён: tests/repeated_parse.rs |
| 59 | `using_methodname` | Перенесён: tests/repeated_parse.rs |
| 71 | `using_methodname` | Перенесён: tests/repeated_parse.rs |
| 97 | `using_methodname` | Перенесён: tests/repeated_parse.rs |
| 118 | `when_add_arguments_using_multiple_methods_then_all_added` | Случай комбинирует deprecated arguments(); поддержанные argument/addArgument/command проверены отдельно. |

## command.asterisk.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 17 | `when_no_arguments_then_asterisk_action_not_called` | EventEmitter, command:* и wildcard-команды не используются. |
| 29 | `when_unrecognised_argument_then_asterisk_action_called` | EventEmitter, command:* и wildcard-команды не используются. |
| 37 | `when_recognised_command_then_asterisk_action_not_called` | EventEmitter, command:* и wildcard-команды не используются. |
| 46 | `when_unrecognised_command_argument_then_asterisk_action_called` | EventEmitter, command:* и wildcard-команды не используются. |
| 55 | `when_unrecognised_argument_and_known_option_then_asterisk_action_called` | EventEmitter, command:* и wildcard-команды не используются. |
| 70 | `when_non_command_argument_and_unknown_option_then_error_for_unknown_option` | EventEmitter, command:* и wildcard-команды не используются. |
| 89 | `when_no_arguments_then_listener_not_called` | EventEmitter, command:* и wildcard-команды не используются. |
| 97 | `when_unrecognised_argument_then_listener_called` | EventEmitter, command:* и wildcard-команды не используются. |
| 105 | `when_recognised_command_then_listener_not_called` | EventEmitter, command:* и wildcard-команды не используются. |
| 114 | `when_unrecognised_command_argument_then_listener_called` | EventEmitter, command:* и wildcard-команды не используются. |
| 123 | `when_unrecognised_command_argument_and_unknown_option_then_listener_called` | EventEmitter, command:* и wildcard-команды не используются. |

## command.chain.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_call_command_with_description_for_stand_alone_executable_then_returns_this` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 15 | `when_call_addcommand_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 21 | `when_call_argument_then_returns_this` | Перенесён: tests/chain.rs |
| 27 | `when_call_addargument_then_returns_this` | Перенесён: tests/chain.rs |
| 33 | `when_set_arguments_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 39 | `when_call_addhelpcommand_cmd_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 45 | `when_call_deprecated_addhelpcommand_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 51 | `when_call_deprecated_addhelpcommand_boolean_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 59 | `when_call_deprecated_addhelpcommand_string_string_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 67 | `when_call_helpcommand_name_then_returns_this` | Перенесён: tests/chain.rs |
| 73 | `when_call_helpcommand_name_description_then_returns_this` | Перенесён: tests/chain.rs |
| 79 | `when_call_helpcommand_boolean_then_returns_this` | Перенесён: tests/chain.rs |
| 87 | `when_call_exitoverride_then_returns_this` | Перенесён: tests/chain.rs |
| 93 | `when_call_action_then_returns_this` | Перенесён: tests/chain.rs |
| 99 | `when_call_addoption_then_returns_this` | Перенесён: tests/chain.rs |
| 105 | `when_call_option_then_returns_this` | Перенесён: tests/chain.rs |
| 111 | `when_call_requiredoption_then_returns_this` | Перенесён: tests/chain.rs |
| 117 | `when_call_combineflagandoptionalvalue_then_returns_this` | Перенесён: tests/chain.rs |
| 123 | `when_call_allowunknownoption_then_returns_this` | Перенесён: tests/chain.rs |
| 129 | `when_call_allowexcessarguments_then_returns_this` | Перенесён: tests/chain.rs |
| 135 | `when_call_storeoptionsasproperties_then_returns_this` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 141 | `when_call_version_then_returns_this` | Перенесён: tests/chain.rs |
| 147 | `when_set_description_then_returns_this` | Перенесён: tests/chain.rs |
| 153 | `when_set_alias_then_returns_this` | Псевдонимы команд не используются. |
| 159 | `when_set_aliases_then_returns_this` | Псевдонимы команд не используются. |
| 165 | `when_set_usage_then_returns_this` | Перенесён: tests/chain.rs |
| 171 | `when_set_name_then_returns_this` | Перенесён: tests/chain.rs |
| 177 | `when_call_helpoption_flags_then_returns_this` | Перенесён: tests/chain.rs |
| 184 | `when_call_addhelpoption_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 190 | `when_call_addhelptext_then_returns_this` | Перенесён: tests/chain.rs |
| 196 | `when_call_configurehelp_then_returns_this` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 202 | `when_call_configureoutput_then_returns_this` | Перенесён: tests/chain.rs |
| 208 | `when_call_passthroughoptions_then_returns_this` | Перенесён: tests/chain.rs |
| 214 | `when_call_enablepositionaloptions_then_returns_this` | Перенесён: tests/chain.rs |
| 220 | `when_call_hook_then_returns_this` | Lifecycle hooks и события не используются. |
| 226 | `when_call_setoptionvalue_then_returns_this` | Публичные setters и сведения об источниках опций не используются. |
| 232 | `when_call_setoptionvaluewithsource_then_returns_this` | Публичные setters и сведения об источниках опций не используются. |
| 238 | `when_call_showhelpaftererror_then_returns_this` | Эти переключатели не используются; стандартные suggestions включены. |
| 244 | `when_call_showsuggestionaftererror_then_returns_this` | Эти переключатели не используются; стандартные suggestions включены. |
| 250 | `when_call_copyinheritedsettings_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 257 | `when_set_namefromfilename_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 263 | `when_set_helpgroup_heading_then_returns_this` | Группы команд и опций не используются. |
| 269 | `when_set_commandsgroup_heading_then_returns_this` | Группы команд и опций не используются. |
| 275 | `when_set_optionsgroup_heading_then_returns_this` | Группы команд и опций не используются. |

## command.commandHelp.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 7 | `when_program_has_command_then_appears_in_help` | Перенесён: tests/help.rs |
| 14 | `when_program_has_command_with_optional_arg_then_appears_in_help` | Перенесён: tests/help.rs |

## command.configureHelp.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_configure_program_then_affects_program_helpinformation` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 16 | `when_configure_program_then_affects_subcommand_helpinformation` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 27 | `when_configure_with_unknown_property_then_createhelp_has_unknown_property` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 33 | `when_configure_with_unknown_property_then_helper_passed_to_formathelp_has_unknown_property` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |

## command.configureOutput.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 7 | `when_default_writeerr_then_error_on_stderr` | Перенесён: tests/default_output.rs |
| 21 | `when_custom_writeerr_then_error_on_custom_output` | Перенесён: tests/command.rs / src/help.rs tests |
| 37 | `when_default_write_then_version_on_stdout` | Перенесён: tests/default_output.rs |
| 49 | `when_custom_write_then_version_on_custom_output` | Перенесён: tests/command.rs / src/help.rs tests |
| 66 | `when_default_write_then_help_on_stdout` | Перенесён: tests/default_output.rs |
| 74 | `when_custom_write_then_help_error_on_custom_output` | Перенесён: tests/command.rs / src/help.rs tests |
| 85 | `when_default_writeerr_then_help_error_on_stderr` | Перенесён: tests/default_output.rs |
| 93 | `when_custom_writeerr_then_help_error_on_custom_output` | Перенесён: tests/command.rs / src/help.rs tests |
| 104 | `when_default_getouthelpwidth_then_help_helpwidth_from_stdout` | Подмена глобальных stdio Node, getters, outputError, цвета и настраиваемая ширина не используются; writeOut/writeErr проверены. |
| 127 | `when_custom_getouthelpwidth_then_help_helpwidth_custom` | Подмена глобальных stdio Node, getters, outputError, цвета и настраиваемая ширина не используются; writeOut/writeErr проверены. |
| 147 | `when_default_geterrhelpwidth_then_help_error_helpwidth_from_stderr` | Подмена глобальных stdio Node, getters, outputError, цвета и настраиваемая ширина не используются; writeOut/writeErr проверены. |
| 169 | `when_custom_geterrhelpwidth_then_help_error_helpwidth_custom` | Подмена глобальных stdio Node, getters, outputError, цвета и настраиваемая ширина не используются; writeOut/writeErr проверены. |
| 189 | `when_custom_getouthelpwidth_and_configurehelp_helpwidth_then_help_helpwidth_from_configurehelp` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 210 | `when_custom_geterrhelpwidth_and_configurehelp_helpwidth_then_help_error_helpwidth_from_configurehelp` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 231 | `when_no_custom_setup_and_call_formathelp_direct_then_effective_helpwidth_is_fallback_80` | Перенесён: tests/command.rs / src/help.rs tests |
| 246 | `when_no_custom_setup_and_call_formatitem_direct_then_effective_helpwidth_is_fallback_80` | Перенесён: tests/command.rs / src/help.rs tests |
| 264 | `when_set_configureoutput_then_get_configureoutput` | Подмена глобальных stdio Node, getters, outputError, цвета и настраиваемая ширина не используются; writeOut/writeErr проверены. |
| 280 | `when_custom_outputerr_and_error_then_outputerr_called` | Подмена глобальных stdio Node, getters, outputError, цвета и настраиваемая ширина не используются; writeOut/writeErr проверены. |
| 300 | `when_custom_outputerr_and_writeerr_and_error_then_outputerr_passed_writeerr` | Подмена глобальных stdio Node, getters, outputError, цвета и настраиваемая ширина не используются; writeOut/writeErr проверены. |
| 316 | `when_configureoutput_after_copyinheritedsettings_then_original_unchanged` | Перенесён: tests/command.rs / src/help.rs tests |
| 333 | `when_envvar_then_getfoohascolors_returns_expected` | Подмена глобальных stdio Node, getters, outputError, цвета и настраиваемая ширина не используются; writeOut/writeErr проверены. |

## command.copySettings.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_add_subcommand_with_command_then_calls_copyinheritedsettings_from_parent` | Перенесён: tests/repeated_parse.rs / tests/command.rs |
| 25 | `when_copyinheritedsettings_then_copies_outputconfiguration_config` | Перенесён: tests/repeated_parse.rs / tests/command.rs |
| 34 | `when_copyinheritedsettings_then_copies_helpoption_false` | Перенесён: tests/repeated_parse.rs / tests/command.rs |
| 43 | `when_copyinheritedsettings_then_copies_helpoption_flags_description` | Перенесён: tests/repeated_parse.rs / tests/command.rs |
| 57 | `when_copyinheritedsettings_then_copies_custom_help_command` | Перенесён: tests/repeated_parse.rs / tests/command.rs |
| 70 | `when_copyinheritedsettings_then_does_not_copy_help_enable_override` | Перенесён: tests/repeated_parse.rs / tests/command.rs |
| 81 | `when_copyinheritedsettings_then_copies_configurehelp_config` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 91 | `when_copyinheritedsettings_then_copies_exitoverride` | Перенесён: tests/repeated_parse.rs / tests/command.rs |
| 101 | `when_copyinheritedsettings_then_copies_storeoptionsasproperties` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 111 | `when_copyinheritedsettings_then_copies_combineflagandoptionalvalue` | Перенесён: tests/repeated_parse.rs / tests/command.rs |
| 121 | `when_copyinheritedsettings_then_copies_allowexcessarguments` | Перенесён: tests/repeated_parse.rs / tests/command.rs |
| 131 | `when_copyinheritedsettings_then_copies_enablepositionaloptions` | Перенесён: tests/repeated_parse.rs / tests/command.rs |
| 141 | `when_copyinheritedsettings_then_copies_showhelpaftererror` | Эти переключатели не используются; стандартные suggestions включены. |

## command.createArgument.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 24 | `when_override_createargument_then_used_for_argument` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 31 | `when_override_createargument_then_used_for_arguments` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 38 | `when_override_createargument_and_createcommand_then_used_for_argument_of_command` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |

## command.createHelp.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_override_createcommand_then_affects_help` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |

## command.createOption.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 19 | `when_override_createoption_then_used_for_option` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 26 | `when_override_createoption_then_used_for_requiredoption` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 33 | `when_override_createoption_then_used_for_version` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 40 | `when_override_createoption_then_used_for_help_option_in_visibleoptions` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |

## command.default.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 14 | `when_default_subcommand_and_no_command_then_call_default` | Подкоманда по умолчанию не используется. |
| 19 | `when_default_subcommand_and_unrecognised_argument_then_call_default_with_argument` | Подкоманда по умолчанию не используется. |
| 24 | `when_default_subcommand_and_unrecognised_option_then_call_default_with_option` | Подкоманда по умолчанию не используется. |
| 43 | `when_default_subcommand_and_no_command_then_call_default` | Подкоманда по умолчанию не используется. |
| 49 | `when_default_subcommand_and_unrecognised_argument_then_call_default` | Подкоманда по умолчанию не используется. |
| 55 | `when_default_subcommand_and_unrecognised_option_then_call_default` | Подкоманда по умолчанию не используется. |
| 76 | `when_default_subcommand_and_no_command_then_call_default` | Подкоманда по умолчанию не используется. |
| 82 | `when_default_subcommand_and_unrecognised_argument_then_call_default` | Подкоманда по умолчанию не используется. |
| 88 | `when_default_subcommand_and_unrecognised_option_then_call_default` | Подкоманда по умолчанию не используется. |

## command.description.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_set_description_then_get_description` | Перенесён: tests/command.rs |

## command.error.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_error_called_with_message_then_message_displayed_on_stderr` | Перенесён: tests/command.rs |
| 18 | `when_error_called_with_no_exitcode_then_process_exit_1` | Перенесён: tests/command.rs |
| 32 | `when_error_called_with_exitcode_2_then_process_exit_2` | Перенесён: tests/command.rs |
| 45 | `when_error_called_with_code_and_exitoverride_then_throws_with_code` | Перенесён: tests/command.rs |

## command.executableSubcommand.inspect.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 19 | `when_execargv_empty_then_spawn_execargs_empty` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 24 | `when_execargv_harmony_then_spawn_execargs_harmony` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 34 | `when_execargv_inspect_then_spawn_execargs_using_port_9230` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 44 | `when_execargv_inspect_9240_then_spawn_execargs_using_port_9241` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 54 | `when_execargv_inspect_0_then_spawn_execargs_inspect_0` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 64 | `when_execargv_inspect_127_0_0_1_9250_then_spawn_execargs_inspect_127_0_0_1_9251` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 74 | `when_execargv_inspect_localhost_9260_then_spawn_execargs_inspect_localhost_9261` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 84 | `when_execargv_inspect_port_9270_then_spawn_execargs_inspect_port_127_0_0_1_9271` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |

## command.executableSubcommand.lookup.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 18 | `when_subcommand_file_missing_then_error` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 29 | `when_alias_subcommand_file_missing_then_error` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 40 | `when_subcommand_file_has_no_suffix_then_lookup_succeeds` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 45 | `when_alias_subcommand_file_has_no_suffix_then_lookup_succeeds` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 50 | `when_subcommand_target_executablefile_has_no_suffix_then_lookup_succeeds` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 55 | `when_subcommand_file_suffix_js_then_lookup_succeeds` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 60 | `when_alias_subcommand_file_suffix_js_then_lookup_succeeds` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 65 | `when_subcommand_target_executablefile_has_suffix_js_then_lookup_succeeds` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 107 | `when_subcommand_suffix_is_cjs_then_lookup_succeeds` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 117 | `when_subcommand_suffix_is_mjs_then_lookup_succeeds` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 127 | `when_subsubcommand_then_lookup_sub_sub_command` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |

## command.executableSubcommand.mock.test.cjs

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 60 | `when_subcommand_executable_fails_with_other_error_and_exitoverride_then_return_in_custom_wrapper` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 86 | `when_subcommand_executable_fails_with_other_error_then_exit` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |

## command.executableSubcommand.search.test.cjs

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 50 | `when_no_script_arg_or_executabledir_then_no_search_for_local_file` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 61 | `when_script_arg_then_search_for_local_files` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 72 | `when_executabledir_then_search_for_local_files` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 89 | `when_named_pm_and_no_script_arg_or_executabledir_then_spawn_pm_sub_as_command` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 99 | `when_named_pm_and_script_arg_then_still_spawn_pm_sub_as_command` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 109 | `when_no_name_and_script_arg_then_spawn_script_sub_as_command` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 118 | `when_named_pm_and_script_arg_and_executablefile_then_spawn_executablefile_as_command` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 132 | `when_construct_with_name_pm_and_script_arg_then_spawn_local_pm_sub_js` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 147 | `when_name_pm_and_script_arg_then_spawn_local_pm_sub_js` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 163 | `when_script_arg_then_spawn_local_script_sub_js` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 178 | `when_name_pm_and_script_arg_and_only_script_sub_js_then_fallback_to_spawn_local_script_sub_js` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 195 | `when_name_pm_and_executabledir_then_spawn_local_pm_sub_js` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 209 | `when_script_arg_and_relative_executabledir_then_spawn_relative_script_sub_js` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 222 | `when_script_arg_and_absolute_executabledir_then_spawn_absolute_script_sub_js` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 239 | `when_script_arg_is_link_and_and_link_sub_relative_to_link_target_then_spawn_local_link_sub` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 259 | `when_name_pm_and_script_arg_and_relative_executablefile_then_spawn_local_exec_js` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 277 | `when_name_pm_and_script_arg_and_absolute_executablefile_then_spawn_local_exec_js` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 296 | `when_script_arg_then_search_for_local_script_sub_js_ts_tsx_mpjs_cjs` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |

## command.executableSubcommand.signals.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 18 | `when_program_sent_signal_then_executablesubcommand_sent_signal_too` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 39 | `when_executable_subcommand_sent_signal_then_program_exit_code_is_non_zero` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 44 | `when_command_has_exitoverride_and_executable_subcommand_sent_signal_then_exit_code_is_non_zero` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 54 | `when_command_has_exitoverride_and_executable_subcommand_fails_then_program_exit_code_is_subcommand_exit_code` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |

## command.executableSubcommand.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_no_command_specified_and_executable_subcommand_then_display_help` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |

## command.exitOverride.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 21 | `when_specify_unknown_program_option_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 42 | `when_specify_unknown_command_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 64 | `when_supply_custom_handler_then_throw_custom_error` | exitOverride(fn) не используется; стандартный перехват и явный exit_callback проверены. |
| 91 | `when_specify_option_without_required_value_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 113 | `when_specify_command_without_required_argument_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 137 | `when_specify_program_without_required_argument_and_no_action_handler_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 158 | `when_specify_excess_argument_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 182 | `when_specify_command_with_excess_argument_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 207 | `when_specify_help_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 227 | `when_executable_subcommand_and_no_command_specified_then_throw_commandererror` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 242 | `when_specify_version_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 258 | `when_executablesubcommand_succeeds_then_call_exitoverride` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 277 | `when_mandatory_program_option_missing_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 298 | `when_option_argument_not_in_choices_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 321 | `when_command_argument_not_in_choices_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 344 | `when_custom_processing_for_option_throws_invalidargumenterror_then_catch_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 368 | `when_custom_processing_for_argument_throws_invalidargumenterror_then_catch_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 394 | `when_has_conflicting_option_then_throw_commandererror` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 417 | `when_call_error_then_throw_commandererror` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 433 | `when_no_override_and_error_then_exit_1` | Перенесён: tests/command.rs / tests/options.rs / tests/arguments.rs |
| 442 | `when_custom_processing_throws_custom_error_then_throw_custom_error` | По контракту задания parser возвращает InvalidArgumentError; произвольная ошибка сохраняется для action и проверена отдельно. |

## command.help.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 13 | `when_call_helpinformation_for_program_then_help_format_is_as_expected_usage_options_commands` | Перенесён: tests/help.rs / tests/command.rs |
| 31 | `when_use_description_for_command_then_help_includes_description` | Перенесён: tests/help.rs / tests/command.rs |
| 39 | `when_call_help_then_exit` | Перенесён: tests/help.rs / tests/command.rs |
| 49 | `when_specify_help_then_exit` | Перенесён: tests/help.rs / tests/command.rs |
| 59 | `when_call_help_cb_then_display_cb_output_and_exit` | Deprecated callback-переформатирование help не используется. |
| 77 | `when_call_outputhelp_cb_then_display_cb_output` | Deprecated callback-переформатирование help не используется. |
| 89 | `when_call_deprecated_outputhelp_cb_with_wrong_callback_return_type_then_throw` | Deprecated API и callback-переформатирование help не используются. |
| 96 | `when_command_sets_deprecated_nohelp_then_not_displayed_in_helpinformation` | Deprecated API и callback-переформатирование help не используются. |
| 103 | `when_command_sets_hidden_then_not_displayed_in_helpinformation` | Скрытые команды не используются; скрытые опции перенесены. |
| 110 | `when_addcommand_with_hidden_true_then_not_displayed_in_helpinformation` | addCommand и отдельно созданные подкоманды не используются; используется command. |
| 119 | `when_help_short_flag_masked_then_not_displayed_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 128 | `when_both_help_flags_masked_then_not_displayed_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 135 | `when_call_help_then_output_on_stdout` | Перенесён: tests/help.rs / tests/command.rs |
| 151 | `when_call_help_with_error_true_then_output_on_stderr` | Перенесён: tests/help.rs / tests/command.rs |
| 167 | `when_no_options_then_options_not_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 175 | `when_negated_option_then_option_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 183 | `when_option_hidehelp_then_option_not_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 192 | `when_option_hidehelp_true_then_option_not_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 201 | `when_option_hidehelp_false_then_option_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 210 | `when_option_has_default_value_then_default_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 217 | `when_option_has_default_value_description_then_default_description_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 229 | `when_option_has_choices_then_choices_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 238 | `when_option_has_choices_and_default_then_both_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 251 | `when_argument_then_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 258 | `when_argument_described_then_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 265 | `when_argument_described_with_default_then_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 275 | `when_arguments_described_in_deprecated_way_then_included_in_helpinformation` | Deprecated API и callback-переформатирование help не используются. |
| 285 | `when_arguments_described_in_deprecated_way_and_empty_description_then_arguments_still_included_in_helpinformation` | Deprecated API и callback-переформатирование help не используются. |
| 295 | `when_argument_has_choices_then_choices_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |
| 307 | `when_argument_has_choices_and_default_then_both_included_in_helpinformation` | Перенесён: tests/help.rs / tests/command.rs |

## command.helpCommand.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_program_has_no_subcommands_then_no_automatic_help_command` | Перенесён: tests/command.rs |
| 14 | `when_program_has_no_subcommands_and_add_help_command_then_has_help_command` | Перенесён: tests/command.rs |
| 21 | `when_program_has_subcommands_then_has_automatic_help_command` | Перенесён: tests/command.rs |
| 28 | `when_program_has_subcommands_and_specify_only_unknown_option_then_display_help` | Перенесён: tests/command.rs |
| 41 | `when_program_has_subcommands_and_suppress_help_command_then_no_help_command` | Перенесён: tests/command.rs |
| 49 | `when_add_custom_help_command_then_custom_help_command` | Перенесён: tests/command.rs |
| 68 | `when_program_help_then_program` | Перенесён: tests/command.rs |
| 80 | `when_program_help_unknown_then_program` | Перенесён: tests/command.rs |
| 92 | `when_program_help_sub1_then_sub1` | Перенесён: tests/command.rs |
| 104 | `when_program_sub1_help_sub2_then_sub2` | Перенесён: tests/command.rs |
| 117 | `when_default_command_and_program_help_then_program` | Подкоманда по умолчанию не используется. |
| 129 | `when_no_long_help_flag_then_help_sub_works` | Перенесён: tests/command.rs |
| 143 | `when_no_help_options_in_sub_then_help_sub_works` | Перенесён: tests/command.rs |
| 156 | `when_different_help_options_in_sub_then_help_sub_works` | Перенесён: tests/command.rs |

## command.helpOption.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 7 | `when_helpoption_has_custom_flags_then_custom_short_flag_invokes_help` | Перенесён: tests/command.rs |
| 18 | `when_helpoption_has_custom_flags_then_custom_long_flag_invokes_help` | Перенесён: tests/command.rs |
| 29 | `when_helpoption_has_just_custom_short_flag_then_custom_short_flag_invokes_help` | Перенесён: tests/command.rs |
| 40 | `when_helpoption_has_just_custom_long_flag_then_custom_long_flag_invokes_help` | Перенесён: tests/command.rs |
| 51 | `when_helpoption_has_custom_description_then_helpinformation_include_custom_description` | Перенесён: tests/command.rs |
| 58 | `when_helpoption_has_just_flags_then_helpinformation_includes_default_description` | Перенесён: tests/command.rs |
| 65 | `when_helpoption_has_just_description_then_helpinformation_includes_default_flags` | Перенесён: tests/command.rs |
| 72 | `when_helpoption_false_then_helpinformation_does_not_include_help` | Перенесён: tests/command.rs |
| 79 | `when_helpoption_false_then_help_is_an_unknown_option` | Перенесён: tests/command.rs |
| 90 | `when_helpoption_false_then_h_is_an_unknown_option` | Перенесён: tests/command.rs |
| 101 | `when_helpoption_false_then_unknown_command_error_does_not_suggest_help` | Перенесён: tests/command.rs |
| 112 | `when_helpoption_true_after_false_then_helpinformation_does_include_help` | Перенесён: tests/command.rs |
| 120 | `when_helpoption_true_after_customise_then_helpinformation_still_customised` | Перенесён: tests/command.rs |

## command.hook.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_hook_event_wrong_then_throw` | Lifecycle hooks и события не используются. |
| 13 | `when_no_action_then_action_hooks_not_called` | Lifecycle hooks и события не используются. |
| 22 | `when_hook_preaction_then_hook_called_before_action` | Lifecycle hooks и события не используются. |
| 32 | `when_hook_postaction_then_hook_called_after_action` | Lifecycle hooks и события не используются. |
| 42 | `when_hook_preaction_twice_then_hooks_called_fifo` | Lifecycle hooks и события не используются. |
| 53 | `when_hook_postaction_twice_then_hooks_called_lifo` | Lifecycle hooks и события не используются. |
| 64 | `when_hook_preaction_at_program_and_sub_then_hooks_called_program_then_sub` | Lifecycle hooks и события не используются. |
| 76 | `when_hook_postaction_at_program_and_sub_then_hooks_called_sub_then_program` | Lifecycle hooks и события не используются. |
| 88 | `when_hook_everything_then_hooks_called_nested` | Lifecycle hooks и события не используются. |
| 116 | `when_hook_on_program_then_passed_program_program` | Lifecycle hooks и события не используются. |
| 127 | `when_hook_on_program_and_call_sub_then_passed_program_sub` | Lifecycle hooks и события не используются. |
| 139 | `when_hook_on_sub_and_call_sub_then_passed_sub_sub` | Lifecycle hooks и события не используются. |
| 153 | `when_hook_program_on_preaction_then_thiscommand_has_options_set` | Lifecycle hooks и события не используются. |
| 164 | `when_hook_program_on_preaction_and_call_sub_then_thiscommand_has_program_options_set` | Lifecycle hooks и события не используются. |
| 173 | `when_hook_program_on_preaction_and_call_sub_then_actioncommand_has_sub_options_set` | Lifecycle hooks и события не используются. |
| 185 | `when_hook_program_on_preaction_then_actioncommand_has_args_set` | Lifecycle hooks и события не используются. |
| 196 | `when_hook_program_on_preaction_then_actioncommand_has_args_set_with_options_removed` | Lifecycle hooks и события не используются. |
| 208 | `when_hook_program_on_preaction_and_call_sub_then_thiscommand_has_program_args_set` | Lifecycle hooks и события не используются. |
| 220 | `when_hook_program_on_preaction_and_call_sub_then_actioncommand_has_sub_args_set` | Lifecycle hooks и события не используются. |
| 234 | `when_async_preaction_then_async_from_preaction` | Lifecycle hooks и события не используются. |
| 249 | `when_async_postaction_then_async_from_postaction` | Lifecycle hooks и события не используются. |
| 264 | `when_async_action_then_async_from_action` | Lifecycle hooks и события не используются. |
| 280 | `when_async_first_preaction_then_async_from_first_preaction` | Lifecycle hooks и события не используются. |
| 296 | `when_async_second_preaction_then_async_from_second_preaction` | Lifecycle hooks и события не используются. |
| 312 | `when_async_hook_everything_then_hooks_called_nested` | Lifecycle hooks и события не используются. |
| 361 | `presubcommand_hook_should_work` | Lifecycle hooks и события не используются. |
| 381 | `presubcommand_hook_should_effective_for_direct_subcommands` | Lifecycle hooks и события не используются. |

## command.name.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 7 | `when_construct_with_name_then_name_is_set` | Конструктор с именем заменён Command::new().name(...); harness вызывает только new Command(). |
| 12 | `when_set_program_name_and_parse_then_name_is_as_assigned` | Перенесён: tests/command.rs |
| 19 | `when_program_name_not_set_and_parse_with_script_argument_then_plain_name_is_found_from_script_name` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |
| 27 | `when_command_name_not_set_and_no_script_argument_in_parse_then_name_is_program` | Перенесён: tests/command.rs |
| 33 | `when_add_command_then_command_is_named` | Перенесён: tests/command.rs |
| 39 | `when_set_program_name_then_name_appears_in_help` | Перенесён: tests/command.rs |
| 46 | `when_pass_path_to_namefromfilename_then_name_is_plain_name` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |
| 52 | `when_pass_import_meta_filename_to_namefromfilename_then_name_is_plain_name_of_this_file` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |

## command.nested.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 5 | `when_call_nested_subcommand_then_runs` | Перенесён: tests/command.rs |

## command.onCommand.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_action_handler_for_subcommand_then_emit_command_subcommand` | EventEmitter, command:* и wildcard-команды не используются. |
| 18 | `when_no_action_handler_for_subcommand_then_still_emit_command_subcommand` | EventEmitter, command:* и wildcard-команды не используются. |
| 27 | `when_subcommand_has_argument_then_emit_command_subcommand_with_argument` | EventEmitter, command:* и wildcard-команды не используются. |

## command.option-misuse.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 12 | `when_pass_option_to_option_then_throw` | Типы Rust исключают передачу Opt вместо строки флагов. |
| 23 | `when_pass_option_to_requiredoption_then_throw` | Типы Rust исключают передачу Opt вместо строки флагов. |

## command.parse.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 14 | `when_no_args_then_use_process_argv_and_app_script_args` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |
| 24 | `when_no_args_and_electron_properties_and_not_default_app_then_use_process_argv_and_app_args` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |
| 36 | `when_args_then_app_script_args` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |
| 43 | `when_args_from_node_then_app_script_args` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |
| 50 | `when_args_from_electron_and_not_default_app_then_app_args` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |
| 60 | `when_args_from_electron_and_default_app_then_app_script_args` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |
| 70 | `when_args_from_user_then_args` | Перенесён: tests/repeated_parse.rs |
| 77 | `when_args_from_silly_then_throw` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |
| 87 | `when_node_execargv_includes_flag_then_app_args` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |
| 105 | `when_call_parse_then_returns_program` | Перенесён: tests/repeated_parse.rs |
| 114 | `when_await_parseasync_then_returns_program` | parseAsync не используется; перенесён синхронный parse. |
| 125 | `when_parse_strings_instead_of_array_then_throw` | Rust API принимает только последовательность строк; передача не-массива исключена типами. |
| 134 | `when_parse_called_then_parameter_does_not_change` | Перенесён: tests/repeated_parse.rs |
| 144 | `when_parse_called_and_parsed_args_later_changed_then_parameter_does_not_change` | Перенесён: tests/repeated_parse.rs |
| 156 | `when_parse_called_and_param_later_changed_then_parsed_args_do_not_change` | Перенесён: tests/repeated_parse.rs |
| 171 | `when_parse_called_then_parameter_does_not_change` | parseAsync не используется; перенесён синхронный parse. |
| 181 | `when_parseasync_called_and_parsed_args_later_changed_then_parameter_does_not_change` | parseAsync не используется; перенесён синхронный parse. |
| 193 | `when_parseasync_called_and_param_later_changed_then_parsed_args_do_not_change` | parseAsync не используется; перенесён синхронный parse. |
| 208 | `when_use_boolean_options_then_option_values_reset` | Перенесён: tests/repeated_parse.rs |
| 220 | `when_use_options_with_option_argument_then_option_values_and_sources_reset` | Перенесён: tests/repeated_parse.rs |
| 236 | `when_use_options_with_option_argument_and_default_then_option_values_and_sources_reset` | Перенесён: tests/repeated_parse.rs |
| 252 | `when_use_negated_options_then_option_values_reset` | Перенесён: tests/repeated_parse.rs |
| 264 | `when_use_variadic_option_then_option_values_reset` | Перенесён: tests/repeated_parse.rs |
| 274 | `when_use_collect_example_then_option_value_resets` | Перенесён: tests/repeated_parse.rs |
| 288 | `when_use_increaseverbosity_example_then_option_value_resets` | Перенесён: tests/repeated_parse.rs |
| 309 | `when_use_parse_and_parseasync_then_option_values_reset` | parseAsync не используется; перенесён синхронный parse. |
| 320 | `when_call_subcommand_then_option_values_reset_program_and_subcommand` | Перенесён: tests/repeated_parse.rs |
| 342 | `when_call_different_subcommand_then_no_reset_because_lazy` | Перенесён: tests/repeated_parse.rs |
| 357 | `when_parse_with_different_implied_program_name_then_name_changes` | Node/Electron/eval, process.argv и вывод имени из script path не используются; from:user перенесён. |
| 367 | `when_parse_with_different_arguments_then_args_change` | Перенесён: tests/repeated_parse.rs |
| 380 | `when_parse_with_different_arguments_then_rawargs_change` | Перенесён: tests/repeated_parse.rs |
| 394 | `when_parse_with_different_arguments_then_processedargs_change` | Перенесён: tests/repeated_parse.rs |
| 409 | `when_parse_subcommand_then_reset_state_before_presubcommand_hook_called` | Lifecycle hooks и события не используются. |
| 434 | `when_using_storeoptionsasproperties_then_throw_on_second_parse` | storeOptionsAsProperties не используется; значения доступны через opts. |

## command.parseOptions.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 26 | `when_specify_subcommand_and_argument_then_program_args_not_empty` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 33 | `when_specify_subcommand_and_option_and_argument_then_program_args_not_empty` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 40 | `when_arguments_to_executable_include_option_flags_then_argument_order_preserved` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 71 | `when_empty_args_then_empty_results` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 77 | `when_only_operands_then_results_has_all_operands` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 86 | `when_subcommand_and_operand_then_results_has_subcommand_and_operand` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 92 | `when_program_has_flag_then_option_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 98 | `when_program_has_option_with_value_then_option_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 104 | `when_program_has_flag_before_operand_then_option_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 110 | `when_program_has_flag_after_operand_then_option_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 116 | `when_program_has_flag_after_subcommand_then_option_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 122 | `when_program_has_unknown_option_then_option_returned_in_unknown` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 128 | `when_program_has_unknown_option_before_operands_then_all_unknown_in_same_order` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 134 | `when_program_has_unknown_option_after_operand_then_option_returned_in_unknown` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 140 | `when_program_has_flag_after_unknown_option_then_flag_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 146 | `when_subcommand_has_flag_then_flag_returned_as_unknown` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 152 | `when_program_has_literal_before_known_flag_then_option_returned_as_operand` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 158 | `when_program_has_literal_before_unknown_option_then_option_returned_as_operand` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 164 | `when_program_has_literal_after_unknown_option_then_literal_preserved_too` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 176 | `when_program_has_known_flag_and_operand_then_option_removed_and_operand_returned` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 183 | `when_program_has_mixed_arguments_then_known_options_removed_and_rest_returned_in_same_order` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 198 | `when_subcommand_has_mixed_arguments_then_program_flags_removed_and_rest_returned_in_same_order` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 259 | `when_program_has_combo_known_boolean_short_flags_then_arg_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 266 | `when_program_has_combo_unknown_short_flags_then_arg_preserved` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 273 | `when_program_has_combo_known_short_option_and_required_value_then_arg_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 280 | `when_program_has_combo_known_short_option_and_optional_value_then_arg_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 287 | `when_program_has_known_combo_short_boolean_flags_and_required_value_then_arg_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 294 | `when_program_has_known_combo_short_boolean_flags_and_optional_value_then_arg_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 301 | `when_program_has_known_long_flag_value_then_arg_removed` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 308 | `when_program_has_unknown_long_flag_value_then_arg_preserved` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 315 | `when_program_has_combo_optional_and_combineflagandoptionalvalue_then_treat_as_value` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 322 | `when_program_has_combo_optional_and_combineflagandoptionalvalue_true_then_treat_as_value` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |
| 329 | `when_program_has_combo_optional_and_combineflagandoptionalvalue_false_then_treat_as_boolean` | Перенесён: src/command.rs tests / tests/repeated_parse.rs |

## command.positionalOptions.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 17 | `when_option_before_command_argument_then_option_parsed` | Перенесён: tests/positional_options.rs |
| 24 | `when_known_option_after_command_argument_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 31 | `when_unknown_option_after_command_argument_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 37 | `when_action_handler_and_unknown_option_after_command_argument_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 47 | `when_help_option_without_command_argument_then_help_called` | Перенесён: tests/positional_options.rs |
| 60 | `when_help_option_after_command_argument_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 66 | `when_version_option_after_command_argument_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 91 | `when_global_option_before_subcommand_then_global_option_parsed` | Перенесён: tests/positional_options.rs |
| 97 | `when_shared_option_after_subcommand_then_parsed_by_subcommand` | Перенесён: tests/positional_options.rs |
| 104 | `when_shared_option_after_subcommand_argument_then_parsed_by_subcommand` | Перенесён: tests/positional_options.rs |
| 112 | `when_shared_option_before_and_after_subcommand_then_both_parsed` | Перенесён: tests/positional_options.rs |
| 133 | `when_user_args_json_stringify_args_then_program_sub_help_called_programhelp_subhelp` | Перенесён: tests/positional_options.rs |
| 172 | `when_program_option_before_sub_option_then_program_option_read_by_program` | Подкоманда по умолчанию не используется. |
| 178 | `when_program_option_before_sub_option_then_sub_option_read_by_sub` | Подкоманда по умолчанию не используется. |
| 184 | `when_shared_option_before_sub_argument_then_option_read_by_program` | Подкоманда по умолчанию не используется. |
| 190 | `when_shared_option_after_sub_argument_then_option_read_by_sub` | Подкоманда по умолчанию не используется. |
| 204 | `when_user_args_json_stringify_args_then_program_sub_help_called_programhelp_subhelp` | Подкоманда по умолчанию не используется. |
| 242 | `when_option_before_command_argument_then_option_parsed` | Перенесён: tests/positional_options.rs |
| 249 | `when_known_option_after_command_argument_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 256 | `when_unknown_option_after_command_argument_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 262 | `when_action_handler_and_unknown_option_after_command_argument_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 272 | `when_help_option_after_command_argument_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 278 | `when_version_option_after_command_argument_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 285 | `when_shared_option_before_sub_and_after_sub_and_after_sub_parameter_then_all_three_parsed` | Перенесён: tests/positional_options.rs |
| 312 | `when_option_before_command_argument_then_option_parsed` | Подкоманда по умолчанию не используется. |
| 319 | `when_known_option_after_command_argument_then_option_passed_through` | Подкоманда по умолчанию не используется. |
| 326 | `when_unknown_option_after_command_argument_then_option_passed_through` | Подкоманда по умолчанию не используется. |
| 332 | `when_action_handler_and_unknown_option_after_command_argument_then_option_passed_through` | Подкоманда по умолчанию не используется. |
| 360 | `when_global_option_before_parameter_then_global_option_parsed` | Перенесён: tests/positional_options.rs |
| 366 | `when_global_option_after_parameter_then_global_option_parsed` | Перенесён: tests/positional_options.rs |
| 372 | `when_global_option_after_parameter_with_same_name_as_subcommand_then_global_option_parsed` | Перенесён: tests/positional_options.rs |
| 382 | `when_program_not_positional_and_turn_on_passthroughoptions_in_subcommand_then_error` | Перенесён: tests/positional_options.rs |
| 391 | `when_program_not_positional_and_add_subcommand_with_passthroughoptions_then_error` | addCommand не используется; command() и проверка pass-through перенесены. |
| 420 | `when_global_option_before_parameter_then_global_option_parsed` | Перенесён: tests/positional_options.rs |
| 426 | `when_global_option_after_parameter_then_passed_through` | Перенесён: tests/positional_options.rs |
| 432 | `when_subcommand_option_after_subcommand_then_option_parsed` | Перенесён: tests/positional_options.rs |
| 439 | `when_shared_option_after_subcommand_then_parsed_by_subcommand` | Перенесён: tests/positional_options.rs |
| 450 | `when_passthroughoptions_and_unknown_option_then_arguments_from_unknown_passed_through` | Перенесён: tests/positional_options.rs |
| 459 | `when_positionaloptions_and_unknown_option_then_known_options_then_known_option_parsed` | Перенесён: tests/positional_options.rs |
| 479 | `when_passthroughoptions_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 486 | `when_passthroughoptions_true_then_option_passed_through` | Перенесён: tests/positional_options.rs |
| 493 | `when_passthroughoptions_false_then_option_parsed` | Перенесён: tests/positional_options.rs |
| 512 | `when_enablepositionaloptions_then_option_parsed_by_subcommand` | Перенесён: tests/positional_options.rs |
| 519 | `when_enablepositionaloptions_true_then_option_parsed_by_subcommand` | Перенесён: tests/positional_options.rs |
| 526 | `when_enablepositionaloptions_false_then_option_parsed_by_program` | Перенесён: tests/positional_options.rs |

## command.registerClash.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_command_name_conflicts_with_existing_name_then_throw` | Перенесён: tests/repeated_parse.rs |
| 17 | `when_command_name_conflicts_with_existing_alias_then_throw` | Псевдонимы команд не используются. |
| 28 | `when_command_alias_conflicts_with_existing_name_then_throw` | Псевдонимы команд не используются. |
| 39 | `when_command_alias_conflicts_with_existing_alias_then_throw` | Псевдонимы команд не используются. |
| 50 | `when_addcommand_name_conflicts_with_existing_name_then_throw` | Этот вариант регистрации вне используемого API; поддержанные конфликты проверяются при parse. |
| 61 | `when_addcommand_alias_conflicts_with_existing_name_then_throw` | Псевдонимы команд не используются. |

## command.showHelpAfterError.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 19 | `when_missing_command_argument_then_shows_help` | Эти переключатели не используются; стандартные suggestions включены. |
| 34 | `when_missing_option_argument_then_shows_help` | Эти переключатели не используются; стандартные suggestions включены. |
| 49 | `when_missing_mandatory_option_then_shows_help` | Эти переключатели не используются; стандартные suggestions включены. |
| 64 | `when_unknown_option_then_shows_help` | Эти переключатели не используются; стандартные suggestions включены. |
| 78 | `when_too_many_command_arguments_then_shows_help` | Эти переключатели не используются; стандартные suggestions включены. |
| 93 | `when_unknown_command_then_shows_help` | Эти переключатели не используются; стандартные suggestions включены. |
| 108 | `when_invalid_option_choice_then_shows_help` | Эти переключатели не используются; стандартные suggestions включены. |
| 124 | `when_command_showhelpaftererror_and_error_and_then_shows_full_help` | Эти переключатели не используются; стандартные suggestions включены. |

## command.showSuggestionAfterError.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 23 | `when_unknown_command_and_showsuggestionaftererror_then_show_suggestion` | Эти переключатели не используются; стандартные suggestions включены. |
| 31 | `when_unknown_command_and_showsuggestionaftererror_false_then_do_not_show_suggestion` | Эти переключатели не используются; стандартные suggestions включены. |
| 39 | `when_unknown_option_and_showsuggestionaftererror_then_show_suggestion` | Эти переключатели не используются; стандартные suggestions включены. |
| 47 | `when_unknown_option_and_showsuggestionaftererror_false_then_do_not_show_suggestion` | Эти переключатели не используются; стандартные suggestions включены. |

## command.summary.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 5 | `command_summary_when_set_summary_then_get_summary` | summary не используется; показ description проверен отдельно. |

## command.unknownCommand.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_unknown_argument_in_simple_program_then_error` | Перенесён: tests/command.rs |
| 13 | `when_unknown_command_but_action_handler_taking_arg_then_no_error` | Перенесён: tests/command.rs |
| 22 | `when_unknown_command_but_listener_then_no_error` | EventEmitter, command:* и wildcard-команды не используются. |
| 31 | `when_unknown_command_then_error` | Перенесён: tests/command.rs |
| 42 | `when_unknown_command_and_unknown_option_then_error_is_for_unknown_command` | Перенесён: tests/command.rs |

## command.unknownOption.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_specify_unknown_option_with_subcommand_and_action_handler_then_error` | Перенесён: tests/command.rs |
| 20 | `when_specify_unknown_option_with_subcommand_argument_and_action_handler_then_error` | Перенесён: tests/command.rs |
| 32 | `when_specify_unknown_option_with_program_and_action_handler_then_error` | Перенесён: tests/command.rs |
| 44 | `when_specify_unknown_option_with_program_argument_and_action_handler_then_error` | Перенесён: tests/command.rs |
| 57 | `when_specify_unknown_option_with_simple_program_then_error` | Перенесён: tests/command.rs |
| 67 | `when_specify_unknown_global_option_before_subcommand_then_error` | Перенесён: tests/command.rs |

## command.usage.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_default_usage_and_check_program_help_then_starts_with_default_usage` | Перенесён: tests/command.rs |
| 15 | `when_custom_usage_and_check_program_help_then_starts_with_custom_usage` | Перенесён: tests/command.rs |
| 26 | `when_default_usage_and_check_subcommand_help_then_starts_with_default_usage_including_program_name` | Перенесён: tests/command.rs |
| 36 | `when_custom_usage_and_check_subcommand_help_then_starts_with_custom_usage_including_program_name` | Перенесён: tests/command.rs |
| 47 | `when_has_option_then_options_included_in_usage` | Перенесён: tests/command.rs |
| 55 | `when_no_options_then_options_not_included_in_usage` | Перенесён: tests/command.rs |
| 63 | `when_has_command_then_command_included_in_usage` | Перенесён: tests/command.rs |
| 71 | `when_no_commands_then_command_not_included_in_usage` | Перенесён: tests/command.rs |
| 77 | `when_argument_then_argument_included_in_usage` | Перенесён: tests/command.rs |
| 85 | `when_options_and_command_and_argument_then_all_three_included_in_usage` | Перенесён: tests/command.rs |

## commander.storeOptionsAsProperties.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 10 | `when_default_then_options_not_stored_on_command` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 18 | `when_default_then_options_command_passed_to_action` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 31 | `when_storeoptionsasproperties_then_options_stored_on_command` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 39 | `when_storeoptionsasproperties_true_then_options_stored_on_command` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 49 | `when_storeoptionsasproperties_false_then_options_not_stored_on_command` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 59 | `when_storeoptionsasproperties_then_command_command_passed_to_action` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 70 | `when_storeoptionsasproperties_false_then_opts_command_passed_to_action` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 84 | `when_storeoptionsasproperties_after_adding_option_then_throw` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 92 | `when_storeoptionsasproperties_after_setting_option_value_then_throw` | storeOptionsAsProperties не используется; значения доступны через opts. |

## createCommand.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_createcommand_then_unattached` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 13 | `when_subclass_overrides_createcommand_then_subcommand_is_subclass` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |
| 29 | `when_override_createcommand_then_subcommand_is_custom` | Фабрики, подклассы и пользовательская конфигурация Help не используются. |

## deprecated.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 10 | `when_option_not_given_then_value_is_default` | Deprecated API и callback-переформатирование help не используются. |
| 17 | `when_argument_matches_regexp_then_value_is_as_specified` | Deprecated API и callback-переформатирование help не используются. |
| 24 | `when_argument_does_not_match_regexp_then_value_is_default` | Deprecated API и callback-переформатирование help не используются. |

## help.argumentDescription.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_argument_has_no_description_then_empty_string` | Перенесён: src/help.rs tests |
| 14 | `when_argument_has_description_then_return_description` | Перенесён: src/help.rs tests |
| 21 | `when_argument_has_default_value_then_return_description_and_default_value` | Перенесён: src/help.rs tests |
| 32 | `when_argument_has_default_value_description_then_return_description_and_custom_default_description` | Перенесён: src/help.rs tests |
| 44 | `when_an_argument_has_default_value_and_no_description_then_still_return_default_value` | Перенесён: src/help.rs tests |

## help.boxWrap.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_empty_string_then_return_empty_string` | Перенесён: src/help.rs tests |
| 13 | `when_string_contains_unix_line_breaks_then_return_unix_empty_lines` | Перенесён: src/help.rs tests |
| 20 | `when_string_contains_windows_line_breaks_then_return_unix_empty_lines` | Перенесён: src/help.rs tests |
| 27 | `when_string_contains_only_whitespace_then_returns_empty_string_trim_right` | Перенесён: src/help.rs tests |
| 34 | `when_string_contains_leading_whitespace_then_returns_string_with_leading_whitespace` | Перенесён: src/help.rs tests |
| 41 | `when_string_contains_intermediate_whitespace_then_returns_string_with_intermediate_whitespace` | Перенесён: src/help.rs tests |
| 48 | `when_string_contains_trailing_whitespace_then_returns_string_without_trailing_whitespace_trim_right` | Перенесён: src/help.rs tests |
| 55 | `when_string_contains_escape_sequences_then_returns_string_with_escape_sequences` | Перенесён: src/help.rs tests |
| 62 | `when_string_has_enough_words_to_wrap_then_returns_two_wrapped_lines` | Перенесён: src/help.rs tests |
| 72 | `when_string_has_enough_words_and_escape_sequences_to_wrap_then_returns_wrapped_line_with_break_ignoring_escape_sequences` | Перенесён: src/help.rs tests |
| 85 | `when_first_word_longer_than_width_then_soft_wrap` | Перенесён: src/help.rs tests |
| 92 | `when_second_word_longer_than_width_then_wrap_and_soft_wrap` | Перенесён: src/help.rs tests |
| 99 | `when_set_width_41_then_wrap_at_41` | Перенесён: src/help.rs tests |
| 108 | `when_set_width_12_too_small_then_skip_wrap` | Перенесён: src/help.rs tests |
| 115 | `when_set_width_12_and_set_minwrapwidth_then_skip_wrap` | Настраиваемые ширина и minWidthToWrap не используются; ширина 80 и порог 40 перенесены. |

## help.commandTerm.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 10 | `when_plain_command_then_returns_name` | Перенесён: src/help.rs tests |
| 16 | `when_command_has_alias_then_returns_name_alias` | Псевдонимы команд не используются. |
| 22 | `when_command_has_options_then_returns_name_options` | Перенесён: src/help.rs tests |
| 28 | `when_command_has_argument_then_returns_name_argument` | Перенесён: src/help.rs tests |
| 34 | `when_command_has_everything_then_returns_name_alias_options_argument` | Псевдонимы команд не используются. |

## help.commandUsage.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_single_program_then_program_options` | Перенесён: tests/command.rs |
| 16 | `when_multi_program_then_program_options_command` | Перенесён: tests/command.rs |
| 24 | `when_program_has_alias_then_usage_includes_alias` | Псевдонимы команд не используются. |
| 31 | `when_help_for_subcommand_then_usage_includes_hierarchy` | Перенесён: tests/command.rs |
| 39 | `when_program_has_argument_then_usage_includes_argument` | Перенесён: tests/command.rs |

## help.longestArgumentTermLength.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_no_arguments_then_returns_zero` | Перенесён: src/help.rs tests |
| 15 | `when_has_argument_description_then_returns_argument_length` | Перенесён: src/help.rs tests |
| 25 | `when_has_multiple_argument_descriptions_then_returns_longest` | Перенесён: src/help.rs tests |

## help.longestCommandTermLength.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_no_commands_then_returns_zero` | Перенесён: src/help.rs tests |
| 15 | `when_command_and_no_help_then_returns_length_of_term` | Перенесён: src/help.rs tests |
| 26 | `when_command_with_arg_and_no_help_then_returns_length_of_term` | Перенесён: src/help.rs tests |
| 37 | `when_multiple_commands_then_returns_longest_length` | Перенесён: src/help.rs tests |
| 52 | `when_just_help_command_then_returns_length_of_help_term` | Перенесён: src/help.rs tests |

## help.longestOptionTermLength.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_no_option_then_returns_zero` | Перенесён: src/help.rs tests |
| 16 | `when_just_implicit_help_option_returns_length_of_help_flags` | Перенесён: src/help.rs tests |
| 25 | `when_multiple_option_then_returns_longest_length` | Перенесён: src/help.rs tests |

## help.minWidthToWrap.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_enough_width_then_wrap` | Настраиваемые ширина и minWidthToWrap не используются; ширина 80 и порог 40 перенесены. |
| 22 | `when_not_enough_width_then_no_wrap` | Настраиваемые ширина и minWidthToWrap не используются; ширина 80 и порог 40 перенесены. |
| 33 | `when_make_minwidthtowrap_small_then_wrap` | Настраиваемые ширина и minWidthToWrap не используются; ширина 80 и порог 40 перенесены. |

## help.optionDescription.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_option_has_no_description_then_empty_string` | Перенесён: src/help.rs tests |
| 15 | `when_option_has_description_then_return_description` | Перенесён: src/help.rs tests |
| 22 | `when_boolean_option_has_default_value_true_then_return_description_and_default_value` | Перенесён: src/help.rs tests |
| 32 | `when_boolean_option_has_default_value_string_then_return_description_without_default` | Перенесён: src/help.rs tests |
| 39 | `when_optional_option_has_preset_value_then_return_description_and_default_value` | Перенесён: src/help.rs tests |
| 51 | `when_boolean_option_has_preset_value_then_return_description_without_default` | Перенесён: src/help.rs tests |
| 57 | `when_option_has_env_then_return_description_and_env_name` | Переменные окружения для опций не используются. |
| 64 | `when_option_has_default_value_description_then_return_description_and_custom_default_description` | Перенесён: src/help.rs tests |
| 78 | `when_option_has_default_value_but_not_description_then_return_custom_default` | Перенесён: src/help.rs tests |
| 91 | `when_option_has_choices_then_return_description_and_choices` | Перенесён: src/help.rs tests |

## help.optionTerm.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_s_flags_then_returns_flags` | Перенесён: src/help.rs tests |
| 16 | `when_short_flags_then_returns_flags` | Перенесён: src/help.rs tests |
| 23 | `when_s_short_flags_then_returns_flags` | Перенесён: src/help.rs tests |
| 30 | `when_s_short_flags_then_returns_flags` | Перенесён: src/help.rs tests |

## help.padWidth.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_argument_term_longest_return_argument_length` | Перенесён: src/help.rs tests |
| 18 | `when_option_term_longest_return_option_length` | Перенесён: src/help.rs tests |
| 27 | `when_global_option_term_longest_return_global_option_length` | optsWithGlobals и раздел Global Options не используются. |
| 39 | `when_command_term_longest_return_command_length` | Перенесён: src/help.rs tests |

## help.preformatted.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_single_line_then_false` | Перенесён: src/help.rs tests |
| 11 | `when_single_line_with_leading_whitespace_then_false` | Перенесён: src/help.rs tests |
| 16 | `when_unix_line_break_not_followed_by_whitespace_then_false` | Перенесён: src/help.rs tests |
| 21 | `when_windows_line_break_not_followed_by_whitespace_then_false` | Перенесён: src/help.rs tests |
| 26 | `when_unix_line_followed_by_whitespace_then_true` | Перенесён: src/help.rs tests |
| 31 | `when_windows_line_break_followed_by_whitespace_then_true` | Перенесён: src/help.rs tests |
| 36 | `when_empty_unix_lines_then_false` | Перенесён: src/help.rs tests |
| 41 | `when_empty_windows_lines_then_false` | Перенесён: src/help.rs tests |
| 47 | `end_to_end_when_option_description_is_preformatted_then_manual_format_is_preserved` | Перенесён: src/help.rs tests |

## help.showGlobalOptions.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_default_configuration_then_global_options_hidden` | optsWithGlobals и раздел Global Options не используются. |
| 13 | `when_showglobaloptions_true_then_program_options_shown` | optsWithGlobals и раздел Global Options не используются. |
| 20 | `when_showglobaloptions_true_and_no_global_options_then_global_options_header_not_shown` | optsWithGlobals и раздел Global Options не используются. |
| 27 | `when_showglobaloptions_true_and_nested_commands_then_combined_nested_options_shown_program_last` | optsWithGlobals и раздел Global Options не используются. |
| 38 | `when_showglobaloptions_true_and_sortoptions_true_then_global_options_sorted` | optsWithGlobals и раздел Global Options не используются. |

## help.sortCommands.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_unsorted_then_commands_in_order_added` | Настраиваемая сортировка help не используется; порядок регистрации сохранён. |
| 22 | `when_sortsubcommands_true_then_commands_sorted` | Настраиваемая сортировка help не используется; порядок регистрации сохранён. |

## help.sortOptions.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_unsorted_then_options_in_order_added` | Настраиваемая сортировка help не используется; порядок регистрации сохранён. |
| 22 | `when_sortoptions_true_then_options_sorted_alphabetically` | Настраиваемая сортировка help не используется; порядок регистрации сохранён. |
| 36 | `when_both_short_and_long_flags_then_sort_on_short_flag` | Настраиваемая сортировка help не используется; порядок регистрации сохранён. |
| 50 | `when_lone_short_and_long_flags_then_sort_on_lone_flag` | Настраиваемая сортировка help не используется; порядок регистрации сохранён. |
| 64 | `when_mixed_case_flags_then_sort_is_case_insensitive` | Настраиваемая сортировка help не используется; порядок регистрации сохранён. |
| 78 | `when_negated_option_then_sort_negated_option_separately` | Настраиваемая сортировка help не используется; порядок регистрации сохранён. |

## help.style.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 36 | `styletitle` | Цвета и пользовательские стили исключены заданием. |
| 50 | `styleusage` | Цвета и пользовательские стили исключены заданием. |
| 61 | `stylecommanddescription` | Цвета и пользовательские стили исключены заданием. |
| 77 | `styleoptiondescription` | Цвета и пользовательские стили исключены заданием. |
| 93 | `stylesubcommanddescription` | Цвета и пользовательские стили исключены заданием. |
| 114 | `styleargumentdescription` | Цвета и пользовательские стили исключены заданием. |
| 127 | `styledescriptiontext` | Цвета и пользовательские стили исключены заданием. |
| 144 | `styleoptionterm` | Цвета и пользовательские стили исключены заданием. |
| 154 | `stylesubcommandterm` | Цвета и пользовательские стили исключены заданием. |
| 172 | `styleargumentterm` | Цвета и пользовательские стили исключены заданием. |
| 185 | `styleoptiontext` | Цвета и пользовательские стили исключены заданием. |
| 200 | `styleargumenttext` | Цвета и пользовательские стили исключены заданием. |
| 217 | `stylesubcommandtext` | Цвета и пользовательские стили исключены заданием. |
| 233 | `stylecommandtext` | Цвета и пользовательские стили исключены заданием. |
| 269 | `when_getouthascolors_returns_true_then_help_has_color` | Цвета и пользовательские стили исключены заданием. |
| 276 | `when_getouthascolors_returns_false_then_help_does_not_have_color` | Цвета и пользовательские стили исключены заданием. |
| 283 | `when_getouthascolors_returns_false_then_style_still_called` | Цвета и пользовательские стили исключены заданием. |

## help.subcommandDescription.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_program_has_no_summary_or_description_then_empty_string` | Перенесён: tests/help.rs |
| 15 | `when_program_has_summary_then_return_summary` | summary не используется; показ description проверен отдельно. |
| 23 | `when_program_has_description_then_return_description` | Перенесён: tests/help.rs |
| 31 | `when_program_has_summary_and_description_then_return_summary` | summary не используется; показ description проверен отдельно. |

## help.suggestion.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 64 | `when_cli_of_arg_and_commands_json_stringify_commandnames_then_suggest_expected_because_description` | Перенесён: tests/suggestions.rs |
| 76 | `when_similar_alias_then_suggest_alias` | Псевдонимы команд не используются. |
| 83 | `when_similar_hidden_alias_then_not_suggested` | Псевдонимы команд не используются. |
| 90 | `when_similar_command_and_alias_then_suggest_both` | Псевдонимы команд не используются. |
| 99 | `when_implicit_help_command_then_help_is_candidate_for_suggestion` | Перенесён: tests/suggestions.rs |
| 106 | `when_help_command_disabled_then_not_candidate_for_suggestion` | Перенесён: tests/suggestions.rs |
| 114 | `when_default_help_option_then_help_is_candidate_for_suggestion` | Перенесён: tests/suggestions.rs |
| 120 | `when_custom_help_option_then_custom_help_is_candidate_for_suggestion` | Перенесён: tests/suggestions.rs |
| 127 | `when_help_option_disabled_then_not_candidate_for_suggestion` | Перенесён: tests/suggestions.rs |
| 134 | `when_command_listener_and_unknown_command_then_no_suggestion` | EventEmitter, command:* и wildcard-команды не используются. |
| 179 | `when_cli_of_arg_and_options_json_stringify_commandnames_then_suggest_expected_because_description` | Перенесён: tests/suggestions.rs |
| 191 | `when_no_options_then_no_suggestion` | Перенесён: tests/suggestions.rs |
| 199 | `when_subcommand_option_then_candidate_for_subcommand_option_suggestion` | Перенесён: tests/suggestions.rs |
| 206 | `when_global_option_then_candidate_for_subcommand_option_suggestion` | Перенесён: tests/suggestions.rs |
| 214 | `when_global_option_but_positionaloptions_then_not_candidate_for_subcommand_suggestion` | Перенесён: tests/suggestions.rs |
| 223 | `when_global_and_local_options_then_both_candidates` | Перенесён: tests/suggestions.rs |
| 231 | `when_command_hidden_then_not_suggested_as_candidate` | Скрытые команды не используются; скрытые опции перенесены. |
| 238 | `when_option_hidden_then_not_suggested_as_candidate` | Перенесён: tests/suggestions.rs |
| 245 | `when_may_be_duplicate_identical_candidates_then_only_return_one` | Перенесён: tests/suggestions.rs |

## help.visibleArguments.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_no_arguments_then_empty_array` | Перенесён: tests/help.rs |
| 15 | `when_argument_but_no_argument_description_then_empty_array` | Перенесён: tests/help.rs |
| 22 | `when_argument_and_argument_description_then_returned` | Перенесён: tests/help.rs |
| 34 | `when_argument_and_legacy_argument_description_then_returned` | Deprecated API и callback-переформатирование help не используются. |
| 49 | `when_arguments_and_some_described_then_all_returned` | Перенесён: tests/help.rs |
| 63 | `when_arguments_and_some_legacy_described_then_all_returned` | Deprecated API и callback-переформатирование help не используются. |

## help.visibleCommands.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_no_subcommands_then_empty_array` | Перенесён: tests/help.rs |
| 15 | `when_add_command_then_visible_with_help` | Перенесён: tests/help.rs |
| 25 | `when_commands_hidden_then_not_visible` | Скрытые команды не используются; скрытые опции перенесены. |

## help.visibleGlobalOptions.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_default_configuration_then_return_empty_array` | optsWithGlobals и раздел Global Options не используются. |
| 14 | `when_showglobaloptions_true_then_return_program_options` | optsWithGlobals и раздел Global Options не используются. |
| 25 | `when_showglobaloptions_true_and_program_has_version_then_return_version` | optsWithGlobals и раздел Global Options не используются. |
| 36 | `when_showglobaloptions_true_and_nested_commands_then_return_combined_global_options` | optsWithGlobals и раздел Global Options не используются. |

## help.visibleOptions.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_no_options_then_just_help_visible` | Перенесён: src/help.rs tests |
| 18 | `when_no_options_and_no_help_option_then_empty_array` | Перенесён: src/help.rs tests |
| 25 | `when_add_option_then_visible_with_help` | Перенесён: src/help.rs tests |
| 35 | `when_option_hidden_then_not_visible` | Перенесён: src/help.rs tests |
| 49 | `when_default_then_help_term_is_h_help` | Перенесён: src/help.rs tests |
| 56 | `when_short_flag_obscured_then_help_term_is_help` | Перенесён: src/help.rs tests |
| 64 | `when_long_flag_obscured_then_help_term_is_h` | Перенесён: src/help.rs tests |
| 72 | `when_help_flags_obscured_then_implicit_help_hidden` | Перенесён: src/help.rs tests |

## helpGroup.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_add_one_option_with_helpgroup_then_help_contains_group` | Группы команд и опций не используются. |
| 16 | `when_add_two_options_with_helpgroup_then_help_contains_group` | Группы команд и опций не используются. |
| 26 | `when_add_one_command_with_helpgroup_then_help_contains_group` | Группы команд и опций не используются. |
| 33 | `when_add_two_commands_with_helpgroup_then_help_contains_group` | Группы команд и опций не используются. |
| 43 | `when_add_one_option_then_help_contains_group` | Группы команд и опций не используются. |
| 51 | `when_add_two_options_then_help_contains_group_with_two_options` | Группы команд и опций не используются. |
| 60 | `when_add_options_with_different_groups_then_help_contains_two_groups` | Группы команд и опций не используются. |
| 71 | `when_implicit_help_option_then_help_option_not_affected` | Группы команд и опций не используются. |
| 79 | `when_option_with_helpgroup_then_helpgroup_wins` | Группы команд и опций не используются. |
| 87 | `when_add_no_options_with_heading_then_heading_does_not_appear` | Группы команд и опций не используются. |
| 94 | `when_add_no_visible_options_with_heading_then_heading_does_not_appear` | Группы команд и опций не используются. |
| 102 | `when_helpoption_flags_then_help_option_in_group` | Группы команд и опций не используются. |
| 110 | `when_helpoption_true_then_help_option_in_group` | Группы команд и опций не используются. |
| 118 | `when_version_str_then_version_option_in_group` | Группы команд и опций не используются. |
| 126 | `when_set_sortoptions_then_options_are_sorted_within_groups` | Группы команд и опций не используются. |
| 140 | `when_set_sortoptions_then_groups_are_in_order_added_not_sorted` | Группы команд и опций не используются. |
| 155 | `when_add_one_command_then_help_contains_group` | Группы команд и опций не используются. |
| 163 | `when_add_two_commands_then_help_contains_group_with_two_commands` | Группы команд и опций не используются. |
| 172 | `when_add_commands_with_different_groups_then_help_contains_two_groups` | Группы команд и опций не используются. |
| 183 | `when_implicit_help_command_then_help_command_not_affected` | Группы команд и опций не используются. |
| 191 | `when_command_with_custom_helpgroup_then_helpgroup_wins` | Группы команд и опций не используются. |
| 199 | `when_add_no_commands_with_heading_then_heading_does_not_appear` | Группы команд и опций не используются. |
| 206 | `when_add_no_visible_command_with_heading_then_heading_does_not_appear` | Группы команд и опций не используются. |
| 214 | `when_helpcommand_name_then_help_command_in_group` | Группы команд и опций не используются. |
| 223 | `when_helpcommand_true_then_help_command_in_group` | Группы команд и опций не используются. |
| 232 | `when_set_sortcommands_then_commands_are_sorted_within_groups` | Группы команд и опций не используются. |
| 246 | `when_set_sortoptions_then_groups_are_in_order_added_not_sorted` | Группы команд и опций не используются. |

## imports.test.cjs

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 27 | `program` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 31 | `command` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 35 | `option` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 39 | `argument` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 43 | `help` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 47 | `commandererror` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 51 | `invalidargumenterror` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 55 | `invalidoptionargumenterror` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 63 | `createcommand` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 67 | `createoption` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 71 | `createargument` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 77 | `program` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 81 | `command` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 85 | `option` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 89 | `argument` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 93 | `help` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 97 | `commandererror` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 104 | `invalidargumenterror` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 111 | `invalidoptionargumenterror` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 119 | `createcommand` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 123 | `createoption` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 130 | `createargument` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |

## imports.test.mjs

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 26 | `program` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 30 | `command` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 34 | `option` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 38 | `argument` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 42 | `help` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 46 | `commandererror` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 50 | `invalidargumenterror` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 54 | `invalidoptionargumenterror` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 62 | `createcommand` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 66 | `createoption` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |
| 70 | `createargument` | Проверки npm, CommonJS, ESM и экспортов Node не имеют Rust-эквивалента. |

## incrementNodeInspectorPort.test.cjs

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 39 | `when_inspect_then_bump_port` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 48 | `when_inspect_100_then_bump_port` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 57 | `when_inspect_1_2_3_4_100_then_bump_port` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 66 | `when_inspect_1_2_3_4_then_bump_port` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 75 | `when_inspect_brk_then_bump_port` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 84 | `when_inspect_brk_100_then_bump_port` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 93 | `when_inspect_brk_1_2_3_4_then_bump_port` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 102 | `when_inspect_brk_1_2_3_4_100_then_bump_port` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 111 | `when_inspect_port_100_then_bump_port` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 120 | `when_inspect_port_1_2_3_4_100_then_bump_port` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 129 | `when_inspect_unexpected_then_unchanged` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |
| 138 | `when_frozen_intrinsics_then_unchanged` | Исполняемые подкоманды, поиск файлов и Node inspector не используются. |

## negatives.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 44 | `when_option_argument_for_short_optional_is_value_then_consumed_consume` | Перенесён: tests/arguments.rs |
| 53 | `when_option_argument_for_long_optional_is_value_then_consumed_consume` | Перенесён: tests/arguments.rs |
| 62 | `when_option_argument_for_short_optional_is_value_then_consumed_consume` | Перенесён: tests/arguments.rs |
| 74 | `when_option_argument_for_long_optional_is_value_then_consumed_consume` | Перенесён: tests/arguments.rs |
| 86 | `when_command_argument_is_value_then_consumed_consume` | Перенесён: tests/arguments.rs |
| 97 | `when_digit_option_defined_and_option_argument_is_s_then_negative_not_consumed` | Перенесён: tests/arguments.rs |
| 108 | `when_digit_option_defined_and_command_argument_is_s_then_negative_not_consumed` | Перенесён: tests/arguments.rs |
| 122 | `when_complex_example_with_negative_numbers_then_all_consumed` | Перенесён: tests/arguments.rs |
| 154 | `when_program_has_digit_option_then_negatives_not_allowed_in_leaf_command` | Перенесён: tests/arguments.rs |
| 168 | `when_default_command_without_digit_option_then_negatives_accepted` | Подкоманда по умолчанию не используется. |
| 181 | `when_default_command_with_digit_option_then_negative_throws` | Подкоманда по умолчанию не используется. |
| 191 | `when_program_has_subcommand_and_action_handler_then_negative_command_argument_unsupported` | Перенесён: tests/arguments.rs |

## option.bad-flags.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 20 | `when_construct_option_with_flags_flags_then_throw` | Перенесён: tests/repeated_parse.rs |
| 45 | `when_construct_option_with_flags_flags_then_do_not_throw` | Перенесён: tests/repeated_parse.rs |

## option.chain.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_call_default_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 12 | `when_call_argparser_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 18 | `when_call_makeoptionmandatory_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 24 | `when_call_hidehelp_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 30 | `when_call_choices_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 36 | `when_call_env_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 42 | `when_call_conflicts_then_returns_this` | Этот JS-метод/идентичность объекта не используется; Rust Opt/Argument возвращают владение, Command — &mut Self. |
| 48 | `when_call_helpgroup_heading_then_returns_this` | Группы команд и опций не используются. |

## options.bool.combo.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 19 | `when_option_not_specified_then_value_is_undefined` | Перенесён: tests/options.rs |
| 25 | `when_positive_option_then_value_is_true` | Перенесён: tests/options.rs |
| 31 | `when_negative_option_then_value_is_false` | Перенесён: tests/options.rs |
| 37 | `when_last_option_is_positive_then_value_is_true` | Перенесён: tests/options.rs |
| 43 | `when_last_option_is_negative_then_value_is_false` | Перенесён: tests/options.rs |
| 62 | `when_option_not_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 68 | `when_positive_option_then_value_is_true` | Перенесён: tests/options.rs |
| 74 | `when_negative_option_then_value_is_false` | Перенесён: tests/options.rs |
| 83 | `when_option_not_specified_then_value_is_false` | Перенесён: tests/options.rs |
| 89 | `when_positive_option_then_value_is_true` | Перенесён: tests/options.rs |
| 95 | `when_negative_option_then_value_is_false` | Перенесён: tests/options.rs |
| 105 | `when_option_not_specified_then_value_is_default` | Перенесён: tests/options.rs |
| 111 | `when_positive_option_then_value_is_true` | Перенесён: tests/options.rs |
| 117 | `when_negative_option_then_value_is_false` | Перенесён: tests/options.rs |
| 137 | `when_option_not_specified_then_value_is_default` | Перенесён: tests/options.rs |
| 143 | `when_positive_option_then_value_is_preset` | Перенесён: tests/options.rs |
| 149 | `when_negative_option_then_value_is_false` | Перенесён: tests/options.rs |
| 157 | `when_lone_negative_then_default_value_is_true` | Перенесён: tests/options.rs |
| 164 | `when_boolean_combo_and_negative_first_then_no_implicit_default` | Перенесён: tests/options.rs |
| 174 | `when_boolean_combo_and_negative_second_then_no_implicit_default` | Перенесён: tests/options.rs |
| 183 | `when_boolean_combo_and_negative_second_with_explicit_default_then_get_explicit_default` | Перенесён: tests/options.rs |

## options.bool.small.combined.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 5 | `when_when_multiple_short_flags_specified_then_parsed_as_short_option_group` | Перенесён: tests/options.rs |

## options.bool.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_boolean_option_not_specified_then_value_is_undefined` | Перенесён: tests/options.rs |
| 16 | `when_boolean_option_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 23 | `when_negatable_boolean_option_not_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 30 | `when_negatable_boolean_option_specified_then_value_is_false` | Перенесён: tests/options.rs |
| 39 | `when_boolean_option_not_specified_then_value_is_undefined` | Перенесён: tests/options.rs |
| 52 | `when_boolean_option_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 65 | `when_negatable_boolean_option_not_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 78 | `when_negatable_boolean_option_specified_then_value_is_false` | Перенесён: tests/options.rs |
| 96 | `when_option_not_specified_then_value_is_default` | Перенесён: tests/options.rs |
| 104 | `when_option_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 112 | `when_combo_option_and_negated_then_value_is_false` | Перенесён: tests/options.rs |
| 125 | `when_option_not_specified_then_value_is_undefined` | Перенесён: tests/options.rs |
| 132 | `when_flag_specified_then_value_is_true` | Перенесён: tests/options.rs |

## options.camelcase.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_option_defined_with_word_word_then_option_property_is_wordword` | Перенесён: tests/options.rs |
| 15 | `when_option_defined_with_word_word_then_option_property_is_wordword` | Перенесён: tests/options.rs |
| 22 | `when_option_defined_with_word_word_then_option_property_is_wordword` | Перенесён: tests/options.rs |
| 29 | `when_option_defined_with_word_word_word_then_option_property_is_wordwordword` | Перенесён: tests/options.rs |
| 36 | `when_option_defined_with_word_word_word_then_option_property_is_wordwordword` | Перенесён: tests/options.rs |
| 43 | `when_option_defined_with_word_then_option_property_is_word` | Перенесён: tests/options.rs |

## options.choices.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 7 | `when_option_argument_in_choices_then_option_set` | Перенесён: tests/options.rs |
| 16 | `when_option_argument_is_not_in_choices_then_error` | Перенесён: tests/options.rs |
| 28 | `when_choices_called_then_parameter_does_not_change` | Перенесён: tests/options.rs |
| 36 | `when_choices_called_and_argchoices_later_changed_then_parameter_does_not_change` | Перенесён: tests/options.rs |
| 44 | `when_choices_called_and_parameter_changed_the_choices_does_not_change` | Перенесён: tests/options.rs |

## options.conflicts.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 35 | `should_call_action_if_there_are_no_explicit_conflicting_options_set` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 43 | `should_call_action_when_there_are_no_implicit_conflicting_options_set` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 51 | `should_exit_with_error_if_conflicting_options_were_set` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 65 | `should_report_the_env_variable_as_the_conflicting_option_source_when_conflicting_option_is_set` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 81 | `should_report_the_env_variable_as_the_configured_option_source_when_configured_option_is_set` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 97 | `should_report_both_env_variables_as_sources_when_configured_option_and_conflicting_option_are_set` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 114 | `should_allow_default_value_with_a_conflicting_option` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 130 | `should_report_conflict_on_negated_option_flag` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 150 | `should_report_conflict_on_negated_option_env_variable` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 172 | `should_report_correct_error_for_shorthand_negated_option` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 191 | `should_report_correct_error_for_positive_option_when_negated_is_configured` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 208 | `should_report_correct_error_for_negated_option_when_positive_is_configured` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 227 | `should_report_correct_error_for_positive_env_variable_when_negated_is_configured` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 248 | `should_report_correct_error_for_negated_env_variable_when_positive_is_configured` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 269 | `should_report_correct_error_for_positive_option_with_string_value_when_negated_is_configured` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 289 | `should_report_correct_error_for_negated_option_with_preset_when_negated_is_configured` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 309 | `should_not_throw_error_when_conflicts_is_invoked_with_a_single_string_that_includes_another_option` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 327 | `should_throw_error_when_conflicts_is_invoked_with_a_single_string_that_equals_another_option` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 343 | `when_conflict_on_program_calling_action_subcommand_then_throw_conflict` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 360 | `when_conflict_on_program_calling_action_subcommand_with_help_then_show_help` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |
| 377 | `when_conflict_on_program_calling_external_subcommand_then_throw_conflict` | conflicts и диагностика конфликтующих значений не используются (регистрационные конфликты флагов проверены отдельно). |

## options.custom-processing.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 23 | `when_option_not_specified_then_callback_not_called` | Перенесён: tests/options.rs |
| 31 | `when_option_not_specified_then_value_is_undefined` | Перенесён: tests/options.rs |
| 38 | `when_starting_value_is_defined_and_option_not_specified_then_callback_not_called` | Перенесён: tests/options.rs |
| 46 | `when_starting_value_is_defined_and_option_not_specified_then_value_is_starting_value` | Перенесён: tests/options.rs |
| 55 | `when_option_specified_then_callback_called_with_value_and_previous` | Перенесён: tests/options.rs |
| 64 | `when_option_specified_then_value_is_as_returned_from_callback` | Перенесён: tests/options.rs |
| 74 | `when_starting_value_is_defined_and_option_specified_then_callback_called_with_value_and_starting_value` | Перенесён: tests/options.rs |
| 87 | `when_option_specified_multiple_times_then_callback_called_with_value_and_previousvalue` | Перенесён: tests/options.rs |
| 99 | `when_parsefloat_1e2_then_value_is_100` | Перенесён: tests/options.rs |
| 106 | `when_myparseint_1_then_value_is_1` | Перенесён: tests/options.rs |
| 113 | `when_increaseverbosity_v_v_v_then_value_is_3` | Перенесён: tests/options.rs |
| 125 | `when_collect_c_a_c_b_c_c_then_value_is_a_b_c` | Перенесён: tests/options.rs |
| 132 | `when_commaseparatedlist_x_y_z_then_value_is_x_y_z` | Перенесён: tests/options.rs |

## options.default.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_boolean_option_with_boolean_default_then_value_is_default` | Перенесён: tests/options.rs |
| 13 | `when_boolean_option_with_number_zero_default_then_value_is_zero` | Перенесён: tests/options.rs |
| 20 | `when_boolean_option_with_string_default_then_value_is_default` | Перенесён: tests/options.rs |
| 27 | `when_required_option_argument_and_default_number_then_value_is_default` | Перенесён: tests/options.rs |
| 34 | `when_required_option_argument_and_default_string_then_value_is_default` | Перенесён: tests/options.rs |
| 41 | `when_optional_option_argument_and_default_number_then_value_is_default` | Перенесён: tests/options.rs |
| 48 | `when_optional_option_argument_and_default_string_then_value_is_default` | Перенесён: tests/options.rs |
| 55 | `when_negated_and_default_string_then_value_is_default` | Перенесён: tests/options.rs |
| 65 | `when_boolean_option_with_boolean_default_then_value_is_default` | Перенесён: tests/options.rs |
| 72 | `when_boolean_option_with_number_zero_default_then_value_is_zero` | Перенесён: tests/options.rs |
| 79 | `when_boolean_option_with_string_default_then_value_is_default` | Перенесён: tests/options.rs |
| 86 | `when_required_option_argument_and_default_number_then_value_is_default` | Перенесён: tests/options.rs |
| 93 | `when_required_option_argument_and_default_string_then_value_is_default` | Перенесён: tests/options.rs |
| 100 | `when_optional_option_argument_and_default_number_then_value_is_default` | Перенесён: tests/options.rs |
| 107 | `when_optional_option_argument_and_default_string_then_value_is_default` | Перенесён: tests/options.rs |
| 114 | `when_negated_and_default_string_then_value_is_default` | Перенесён: tests/options.rs |
| 125 | `when_boolean_option_with_boolean_default_then_value_is_true` | Перенесён: tests/options.rs |
| 132 | `when_boolean_option_with_number_zero_default_then_value_is_true` | Перенесён: tests/options.rs |
| 139 | `when_boolean_option_with_string_default_then_value_is_true` | Перенесён: tests/options.rs |
| 146 | `when_required_option_argument_and_default_number_then_value_is_from_args` | Перенесён: tests/options.rs |
| 153 | `when_required_option_argument_and_default_string_then_value_is_from_args` | Перенесён: tests/options.rs |
| 160 | `when_optional_option_argument_and_default_number_and_option_argument_specified_then_value_is_from_args` | Перенесён: tests/options.rs |
| 167 | `when_optional_option_argument_and_default_string_and_option_argument_specified_then_value_is_from_args` | Перенесён: tests/options.rs |
| 174 | `when_optional_option_argument_and_default_number_and_option_argument_not_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 181 | `when_optional_option_argument_and_default_string_and_option_argument_not_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 188 | `when_negated_and_default_string_then_value_is_false` | Перенесён: tests/options.rs |

## options.dual-options.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_positive_option_then_stored_in_positiveoptions` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 18 | `when_negative_option_then_stored_in_negativeoptions` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 27 | `when_unrelated_positive_and_negative_options_then_no_dual_options` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 34 | `when_related_positive_and_negative_options_then_stored_as_dual_option` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 41 | `when_related_negative_and_positive_options_then_stored_as_dual_option` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 53 | `when_negativeoption_with_false_then_return_true` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 58 | `when_negativeoption_with_true_then_return_false` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 63 | `when_positiveoption_with_false_then_return_false` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 68 | `when_positiveoption_with_true_then_return_true` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 79 | `when_negativeoption_with_false_then_return_true` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 84 | `when_negativeoption_with_string_then_return_false` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 89 | `when_positiveoption_with_false_then_return_false` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 94 | `when_positiveoption_with_true_then_return_true` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |

## options.env.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 11 | `when_env_undefined_and_no_cli_then_option_undefined` | Переменные окружения для опций не используются. |
| 18 | `when_env_defined_and_no_cli_then_option_from_env` | Переменные окружения для опций не используются. |
| 27 | `when_env_defined_and_cli_then_option_from_cli` | Переменные окружения для опций не используются. |
| 36 | `when_env_defined_and_value_source_is_config_then_option_from_env` | Переменные окружения для опций не используются. |
| 46 | `when_env_defined_and_value_source_is_unspecified_then_option_unchanged` | Переменные окружения для опций не используются. |
| 56 | `when_default_and_env_undefined_and_no_cli_then_option_from_default` | Переменные окружения для опций не используются. |
| 65 | `when_default_and_env_defined_and_no_cli_then_option_from_env` | Переменные окружения для опций не используются. |
| 76 | `when_default_and_env_defined_and_cli_then_option_from_cli` | Переменные окружения для опций не используются. |
| 91 | `when_env_undefined_and_no_cli_then_option_undefined` | Переменные окружения для опций не используются. |
| 98 | `when_env_defined_with_value_and_no_cli_then_option_true` | Переменные окружения для опций не используются. |
| 107 | `when_env_is_and_no_cli_then_option_true` | Переменные окружения для опций не используются. |
| 117 | `when_env_is_0_and_no_cli_then_option_true` | Переменные окружения для опций не используются. |
| 127 | `when_env_is_false_and_no_cli_then_option_true` | Переменные окружения для опций не используются. |
| 139 | `when_env_undefined_and_no_cli_then_option_true` | Переменные окружения для опций не используются. |
| 146 | `when_env_defined_and_no_cli_then_option_false` | Переменные окружения для опций не используются. |
| 157 | `when_env_undefined_and_no_cli_then_option_undefined` | Переменные окружения для опций не используются. |
| 166 | `when_env_defined_and_no_cli_then_option_true` | Переменные окружения для опций не используются. |
| 177 | `when_env_defined_and_cli_no_foo_then_option_false` | Переменные окружения для опций не используются. |
| 188 | `when_no_env_defined_and_no_cli_then_option_false` | Переменные окружения для опций не используются. |
| 199 | `when_no_env_defined_and_cli_foo_then_option_true` | Переменные окружения для опций не используются. |
| 212 | `when_env_defined_and_no_cli_then_custom_parse_from_env` | Переменные окружения для опций не используются. |
| 227 | `when_env_defined_and_no_cli_then_array_from_env` | Переменные окружения для опций не используются. |
| 238 | `when_env_defined_and_cli_then_array_from_cli` | Переменные окружения для опций не используются. |
| 251 | `when_env_defined_on_another_subcommand_then_env_not_applied` | Переменные окружения для опций не используются. |
| 269 | `when_env_and_cli_defined_then_only_emit_option_event_for_cli` | Переменные окружения для опций не используются. |
| 286 | `when_env_and_cli_defined_then_only_parse_value_for_cli` | Переменные окружения для опций не используются. |
| 304 | `when_env_defined_then_emit_optionenv_and_not_option` | Переменные окружения для опций не используются. |
| 321 | `when_env_defined_for_required_then_emit_optionenv_with_value` | Переменные окружения для опций не используются. |
| 334 | `when_env_defined_for_optional_then_emit_optionenv_with_value` | Переменные окружения для опций не используются. |
| 347 | `when_env_defined_for_boolean_then_emit_optionenv_with_no_param` | Переменные окружения для опций не используются. |
| 359 | `when_env_defined_for_negated_boolean_then_emit_optionenv_with_no_param` | Переменные окружения для опций не используются. |

## options.flags.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_only_short_flag_defined_and_not_specified_then_value_is_undefined` | Перенесён: tests/options.rs |
| 15 | `when_only_short_flag_defined_and_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 22 | `when_only_long_flag_defined_and_not_specified_then_value_is_undefined` | Перенесён: tests/options.rs |
| 29 | `when_only_long_flag_defined_and_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 36 | `when_short_long_flags_defined_and_short_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 43 | `when_short_long_flags_defined_and_long_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 50 | `when_short_long_flags_defined_and_short_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 57 | `when_short_long_flags_defined_and_long_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 64 | `when_short_long_flags_defined_and_short_specified_then_value_is_true` | Перенесён: tests/options.rs |
| 71 | `when_short_long_flags_defined_and_long_specified_then_value_is_true` | Перенесён: tests/options.rs |

## options.getset.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_storeoptionsasproperties_is_storeoptionsasproperties_and_option_specified_on_cli_then_value_returned_by_getoptionvalue` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 19 | `when_storeoptionsasproperties_is_storeoptionsasproperties_and_setoptionvalue_then_value_returned_by_opts` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 32 | `when_setoptionvaluewithsource_then_value_returned_by_opts` | Публичные setters и сведения об источниках опций не используются. |
| 41 | `when_setoptionvaluewithsource_then_source_returned_by_getoptionvaluesource` | Публичные setters и сведения об источниках опций не используются. |
| 49 | `when_option_value_parsed_from_env_then_option_source_is_env` | Публичные setters и сведения об источниках опций не используются. |
| 58 | `when_option_value_parsed_from_cli_then_option_source_is_cli` | Публичные setters и сведения об источниках опций не используются. |
| 65 | `when_setoptionvalue_then_clears_previous_source` | Публичные setters и сведения об источниках опций не используются. |

## options.implies.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_source_undefined_and_implied_undefined_then_implied_is_undefined` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 17 | `when_source_default_and_implied_undefined_then_implied_is_undefined` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 28 | `when_source_from_env_and_implied_undefined_then_implied_is_implied` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 40 | `when_source_from_cli_and_implied_undefined_then_implied_is_implied` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 49 | `when_source_cli_and_implied_default_then_implied_is_implied` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 58 | `when_source_cli_and_env_default_then_implied_is_env` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 71 | `when_imply_non_option_then_ok_and_stored` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 78 | `when_imply_multiple_values_then_store_multiple_values` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 88 | `when_imply_multiple_times_then_store_multiple_values` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 100 | `when_imply_from_positive_option_then_positive_implied` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 109 | `when_imply_from_negative_option_then_negative_implied` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 118 | `when_imply_from_lone_negative_option_then_negative_implied` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 125 | `when_imply_from_negative_option_with_preset_then_negative_implied` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 136 | `when_chained_implies_then_only_explicitly_trigger` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 146 | `when_looped_implies_then_no_infinite_loop` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 155 | `when_conflict_with_implied_value_then_throw` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 167 | `when_requiredoption_with_implied_value_then_not_throw` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 180 | `when_implies_on_program_and_use_subcommand_then_program_updated` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 188 | `when_option_with_implies_used_multiple_times_then_implied_gets_single_value` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 197 | `when_implied_option_has_custom_processing_then_custom_processing_not_called` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |
| 210 | `when_passed_string_then_treat_as_boolean` | implies и внутренний DualOptions не используются; обычная пара положительной и отрицательной опций проверена отдельно. |

## options.mandatory.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 11 | `when_program_has_required_value_specified_then_value_as_specified` | Перенесён: tests/command.rs |
| 18 | `when_program_has_option_with_name_different_than_property_then_still_recognised` | Перенесён: tests/command.rs |
| 25 | `when_program_has_required_value_default_then_default_value` | Перенесён: tests/command.rs |
| 32 | `when_program_has_optional_value_flag_specified_then_true` | Перенесён: tests/command.rs |
| 39 | `when_program_has_optional_value_default_then_default_value` | Перенесён: tests/command.rs |
| 46 | `when_program_has_value_no_flag_specified_with_value_then_specified_value` | Перенесён: tests/command.rs |
| 55 | `when_program_has_mandatory_yes_no_flag_specified_with_flag_then_true` | Перенесён: tests/command.rs |
| 64 | `when_program_has_mandatory_yes_mandatory_no_flag_specified_with_flag_then_true` | Перенесён: tests/command.rs |
| 73 | `when_program_has_yes_no_flag_specified_negated_then_false` | Перенесён: tests/command.rs |
| 82 | `when_program_has_required_value_specified_and_subcommand_then_specified_value` | Перенесён: tests/command.rs |
| 94 | `when_program_has_required_option_not_specified_then_error` | Перенесён: tests/command.rs |
| 103 | `when_program_has_optional_option_not_specified_then_error` | Перенесён: tests/command.rs |
| 112 | `when_program_has_yes_no_not_specified_then_error` | Перенесён: tests/command.rs |
| 123 | `when_program_has_required_value_not_specified_and_subcommand_then_error` | Перенесён: tests/command.rs |
| 137 | `when_command_has_required_value_specified_then_no_error_and_option_has_specified_value` | Перенесён: tests/command.rs |
| 152 | `when_command_has_required_value_specified_using_env_then_no_error_and_option_has_specified_value` | Переменные окружения для опций не используются. |
| 169 | `when_command_has_required_value_not_specified_then_error` | Перенесён: tests/command.rs |
| 181 | `when_command_has_required_value_but_not_called_then_no_error` | Перенесён: tests/command.rs |
| 196 | `when_program_has_required_option_not_specified_and_help_then_help` | Перенесён: tests/command.rs |
| 208 | `when_program_has_required_option_not_specified_and_subcommand_help_then_help` | Перенесён: tests/command.rs |

## options.optional.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 7 | `when_option_not_specified_then_value_is_undefined` | Перенесён: tests/options.rs |
| 14 | `when_option_specified_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 22 | `when_option_specified_without_value_then_value_is_true` | Перенесён: tests/options.rs |
| 29 | `when_option_specified_without_value_and_following_option_then_value_is_true` | Перенесён: tests/options.rs |
| 40 | `when_option_not_specified_then_value_is_default` | Перенесён: tests/options.rs |
| 48 | `when_option_specified_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 57 | `when_option_specified_without_value_then_value_is_true` | Перенесён: tests/options.rs |
| 65 | `when_option_specified_without_value_and_preset_then_value_is_preset` | Перенесён: tests/options.rs |

## options.opts.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_version_used_with_storeoptionsasproperties_then_version_in_opts` | storeOptionsAsProperties не используется; значения доступны через opts. |
| 17 | `when_version_used_with_storeoptionsasproperties_false_then_version_not_in_opts` | Перенесён: tests/options.rs / tests/repeated_parse.rs |
| 29 | `when_storeoptionsasproperties_is_storeoptionsasproperties_and_boolean_flag_not_specified_then_not_in_opts` | Перенесён: tests/options.rs / tests/repeated_parse.rs |
| 38 | `when_storeoptionsasproperties_is_storeoptionsasproperties_and_boolean_flag_specified_then_value_true` | Перенесён: tests/options.rs / tests/repeated_parse.rs |
| 46 | `when_storeoptionsasproperties_is_storeoptionsasproperties_and_option_with_required_value_not_specified_then_not_in_opts` | Перенесён: tests/options.rs / tests/repeated_parse.rs |
| 55 | `when_storeoptionsasproperties_is_storeoptionsasproperties_and_option_with_required_value_specified_then_value_as_specified` | Перенесён: tests/options.rs / tests/repeated_parse.rs |
| 64 | `when_storeoptionsasproperties_is_storeoptionsasproperties_and_option_with_default_value_not_specified_then_default_value_in_opts` | Перенесён: tests/options.rs / tests/repeated_parse.rs |

## options.optsWithGlobals.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_variety_of_options_used_with_program_then_opts_is_same_as_optswithglobals` | optsWithGlobals и раздел Global Options не используются. |
| 21 | `when_options_in_sub_and_program_then_optswithglobals_includes_both` | optsWithGlobals и раздел Global Options не используются. |
| 36 | `when_options_in_sub_and_subsub_then_optswithglobals_includes_both` | optsWithGlobals и раздел Global Options не используются. |
| 54 | `when_same_named_option_in_sub_and_program_then_optswithglobals_includes_global` | optsWithGlobals и раздел Global Options не используются. |
| 71 | `when_option_used_with_simple_command_then_source_is_defined` | optsWithGlobals и раздел Global Options не используются. |
| 79 | `when_option_used_with_program_then_source_is_defined` | optsWithGlobals и раздел Global Options не используются. |
| 91 | `when_option_used_with_subcommand_then_source_is_defined` | optsWithGlobals и раздел Global Options не используются. |
| 103 | `when_same_named_option_in_sub_and_program_then_source_is_defined_by_global` | optsWithGlobals и раздел Global Options не используются. |

## options.preset.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_boolean_option_with_string_preset_used_then_value_is_preset` | Перенесён: tests/repeated_parse.rs |
| 13 | `when_boolean_option_with_number_preset_used_then_value_is_preset` | Перенесён: tests/repeated_parse.rs |
| 20 | `when_optional_with_string_preset_used_then_value_is_preset` | Перенесён: tests/repeated_parse.rs |
| 27 | `when_optional_with_number_preset_used_then_value_is_preset` | Перенесён: tests/repeated_parse.rs |
| 34 | `when_optional_with_string_preset_used_with_option_argument_then_value_is_as_specified` | Перенесён: tests/repeated_parse.rs |
| 41 | `when_optional_with_preset_and_coerce_used_then_preset_is_coerced` | Перенесён: tests/repeated_parse.rs |
| 50 | `when_optional_with_preset_and_variadic_used_then_preset_is_concatenated` | Перенесён: tests/repeated_parse.rs |
| 57 | `when_negated_with_string_preset_used_then_value_is_preset` | Перенесён: tests/repeated_parse.rs |

## options.registerClash.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 6 | `when_short_option_flag_conflicts_then_throws` | Перенесён: tests/repeated_parse.rs |
| 20 | `when_long_option_flag_conflicts_then_throws` | Перенесён: tests/repeated_parse.rs |
| 32 | `when_use_help_options_separately_then_does_not_throw` | Перенесён: tests/repeated_parse.rs |
| 39 | `when_reuse_flags_in_subcommand_then_does_not_throw` | Перенесён: tests/repeated_parse.rs |
| 49 | `when_short_option_flags_conflicts_then_throws` | Перенесён: tests/repeated_parse.rs |
| 58 | `when_long_option_flags_conflicts_then_throws` | Перенесён: tests/repeated_parse.rs |

## options.required.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_option_not_specified_then_value_is_undefined` | Перенесён: tests/options.rs |
| 15 | `when_option_specified_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 23 | `when_option_value_not_specified_then_error` | Перенесён: tests/options.rs |
| 41 | `when_option_not_specified_then_value_is_default` | Перенесён: tests/options.rs |
| 49 | `when_option_specified_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 58 | `when_option_value_not_specified_then_error` | Перенесён: tests/options.rs |

## options.twice.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 9 | `when_boolean_option_used_twice_then_value_is_true` | Перенесён: tests/repeated_parse.rs |
| 16 | `when_boolean_option_with_default_used_twice_then_value_is_true` | Перенесён: tests/repeated_parse.rs |
| 23 | `when_boolean_option_with_preset_used_twice_then_value_is_preset` | Перенесён: tests/repeated_parse.rs |
| 30 | `when_option_with_required_argument_used_twice_then_value_is_from_second_use` | Перенесён: tests/repeated_parse.rs |
| 37 | `when_option_with_optional_argument_used_second_time_without_value_then_value_is_true` | Перенесён: tests/repeated_parse.rs |
| 44 | `when_option_with_optional_argument_used_second_time_with_value_then_value_is_from_second_use` | Перенесён: tests/repeated_parse.rs |

## options.values.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 21 | `when_option_with_required_value_specified_as_value_and_short_flag_followed_by_value_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 27 | `when_option_with_required_value_specified_as_value_and_short_flag_concatenated_with_value_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 33 | `when_option_with_required_value_specified_as_value_and_long_flag_followed_by_value_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 39 | `when_option_with_required_value_specified_as_value_and_long_flag_value_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 55 | `when_short_flag_followed_by_value_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 61 | `when_short_flag_concatenated_with_value_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 67 | `when_long_flag_followed_by_value_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 73 | `when_long_flag_value_then_value_is_as_specified` | Перенесён: tests/options.rs |
| 79 | `when_long_flag_followed_empty_string_then_value_is_empty_string` | Перенесён: tests/options.rs |

## options.variadic.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_variadic_with_value_missing_then_error` | Перенесён: tests/options.rs |
| 17 | `when_variadic_with_one_value_then_set_in_array` | Перенесён: tests/options.rs |
| 25 | `when_variadic_with_two_values_then_set_in_array` | Перенесён: tests/options.rs |
| 33 | `when_variadic_with_repeated_values_then_set_in_array` | Перенесён: tests/options.rs |
| 43 | `when_variadic_used_with_choices_and_one_value_then_set_in_array` | Перенесён: tests/options.rs |
| 56 | `when_variadic_used_with_choices_and_two_values_then_set_in_array` | Перенесён: tests/options.rs |
| 69 | `when_variadic_with_short_combined_argument_then_not_variadic` | Перенесён: tests/options.rs |
| 77 | `when_variadic_with_long_combined_argument_then_not_variadic` | Перенесён: tests/options.rs |
| 85 | `when_variadic_with_value_followed_by_option_then_option_not_eaten` | Перенесён: tests/options.rs |
| 98 | `when_variadic_with_no_value_and_default_then_set_to_default` | Перенесён: tests/options.rs |
| 110 | `when_variadic_with_coercion_then_coercion_sets_value` | Перенесён: tests/options.rs |
| 126 | `when_variadic_not_specified_then_value_undefined` | Перенесён: tests/options.rs |
| 134 | `when_variadic_used_as_boolean_flag_then_value_true` | Перенесён: tests/options.rs |
| 142 | `when_variadic_with_one_value_then_set_in_array` | Перенесён: tests/options.rs |
| 150 | `when_variadic_with_two_values_then_set_in_array` | Перенесён: tests/options.rs |
| 160 | `when_option_flags_has_word_character_before_dots_then_is_variadic` | Перенесён: tests/options.rs |
| 167 | `when_option_flags_has_special_characters_before_dots_then_not_variadic` | Перенесён: tests/options.rs |
| 176 | `when_option_has_default_array_then_specified_value_is_used_instead_of_default_not_appended` | Перенесён: tests/options.rs |
| 184 | `when_option_has_default_array_then_specified_value_is_used_instead_of_default_not_appended` | Перенесён: tests/options.rs |

## options.version.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 8 | `when_no_version_and_specify_version_then_unknown_option_error` | Перенесён: tests/repeated_parse.rs |
| 20 | `when_no_version_then_helpinformation_does_not_include_version` | Перенесён: tests/repeated_parse.rs |
| 29 | `when_specify_default_short_flag_then_display_version` | Перенесён: tests/repeated_parse.rs |
| 45 | `when_specify_default_long_flag_then_display_version` | Перенесён: tests/repeated_parse.rs |
| 61 | `when_default_version_then_helpinformation_includes_default_version_help` | Перенесён: tests/repeated_parse.rs |
| 72 | `when_specify_custom_short_flag_then_display_version` | Перенесён: tests/repeated_parse.rs |
| 88 | `when_specify_just_custom_short_flag_then_display_version` | Перенесён: tests/repeated_parse.rs |
| 104 | `when_specify_custom_long_flag_then_display_version` | Перенесён: tests/repeated_parse.rs |
| 120 | `when_specify_just_custom_long_flag_then_display_version` | Перенесён: tests/repeated_parse.rs |
| 136 | `when_custom_version_then_helpinformation_includes_custom_version_help` | Перенесён: tests/repeated_parse.rs |
| 149 | `when_have_version_version_and_specify_version_then_command_called` | Перенесён: tests/repeated_parse.rs |
| 159 | `when_have_version_version_and_specify_version_then_version_displayed` | Перенесён: tests/repeated_parse.rs |
| 177 | `when_specify_version_then_can_get_version` | Getter version не используется; регистрация и вывод версии перенесены. |

## useColor.test.js

| Строка | Случай (snake_case) | Решение |
| ---: | --- | --- |
| 27 | `when_no_env_defined_then_returns_undefined` | Цвета и пользовательские стили исключены заданием. |
| 36 | `when_no_color_defined_then_returns_false` | Цвета и пользовательские стили исключены заданием. |
| 41 | `when_no_color_empty_then_returns_undefined` | Цвета и пользовательские стили исключены заданием. |
| 48 | `when_clicolor_force_defined_then_returns_true` | Цвета и пользовательские стили исключены заданием. |
| 53 | `when_clicolor_force_empty_then_returns_true` | Цвета и пользовательские стили исключены заданием. |
| 59 | `when_clicolor_force_and_no_color_defined_then_returns_false` | Цвета и пользовательские стили исключены заданием. |
| 84 | `when_force_color_value_then_returns_result` | Цвета и пользовательские стили исключены заданием. |

