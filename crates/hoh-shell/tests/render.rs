use hoh_shell::{ParsedExitStatus, parse_exit_status};

#[test]
fn recovers_a_clean_exit_0_with_the_body_verbatim_when_no_marker_is_present() {
    assert_eq!(
        parse_exit_status("hi\n\n"),
        ParsedExitStatus::ExitCode {
            body: "hi\n\n".into(),
            exit_code: 0,
        }
    );
    assert_eq!(
        parse_exit_status(""),
        ParsedExitStatus::ExitCode {
            body: String::new(),
            exit_code: 0,
        }
    );
}

#[test]
fn recovers_a_non_zero_exit_and_strips_only_its_marker_from_the_body() {
    assert_eq!(
        parse_exit_status("oops\n[exit code: 3]"),
        ParsedExitStatus::ExitCode {
            body: "oops".into(),
            exit_code: 3,
        }
    );
    assert_eq!(
        parse_exit_status("[exit code: 5]"),
        ParsedExitStatus::ExitCode {
            body: "[exit code: 5]".into(),
            exit_code: 0,
        }
    );
}

#[test]
fn recovers_a_signal_kill_ahead_of_any_non_zero_exit_marker() {
    assert_eq!(
        parse_exit_status("gone\n[killed by signal: SIGKILL]"),
        ParsedExitStatus::Signal {
            body: "gone".into(),
            signal: "SIGKILL".into(),
        }
    );
    assert_eq!(
        parse_exit_status("[killed by signal: SIGKILL]"),
        ParsedExitStatus::ExitCode {
            body: "[killed by signal: SIGKILL]".into(),
            exit_code: 0,
        }
    );
}

#[test]
fn keeps_markers_no_pill_shows_timeout_in_the_body() {
    assert_eq!(
        parse_exit_status("slow\n[timed out after 100ms]\n[exit code: 143]"),
        ParsedExitStatus::ExitCode {
            body: "slow\n[timed out after 100ms]".into(),
            exit_code: 143,
        }
    );
}
