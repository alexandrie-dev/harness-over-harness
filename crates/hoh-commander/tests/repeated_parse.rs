mod support;
use hoh_commander::{Argument, Opt, ParseOptions};
use serde_json::{Value, json};
use support::{failure, program};

// options.twice.test.js: all six cases.
#[test]
fn when_repeated_option_then_second_value_wins() {
    for (option, args, expected) in [
        (Opt::new("-d, --debug", ""), vec!["-d", "-d"], json!(true)),
        (
            Opt::new("-d, --debug", "description").default("foo"),
            vec!["-d", "-d"],
            json!(true),
        ),
        (
            Opt::new("-d, --debug", "").preset("foo"),
            vec!["-d", "-d"],
            json!("foo"),
        ),
        (
            Opt::new("-p, --port <port-number>", ""),
            vec!["-p", "1", "-p", "2"],
            json!("2"),
        ),
        (
            Opt::new("--donate [amount]", ""),
            vec!["--donate", "123", "--donate"],
            json!(true),
        ),
        (
            Opt::new("--donate [amount]", ""),
            vec!["--donate", "--donate", "123"],
            json!("123"),
        ),
    ] {
        let mut p = program();
        let key = option.attribute_name();
        p.add_option(option);
        p.parse(args, ParseOptions::USER).unwrap();
        assert_eq!(p.opts()[&key], expected);
    }
}
// options.preset.test.js: all eight cases.
#[test]
fn when_preset_used_then_preserve_type_or_coerce() {
    for flags in ["--debug", "--debug [value]", "--no-debug"] {
        for preset in [json!("foo"), json!(123)] {
            let mut p = program();
            p.add_option(Opt::new(flags, "").preset(preset.clone()));
            p.parse(
                [flags.split_whitespace().next().unwrap()],
                ParseOptions::USER,
            )
            .unwrap();
            assert_eq!(p.opts()["debug"], preset);
        }
    }
    let mut p = program();
    p.add_option(Opt::new("--debug [value]", "").preset("foo"));
    p.parse(["--debug", "bar"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["debug"], "bar");
    let mut p = program();
    p.add_option(
        Opt::new("--number [value]", "")
            .preset("123")
            .arg_parser(|s, _| Ok(json!(s.parse::<i64>().unwrap()))),
    );
    p.parse(["--number"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["number"], 123);
    let mut p = program();
    p.add_option(Opt::new("--debug [value...]", "").preset("foo"));
    p.parse(["--debug", "--debug", "bar"], ParseOptions::USER)
        .unwrap();
    assert_eq!(p.opts()["debug"], json!(["foo", "bar"]));
}
// command.parse.test.js: user arguments are readonly snapshots; lazy reset.
#[test]
fn when_parse_called_then_input_and_command_snapshots_are_independent() {
    let mut input = vec!["a".to_string(), "b".into()];
    let mut p = program();
    p.argument("[args...]", "");
    let returned = p.parse(&input, ParseOptions::USER).unwrap();
    assert_eq!(returned.args, input);
    assert_eq!(p.raw_args, input);
    p.args.push("c".into());
    assert_eq!(input, ["a", "b"]);
    input.push("d".into());
    assert_eq!(p.raw_args, ["a", "b"]);
}
#[test]
fn when_parse_repeated_then_options_and_arguments_reset() {
    for (option, args, before, after) in [
        (
            Opt::new("--debug", ""),
            vec!["--debug"],
            Some(json!(true)),
            None,
        ),
        (
            Opt::new("--debug <value>", ""),
            vec!["--debug", "a"],
            Some(json!("a")),
            None,
        ),
        (
            Opt::new("--debug <value>", "").default("default"),
            vec!["--debug", "a"],
            Some(json!("a")),
            Some(json!("default")),
        ),
        (
            Opt::new("--no-debug", ""),
            vec!["--no-debug"],
            Some(json!(false)),
            Some(json!(true)),
        ),
        (
            Opt::new("--debug <value...>", ""),
            vec!["--debug", "a", "b"],
            Some(json!(["a", "b"])),
            None,
        ),
    ] {
        let mut p = program();
        p.add_option(option);
        p.parse(args, ParseOptions::USER).unwrap();
        assert_eq!(p.opts().get("debug"), before.as_ref());
        p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
        assert_eq!(p.opts().get("debug"), after.as_ref());
    }
    let mut p = program();
    p.argument("[args...]", "");
    p.parse(["a", "b"], ParseOptions::USER).unwrap();
    p.parse(["c"], ParseOptions::USER).unwrap();
    assert_eq!(p.args, ["c"]);
    assert_eq!(p.raw_args, ["c"]);
    assert_eq!(p.processed_args, [Some(json!(["c"]))]);
}
#[test]
fn when_collect_and_verbosity_then_each_parse_uses_original_default() {
    let mut p = program();
    p.option_with_parser("--collect <value>", "", |s, previous| {
        let mut values = previous
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        values.push(json!(s));
        Ok(json!(values))
    })
    .option_with_parser_and_default(
        "-v",
        "",
        |_, previous| Ok(json!(previous.and_then(Value::as_i64).unwrap_or(0) + 1)),
        0,
    );
    p.parse(["--collect", "a", "-vv"], ParseOptions::USER)
        .unwrap();
    p.parse(["--collect", "b", "-v"], ParseOptions::USER)
        .unwrap();
    assert_eq!(p.opts()["collect"], json!(["b"]));
    assert_eq!(p.opts()["v"], 1);
}
#[test]
fn when_call_different_subcommand_then_previous_child_not_reset() {
    let mut p = program();
    p.option("--global", "");
    p.command("one").option("--local", "");
    p.command("two").option("--local", "");
    p.parse(["--global", "one", "--local"], ParseOptions::USER)
        .unwrap();
    p.parse(["two"], ParseOptions::USER).unwrap();
    assert!(p.opts().is_empty());
    assert_eq!(p.commands[0].opts()["local"], true);
    assert!(p.commands[1].opts().is_empty());
    p.parse(["one"], ParseOptions::USER).unwrap();
    assert!(p.commands[0].opts().is_empty());
}
// command.allowExcessArguments.test.js: enabled and disabled, zero/one/variadic.
#[test]
fn when_excess_arguments_then_check_allow_setting_and_argument_grammar() {
    for sub in [false, true] {
        for allow in [false, true] {
            for grammar in [None, Some("<arg>"), Some("[arg]"), Some("[args...]")] {
                let mut p = program();
                p.allow_excess_arguments(allow);
                let cmd = if sub { p.command("sub") } else { &mut p };
                if let Some(grammar) = grammar {
                    cmd.argument(grammar, "");
                }
                let mut args = if sub { vec!["sub"] } else { vec![] };
                args.extend(["a", "b"]);
                if allow || grammar == Some("[args...]") {
                    p.parse(args, ParseOptions::USER).unwrap();
                } else {
                    assert_eq!(failure(&mut p, &args).code, "commander.excessArguments");
                }
                if grammar.is_some() {
                    let args = if sub { vec!["sub", "a"] } else { vec!["a"] };
                    p.parse(args, ParseOptions::USER).unwrap();
                }
            }
        }
    }
}
#[test]
fn when_multiple_argument_methods_then_arguments_added_in_order() {
    let mut p = program();
    p.argument("first", "")
        .add_argument(Argument::new("[second]", ""));
    p.parse(["a", "b"], ParseOptions::USER).unwrap();
    assert_eq!(p.processed_args, [Some(json!("a")), Some(json!("b"))]);
}
// options.version.test.js: standard and custom flags, help and dispatch.
#[test]
fn when_version_flags_then_exact_stdout_and_exit() {
    let mut p = program();
    assert_eq!(
        failure(&mut p, &["--version"]).code,
        "commander.unknownOption"
    );
    assert!(!p.help_information().contains("--version"));
    for flags in ["-V, --version", "-R, --release", "-R", "--release"] {
        let mut p = program();
        let output = support::Capture::default();
        output.install(&mut p);
        p.version_with_flags("1.2.3", flags, "custom version description");
        assert!(p.help_information().contains("custom version description"));
        let opt = Opt::new(flags, "");
        for flag in opt.short.iter().chain(opt.long.iter()) {
            output.out.borrow_mut().clear();
            assert_eq!(failure(&mut p, &[flag]).exit_code, 0);
            assert_eq!(*output.out.borrow(), "1.2.3\n");
            assert!(p.opts().is_empty());
        }
    }
    let mut p = program();
    p.version("1.2.3");
    p.command("version").action(|_, _, _| Ok(()));
    p.parse(["version"], ParseOptions::USER).unwrap();
    assert_eq!(failure(&mut p, &["--version"]).code, "commander.version");
}

#[test]
fn typed_presets_are_not_coerced_by_choices_or_variadic_collection() {
    let mut p = program();
    p.add_option(Opt::new("--number [value]", "").preset(80).choices(["80"]));
    assert_eq!(
        failure(&mut p, &["--number"]).code,
        "commander.invalidArgument"
    );
    let mut p = program();
    p.add_option(Opt::new("--number [value...]", "").preset(80));
    p.parse(["--number"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["number"], json!([80]));
    let mut p = program();
    p.add_option(Opt::new("--number", "").choices(["undefined"]));
    assert_eq!(
        failure(&mut p, &["--number"]).code,
        "commander.invalidArgument"
    );
}
#[test]
fn when_optional_flag_in_group_without_combination_then_parser_called() {
    let mut p = program();
    p.combine_flag_and_optional_value(false)
        .option_with_parser("-p [value]", "", |raw, _| {
            assert_eq!(raw, "undefined");
            Ok(json!(42))
        })
        .option("-a", "");
    p.parse(["-pa"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["p"], 42);
    assert_eq!(p.opts()["a"], true);
}

#[test]
fn when_specify_subcommand_and_arguments_then_program_args_not_empty() {
    for args in [vec!["doit", "myid"], vec!["doit", "--better", "myid"]] {
        let mut p = program();
        p.command("doit [id]")
            .option("--better", "")
            .action(|_, _, _| Ok(()));
        p.parse(&args, ParseOptions::USER).unwrap();
        assert!(!p.args.is_empty());
        assert_eq!(p.args, args);
    }
    let mut p = program();
    p.allow_unknown_option(true)
        .option("--global-flag", "")
        .option("--global-value <value>", "")
        .argument("[arg...]", "");
    p.parse(
        [
            "aaa",
            "--global-flag",
            "bbb",
            "--unknown",
            "ccc",
            "--global-value",
            "value",
        ],
        ParseOptions::USER,
    )
    .unwrap();
    assert_eq!(p.args, ["aaa", "bbb", "--unknown", "ccc"]);
    let mut p = program();
    p.option("--global-flag", "")
        .option("--global-value <value>", "")
        .command("sub [args...]")
        .option("--sub-flag", "");
    p.parse(
        [
            "--global-flag",
            "sub",
            "--sub-flag",
            "arg",
            "--global-value",
            "value",
        ],
        ParseOptions::USER,
    )
    .unwrap();
    assert_eq!(p.args, ["sub", "--sub-flag", "arg"]);
}

#[test]
fn when_argument_added_using_supported_methods_then_same_properties() {
    for (signature, name, required, variadic) in [
        ("<explicit-required>", "explicit-required", true, false),
        ("implicit-required", "implicit-required", true, false),
        ("[optional]", "optional", false, false),
        ("<explicit-required...>", "explicit-required", true, true),
        ("implicit-required...", "implicit-required", true, true),
        ("[optional...]", "optional", false, true),
    ] {
        for method in 0..3 {
            let mut p = program();
            let cmd = match method {
                0 => p.argument(signature, ""),
                1 => p.add_argument(Argument::new(signature, "")),
                _ => p.command(&format!("sub {signature}")),
            };
            let arg = &cmd.registered_arguments[0];
            assert_eq!(arg.name(), name);
            assert_eq!(arg.required, required);
            assert_eq!(arg.variadic, variadic);
            assert!(arg.description.is_empty());
        }
    }
    for method in 0..3 {
        let mut p = program();
        let cmd = match method {
            0 => p.argument("<first>", "").argument("[second]", ""),
            1 => p
                .add_argument(Argument::new("<first>", ""))
                .add_argument(Argument::new("[second]", "")),
            _ => p.command("sub <first> [second]"),
        };
        assert_eq!(
            cmd.registered_arguments
                .iter()
                .map(Argument::name)
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }
}

#[test]
fn when_invalid_option_flags_or_clashes_then_definition_error() {
    for flags in [
        "-a, -b",
        "-a, -b <value>",
        "-a, -b, --long",
        "--one, --two, --three",
        "-ws",
        "---triple",
        "sdkjhskjh",
        "-a,-b",
        "-a|-b",
        "-a -b",
    ] {
        let mut p = program();
        p.add_option(Opt::new(flags, ""));
        let e = p.parse([] as [&str; 0], ParseOptions::USER).err().unwrap();
        assert!(e.commander().is_none());
        assert!(e.to_string().starts_with("option creation failed"));
    }
    for flags in [
        "-s",
        "--long",
        "-b, --both",
        "--both, -b",
        "--ws, --workspace",
        "-b,--both <comma>",
        "-b|--both <bar>",
        "-b --both [space]",
        "-v, --variadic <files...>",
    ] {
        let mut p = program();
        p.add_option(Opt::new(flags, ""));
        p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    }
    for via_add in [false, true] {
        for flags in ["-c, --conflict", "-H, --cheese"] {
            let mut p = program();
            p.option("-c, --cheese <type>", "cheese type");
            if via_add {
                p.add_option(Opt::new(flags, ""));
            } else {
                p.option(flags, "");
            }
            assert!(
                p.parse([] as [&str; 0], ParseOptions::USER)
                    .err()
                    .unwrap()
                    .to_string()
                    .starts_with("Cannot add option")
            );
        }
    }
    let mut p = program();
    p.option("-h, --help", "display help");
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    let mut p = program();
    p.option("-e, --example", "");
    p.command("sub").option("-e, --example", "");
    p.parse(["sub"], ParseOptions::USER).unwrap();
    let mut p = program();
    p.command("sub");
    p.command("sub");
    assert_eq!(
        p.parse([] as [&str; 0], ParseOptions::USER)
            .err()
            .unwrap()
            .to_string(),
        "cannot add command 'sub' as already have command 'sub'"
    );
}

#[test]
fn when_subcommands_inherit_settings_then_registration_snapshot_applies() {
    let mut p = program();
    p.help_option_enabled(false)
        .combine_flag_and_optional_value(false)
        .allow_excess_arguments(true)
        .enable_positional_options(true);
    p.help_command_with_description("assist [cmd]", "assistance");
    let child = p.command("sub");
    child.option("-a [value]", "").option("-b", "");
    assert!(!child.help_information().contains("--help"));
    child.command("nested");
    assert!(child.help_information().contains("assist [cmd]"));
    child.action(|_, _, _| Ok(()));
    p.parse(["sub", "-ab", "extra"], ParseOptions::USER)
        .unwrap();
    assert_eq!(p.commands[0].opts()["a"], true);
    assert_eq!(p.commands[0].opts()["b"], true);
    let mut p = program();
    p.help_command(true);
    assert!(!p.command("sub").help_information().contains("Commands:"));
    let mut p = program();
    p.help_option("-H, --HELP", "custom");
    assert!(p.command("sub").help_information().contains("-H, --HELP"));
}
