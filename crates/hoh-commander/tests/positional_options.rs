mod support;
use hoh_commander::ParseOptions;
use serde_json::json;
use support::{failure, program};

// command.positionalOptions.test.js: program passThrough and action variants.
#[test]
fn when_pass_through_then_options_after_first_argument_passed_through() {
    for action in [false, true] {
        for unknown in [false, true] {
            for enable in [false, true] {
                let mut p = program();
                p.pass_through_options(enable)
                    .allow_unknown_option(unknown)
                    .option("-d, --debug", "")
                    .argument("[args...]", "");
                if action {
                    p.action(|args, _, command| {
                        assert_eq!(args, command.processed_args);
                        Ok(())
                    });
                }
                p.parse(["arg", "--debug"], ParseOptions::USER).unwrap();
                assert_eq!(
                    p.args,
                    if enable {
                        vec!["arg", "--debug"]
                    } else {
                        vec!["arg"]
                    }
                );
                assert_eq!(
                    p.opts().get("debug"),
                    if enable { None } else { Some(&json!(true)) }
                );
                if enable {
                    p.parse(["arg", "--help", "--version", "--pass"], ParseOptions::USER)
                        .unwrap();
                    assert_eq!(p.args, ["arg", "--help", "--version", "--pass"]);
                }
            }
        }
    }
}
#[test]
fn when_option_before_command_argument_then_option_parsed() {
    let mut p = program();
    p.pass_through_options(true)
        .option("--debug", "")
        .argument("[args...]", "");
    p.parse(["--debug", "arg"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["debug"], true);
    assert_eq!(p.args, ["arg"]);
}
#[test]
fn when_pass_through_unknown_then_remaining_arguments_passed_through() {
    let mut p = program();
    p.pass_through_options(true)
        .allow_unknown_option(true)
        .option("--debug", "")
        .argument("[args...]", "");
    p.parse(["--unknown", "--debug"], ParseOptions::USER)
        .unwrap();
    assert_eq!(p.args, ["--unknown", "--debug"]);
    assert!(!p.opts().contains_key("debug"));
}
#[test]
fn when_positional_unknown_then_known_options_still_parsed() {
    let mut p = program();
    p.enable_positional_options(true)
        .allow_unknown_option(true)
        .option("--debug", "")
        .argument("[args...]", "");
    p.parse(["--unknown", "--debug"], ParseOptions::USER)
        .unwrap();
    assert_eq!(p.args, ["--unknown"]);
    assert_eq!(p.opts()["debug"], true);
}
#[test]
fn when_shared_options_around_subcommand_then_parsed_by_respective_command() {
    for after_argument in [false, true] {
        let mut p = program();
        p.enable_positional_options(true)
            .option("-s, --shared <value>", "");
        p.command("sub [arg]")
            .option("-s, --shared <value>", "")
            .action(|_, _, _| Ok(()));
        let args = if after_argument {
            vec!["--shared", "program", "sub", "arg", "--shared", "local"]
        } else {
            vec!["--shared", "program", "sub", "--shared", "local"]
        };
        p.parse(args, ParseOptions::USER).unwrap();
        assert_eq!(p.opts()["shared"], "program");
        assert_eq!(p.commands[0].opts()["shared"], "local");
    }
}
#[test]
fn when_positional_disabled_then_global_consumes_shared_option_after_subcommand() {
    for enable in [false, true] {
        let mut p = program();
        p.enable_positional_options(enable)
            .option("-d, --debug", "");
        p.command("sub").option("-d, --debug", "");
        p.parse(["sub", "--debug"], ParseOptions::USER).unwrap();
        assert_eq!(p.opts().contains_key("debug"), !enable);
        assert_eq!(p.commands[0].opts().contains_key("debug"), enable);
    }
}
#[test]
fn when_subcommand_pass_through_then_subcommand_consumes_only_its_prefix() {
    let mut p = program();
    p.enable_positional_options(true)
        .option("-s, --shared <value>", "");
    p.command("sub [args...]")
        .pass_through_options(true)
        .option("-s, --shared <value>", "")
        .option("-d, --debug", "")
        .action(|_, _, _| Ok(()));
    p.parse(
        [
            "--shared=global",
            "sub",
            "--shared=local",
            "arg",
            "--shared",
            "--help",
            "--debug",
        ],
        ParseOptions::USER,
    )
    .unwrap();
    assert_eq!(p.opts()["shared"], "global");
    assert_eq!(p.commands[0].opts()["shared"], "local");
    assert_eq!(p.commands[0].args, ["arg", "--shared", "--help", "--debug"]);
    p.parse(["sub", "--debug", "arg"], ParseOptions::USER)
        .unwrap();
    assert_eq!(p.commands[0].opts()["debug"], true);
    assert_eq!(p.commands[0].args, ["arg"]);
}
#[test]
fn when_program_action_and_positional_options_then_global_after_operand_still_parsed() {
    let mut p = program();
    p.enable_positional_options(true)
        .option("--global", "")
        .argument("<args...>", "")
        .action(|_, _, _| Ok(()));
    p.command("sub");
    for args in [
        vec!["--global", "foo"],
        vec!["foo", "--global"],
        vec!["foo", "sub", "--global"],
    ] {
        p.parse(args, ParseOptions::USER).unwrap();
        assert_eq!(p.opts()["global"], true);
    }
}
#[test]
fn when_program_pass_through_and_subcommand_then_subcommand_options_parsed() {
    let mut p = program();
    p.pass_through_options(true)
        .option("-g, --global", "")
        .argument("<args...>", "")
        .action(|_, _, _| Ok(()));
    p.command("sub [arg]")
        .option("-g, --group", "")
        .option("--debug", "")
        .action(|_, _, _| Ok(()));
    p.parse(["sub", "--debug", "-g"], ParseOptions::USER)
        .unwrap();
    assert_eq!(p.commands[0].opts()["debug"], true);
    assert_eq!(p.commands[0].opts()["group"], true);
    assert!(!p.opts().contains_key("global"));
}
#[test]
fn when_help_requested_then_parent_or_child_receives_help() {
    for args in [
        vec![],
        vec!["sub"],
        vec!["--help"],
        vec!["sub", "--help"],
        vec!["sub", "foo", "--help"],
        vec!["help"],
        vec!["help", "sub"],
    ] {
        let mut p = program();
        p.enable_positional_options(true);
        p.command("sub [arg]").action(|_, _, _| Ok(()));
        if args == ["sub"] {
            p.parse(args, ParseOptions::USER).unwrap();
        } else {
            assert!(
                ["commander.help", "commander.helpDisplayed"]
                    .contains(&failure(&mut p, &args).code.as_str())
            );
        }
    }
}
// parseOptions literal routing and short optional-value combination.
#[test]
fn when_unknown_sink_and_literal_then_literal_is_preserved() {
    let mut p = program();
    p.allow_unknown_option(true)
        .argument("[args...]", "")
        .option("--debug", "");
    p.parse(
        ["one", "--unknown", "two", "--", "--debug"],
        ParseOptions::USER,
    )
    .unwrap();
    assert_eq!(p.args, ["one", "--unknown", "two", "--", "--debug"]);
    assert!(!p.opts().contains_key("debug"));
}
#[test]
fn when_combine_flag_optional_value_disabled_then_short_group_processed() {
    let mut p = program();
    p.combine_flag_and_optional_value(false)
        .option("-a [value]", "")
        .option("-b", "");
    p.parse(["-ab"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["a"], true);
    assert_eq!(p.opts()["b"], true);
}

#[test]
fn when_program_not_positional_and_child_pass_through_then_error() {
    let mut p = program();
    p.command("sub").pass_through_options(true);
    assert_eq!(
        p.parse(["sub"], ParseOptions::USER)
            .err()
            .unwrap()
            .to_string(),
        "passThroughOptions cannot be used for 'sub' without turning on enablePositionalOptions for parent command(s)"
    );
}
