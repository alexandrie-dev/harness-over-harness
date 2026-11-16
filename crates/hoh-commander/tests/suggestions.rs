mod support;
use hoh_commander::Opt;
use support::{failure, program};

// help.suggestion.test.js: both complete distance tables (28 source cases).
#[test]
fn when_cli_typo_then_suggest_closest_similar_commands_and_options() {
    let cases: &[(&str, &[&str], Option<&str>)] = &[
        ("yyy", &["zzz"], None),
        ("a", &["b"], None),
        ("a", &["ab"], Some("ab")),
        ("ab", &["a"], None),
        ("at", &["cat"], Some("cat")),
        ("cat", &["at"], Some("at")),
        ("bat", &["cat"], Some("cat")),
        ("act", &["cat"], Some("cat")),
        ("cxx", &["cat"], None),
        ("caxx", &["cart"], Some("cart")),
        ("1234567", &["1234567890"], Some("1234567890")),
        ("123456", &["1234567890"], None),
        ("xat", &["rat", "cat", "bat"], Some("bat, cat, rat")),
        ("cart", &["camb", "cant", "bard"], Some("cant")),
    ];
    for (word, names, expected) in cases {
        for options in [false, true] {
            let mut p = program();
            p.help_option_enabled(false).help_command(false);
            for name in *names {
                if options {
                    p.option(&format!("--{name}"), "");
                } else {
                    p.command(name);
                }
            }
            let word = if options {
                format!("--{word}")
            } else {
                word.to_string()
            };
            let e = failure(&mut p, &[&word]);
            let suggestion = expected
                .map(|s| {
                    let s = if options {
                        s.split(", ")
                            .map(|s| format!("--{s}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    } else {
                        s.to_string()
                    };
                    format!(
                        "\n(Did you mean {}{s}?)",
                        if s.contains(", ") { "one of " } else { "" }
                    )
                })
                .unwrap_or_default();
            assert_eq!(
                e.message,
                format!(
                    "error: unknown {} '{word}'{suggestion}",
                    if options { "option" } else { "command" }
                )
            );
        }
    }
}
#[test]
fn when_implicit_help_command_then_candidate_unless_disabled() {
    let mut p = program();
    p.command("sub");
    assert_eq!(
        failure(&mut p, &["hepl"]).message,
        "error: unknown command 'hepl'\n(Did you mean help?)"
    );
    p.help_command(false);
    assert_eq!(
        failure(&mut p, &["hepl"]).message,
        "error: unknown command 'hepl'"
    );
}
#[test]
fn when_default_or_custom_help_option_then_candidate_unless_disabled() {
    let mut p = program();
    assert_eq!(
        failure(&mut p, &["--hepl"]).message,
        "error: unknown option '--hepl'\n(Did you mean --help?)"
    );
    p.help_option("-H, --custom-help", "help");
    assert_eq!(
        failure(&mut p, &["--custom-hepl"]).message,
        "error: unknown option '--custom-hepl'\n(Did you mean --custom-help?)"
    );
    p.help_option_enabled(false);
    assert_eq!(
        failure(&mut p, &["--hepl"]).message,
        "error: unknown option '--hepl'"
    );
}
#[test]
fn when_local_and_global_options_then_both_candidates_unless_positional() {
    for positional in [false, true] {
        let mut p = program();
        p.enable_positional_options(positional).option("--cat", "");
        p.command("sub").option("--rat", "");
        assert_eq!(
            failure(&mut p, &["sub", "--bat"]).message,
            if positional {
                "error: unknown option '--bat'\n(Did you mean --rat?)"
            } else {
                "error: unknown option '--bat'\n(Did you mean one of --cat, --rat?)"
            }
        );
    }
}
#[test]
fn when_option_hidden_then_not_suggested() {
    let mut p = program();
    p.add_option(Opt::new("--secret", "").hide_help(true));
    assert_eq!(
        failure(&mut p, &["--secrt"]).message,
        "error: unknown option '--secrt'"
    );
}
#[test]
fn when_duplicate_identical_candidates_then_only_one_returned() {
    let mut p = program();
    p.command("sub");
    assert_eq!(
        failure(&mut p, &["sub", "--hepl"]).message,
        "error: unknown option '--hepl'\n(Did you mean --help?)"
    );
}
