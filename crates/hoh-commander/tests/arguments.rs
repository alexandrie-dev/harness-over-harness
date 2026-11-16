mod support;
use hoh_commander::{Argument, InvalidArgumentError, ParseOptions};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::{failure, program};

// argument.required.test.js, argument.chain.test.js.
#[test]
fn when_name_with_brackets_then_required_matches() {
    for (name, required) in [("<a>", true), ("[a]", false), ("a", true)] {
        let arg = Argument::new(name, "description");
        assert_eq!(arg.name(), "a");
        assert_eq!(arg.description, "description");
        assert_eq!(arg.required, required);
    }
}
#[test]
fn when_arg_required_then_required() {
    assert!(Argument::new("[a]", "").arg_required().required);
}
#[test]
fn when_arg_optional_then_optional() {
    assert!(!Argument::new("<a>", "").arg_optional().required);
}

// argument.choices.test.js.
#[test]
fn when_command_argument_in_choices_then_argument_set() {
    let mut p = program();
    p.add_argument(Argument::new("colour", "").choices(["red", "blue"]));
    p.parse(["red"], ParseOptions::USER).unwrap();
    assert_eq!(p.processed_args, [Some(json!("red"))]);
}
#[test]
fn when_command_argument_not_in_choices_then_error() {
    let mut p = program();
    p.add_argument(Argument::new("colour", "").choices(["red", "blue"]));
    let e = failure(&mut p, &["orange"]);
    assert_eq!(e.code, "commander.invalidArgument");
    assert_eq!(
        e.message,
        "error: command-argument value 'orange' is invalid for argument 'colour'. Allowed choices are red, blue."
    );
}
#[test]
fn when_argument_choices_or_parameter_changed_then_other_is_unchanged() {
    let mut param = vec!["red".to_string(), "blue".into()];
    let mut arg = Argument::new("colour", "").choices(param.clone());
    assert_eq!(param, ["red", "blue"]);
    arg.arg_choices.as_mut().unwrap().push("green".into());
    assert_eq!(param, ["red", "blue"]);
    param.push("orange".into());
    let mut p = program();
    p.add_argument(arg);
    assert_eq!(
        failure(&mut p, &["orange"]).code,
        "commander.invalidArgument"
    );
}
// argument.variadic.test.js, args.variadic.test.js.
#[test]
fn when_variadic_no_extra_arguments_then_empty_array() {
    let mut p = program();
    p.argument("[args...]", "");
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert_eq!(p.processed_args, [Some(json!([]))]);
    p.command("sub [args...]").action(|args, _, _| {
        assert_eq!(args, [Some(json!([]))]);
        Ok(())
    });
    p.parse(["sub"], ParseOptions::USER).unwrap();
}
#[test]
fn when_variadic_extra_arguments_then_array() {
    let mut p = program();
    p.argument("first", "").argument("[args...]", "");
    p.parse(["a", "b", "c"], ParseOptions::USER).unwrap();
    assert_eq!(
        p.processed_args,
        [Some(json!("a")), Some(json!(["b", "c"]))]
    );
    p.command("sub <first> [args...]").action(|args, _, _| {
        assert_eq!(args, [Some(json!("a")), Some(json!(["b", "c"]))]);
        Ok(())
    });
    p.parse(["sub", "a", "b", "c"], ParseOptions::USER).unwrap();
}
#[test]
fn when_variadic_argument_not_last_then_error() {
    let mut p = program();
    p.argument("[args...]", "").argument("[other]", "");
    assert_eq!(
        p.parse([] as [&str; 0], ParseOptions::USER)
            .err()
            .unwrap()
            .to_string(),
        "only the last argument can be variadic 'args'"
    );
    let mut p = program();
    p.command("sub [args...] [other]");
    assert_eq!(
        p.parse(["sub"], ParseOptions::USER)
            .err()
            .unwrap()
            .to_string(),
        "only the last argument can be variadic 'args'"
    );
}
#[test]
fn when_variadic_argument_then_usage_shows_variadic() {
    let mut p = program();
    p.argument("<args...>", "");
    assert_eq!(p.get_usage(), "[options] <args...>");
}
#[test]
fn when_variadic_with_choices_then_array_and_default_replaced() {
    for values in [vec!["red"], vec!["red", "blue"]] {
        for default in [json!(["default"]), json!([])] {
            let mut p = program();
            p.add_argument(
                Argument::new("[colour...]", "")
                    .choices(["red", "blue"])
                    .default(default),
            );
            p.parse(&values, ParseOptions::USER).unwrap();
            assert_eq!(p.processed_args, [Some(json!(values))]);
        }
    }
}
// argument.custom-processing.test.js.
#[test]
fn when_argument_not_specified_then_callback_not_called_and_default_used() {
    for action in [false, true] {
        for default in [None, Some(json!(1))] {
            for parser in [false, true] {
                let mut p = program();
                let mut arg = Argument::new("[n]", "number");
                if let Some(v) = &default {
                    arg = arg.default(v.clone());
                }
                if parser {
                    arg =
                        arg.arg_parser(|_, _| panic!("must not call parser for omitted argument"));
                }
                p.add_argument(arg);
                if action {
                    let expected = default.clone();
                    p.action(move |args, _, _| {
                        assert_eq!(args, std::slice::from_ref(&expected));
                        Ok(())
                    });
                }
                p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
                assert_eq!(p.processed_args, std::slice::from_ref(&default));
            }
        }
    }
}
#[test]
fn when_argument_specified_then_callback_receives_raw_and_previous() {
    for action in [false, true] {
        for default in [None, Some(json!(1))] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let capture = calls.clone();
            let mut arg = Argument::new("[n]", "number").arg_parser(move |raw, previous| {
                capture
                    .lock()
                    .unwrap()
                    .push((raw.to_string(), previous.cloned()));
                Ok(json!(2))
            });
            if let Some(v) = &default {
                arg = arg.default(v.clone());
            }
            let mut p = program();
            p.add_argument(arg);
            if action {
                p.action(|args, _, _| {
                    assert_eq!(args, [Some(json!(2))]);
                    Ok(())
                });
            }
            p.parse(["alpha"], ParseOptions::USER).unwrap();
            assert_eq!(*calls.lock().unwrap(), [("alpha".into(), default)]);
            assert_eq!(p.args, ["alpha"]);
            assert_eq!(p.processed_args, [Some(json!(2))]);
        }
    }
}
#[test]
fn when_variadic_argument_specified_multiple_times_then_callback_gets_previous_value() {
    let mut p = program();
    p.add_argument(
        Argument::new("<n...>", "number").arg_parser(|raw, previous| {
            if raw == "1" {
                assert!(previous.is_none());
            } else {
                assert_eq!(previous, Some(&json!("callback")));
            }
            Ok(json!("callback"))
        }),
    );
    p.parse(["1", "2"], ParseOptions::USER).unwrap();
    assert_eq!(p.processed_args, [Some(json!("callback"))]);
}
#[test]
fn when_parse_float_then_argument_is_one_hundred() {
    let mut p = program();
    p.add_argument(
        Argument::new("<number>", "float argument")
            .arg_parser(|s, _| Ok(json!(s.parse::<f64>().unwrap()))),
    );
    p.parse(["1e2"], ParseOptions::USER).unwrap();
    assert_eq!(p.processed_args, [Some(json!(100.0))]);
}
#[test]
fn when_default_for_required_argument_then_error() {
    let mut p = program();
    p.add_argument(Argument::new("<number>", "float argument").default(4));
    assert_eq!(
        p.parse(["2"], ParseOptions::USER)
            .err()
            .unwrap()
            .to_string(),
        "a default value for a required argument is never used: 'number'"
    );
}
#[test]
fn when_argument_parser_rejects_then_commander_error() {
    let mut p = program();
    p.add_argument(
        Argument::new("[n]", "number")
            .arg_parser(|_, _| Err(InvalidArgumentError::new("Not a number."))),
    );
    let e = failure(&mut p, &["x"]);
    assert_eq!(
        e.message,
        "error: command-argument value 'x' is invalid for argument 'n'. Not a number."
    );
}
// args.literal.test.js.
#[test]
fn when_arguments_include_double_dash_then_stop_processing_options() {
    let mut p = program();
    p.option("--debug", "").argument("[args...]", "");
    p.parse(["--", "--debug", "--", "x"], ParseOptions::USER)
        .unwrap();
    assert_eq!(p.args, ["--debug", "--", "x"]);
    assert!(!p.opts().contains_key("debug"));
}
// negatives.test.js (all 17 values across optional flags, variadics, positional and digit flags).
#[test]
fn when_negative_number_then_consumed_unless_digit_option_defined() {
    let cases = [
        ("-.1", true),
        ("-123", true),
        ("-123.45", true),
        ("-1e3", true),
        ("-1e+3", true),
        ("-1e-3", true),
        ("-1.2e3", true),
        ("-1.2e+3", true),
        ("-1.2e-3", true),
        ("-1e-3.0", false),
        ("--1 ", false),
        ("-0", true),
        ("1", true),
        ("-1x", false),
        ("-x-1 ", false),
        ("", true),
        ("-0x1234", false),
    ];
    for (value, consume) in cases {
        for flag in ["-o", "--optional"] {
            for variadic in [false, true] {
                let mut p = program();
                p.option(
                    if variadic {
                        "-o, --optional [value...]"
                    } else {
                        "-o, --optional [value]"
                    },
                    "optional option",
                );
                let args = if variadic {
                    vec![flag, "first", value]
                } else {
                    vec![flag, value]
                };
                let result = p.parse(args, ParseOptions::USER);
                assert_eq!(result.is_ok(), consume, "{value}");
                assert_eq!(
                    p.opts()["optional"],
                    if variadic {
                        if consume {
                            json!(["first", value])
                        } else {
                            json!(["first"])
                        }
                    } else if consume {
                        json!(value)
                    } else {
                        json!(true)
                    }
                );
            }
        }
        let mut p = program();
        p.argument("<value>", "argument");
        assert_eq!(p.parse([value], ParseOptions::USER).is_ok(), consume);
        for positional in [false, true] {
            let mut p = program();
            p.option("-9", "register option using digit");
            let args = if positional {
                p.argument("[value]", "argument");
                vec![value]
            } else {
                p.option("-o, --optional [value]", "optional option");
                vec!["-o", value]
            };
            assert_eq!(
                p.parse(args, ParseOptions::USER).is_ok(),
                !value.starts_with('-')
            );
        }
    }
}
#[test]
fn when_optional_argument_omitted_then_undefined_differs_from_null_default() {
    let mut p = program();
    p.argument("[first]", "")
        .add_argument(Argument::new("[second]", "").default(Value::Null));
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert_eq!(p.processed_args, [None, Some(Value::Null)]);
}

