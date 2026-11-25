use hoh::args::parse_dsh_args;
use hoh_commander::{Output, ParseError};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
fn parse(args: &[&str], desktop: bool) -> (Result<Value, i32>, String, String) {
    let out = Arc::new(Mutex::new(String::new()));
    let err = Arc::new(Mutex::new(String::new()));
    let o = out.clone();
    let e = err.clone();
    let output = Output::new(
        move |s| o.lock().unwrap().push_str(s),
        move |s| e.lock().unwrap().push_str(s),
    );
    let result = parse_dsh_args(
        &args.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
        "1.2.3",
        desktop,
        output,
    )
    .map(|v| serde_json::to_value(v).unwrap())
    .map_err(|e| match e {
        ParseError::Commander(e) => e.exit_code,
        ParseError::Other(e) => panic!("{e}"),
    });
    let stdout = out.lock().unwrap().clone();
    let stderr = err.lock().unwrap().clone();
    (result, stdout, stderr)
}
fn value(args: &[&str]) -> Value {
    parse(args, false).0.unwrap()
}
fn code(args: &[&str]) -> i32 {
    parse(args, false).0.unwrap_err()
}
#[test]
fn routes_profile_boots_and_shorthand_handing_the_rest_to_the_app() {
    for args in [vec!["--profile", "tui"], vec!["tui"]] {
        assert_eq!(
            value(&args),
            json!({"mode":"profile","profile":"tui","patches":[],"args":[]})
        );
    }
    assert_eq!(
        value(&["--profile", "tui", "--patch", "a.yml", "--patch", "b.yml"]),
        json!({"mode":"profile","profile":"tui","patches":["a.yml","b.yml"],"args":[]})
    );
    assert_eq!(
        value(&["--profile", "rescue", "--from-default-profile", "web"]),
        json!({"mode":"profile","profile":"rescue","fromDefaultProfile":"web","patches":[],"args":[]})
    );
    assert_eq!(
        value(&["web", "--patch", "web.yml"]),
        json!({"mode":"profile","profile":"web","patches":["web.yml"],"args":[]})
    );
}
#[test]
fn ends_the_launcher_flags_at_the_first_token_it_does_not_own() {
    for (profile, args) in [
        ("tui", vec!["--resume", "abc"]),
        ("web", vec!["-h"]),
        (
            "web",
            vec![
                "--host",
                "127.0.0.1",
                "--port",
                "8080",
                "--no-open",
                "--future-web-flag",
            ],
        ),
        ("headless", vec!["run", "the", "tests"]),
        (
            "rescue",
            vec!["--resume", "abc", "--from-default-profile", "web"],
        ),
    ] {
        let mut argv = vec!["--profile", profile];
        argv.extend(args.clone());
        assert_eq!(
            value(&argv),
            json!({"mode":"profile","profile":profile,"patches":[],"args":args})
        );
    }
    assert_eq!(
        value(&[
            "--profile",
            "tui",
            "--patch",
            "a.yml",
            "--resume",
            "b",
            "--patch",
            "late.yml"
        ])["args"],
        json!(["--resume", "b", "--patch", "late.yml"])
    );
    assert_eq!(
        value(&["web", "--from-default-profile", "web"])["fromDefaultProfile"],
        "web"
    );
}
#[test]
fn expands_profiles_without_looking_up_profiles() {
    let variants = [
        vec![],
        vec!["task", "words"],
        vec!["--help"],
        vec!["-h"],
        vec!["web"],
        vec!["--patch", "a.yml", "--patch", "b.yml"],
        vec!["--from-default-profile", "web", "--help"],
        vec!["--dump-config"],
        vec!["--dump-default-config"],
        vec!["--dump-config-schema"],
        vec!["--patch", "a.yml", "--resume", "id", "--patch", "late.yml"],
        vec!["--", "--help"],
        vec!["--", "--", "task"],
        vec!["plugin"],
        vec!["--", "plugin"],
    ];
    for profile in [
        "web",
        "headless",
        "sdk",
        "sdk-minimal",
        "acp",
        "tui",
        "custom",
        "run",
        "help",
    ] {
        for args in &variants {
            let mut shorthand = vec![profile];
            shorthand.extend(args.clone());
            let mut full = vec!["--profile", profile];
            full.extend(args.clone());
            assert_eq!(
                parse(&shorthand, false),
                parse(&full, false),
                "{shorthand:?}"
            );
        }
    }
}
#[test]
fn reserves_leading_plugin_for_management_and_forwards_later_command_names() {
    assert_eq!(value(&["--profile", "plugin"])["mode"], "profile");
    assert_eq!(
        value(&["--profile", "x", "plugin", "add", "y"])["args"],
        json!(["plugin", "add", "y"])
    );
    assert_eq!(value(&["headless", "web"])["args"], json!(["web"]));
    for a in [vec!["--patch", "a.yml", "tui"], vec!["--", "tui"]] {
        assert_eq!(code(&a), 1);
    }
}
#[test]
fn rejects_invalid_shorthand() {
    for a in [
        vec![""],
        vec!["desktop"],
        vec!["Desktop"],
        vec!["DESKTOP"],
        vec!["custom", "--patch="],
        vec!["custom", "--from-default-profile="],
        vec!["custom", "--dump-config", "--dump-default-config"],
        vec!["custom", "--dump-default-config", "--patch", "a.yml"],
        vec!["custom", "--dump-config", "task"],
    ] {
        assert_eq!(code(&a), 1, "{a:?}");
    }
}
#[test]
fn prints_the_launcher_version_for_shorthand() {
    for flag in ["-V", "--version"] {
        let (result, out, _) = parse(&["custom", flag], false);
        assert_eq!(result, Err(0));
        assert_eq!(out, "1.2.3\n");
        assert_eq!(
            value(&["custom", "task", flag])["args"],
            json!(["task", flag])
        );
    }
}
#[test]
fn rejects_repeated_profile_selection() {
    for a in [
        vec!["web", "--profile", "tui"],
        vec!["--profile", "web", "--profile", "tui"],
        vec!["--profile=web", "--profile=web"],
        vec!["plugin", "--profile", "web", "--profile", "tui", "add", "x"],
    ] {
        let (result, _, err) = parse(&a, false);
        assert_eq!(result, Err(1));
        assert!(err.contains("Профиль можно указать только один раз"), "{err}");
    }
}
#[test]
fn forwards_late_profile_options_to_the_application() {
    assert_eq!(
        value(&["web", "task", "--profile", "tui"])["args"],
        json!(["task", "--profile", "tui"])
    );
}
#[test]
fn routes_the_plugin_pnpm_forwarder() {
    for args in [
        vec!["add", "turtle-ui"],
        vec!["remove", "turtle-ui"],
        vec!["why", "@deepseek-ai/cordis"],
        vec!["add", "--save-dev", "x"],
        vec!["add", "--help"],
        vec!["add", "-h", "--profile", "x"],
        vec!["--filter", "x", "--profile", "y", "add", "z"],
    ] {
        let mut a = vec!["plugin", "--profile", "tui"];
        a.extend(args.clone());
        assert_eq!(
            value(&a),
            json!({"mode":"plugin","profile":"tui","args":args})
        );
    }
    for a in [
        vec!["plugin", "add", "x", "--profile", "tui"],
        vec!["plugin", "--filter", "x", "add", "--profile", "tui"],
    ] {
        assert_eq!(code(&a), 1);
    }
}
#[test]
fn prints_the_plugin_forwarder_help() {
    for a in [
        vec!["plugin", "--help"],
        vec!["plugin", "-h"],
        vec!["plugin", "--profile", "tui", "--help"],
        vec![
            "plugin",
            "--profile",
            "tui",
            "--filter",
            "x",
            "add",
            "--help",
        ],
    ] {
        let (result, out, _) = parse(&a, false);
        assert_eq!(result, Err(0));
        assert!(out.contains("Использование: hoh plugin [параметры] [аргументы…]"));
    }
}
#[test]
fn permits_installed_desktop_plugin_commands() {
    for profile in ["desktop", "Desktop", "DESKTOP"] {
        let a = ["plugin", "--profile", profile, "add", "example-plugin"];
        assert_eq!(
            parse(&a, true).0.unwrap(),
            json!({"mode":"plugin","profile":"desktop","args":["add","example-plugin"]})
        );
        assert_eq!(code(&a), 1);
    }
}
#[test]
fn keeps_desktop_boot_and_inspection_reserved_in_its_installed_cli() {
    for a in [
        vec!["desktop"],
        vec!["--profile", "Desktop"],
        vec!["desktop", "--dump-config"],
        vec!["DESKTOP", "--dump-default-config"],
        vec!["desktop", "--dump-config-schema"],
    ] {
        assert_eq!(parse(&a, true).0, Err(1));
    }
}
#[test]
fn routes_profile_and_web_config_dumps() {
    for flag in ["--dump-config", "--dump-default-config"] {
        for prefix in [vec!["--profile", "web"], vec!["web"]] {
            let mut a = prefix;
            a.push(flag);
            assert_eq!(
                value(&a),
                json!({"mode":"dump-config","profile":"web","defaultOnly":flag=="--dump-default-config","patches":[]})
            );
        }
    }
    assert_eq!(
        value(&[
            "--profile",
            "rescue",
            "--from-default-profile",
            "web",
            "--dump-config"
        ])["fromDefaultProfile"],
        "web"
    );
    assert_eq!(
        value(&["--profile", "tui", "--dump-config", "--patch", "x.yml"])["patches"],
        json!(["x.yml"])
    );
}
#[test]
fn routes_schema_dumps_with_ordered_overlays_and_explicit_initialization() {
    assert_eq!(
        value(&["--profile", "web", "--dump-config-schema"]),
        json!({"mode":"dump-config-schema","profile":"web","patches":[]})
    );
    assert_eq!(
        value(&[
            "rescue",
            "--from-default-profile",
            "web",
            "--patch",
            "first.yml",
            "--dump-config-schema",
            "--patch",
            "second.json"
        ]),
        json!({"mode":"dump-config-schema","profile":"rescue","fromDefaultProfile":"web","patches":["first.yml","second.json"]})
    );
    assert_eq!(
        value(&["web", "--port", "8080", "--dump-config-schema"])["args"],
        json!(["--port", "8080", "--dump-config-schema"])
    );
}
#[test]
fn rejects_competing_dump_flags() {
    for flags in [
        vec!["--dump-config", "--dump-default-config"],
        vec!["--dump-config", "--dump-config-schema"],
        vec!["--dump-default-config", "--dump-config-schema"],
        vec![
            "--dump-config",
            "--dump-default-config",
            "--dump-config-schema",
        ],
    ] {
        let mut a = vec!["web"];
        a.extend(flags.clone());
        assert_eq!(code(&a), 1);
        let mut a = vec!["--profile", "web"];
        a.extend(flags.into_iter().rev());
        assert_eq!(code(&a), 1);
    }
}
#[test]
fn rejects_invalid_schema_dump_inputs() {
    for a in [
        vec!["--dump-config-schema"],
        vec!["desktop", "--dump-config-schema"],
        vec!["--profile", "Desktop", "--dump-config-schema"],
        vec!["web", "--dump-config-schema", "task"],
        vec!["web", "--dump-config-schema", "--port", "8080"],
        vec!["web", "--dump-config-schema", "--help"],
        vec!["web", "--dump-config-schema", "--", "--patch", "late.yml"],
        vec!["web", "--dump-config-schema", "--patch="],
        vec!["rescue", "--dump-config-schema", "--from-default-profile="],
    ] {
        assert_eq!(code(&a), 1, "{a:?}");
    }
}
#[test]
fn rejects_missing_profile_removed_flags_and_contradictory_inputs() {
    for a in [
        vec![],
        vec!["--config", "c.yml"],
        vec!["-p", "task"],
        vec!["--profile", ""],
        vec!["--profile", "x", "--from-default-profile="],
        vec!["--profile", "x", "--from-default-profile"],
        vec!["--profile", "x", "--patch="],
        vec!["--dump-config"],
        vec!["--profile", "x", "--dump-config", "--dump-default-config"],
        vec![
            "--profile",
            "x",
            "--dump-default-config",
            "--patch",
            "p.yml",
        ],
        vec!["--profile", "x", "--dump-config", "task"],
        vec!["--bogus"],
        vec!["web", "--dump-config", "--dump-default-config"],
        vec!["web", "--dump-default-config", "--patch", "w.yml"],
        vec!["web", "--patch="],
        vec!["web", "--dump-config", "--port", "8080"],
        vec!["--profile", "web", "--dump-config", "-h"],
        vec!["plugin", "add", "x"],
        vec!["plugin", "--profile", "tui"],
        vec!["plugin", "--profile", ""],
        vec!["--profile", "desktop"],
        vec!["--profile", "Desktop"],
        vec!["--profile", "DESKTOP"],
        vec!["--profile", "desktop", "--dump-config"],
        vec!["plugin", "--profile", "desktop", "add", "x"],
        vec!["plugin", "--profile", "Desktop", "add", "x"],
        vec![
            "--from-default-profile",
            "web",
            "plugin",
            "--profile",
            "x",
            "add",
            "y",
        ],
    ] {
        assert_eq!(code(&a), 1, "{a:?}");
    }
}
#[test]
fn keeps_its_own_help_for_an_invocation_with_no_app_to_hand_it_to() {
    for flag in ["--help", "-h"] {
        let (result, out, _) = parse(&[flag], false);
        assert_eq!(result, Err(0));
        assert!(!out.contains("help [command]"));
        assert_eq!(
            out.strip_suffix('\n').unwrap(),
            include_str!("expected/launcher-help.txt")
        );
    }
    assert_eq!(code(&["--version"]), 0);
}

#[test]
fn routes_plugin_new_without_a_profile() {
    assert_eq!(
        value(&["plugin", "new", "turn-note"]),
        json!({"mode":"plugin-new","id":"turn-note"})
    );
}
