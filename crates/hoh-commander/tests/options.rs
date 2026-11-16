mod support;
use hoh_commander::{InvalidArgumentError, Opt, ParseOptions};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};
use support::{failure, program};

// options.bool.test.js, options.flags.test.js, options.bool.small.combined.test.js
#[test]
fn when_boolean_option_not_specified_then_value_is_undefined() {
    let mut p = program();
    p.option("-d, --debug", "description");
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert!(!p.opts().contains_key("debug"));
}
#[test]
fn when_boolean_option_specified_then_value_is_true() {
    for flag in ["-d", "--debug"] {
        let mut p = program();
        p.option("-d, --debug", "description");
        p.parse([flag], ParseOptions::USER).unwrap();
        assert_eq!(p.opts()["debug"], true);
    }
}
#[test]
fn when_negatable_boolean_option_not_specified_then_value_is_true() {
    let mut p = program();
    p.option("--no-cheese", "");
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["cheese"], true);
}
#[test]
fn when_negatable_boolean_option_specified_then_value_is_false() {
    let mut p = program();
    p.option("--no-cheese", "");
    p.parse(["--no-cheese"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["cheese"], false);
}
#[test]
fn when_multiple_short_flags_specified_then_parsed_as_short_option_group() {
    let mut p = program();
    p.option("-a", "").option("-b", "").option("-c", "");
    p.parse(["-abc"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts().values().collect::<Vec<_>>(), vec![&json!(true); 3]);
}
#[test]
fn when_flags_defined_with_separators_then_short_and_long_work() {
    for flags in ["-p,--pepper", "-p|--pepper", "-p --pepper", "--pepper, -p"] {
        for flag in ["-p", "--pepper"] {
            let mut p = program();
            p.option(flags, "");
            p.parse([flag], ParseOptions::USER).unwrap();
            assert_eq!(p.opts()["pepper"], true);
        }
    }
}
#[test]
fn when_only_short_or_long_flag_defined_then_property_matches() {
    for (flags, name) in [("-p", "p"), ("--pepper", "pepper")] {
        let mut p = program();
        p.option(flags, "");
        p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
        assert!(!p.opts().contains_key(name));
        p.parse([flags], ParseOptions::USER).unwrap();
        assert_eq!(p.opts()[name], true);
    }
}
// options.camelcase.test.js (all six cases).
#[test]
fn when_option_defined_with_kebab_case_then_property_is_camel_case() {
    for (flag, key) in [
        ("--word-word", "wordWord"),
        ("--word-wORD", "wordWORD"),
        ("--word-WORD", "wordWORD"),
        ("--word-word-word", "wordWordWord"),
        ("--word-WORD-word", "wordWORDWord"),
        ("--Word", "Word"),
    ] {
        let mut p = program();
        p.option(flag, "");
        p.parse([flag], ParseOptions::USER).unwrap();
        assert_eq!(p.opts()[key], true);
    }
}
// options.values.test.js (20 required value matrix cases + five optional).
#[test]
fn when_option_with_required_value_specified_then_value_is_as_specified() {
    for value in ["str", "80", "-", "-5", "--flag"] {
        for args in [
            vec!["-p".into(), value.into()],
            vec![format!("-p{value}")],
            vec!["--port".into(), value.into()],
            vec![format!("--port={value}")],
        ] {
            let mut p = program();
            p.option("-p,--port <number>", "specify port");
            p.parse(&args, ParseOptions::USER).unwrap();
            assert_eq!(p.opts()["port"], value);
        }
    }
}
#[test]
fn when_optional_value_specified_in_different_forms_then_value_is_as_specified() {
    for args in [
        vec!["-p", "80"],
        vec!["-p80"],
        vec!["--port", "80"],
        vec!["--port=80"],
    ] {
        let mut p = program();
        p.option("-p,--port [number]", "specify port");
        p.parse(args, ParseOptions::USER).unwrap();
        assert_eq!(p.opts()["port"], "80");
    }
    let mut p = program();
    p.option(
        "-c, --cheese [type]",
        "optionally specify the type of cheese",
    );
    p.parse(["--cheese", ""], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["cheese"], "");
}
// options.required.test.js + options.optional.test.js.
#[test]
fn when_required_option_value_not_specified_then_error() {
    for default in [None, Some(json!("default"))] {
        let mut p = program();
        let mut opt = Opt::new("-c, --cheese <type>", "add cheese");
        if let Some(default) = default {
            opt = opt.default(default);
        }
        p.add_option(opt);
        let e = failure(&mut p, &["--cheese"]);
        assert_eq!(e.code, "commander.optionMissingArgument");
        assert_eq!(
            e.message,
            "error: option '-c, --cheese <type>' argument missing"
        );
    }
}
#[test]
fn when_option_not_specified_then_undefined_or_default() {
    for grammar in ["<type>", "[type]"] {
        for default in [None, Some(json!("default"))] {
            let mut p = program();
            let mut opt = Opt::new(format!("-c, --cheese {grammar}"), "add cheese");
            if let Some(value) = &default {
                opt = opt.default(value.clone());
            }
            p.add_option(opt);
            p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
            assert_eq!(p.opts().get("cheese"), default.as_ref());
            p.parse(["--cheese", "blue"], ParseOptions::USER).unwrap();
            assert_eq!(p.opts()["cheese"], "blue");
        }
    }
}
#[test]
fn when_optional_option_specified_without_value_then_true() {
    for args in [vec!["--cheese"], vec!["--cheese", "--debug"]] {
        for default in [None, Some(json!("default"))] {
            let mut p = program();
            let mut opt = Opt::new("-c, --cheese [type]", "");
            if let Some(default) = default {
                opt = opt.default(default);
            }
            p.add_option(opt).option("--debug", "");
            p.parse(args.clone(), ParseOptions::USER).unwrap();
            assert_eq!(p.opts()["cheese"], true);
        }
    }
}
#[test]
fn when_optional_option_without_value_and_preset_then_value_is_preset() {
    let mut p = program();
    p.add_option(Opt::new("--cheese [type]", "").preset("blue"));
    p.parse(["--cheese"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["cheese"], "blue");
}
// options.default.test.js: all 27 cases, both convenience API and Opt API.
#[test]
fn when_default_and_option_absent_or_present_then_value_matches() {
    for via_option in [false, true] {
        for (flags, default) in [
            ("--debug", json!(false)),
            ("--debug", json!(0)),
            ("--debug", json!("foo")),
            ("--debug <value>", json!(1)),
            ("--debug <value>", json!("foo")),
            ("--debug [value]", json!(1)),
            ("--debug [value]", json!("foo")),
            ("--no-debug", json!("foo")),
        ] {
            let mut p = program();
            if via_option {
                p.option_with_default(flags, "", default.clone());
            } else {
                p.add_option(Opt::new(flags, "").default(default.clone()));
            }
            p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
            assert_eq!(p.opts()["debug"], default);
            let flag = flags.split_whitespace().next().unwrap();
            let args = if flags.contains("value") {
                vec![flag, "bar"]
            } else {
                vec![flag]
            };
            p.parse(args, ParseOptions::USER).unwrap();
            assert_eq!(
                p.opts()["debug"],
                if flags.contains("value") {
                    json!("bar")
                } else {
                    json!(flag != "--no-debug")
                }
            );
            if flags.contains('[') {
                p.parse([flag], ParseOptions::USER).unwrap();
                assert_eq!(p.opts()["debug"], true);
            }
        }
    }
}
// options.bool.combo.test.js: default, positive, negative, last wins, registration order.
#[test]
fn when_boolean_combo_then_explicit_defaults_and_last_option_win() {
    for reverse in [false, true] {
        for default in [
            None,
            Some(json!(true)),
            Some(json!(false)),
            Some(json!("foo")),
        ] {
            let mut p = program();
            let mut positive = Opt::new("--cheese", "");
            if let Some(v) = &default {
                positive = positive.default(v.clone());
            }
            if reverse {
                p.option("--no-cheese", "").add_option(positive);
            } else {
                p.add_option(positive).option("--no-cheese", "");
            }
            p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
            assert_eq!(p.opts().get("cheese"), default.as_ref());
            for (args, expected) in [
                (vec!["--cheese"], true),
                (vec!["--no-cheese"], false),
                (vec!["--no-cheese", "--cheese"], true),
                (vec!["--cheese", "--no-cheese"], false),
            ] {
                p.parse(args, ParseOptions::USER).unwrap();
                assert_eq!(p.opts()["cheese"], expected);
            }
        }
    }
}
// options.custom-processing.test.js.
#[test]
fn when_option_specified_multiple_times_then_callback_gets_previous_value() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let callback = calls.clone();
    let mut p = program();
    p.option_with_parser_and_default(
        "-n <value>",
        "",
        move |raw, previous| {
            callback
                .borrow_mut()
                .push((raw.to_string(), previous.cloned()));
            Ok(json!(raw))
        },
        1,
    );
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert!(calls.borrow().is_empty());
    assert_eq!(p.opts()["n"], 1);
    p.parse(["-n", "2", "-n", "3"], ParseOptions::USER).unwrap();
    assert_eq!(
        *calls.borrow(),
        vec![("2".into(), Some(json!(1))), ("3".into(), Some(json!("2")))]
    );
}
#[test]
fn when_custom_parser_then_value_is_returned_from_callback() {
    let mut p = program();
    p.option_with_parser("--float <value>", "", |s, _| {
        Ok(json!(s.parse::<f64>().unwrap()))
    })
    .option_with_parser("--integer <value>", "", |s, _| {
        Ok(json!(s.parse::<u32>().unwrap()))
    });
    p.parse(["--float", "1e2", "--integer", "1"], ParseOptions::USER)
        .unwrap();
    assert_eq!(p.opts()["float"], json!(100.0));
    assert_eq!(p.opts()["integer"], 1);
}
#[test]
fn when_increase_verbosity_then_value_is_three() {
    let mut p = program();
    p.option_with_parser_and_default(
        "-v, --verbose",
        "",
        |_, previous| Ok(json!(previous.and_then(Value::as_u64).unwrap_or(0) + 1)),
        0,
    );
    p.parse(["-v", "-v", "-v"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["verbose"], 3);
}
#[test]
fn when_collect_then_value_is_array() {
    let mut p = program();
    p.option_with_parser("-c <value>", "", |s, prev| {
        let mut a = prev.and_then(Value::as_array).cloned().unwrap_or_default();
        a.push(json!(s));
        Ok(json!(a))
    });
    p.parse(["-c", "a", "-c", "b", "-c", "c"], ParseOptions::USER)
        .unwrap();
    assert_eq!(p.opts()["c"], json!(["a", "b", "c"]));
}
#[test]
fn when_comma_separated_list_then_value_is_array() {
    let mut p = program();
    p.option_with_parser("-c <value>", "", |s, _| {
        Ok(json!(s.split(',').collect::<Vec<_>>()))
    });
    p.parse(["-c", "x,y,z"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["c"], json!(["x", "y", "z"]));
}
// options.choices.test.js: all five cases.
#[test]
fn when_option_argument_in_choices_then_set_else_error() {
    let mut p = program();
    p.add_option(Opt::new("--colour <shade>", "").choices(["red", "blue"]));
    p.parse(["--colour", "red"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["colour"], "red");
    let e = failure(&mut p, &["--colour", "orange"]);
    assert_eq!(e.code, "commander.invalidArgument");
    assert_eq!(
        e.message,
        "error: option '--colour <shade>' argument 'orange' is invalid. Allowed choices are red, blue."
    );
}
#[test]
fn when_choices_or_parameter_changed_then_other_is_unchanged() {
    let mut param = vec!["red".to_string(), "blue".into()];
    let mut opt = Opt::new("--colour <shade>", "").choices(param.clone());
    assert_eq!(param, ["red", "blue"]);
    opt.arg_choices.as_mut().unwrap().push("green".into());
    assert_eq!(param, ["red", "blue"]);
    param.push("orange".into());
    let mut p = program();
    p.add_option(opt);
    assert_eq!(
        failure(&mut p, &["--colour", "orange"]).code,
        "commander.invalidArgument"
    );
}
// options.variadic.test.js: all 19 cases including the attached-value exceptions.
#[test]
fn when_variadic_then_values_collected_until_option() {
    for grammar in ["<value...>", "[value...]"] {
        for values in [vec!["a"], vec!["a", "b"], vec!["a", "b", "a"]] {
            let mut p = program();
            p.option(&format!("-v, --values {grammar}"), "")
                .option("--debug", "");
            let args = [vec!["--values"], values.clone(), vec!["--debug"]].concat();
            p.parse(args, ParseOptions::USER).unwrap();
            assert_eq!(p.opts()["values"], json!(values));
            assert_eq!(p.opts()["debug"], true);
        }
    }
}
#[test]
fn when_variadic_missing_then_required_error_optional_true_or_default() {
    let mut p = program();
    p.option("--values <value...>", "");
    assert_eq!(
        failure(&mut p, &["--values"]).code,
        "commander.optionMissingArgument"
    );
    let mut p = program();
    p.option("--values [value...]", "");
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert!(!p.opts().contains_key("values"));
    p.parse(["--values"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["values"], true);
    let mut p = program();
    p.option_with_default("--values <value...>", "", json!(["default"]));
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["values"], json!(["default"]));
}
#[test]
fn when_variadic_with_combined_argument_then_not_variadic() {
    for arg in ["-vfirst", "--values=first"] {
        let mut p = program();
        p.option("-v, --values <value...>", "")
            .argument("[arg]", "");
        p.parse([arg, "second"], ParseOptions::USER).unwrap();
        assert_eq!(p.opts()["values"], json!(["first"]));
        assert_eq!(p.args, ["second"]);
    }
}
#[test]
fn when_variadic_with_choices_then_array_and_default_replaced() {
    for grammar in ["<value...>", "[value...]"] {
        for values in [vec!["red"], vec!["red", "blue"]] {
            for default in [json!(["default"]), json!([])] {
                let mut p = program();
                p.add_option(
                    Opt::new(format!("--colour {grammar}"), "")
                        .choices(["red", "blue"])
                        .default(default),
                );
                p.parse(
                    [vec!["--colour"], values.clone()].concat(),
                    ParseOptions::USER,
                )
                .unwrap();
                assert_eq!(p.opts()["colour"], json!(values));
            }
        }
    }
}
#[test]
fn when_variadic_with_coercion_then_coercion_sets_value() {
    let mut p = program();
    p.option_with_parser_and_default(
        "--number <n...>",
        "",
        |s, prev| {
            Ok(json!(
                prev.and_then(Value::as_i64).unwrap_or(0) + s.parse::<i64>().unwrap()
            ))
        },
        0,
    );
    p.parse(["--number", "1", "2"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["number"], 3);
}
#[test]
fn when_word_character_before_dots_then_variadic_else_not() {
    for c in ['a', 'Z', '1', '_', '$', '.', ','] {
        let opt = Opt::new(format!("--values <{c}...>"), "");
        assert_eq!(opt.variadic, c.is_ascii_alphanumeric() || c == '_');
    }
}
// Extra regression: JSON equality must not substitute JS array identity.
#[test]
fn when_first_variadic_value_equals_default_then_next_is_appended() {
    let mut p = program();
    p.option_with_default("--values <v...>", "", json!(["a"]));
    p.parse(["--values", "a", "b"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["values"], json!(["a", "b"]));
}
#[test]
fn when_invalid_argument_error_then_prefix_and_code_preserved() {
    let mut p = program();
    p.option_with_parser("--foo <value>", "", |_, _| {
        Err(InvalidArgumentError::new("custom message"))
    });
    let e = failure(&mut p, &["--foo", "bad"]);
    assert_eq!(
        e.message,
        "error: option '--foo <value>' argument 'bad' is invalid. custom message"
    );
    assert_eq!(e.code, "commander.invalidArgument");
}
#[test]
fn when_opts_then_insertion_order_and_undefined_are_preserved() {
    let mut p = program();
    p.option_with_default("--default <value>", "", Value::Null)
        .option("--first", "")
        .option("--second", "");
    p.parse(["--second", "--first"], ParseOptions::USER)
        .unwrap();
    assert_eq!(
        p.opts().keys().map(String::as_str).collect::<Vec<_>>(),
        ["default", "second", "first"]
    );
    assert_eq!(p.opts()["default"], Value::Null);
}

#[test]
fn when_no_in_middle_of_name_then_not_negated() {
    let mut p = program();
    p.option("--module-no-parse", "");
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert!(!p.opts().contains_key("moduleNoParse"));
    p.parse(["--module-no-parse"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["moduleNoParse"], true);
}
#[test]
fn when_boolean_options_in_subcommand_then_defaults_and_values_match() {
    for negate in [false, true] {
        let mut p = program();
        p.command("sub")
            .option(if negate { "--no-cheese" } else { "--cheese" }, "");
        p.parse(["sub"], ParseOptions::USER).unwrap();
        assert_eq!(
            p.commands[0].opts().get("cheese"),
            if negate { Some(&json!(true)) } else { None }
        );
        p.parse(
            ["sub", if negate { "--no-cheese" } else { "--cheese" }],
            ParseOptions::USER,
        )
        .unwrap();
        assert_eq!(p.commands[0].opts()["cheese"], !negate);
    }
}
#[test]
fn when_boolean_combo_with_preset_then_default_preset_and_negation_match() {
    let mut p = program();
    p.add_option(Opt::new("--cheese", "").default("default").preset("preset"))
        .option("--no-cheese", "");
    p.parse([] as [&str; 0], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["cheese"], "default");
    p.parse(["--cheese"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["cheese"], "preset");
    p.parse(["--no-cheese"], ParseOptions::USER).unwrap();
    assert_eq!(p.opts()["cheese"], false);
}
