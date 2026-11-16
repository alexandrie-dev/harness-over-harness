// command.chain.test.js: methods in the supported synchronous API.
mod support;
#[test]
fn when_call_argument_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.argument("[arg]", "") as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_add_argument_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned =
        p.add_argument(hoh_commander::Argument::new("[arg]", "")) as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_help_command_name_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.help_command_with_description("help [cmd]", "display help for command")
        as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_help_command_name_description_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned =
        p.help_command_with_description("help [cmd]", "custom") as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_help_command_boolean_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.help_command(false) as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_exit_override_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.exit_override() as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_action_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.action(|_, _, _| Ok(())) as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_add_option_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned =
        p.add_option(hoh_commander::Opt::new("--debug", "")) as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_option_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.option("--debug", "") as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_required_option_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.required_option("--need <value>", "") as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_combine_flag_and_optional_value_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.combine_flag_and_optional_value(true) as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_allow_unknown_option_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.allow_unknown_option(true) as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_allow_excess_arguments_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.allow_excess_arguments(true) as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_version_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.version("1.2.3") as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_set_description_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.description("description") as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_set_usage_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.usage("custom usage") as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_set_name_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.name("test") as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_help_option_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.help_option("-H, --HELP", "custom") as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_add_help_text_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.add_help_text("after", "custom") as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_configure_output_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.configure_output(hoh_commander::Output::new(|_| {}, |_| {}))
        as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_pass_through_options_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.pass_through_options(true) as *const hoh_commander::Command;
    assert_eq!(original, returned);
}

#[test]
fn when_call_enable_positional_options_then_returns_this() {
    let mut p = support::program();
    let original = &raw const p;
    let returned = p.enable_positional_options(true) as *const hoh_commander::Command;
    assert_eq!(original, returned);
}
