mod support;
use hoh_commander::{Argument, Opt};
use support::program;

// command.help.test.js: expected text copied verbatim.
#[test]
fn when_call_help_information_then_format_is_as_expected() {
    let mut p = program();
    p.command("my-command <file>");
    p.name("test");
    assert_eq!(
        p.help_information(),
        "Usage: test [options] [command]\n\nOptions:\n  -h, --help         display help for command\n\nCommands:\n  my-command <file>\n  help [command]     display help for command\n"
    );
}
#[test]
fn when_command_description_then_help_includes_description() {
    let mut p = program();
    p.command("simple-command")
        .description("custom-description");
    assert!(
        p.help_information()
            .contains("simple-command  custom-description")
    );
}
#[test]
fn when_help_short_flag_masked_then_only_long_help_displayed() {
    let mut p = program();
    p.option("-h, --host", "select host");
    let help = p.help_information();
    assert!(help.contains(" -h, --host"));
    assert!(!help.contains(" -h, --help"));
    assert!(help.contains(" --help"));
}
#[test]
fn when_both_help_flags_masked_then_builtin_not_displayed() {
    let mut p = program();
    p.option("-h, --help", "custom");
    assert!(!p.help_information().contains("display help"));
}
#[test]
fn when_no_options_then_options_section_omitted() {
    let mut p = program();
    p.help_option_enabled(false);
    assert!(!p.help_information().contains("Options"));
}
#[test]
fn when_negated_option_then_included_in_help() {
    let mut p = program();
    p.option("-C, --no-colour", "colourless");
    let help = p.help_information();
    assert!(help.contains("--no-colour"));
    assert!(help.contains("colourless"));
}
#[test]
fn when_option_hidden_then_not_included_in_help() {
    for hide in [true, false] {
        let mut p = program();
        p.add_option(Opt::new("-s,--secret", "secret option").hide_help(hide));
        assert_eq!(p.help_information().contains("secret"), !hide);
    }
}
#[test]
fn when_default_and_choices_then_both_included_in_help() {
    let mut p = program();
    p.add_option(
        Opt::new("-c, --colour <colour>", "")
            .choices(["red", "blue"])
            .default("red"),
    );
    assert!(
        p.help_information()
            .contains("(choices: \"red\", \"blue\", default: \"red\")")
    );
    p.add_argument(
        Argument::new("<colour>", "preferred colour")
            .choices(["red", "blue"])
            .default("red"),
    );
    assert!(p.help_information().contains("preferred colour"));
}
#[test]
fn when_argument_described_then_all_arguments_displayed() {
    let mut p = program();
    p.help_option_enabled(false)
        .argument("<file>", "input source")
        .argument("[extra]", "");
    assert_eq!(
        p.help_information(),
        "Usage:  <file> [extra]\n\nArguments:\n  file   input source\n  extra\n"
    );
    let mut p = program();
    p.help_option_enabled(false)
        .add_argument(Argument::new("[file]", "input source").default("test.txt"));
    assert!(
        p.help_information()
            .contains("file  input source (default: \"test.txt\")")
    );
}
#[test]
fn when_arguments_not_described_then_arguments_section_omitted() {
    let mut p = program();
    p.argument("<file>", "");
    assert!(!p.help_information().contains("Arguments:"));
}
#[test]
fn when_long_term_leaves_less_than_forty_columns_then_description_not_wrapped() {
    let mut p = program();
    p.option("--very-long-option-with-a-value <value>", "a description with many words that will not wrap because less than forty columns remain for it");
    assert!(p.help_information().contains("a description with many words that will not wrap because less than forty columns remain for it\n"));
}
#[test]
fn when_description_exceeds_eighty_columns_then_wrap_at_whitespace() {
    let mut p = program();
    p.description("one two three four five six seven eight nine ten one two three four five six seven eight nine ten");
    assert!(p.help_information().contains("one two three four five six seven eight nine ten one two three four five six\nseven eight nine ten\n"));
}
#[test]
fn sdk_and_acp_full_help_bytes() {
    assert_eq!(
        support::programs::sdk().help_information(),
        "Usage: dsh --profile sdk [options]\n\nServe DeepSeek Harness SDK clients over stdio JSON-RPC.\n\nOptions:\n  -h, --help  show this help\n"
    );
    assert_eq!(
        support::programs::acp().help_information(),
        "Usage: dsh --profile acp [options]\n\nServe automation clients over Agent Client Protocol stdio.\n\nOptions:\n  -h, --help  show this help\n"
    );
}
