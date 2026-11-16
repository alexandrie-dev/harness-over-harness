mod support;
use hoh_commander::{CommanderError, Opt, Output, ParseError, ParseOptions};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use support::{Capture, failure, program};

// command.name.test.js, command.description.test.js, command.usage.test.js.
#[test]
fn when_name_not_set_and_no_script_argument_then_name_is_program() {
    let mut p = program();
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert_eq!(p.get_name(), "program");
}
#[test]
fn when_set_program_name_and_parse_then_name_is_as_assigned() {
    let mut p = program();
    p.name("custom");
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert_eq!(p.get_name(), "custom");
    assert!(p.help_information().starts_with("Usage: custom "));
}
#[test]
fn when_add_command_then_command_is_named() {
    let mut p = program();
    assert_eq!(p.command("sub [file]").get_name(), "sub");
}
#[test]
fn when_set_description_then_get_description() {
    let mut p = program();
    p.description("test description");
    assert_eq!(p.get_description(), "test description");
}
#[test]
fn when_usage_custom_or_default_then_help_starts_with_usage() {
    let mut p = program();
    p.name("test");
    assert!(p.help_information().starts_with("Usage: test [options]\n"));
    p.usage("custom usage");
    assert!(
        p.help_information()
            .starts_with("Usage: test custom usage\n")
    );
    let sub = p.command("sub");
    assert!(
        sub.help_information()
            .starts_with("Usage: test sub [options]\n")
    );
    sub.usage("custom usage");
    assert!(
        sub.help_information()
            .starts_with("Usage: test sub custom usage\n")
    );
}
#[test]
fn when_options_commands_arguments_then_usage_contains_all() {
    let mut p = program();
    p.help_option_enabled(false);
    assert_eq!(p.get_usage(), "");
    p.option("--debug", "");
    assert_eq!(p.get_usage(), "[options]");
    p.command("sub");
    assert_eq!(p.get_usage(), "[options] [command]");
    p.argument("<file>", "");
    assert_eq!(p.get_usage(), "[options] [command] <file>");
    let mut p = program();
    p.help_option_enabled(false).argument("<file>", "");
    assert_eq!(p.get_usage(), "<file>");
}
// command.action.test.js, cmdline.spec.ts action interception.
#[test]
fn when_action_called_then_command_options_and_processed_arguments_passed() {
    let called = Rc::new(Cell::new(0));
    let capture = called.clone();
    let mut p = program();
    p.name("test")
        .option("--debug", "")
        .argument("[file]", "")
        .action(move |args, opts, command| {
            capture.set(capture.get() + 1);
            assert_eq!(args, [Some(json!("f"))]);
            assert_eq!(opts["debug"], true);
            assert_eq!(command.get_name(), "test");
            assert_eq!(command.args, ["f"]);
            Ok(())
        });
    assert!(p.has_action());
    assert!(p.has_action_recursive());
    p.parse(["f", "--debug"], ParseOptions::USER).unwrap();
    assert_eq!(called.get(), 1);
}
#[test]
fn when_action_on_program_and_no_arguments_then_action_called() {
    let called = Rc::new(Cell::new(false));
    let capture = called.clone();
    let mut p = program();
    p.action(move |args, opts, _| {
        assert!(args.is_empty());
        assert!(opts.is_empty());
        capture.set(true);
        Ok(())
    });
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert!(called.get());
}
#[test]
fn when_subcommand_action_then_root_action_is_not_called() {
    let mut p = program();
    p.action(|_, _, _| panic!("root action must not run"));
    p.command("sub").action(|_, _, cmd| {
        assert_eq!(cmd.get_name(), "sub");
        Ok(())
    });
    assert!(p.has_action_recursive());
    p.parse(["sub"], ParseOptions::USER).unwrap();
}
#[test]
fn when_action_throws_non_commander_error_then_it_is_distinguishable() {
    let mut p = program();
    p.action(|_, _, _| Err(ParseError::other(std::io::Error::other("action failure"))));
    let e = p.parse([] as [&str; 0], ParseOptions::USER).err().unwrap();
    assert!(e.commander().is_none());
    assert_eq!(e.to_string(), "action failure");
}
#[test]
fn when_action_calls_error_then_output_and_commander_exit() {
    let mut p = program();
    let output = Capture::default();
    output.install(&mut p);
    p.action(|_, _, cmd| Err(cmd.error("error: rejected").into()));
    assert_eq!(
        failure(&mut p, &[]),
        CommanderError::new(1, "commander.error", "error: rejected")
    );
    assert_eq!(*output.err.borrow(), "error: rejected\n");
}
// command.error.test.js + command.configureOutput.test.js.
#[test]
fn when_error_called_with_code_and_exit_code_then_exact_error() {
    let output = Capture::default();
    let mut p = program();
    output.install(&mut p);
    assert_eq!(
        p.error_with_options("Oops", 2, "custom.code"),
        CommanderError::new(2, "custom.code", "Oops")
    );
    assert_eq!(*output.err.borrow(), "Oops\n");
    assert!(output.out.borrow().is_empty());
    assert_eq!(p.error("default").exit_code, 1);
}
#[test]
fn when_no_override_and_error_then_exit_callback_called() {
    let output = Capture::default();
    let mut p = hoh_commander::Command::new();
    let err = output.err.clone();
    p.configure_output(Output::new(|_| {}, move |s| err.borrow_mut().push_str(s)));
    let exits = Rc::new(RefCell::new(Vec::new()));
    let capture = exits.clone();
    p.exit_callback(move |code| capture.borrow_mut().push(code));
    assert_eq!(p.error("Oops").exit_code, 1);
    assert_eq!(p.error_with_options("Oops", 2, "code").exit_code, 2);
    assert_eq!(*exits.borrow(), [1, 2]);
    p.exit_override();
    p.error("Oops");
    assert_eq!(*exits.borrow(), [1, 2]);
}
#[test]
fn when_help_or_version_then_output_is_routed() {
    let mut p = program();
    let output = Capture::default();
    output.install(&mut p);
    p.version("1.2.3");
    let e = failure(&mut p, &["--version"]);
    assert_eq!(e, CommanderError::new(0, "commander.version", "1.2.3"));
    assert_eq!(*output.out.borrow(), "1.2.3\n");
    assert!(output.err.borrow().is_empty());
    output.out.borrow_mut().clear();
    assert_eq!(failure(&mut p, &["--help"]).code, "commander.helpDisplayed");
    assert_eq!(*output.out.borrow(), p.help_information());
    assert_eq!(p.help_with_error(true).exit_code, 1);
    assert_eq!(*output.err.borrow(), p.help_information());
}
#[test]
fn when_error_handler_replaced_then_grammar_errors_use_replacement() {
    let mut p = program();
    let output = Capture::default();
    output.install(&mut p);
    let json_out = output.out.clone();
    p.error_handler(move |message, status, code| { json_out.borrow_mut().push_str(&format!("{}\n", serde_json::to_string(&json!({"type":"error","message": message.strip_prefix("error: ").unwrap_or(message)})).unwrap())); CommanderError::new(status, code, message) });
    assert_eq!(
        failure(&mut p, &["--unknown"]).code,
        "commander.unknownOption"
    );
    assert!(output.err.borrow().is_empty());
    assert_eq!(
        *output.out.borrow(),
        "{\"type\":\"error\",\"message\":\"unknown option '--unknown'\"}\n"
    );
}
// command.allowUnknownOption.test.js, command.unknownOption.test.js.
#[test]
fn when_unknown_option_then_error_unless_allowed() {
    for allow in [false, true] {
        for subcommand in [false, true] {
            for action in [false, true] {
                for argument in [false, true] {
                    let mut p = program();
                    let cmd = if subcommand { p.command("sub") } else { &mut p };
                    cmd.allow_unknown_option(allow).argument("[args...]", "");
                    if action {
                        cmd.action(|_, _, _| Ok(()));
                    }
                    let mut args = Vec::new();
                    if subcommand {
                        args.push("sub");
                    }
                    if argument {
                        args.push("arg");
                    }
                    args.extend(["--unknown", "extra"]);
                    if allow {
                        p.parse(args, ParseOptions::USER).unwrap();
                        let cmd = if subcommand { &p.commands[0] } else { &p };
                        assert!(cmd.args.contains(&"--unknown".into()));
                    } else {
                        let e = failure(&mut p, &args);
                        assert_eq!(e.code, "commander.unknownOption");
                        assert_eq!(e.message, "error: unknown option '--unknown'");
                    }
                }
            }
        }
    }
}
#[test]
fn when_unknown_global_option_before_subcommand_then_error() {
    let mut p = program();
    p.command("sub");
    let e = failure(&mut p, &["--unknown", "sub"]);
    assert_eq!(e.code, "commander.unknownOption");
}
// command.unknownCommand.test.js.
#[test]
fn when_unknown_command_then_error_unless_action_takes_argument() {
    let mut p = program();
    assert_eq!(
        failure(&mut p, &["unknown"]).code,
        "commander.excessArguments"
    );
    p.argument("[arg]", "").action(|_, _, _| Ok(()));
    p.parse(["unknown"], ParseOptions::USER).unwrap();
    let mut p = program();
    p.command("sub");
    assert_eq!(
        failure(&mut p, &["unknown"]).message,
        "error: unknown command 'unknown'"
    );
    assert_eq!(
        failure(&mut p, &["sbu", "--silly"]).message,
        "error: unknown command 'sbu'\n(Did you mean sub?)"
    );
}
// options.mandatory.test.js: all synchronous cases except env.
#[test]
fn when_mandatory_option_then_default_presence_negation_and_help_checked() {
    for grammar in ["<value>", "[value]"] {
        let mut p = program();
        p.required_option(&format!("--required-option {grammar}"), "");
        assert_eq!(
            failure(&mut p, &[]).message,
            format!("error: required option '--required-option {grammar}' not specified")
        );
        p.parse(["--required-option", "value"], ParseOptions::USER)
            .unwrap();
        assert_eq!(p.opts()["requiredOption"], "value");
        if grammar.starts_with('[') {
            p.parse(["--required-option"], ParseOptions::USER).unwrap();
            assert_eq!(p.opts()["requiredOption"], true);
        }
        assert_eq!(failure(&mut p, &["--help"]).code, "commander.helpDisplayed");
        let mut p = program();
        p.add_option(
            Opt::new(format!("--required-option {grammar}"), "")
                .make_option_mandatory(true)
                .default("default"),
        );
        p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
        assert_eq!(p.opts()["requiredOption"], "default");
    }
    for (positive_mandatory, negative_mandatory) in [(true, false), (true, true), (false, true)] {
        let mut p = program();
        p.add_option(Opt::new("--cheese", "").make_option_mandatory(positive_mandatory))
            .add_option(Opt::new("--no-cheese", "").make_option_mandatory(negative_mandatory));
        assert_eq!(
            failure(&mut p, &[]).code,
            "commander.missingMandatoryOptionValue"
        );
        for (flag, expected) in [("--cheese", true), ("--no-cheese", false)] {
            p.parse([flag], ParseOptions::USER).unwrap();
            assert_eq!(p.opts()["cheese"], expected);
        }
    }
}
#[test]
fn when_required_option_in_ancestor_then_leaf_checks_it_after_help() {
    let mut p = program();
    p.required_option("--global <value>", "");
    p.command("sub").required_option("--local <value>", "");
    assert_eq!(
        failure(&mut p, &["sub"]).message,
        "error: required option '--local <value>' not specified"
    );
    assert_eq!(
        failure(&mut p, &["sub", "--local", "l"]).message,
        "error: required option '--global <value>' not specified"
    );
    p.parse(["--global", "g", "sub", "--local", "l"], ParseOptions::USER)
        .unwrap();
    assert_eq!(p.opts()["global"], "g");
    assert_eq!(p.commands[0].opts()["local"], "l");
    assert_eq!(failure(&mut p, &["sub", "--help"]).exit_code, 0);
    let mut p = program();
    p.command("unused").required_option("--need <value>", "");
    p.command("used").action(|_, _, _| Ok(()));
    p.parse(["used"], ParseOptions::USER).unwrap();
}
// command.exitOverride.test.js: exact messages and winning errors.
#[test]
fn when_missing_argument_then_error_before_excess_arguments() {
    let mut p = program();
    p.argument("<file>", "");
    assert_eq!(
        failure(&mut p, &[]),
        CommanderError::new(
            1,
            "commander.missingArgument",
            "error: missing required argument 'file'"
        )
    );
    assert_eq!(
        failure(&mut p, &["a", "b"]).message,
        "error: too many arguments. Expected 1 argument but got 2: a, b."
    );
    let mut p = program();
    p.command("sub");
    assert_eq!(
        failure(&mut p, &["sub", "a"]).message,
        "error: too many arguments for 'sub'. Expected 0 arguments but got 1: a."
    );
}
#[test]
fn when_help_requested_then_action_not_called_and_errors_suppressed() {
    let mut p = program();
    p.required_option("--needed <value>", "")
        .argument("<file>", "")
        .action(|_, _, _| panic!("help must not call action"));
    assert_eq!(failure(&mut p, &["--unknown", "--help"]).exit_code, 0);
    assert_eq!(
        failure(&mut p, &["--needed", "--help"]).code,
        "commander.missingArgument"
    );
    assert_eq!(
        failure(&mut p, &["--help", "--needed"]).code,
        "commander.optionMissingArgument"
    );
}
#[test]
fn when_allow_excess_arguments_then_preserve_args() {
    let mut p = program();
    p.allow_excess_arguments(true);
    p.parse(["a", "b"], ParseOptions::USER).unwrap();
    assert_eq!(p.args, ["a", "b"]);
    assert!(p.processed_args.is_empty());
}
// command.helpOption.test.js.
#[test]
fn when_custom_help_flags_then_each_invokes_help() {
    for flags in ["-H, --HELP", "-H", "--HELP"] {
        for flag in ["-H", "--HELP"] {
            if !Opt::new(flags, "").is(flag) {
                continue;
            }
            let mut p = program();
            p.help_option(flags, "custom description");
            assert!(p.help_information().contains("custom description"));
            assert_eq!(failure(&mut p, &[flag]).code, "commander.helpDisplayed");
        }
    }
}
#[test]
fn when_help_option_disabled_then_unknown_and_no_suggestion() {
    let mut p = program();
    p.help_option_enabled(false);
    assert!(!p.help_information().contains("--help"));
    for flag in ["-h", "--help", "--hepl"] {
        assert_eq!(
            failure(&mut p, &[flag]).message,
            format!("error: unknown option '{flag}'")
        );
    }
    p.help_option_enabled(true);
    assert!(p.help_information().contains("-h, --help"));
    p.help_option("-H, --HELP", "custom")
        .help_option_enabled(true);
    assert!(p.help_information().contains("-H, --HELP"));
}
// command.helpCommand.test.js.
#[test]
fn when_subcommands_then_implicit_help_command_unless_disabled() {
    let mut p = program();
    assert!(!p.help_information().contains("Commands:"));
    p.help_command(true);
    assert!(p.help_information().contains("help [command]"));
    p.help_command(false);
    p.command("sub");
    assert!(!p.help_information().contains("help [command]"));
    let mut p = program();
    p.name("test");
    p.command("sub");
    assert!(p.help_information().contains("help [command]"));
    assert_eq!(failure(&mut p, &["help"]).code, "commander.help");
    assert_eq!(failure(&mut p, &["help", "unknown"]).exit_code, 1);
    assert_eq!(failure(&mut p, &["help", "sub"]).exit_code, 0);
    p.commands[0].command("nested");
    assert_eq!(failure(&mut p, &["sub", "help", "nested"]).exit_code, 0);
    p.help_command_with_description("assist [cmd]", "show assistance");
    assert!(p.help_information().contains("assist [cmd]"));
    assert_eq!(failure(&mut p, &["assist", "sub"]).exit_code, 0);
}
#[test]
fn when_help_sub_and_child_help_flags_differ_or_disabled_then_help_still_works() {
    for flags in [None, Some("-H"), Some("--HELP")] {
        let mut p = program();
        let sub = p.command("sub");
        if let Some(flags) = flags {
            sub.help_option(flags, "help");
        } else {
            sub.help_option_enabled(false);
        }
        assert_eq!(failure(&mut p, &["help", "sub"]).exit_code, 0);
    }
}
// command.addHelpText.test.js (string positions).
#[test]
fn when_all_simple_help_positions_then_strings_in_order() {
    let mut p = program();
    let output = Capture::default();
    output.install(&mut p);
    p.name("test")
        .add_help_text("after", "A")
        .add_help_text("before", "B")
        .add_help_text("afterAll", "C")
        .add_help_text("beforeAll", "D");
    p.output_help(false);
    assert_eq!(
        *output.out.borrow(),
        format!("D\nB\n{}A\nC\n", p.help_information())
    );
    output.out.borrow_mut().clear();
    p.command("sub")
        .add_help_text("before", "E")
        .add_help_text("after", "F");
    p.commands[0].output_help(false);
    assert_eq!(
        *output.out.borrow(),
        format!("D\nE\n{}F\nC\n", p.commands[0].help_information())
    );
    p.output_help(true);
    assert_eq!(
        *output.err.borrow(),
        format!("D\nB\n{}A\nC\n", p.help_information())
    );
}
#[test]
fn when_silly_help_position_then_error() {
    let mut p = program();
    p.add_help_text("silly", "x");
    assert_eq!(
        p.parse([] as [&str; 0], ParseOptions::USER)
            .err()
            .unwrap()
            .to_string(),
        "Unexpected value for position to addHelpText.\nExpecting one of 'beforeAll', 'before', 'after', 'afterAll'"
    );
}
// command.copySettings.test.js: registration-time output and exit inheritance.
#[test]
fn when_configured_after_registration_then_existing_child_keeps_settings() {
    let mut p = program();
    let first = Capture::default();
    first.install(&mut p);
    p.command("sub");
    let second = Capture::default();
    second.install(&mut p);
    failure(&mut p, &["sub", "--unknown"]);
    assert_eq!(*first.err.borrow(), "error: unknown option '--unknown'\n");
    assert!(second.err.borrow().is_empty());
}

