mod support;
use hoh_commander::{Command, ParseOptions};
use serde_json::json;
use support::{Capture, failure, programs};

fn check(mut p: Command, args: &[&str], bad: &[&str], plugin: bool) -> Command {
    let output = Capture::default();
    output.install_recursive(&mut p);
    let help = if plugin {
        p.commands[0].help_information()
    } else {
        p.help_information()
    };
    assert!(!help.is_empty());
    let cmd = if plugin { &p.commands[0] } else { &p };
    for opt in &cmd.options {
        assert!(help.contains(&opt.flags), "{}", opt.flags);
    }
    if plugin {
        p.commands[0].output_help(false);
    } else {
        p.output_help(false);
    }
    assert!(output.out.lock().unwrap().contains("Usage:"));
    assert_ne!(failure(&mut p, bad).exit_code, 0);
    p.parse(args, ParseOptions::USER).unwrap();
    p
}
#[test]
fn launcher() {
    let p = check(
        programs::dsh(),
        &["--profile", "tui", "--patch", "a.yml", "--resume", "abc"],
        &["--profile"],
        false,
    );
    assert_eq!(p.opts()["profile"], "tui");
    assert_eq!(p.opts()["patch"], json!(["a.yml"]));
}
#[test]
fn plugin() {
    let p = check(
        programs::plugin(),
        &["plugin", "--profile", "tui", "add", "pkg", "--global"],
        &["plugin"],
        true,
    );
    assert_eq!(p.commands[0].opts()["profile"], "tui");
}
#[test]
fn headless() {
    check(
        programs::headless(),
        &["--json", "--approval-policy", "ask", "run", "tests"],
        &["--approval-policy", "invalid"],
        false,
    );
}
#[test]
fn sdk() {
    check(programs::sdk(), &[], &["--invalid"], false);
}
#[test]
fn acp() {
    check(programs::acp(), &[], &["extra"], false);
}
#[test]
fn web() {
    check(
        programs::web(),
        &[
            "--port",
            "8080",
            "--no-open",
            "--trusted-host",
            "a.test",
            "b.test",
        ],
        &["--port"],
        false,
    );
}
