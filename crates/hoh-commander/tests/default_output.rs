//! Default stdio is observed in an isolated test process, without global stream replacement.
use hoh_commander::{Command, ParseOptions};

#[test]
fn exercise_default_writer_in_child() {
    let Ok(case) = std::env::var("HOH_COMMANDER_TEST_DEFAULT_OUTPUT") else {
        return;
    };
    let mut p = Command::new();
    p.name("test").exit_override();
    match case.as_str() {
        "error" => {
            assert_eq!(p.error("error: example").exit_code, 1);
        }
        "version" => {
            p.version("1.2.3");
            assert_eq!(
                p.parse(["--version"], ParseOptions::USER)
                    .err()
                    .unwrap()
                    .commander()
                    .unwrap()
                    .exit_code,
                0
            );
        }
        "help" => {
            assert_eq!(p.help().exit_code, 0);
        }
        "error_help" => {
            assert_eq!(p.help_with_error(true).exit_code, 1);
        }
        _ => panic!("unexpected test case"),
    }
}

// command.configureOutput.test.js: default writeErr, version, help and error-help.
#[test]
fn when_default_writers_then_stdout_and_stderr_receive_the_expected_bytes() {
    let executable = std::env::current_exe().unwrap();
    let help = "Usage: test [options]\n\nOptions:\n  -h, --help  display help for command\n";
    for (case, expected_out, expected_err) in [
        ("error", "", "error: example\n"),
        ("version", "1.2.3\n", ""),
        ("help", help, ""),
        ("error_help", "", help),
    ] {
        let result = std::process::Command::new(&executable)
            .args(["--exact", "exercise_default_writer_in_child", "--nocapture"])
            .env("HOH_COMMANDER_TEST_DEFAULT_OUTPUT", case)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(String::from_utf8(result.stderr).unwrap(), expected_err);
        let stdout = String::from_utf8(result.stdout).unwrap();
        if !expected_out.is_empty() {
            assert_eq!(stdout.matches(expected_out).count(), 1);
        }
        if expected_out.is_empty() {
            assert!(!stdout.contains("Usage:"));
            assert!(!stdout.contains("1.2.3"));
            assert!(!stdout.contains("error: example"));
        }
    }
}
