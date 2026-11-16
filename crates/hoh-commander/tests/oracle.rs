//! Compares the port with commander 15.0.0 itself. `fixtures/commander_oracle.jsonl` holds programs described
//! as lists of builder calls and, for each of many argument lists, what the library printed, threw, parsed and
//! passed to actions. The same programs are built here with the port. Regenerate the file with
//! `fixtures/gen-commander-oracle.mjs`; never edit it by hand to make this test pass.

use std::sync::{Arc, Mutex};

use hoh_commander::{Argument, Command, Opt, Options, Output, ParseError, ParseOptions};
use serde_json::{Value, json};

const ORACLE: &str = include_str!("fixtures/commander_oracle.jsonl");

fn text(value: &Value) -> &str {
    value.as_str().expect("a string in the fixture")
}

fn list(value: &Value) -> &[Value] {
    value.as_array().expect("an array in the fixture")
}

fn strings(value: &Value) -> Vec<String> {
    list(value)
        .iter()
        .map(|item| text(item).to_owned())
        .collect()
}

fn options_json(options: &Options) -> Value {
    Value::Object(
        options
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    )
}

/// Interprets the builder calls exactly as `build` in the generator does.
fn build(command: &mut Command, ops: &[Value], actions: &Arc<Mutex<Vec<Value>>>) {
    for op in ops {
        let a = list(op);
        match text(&a[0]) {
            "name" => drop(command.name(text(&a[1]))),
            "description" => drop(command.description(text(&a[1]))),
            "usage" => drop(command.usage(text(&a[1]))),
            "version" if a.len() == 2 => drop(command.version(text(&a[1]))),
            "version" => drop(command.version_with_flags(text(&a[1]), text(&a[2]), text(&a[3]))),
            "option" if a.len() == 3 => drop(command.option(text(&a[1]), text(&a[2]))),
            "option" => {
                command.option_with_default(text(&a[1]), text(&a[2]), a[3].clone());
            }
            "optionCollect" => {
                command.option_with_parser(text(&a[1]), text(&a[2]), |value, previous| {
                    let mut values = previous
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    values.push(Value::String(value.to_owned()));
                    Ok(Value::Array(values))
                });
            }
            "requiredOption" => drop(command.required_option(text(&a[1]), text(&a[2]))),
            "addOption" => {
                let spec = &a[1];
                let mut option = Opt::new(text(&spec["flags"]), text(&spec["description"]));
                if let Some(choices) = spec.get("choices") {
                    option = option.choices(strings(choices));
                }
                if let Some(default) = spec.get("default") {
                    option = option.default(default.clone());
                    if let Some(description) = spec.get("defaultDescription") {
                        option = option.default_description(text(description));
                    }
                }
                if let Some(preset) = spec.get("preset") {
                    option = option.preset(preset.clone());
                }
                if spec.get("hideHelp").is_some() {
                    option = option.hide_help(true);
                }
                if spec.get("mandatory").is_some() {
                    option = option.make_option_mandatory(true);
                }
                command.add_option(option);
            }
            "argument" => drop(command.argument(text(&a[1]), text(&a[2]))),
            "addArgument" => {
                let spec = &a[1];
                let mut argument = Argument::new(text(&spec["name"]), text(&spec["description"]));
                if let Some(choices) = spec.get("choices") {
                    argument = argument.choices(strings(choices));
                }
                if let Some(default) = spec.get("default") {
                    argument = argument.default(default.clone());
                }
                command.add_argument(argument);
            }
            "helpOption" if a[1] == Value::Bool(false) => drop(command.help_option_enabled(false)),
            "helpOption" => drop(command.help_option(text(&a[1]), text(&a[2]))),
            "helpCommand" => drop(command.help_command(a[1] == Value::Bool(true))),
            "addHelpText" => drop(command.add_help_text(text(&a[1]), text(&a[2]))),
            "allowUnknownOption" => drop(command.allow_unknown_option(true)),
            "allowExcessArguments" => {
                command.allow_excess_arguments(a[1] == Value::Bool(true));
            }
            "passThroughOptions" => drop(command.pass_through_options(true)),
            "enablePositionalOptions" => drop(command.enable_positional_options(true)),
            "helpWidth" => {}
            "combineFlagAndOptionalValue" => {
                command.combine_flag_and_optional_value(a[1] == Value::Bool(true));
            }
            "command" => build(command.command(text(&a[1])), list(&a[2]), actions),
            "action" => {
                let actions = Arc::clone(actions);
                command.action(move |args, options, invoked| {
                    let args: Vec<Value> = args
                        .iter()
                        .map(|value| value.clone().unwrap_or(Value::Null))
                        .collect();
                    actions.lock().expect("actions lock").push(json!({
                        "command": invoked.get_name(),
                        "args": args,
                        "opts": options_json(options),
                    }));
                    Ok(())
                });
            }
            other => panic!("unknown builder call {other} in the fixture"),
        }
    }
}