#[test]
fn when_complex_example_with_negative_numbers_then_all_consumed() {
    let mut p = program();
    p.option("-o [value]", "optional")
        .option("-m <value>", "required option-argument")
        .option("-O [value...]", "optional")
        .option("-M <value...>", "required option-argument")
        .argument("[value...]", "argument");
    p.parse(
        [
            "-10", "-O", "-40", "-41", "-M", "-50", "-51", "-o", "-20", "-m", "-30", "-11",
        ],
        ParseOptions::USER,
    )
    .unwrap();
    assert_eq!(p.opts()["o"], "-20");
    assert_eq!(p.opts()["m"], "-30");
    assert_eq!(p.opts()["O"], json!(["-40", "-41"]));
    assert_eq!(p.opts()["M"], json!(["-50", "-51"]));
    assert_eq!(p.args, ["-10", "-11"]);
}
#[test]
fn when_program_has_digit_option_then_negatives_not_allowed_in_leaf() {
    let mut p = program();
    p.option("-2", "double option");
    p.command("leaf [value...]").action(|_, _, _| Ok(()));
    assert_eq!(
        failure(&mut p, &["leaf", "-1"]).code,
        "commander.unknownOption"
    );
}
#[test]
fn when_program_has_subcommand_and_action_then_negative_command_argument_unsupported() {
    let mut p = program();
    p.argument("[value...]", "").action(|_, _, _| Ok(()));
    p.command("leaf").action(|_, _, _| Ok(()));
    assert_eq!(failure(&mut p, &["-1"]).code, "commander.unknownOption");
}
