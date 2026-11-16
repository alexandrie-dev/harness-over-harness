mod support;
use hoh_commander::{Argument, Command, CommanderError, Opt, Output, ParseError, ParseOptions};
use std::sync::{Arc, Mutex};
use support::{Capture, failure, program};

#[test]
fn command_and_related_values_are_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Command>();
    assert_send::<Opt>();
    assert_send::<Argument>();
    assert_send::<Output>();
    assert_send::<ParseError>();
    assert_send::<CommanderError>();
}

#[test]
fn exit_callback_after_exit_override_is_called() {
    let mut p = program();
    let exits = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&exits);
    p.exit_override()
        .exit_callback(move |code| recorded.lock().unwrap().push(code));
    assert_eq!(failure(&mut p, &["--unknown"]).exit_code, 1);
    assert_eq!(*exits.lock().unwrap(), [1]);
}

#[test]
fn exit_override_after_exit_callback_suppresses_callback() {
    let mut p = program();
    let exits = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&exits);
    p.exit_callback(move |code| recorded.lock().unwrap().push(code))
        .exit_override();
    assert_eq!(failure(&mut p, &["--unknown"]).exit_code, 1);
    assert!(exits.lock().unwrap().is_empty());
}

#[test]
fn empty_help_command_name_displays_program_help_on_stdout() {
    let mut p = program();
    let capture = Capture::default();
    capture.install(&mut p);
    p.name("test").command("sub");
    assert_eq!(failure(&mut p, &["help", ""]).exit_code, 0);
    assert!(
        capture
            .out
            .lock()
            .unwrap()
            .starts_with("Usage: test [options] [command]\n")
    );
    assert!(capture.err.lock().unwrap().is_empty());
}

#[test]
fn definition_errors_are_first_per_command_and_checked_before_parse() {
    let mut p = program();
    p.argument("[rest...]", "")
        .argument("next", "")
        .add_help_text("invalid", "text");
    let never_read = std::iter::from_fn(|| -> Option<&str> {
        panic!("definition validation must precede argv iteration")
    });
    let error = p.parse(never_read, ParseOptions::USER).err().unwrap();
    assert!(error.commander().is_none());
    assert_eq!(
        error.to_string(),
        "only the last argument can be variadic 'rest'"
    );
    assert!(p.get_name().is_empty());
}

#[test]
fn definitions_are_checked_in_whole_tree_depth_first_with_root_first() {
    let mut p = program();
    p.command("first")
        .command("deep")
        .argument("[rest...]", "")
        .argument("next", "");
    p.command("second").add_help_text("invalid", "text");
    assert_eq!(
        p.parse(["second"], ParseOptions::USER)
            .err()
            .unwrap()
            .to_string(),
        "only the last argument can be variadic 'rest'"
    );
    p.option("--root", "").option("--root", "");
    assert_eq!(
        p.parse(["--help"], ParseOptions::USER)
            .err()
            .unwrap()
            .to_string(),
        "Cannot add option '--root' due to conflicting flag '--root'\n-  already used by option '--root'"
    );
}

#[test]
fn pass_through_uses_parent_setting_at_builder_call_time() {
    let mut p = program();
    p.command("sub").argument("[args...]", "");
    p.enable_positional_options(true);
    p.commands[0].pass_through_options(true);
    p.enable_positional_options(false);
    p.parse(["sub", "arg"], ParseOptions::USER).unwrap();
    assert_eq!(p.commands[0].args, ["arg"]);

    let mut p = program();
    p.command("sub")
        .pass_through_options(true)
        .pass_through_options(false);
    p.enable_positional_options(true);
    assert_eq!(
        p.parse([] as [&str; 0], ParseOptions::USER)
            .err()
            .unwrap()
            .to_string(),
        "passThroughOptions cannot be used for 'sub' without turning on enablePositionalOptions for parent command(s)"
    );

    let mut p = program();
    p.enable_positional_options(true)
        .command("sub")
        .pass_through_options(true)
        .argument("[args...]", "");
    p.parse(["sub", "arg"], ParseOptions::USER).unwrap();
}

#[test]
fn duplicate_flag_message_uses_short_owner_and_long_conflicting_flag() {
    let mut p = program();
    p.option("-a, --one", "")
        .option("-b, --two", "")
        .option("-a, --two", "");
    assert_eq!(
        p.parse([] as [&str; 0], ParseOptions::USER)
            .err()
            .unwrap()
            .to_string(),
        "Cannot add option '-a, --two' due to conflicting flag '--two'\n-  already used by option '-a, --one'"
    );
}

#[test]
fn help_uses_separate_output_and_error_widths() {
    let (out, err) = (
        Arc::new(Mutex::new(String::new())),
        Arc::new(Mutex::new(String::new())),
    );
    let (to_out, to_err) = (Arc::clone(&out), Arc::clone(&err));
    let mut p = program();
    let description = "word ".repeat(30);
    p.description(&description).configure_output(
        Output::new(
            move |s| to_out.lock().unwrap().push_str(s),
            move |s| to_err.lock().unwrap().push_str(s),
        )
        .out_help_width(50)
        .err_help_width(120),
    );
    p.output_help(false);
    p.output_help(true);
    assert!(out.lock().unwrap().contains(&format!(
        "{}\n{}\n{}",
        "word ".repeat(10).trim_end(),
        "word ".repeat(10).trim_end(),
        "word ".repeat(10).trim_end()
    )));
    assert!(err.lock().unwrap().contains(&format!(
        "{}\n{}",
        "word ".repeat(24).trim_end(),
        "word ".repeat(6).trim_end()
    )));
}