/// The `helpWidth` call of a program: the number of columns its output reports for help.
fn help_width(ops: &[Value]) -> Option<usize> {
    ops.iter()
        .map(list)
        .find(|a| text(&a[0]) == "helpWidth")
        .map(|a| a[1].as_u64().expect("a width in the fixture") as usize)
}

fn capture(
    command: &mut Command,
    out: &Arc<Mutex<String>>,
    err: &Arc<Mutex<String>>,
    width: Option<usize>,
) {
    let (to_out, to_err) = (Arc::clone(out), Arc::clone(err));
    let mut output = Output::new(
        move |text| to_out.lock().expect("stdout lock").push_str(text),
        move |text| to_err.lock().expect("stderr lock").push_str(text),
    );
    if let Some(columns) = width {
        output = output.out_help_width(columns).err_help_width(columns);
    }
    command.configure_output(output).exit_override();
    for child in &mut command.commands {
        capture(child, out, err, width);
    }
}

/// What the port does for one argument list, in the shape of a fixture record.
fn run(ops: &[Value], argv: &[String]) -> Value {
    let actions = Arc::new(Mutex::new(Vec::new()));
    let (out, err) = (
        Arc::new(Mutex::new(String::new())),
        Arc::new(Mutex::new(String::new())),
    );
    let mut program = Command::new();
    build(&mut program, ops, &actions);
    capture(&mut program, &out, &err, help_width(ops));
    let error = match program.parse(argv, ParseOptions::USER) {
        Ok(_) => Value::Null,
        Err(ParseError::Commander(error)) => {
            json!({ "code": error.code, "exitCode": error.exit_code, "message": error.message })
        }
        Err(ParseError::Other(error)) => panic!("an action failed: {error}"),
    };
    let opts = options_json(program.opts());
    drop(program);
    json!({
        "stdout": *out.lock().expect("stdout lock"),
        "stderr": *err.lock().expect("stderr lock"),
        "error": error,
        "actions": *actions.lock().expect("actions lock"),
        "opts": opts,
    })
}

/// Commander throws from the builder call that makes a definition wrong. The port keeps building and
/// `parse` reports the first such error of the whole tree before it parses anything.
fn definition_error(ops: &[Value]) -> Option<String> {
    let mut program = Command::new();
    build(&mut program, ops, &Arc::new(Mutex::new(Vec::new())));
    let unread = Arc::new(Mutex::new(String::new()));
    capture(&mut program, &unread, &unread, None);
    let no_arguments: [&str; 0] = [];
    match program.parse(no_arguments, ParseOptions::USER) {
        Err(ParseError::Other(error)) => Some(error.to_string()),
        Err(ParseError::Commander(error)) => Some(format!("a commander exit: {}", error.message)),
        Ok(_) => None,
    }
}

#[test]
fn the_port_does_what_commander_does_for_every_recorded_run() {
    let mut programs: Vec<(String, Vec<Value>)> = Vec::new();
    let mut runs = 0;
    let mut refused = 0;
    let mut failures = Vec::new();
    for line in ORACLE.lines() {
        let record: Value = serde_json::from_str(line).expect("a JSON line in the fixture");
        let name = text(&record["program"]);
        if let Some(expected) = record.get("definitionError") {
            refused += 1;
            let actual = definition_error(list(&record["ops"]));
            if actual.as_deref() != Some(text(expected)) {
                failures.push(format!(
                    "definition \"{name}\"\n  commander: {expected}\n  port:      {actual:?}"
                ));
            }
            continue;
        }
        if let Some(ops) = record.get("ops") {
            programs.push((name.to_owned(), list(ops).to_vec()));
            continue;
        }
        let (_, ops) = programs
            .iter()
            .find(|(program, _)| program == name)
            .expect("the program line precedes its runs");
        let argv = strings(&record["argv"]);
        let actual = run(ops, &argv);
        runs += 1;
        for field in ["error", "stdout", "stderr", "actions", "opts"] {
            if actual[field] != record[field] {
                failures.push(format!(
                    "{name} {argv:?}: {field}\n  commander: {}\n  port:      {}",
                    record[field], actual[field]
                ));
            }
        }
    }
    assert!(runs >= 700, "the fixture lost its runs: {runs}");
    assert!(
        refused >= 9,
        "the fixture lost its refused definitions: {refused}"
    );
    assert!(
        failures.is_empty(),
        "{} differences in {runs} runs; the first ones:\n{}",
        failures.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