#[test]
fn when_action_with_argument_then_missing_required_prevents_action() {
    for required in [false, true] {
        for with_sub in [false, true] {
            for supplied in [false, true] {
                let mut p = program();
                p.argument(if required { "<file>" } else { "[file]" }, "");
                if with_sub {
                    p.command("subcommand");
                }
                let called = Rc::new(Cell::new(false));
                let capture = called.clone();
                p.action(move |args, _, _| {
                    capture.set(true);
                    assert_eq!(
                        args,
                        [if supplied {
                            Some(json!("my-file"))
                        } else {
                            None
                        }]
                    );
                    Ok(())
                });
                if required && !supplied {
                    assert_eq!(failure(&mut p, &[]).code, "commander.missingArgument");
                    assert!(!called.get());
                } else {
                    p.parse(
                        if supplied { vec!["my-file"] } else { vec![] },
                        ParseOptions::USER,
                    )
                    .unwrap();
                    assert!(called.get());
                }
            }
        }
    }
}
#[test]
fn when_unknown_option_allowed_and_only_subcommands_then_help_for_error() {
    let mut p = program();
    p.allow_unknown_option(true).command("foo");
    assert_eq!(failure(&mut p, &["--unknown"]).code, "commander.help");
}

#[test]
fn when_call_nested_subcommand_then_runs() {
    let mut p = program();
    let calls = Rc::new(Cell::new(0));
    let capture = calls.clone();
    p.command("sub1").command("sub2").action(move |_, _, _| {
        capture.set(capture.get() + 1);
        Ok(())
    });
    p.parse(["sub1", "sub2"], ParseOptions::USER).unwrap();
    assert_eq!(calls.get(), 1);
    assert!(
        p.commands[0].commands[0]
            .help_information()
            .starts_with("Usage: program sub1 sub2 [options]\n")
    );
}
