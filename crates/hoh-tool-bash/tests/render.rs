//! Ports of the pure rendering cases of tools.spec.ts (`renderJobRead`, `renderResult`) and
//! background.spec.ts (`renderPromoted`).

use hoh_sandbox::SandboxMode;
use hoh_shell::{CollectedOutput, ShellRunResult, ShellSandboxInfo};
use hoh_tool_bash::render::{RenderedRun, render_job_read, render_promoted, render_result};

fn stream(text: &str, truncated: bool) -> CollectedOutput {
    CollectedOutput {
        text: text.to_owned(),
        truncated,
        spill_path: None,
    }
}

fn base() -> ShellRunResult {
    ShellRunResult {
        exit_code: Some(0),
        signal: None,
        timed_out: false,
        aborted: false,
        timeout_ms: 1000.0,
        stdout: stream("", false),
        stderr: stream("", false),
        sandbox: None,
    }
}

fn render(result: &ShellRunResult) -> String {
    render_result(
        &RenderedRun {
            result,
            stopped: None,
        },
        &[],
    )
}

mod render_job_read_cases {
    use super::*;

    fn sandbox(mode: SandboxMode, denied: bool, runner_failed: Option<bool>) -> ShellSandboxInfo {
        ShellSandboxInfo {
            mode,
            denied,
            enforcement: None,
            runner_failed,
        }
    }

    #[test]
    fn returns_the_delta_verbatim_for_a_lossless_read() {
        assert_eq!(render_job_read("out\n", false, &[], None, &[]), "out\n");
        assert_eq!(render_job_read("", false, &[], None, &[]), "");
    }

    #[test]
    fn appends_the_loss_notice_with_the_available_spill_paths() {
        assert_eq!(
            render_job_read("out\n", true, &["/spill/out.log".to_owned()], None, &[]),
            "out\n[some output was dropped from memory; full output: /spill/out.log]"
        );
        assert_eq!(
            render_job_read(
                "out\n",
                true,
                &["/spill/out.log".to_owned(), "/spill/err.log".to_owned()],
                None,
                &[]
            ),
            "out\n[some output was dropped from memory; full output: /spill/out.log, /spill/err.log]"
        );
    }

    #[test]
    fn reports_unavailable_when_a_lossy_read_has_no_safe_spill_path() {
        assert_eq!(
            render_job_read("out\n", true, &[], None, &[]),
            "out\n[some output was dropped from memory; full output: (unavailable)]"
        );
    }

    #[test]
    fn an_empty_lossy_delta_is_the_notice_alone() {
        assert_eq!(
            render_job_read("", true, &["/spill/err.log".to_owned()], None, &[]),
            "[some output was dropped from memory; full output: /spill/err.log]"
        );
    }

    #[test]
    fn inserts_the_separating_newline_only_when_the_delta_lacks_one() {
        assert_eq!(
            render_job_read("tail", true, &[], None, &[]),
            "tail\n[some output was dropped from memory; full output: (unavailable)]"
        );
        assert_eq!(
            render_job_read("tail\n", true, &[], None, &[]),
            "tail\n[some output was dropped from memory; full output: (unavailable)]"
        );
    }

    #[test]
    fn appends_settled_sandbox_denial_and_runner_failure_facts() {
        let denied = sandbox(SandboxMode::ReadOnly, true, None);
        assert!(
            render_job_read(
                "out\n",
                false,
                &[],
                Some(&denied),
                &[SandboxMode::WorkspaceWrite]
            )
            .contains("[sandbox: escalation available")
        );
        assert_eq!(
            render_job_read("tail", false, &[], Some(&denied), &[]),
            "tail\n[sandbox: file access denied under read-only mode]"
        );
        let runner = render_job_read(
            "",
            false,
            &[],
            Some(&sandbox(SandboxMode::WorkspaceWrite, true, Some(true))),
            &[SandboxMode::DangerFullAccess],
        );
        assert!(runner.contains("sandbox runner itself failed under workspace-write mode"));
        assert!(!runner.contains("file access denied"));
    }
}

mod render_result_cases {
    use super::*;

    #[test]
    fn renders_stderr_only_output_without_a_stdout_prefix() {
        let result = ShellRunResult {
            stderr: stream("err\n", false),
            ..base()
        };
        assert_eq!(render(&result), "[stderr]\nerr\n");
    }

    #[test]
    fn adds_a_separator_when_stdout_does_not_end_with_a_newline() {
        let result = ShellRunResult {
            stdout: stream("out", false),
            stderr: stream("err", false),
            ..base()
        };
        assert_eq!(render(&result), "out\n[stderr]\nerr");
    }

    #[test]
    fn appends_exit_code_markers_after_a_newline_for_unterminated_output() {
        let result = ShellRunResult {
            exit_code: Some(7),
            stdout: stream("x", false),
            ..base()
        };
        assert_eq!(render(&result), "x\n[exit code: 7]");
    }

    #[test]
    fn renders_signal_kills_without_the_timeout_marker_when_not_timed_out() {
        let result = ShellRunResult {
            exit_code: None,
            signal: Some("SIGKILL".into()),
            ..base()
        };
        assert_eq!(render(&result), "(no output)\n[killed by signal: SIGKILL]");
    }

    #[test]
    fn reports_a_timeout_that_exited_zero_trapped_signal_without_a_kill_marker() {
        let result = ShellRunResult {
            timed_out: true,
            ..base()
        };
        assert_eq!(render(&result), "(no output)\n[timed out after 1000ms]");
    }

    #[test]
    fn orders_the_timeout_marker_before_a_kill_marker() {
        let result = ShellRunResult {
            exit_code: None,
            signal: Some("SIGTERM".into()),
            timed_out: true,
            ..base()
        };
        assert_eq!(
            render(&result),
            "(no output)\n[timed out after 1000ms]\n[killed by signal: SIGTERM]"
        );
    }

    #[test]
    fn reports_an_outside_stop_by_its_reason_ahead_of_the_signal_marker_the_exit_pill_parses() {
        let result = ShellRunResult {
            exit_code: None,
            signal: Some("SIGTERM".into()),
            ..base()
        };
        assert_eq!(
            render_result(
                &RenderedRun {
                    result: &result,
                    stopped: Some("cancelled by the user")
                },
                &[]
            ),
            "(no output)\n[stopped: cancelled by the user]\n[killed by signal: SIGTERM]"
        );
    }

    #[test]
    fn notes_truncation_with_a_fallback_when_the_spill_path_is_missing() {
        let result = ShellRunResult {
            stdout: stream("tail", true),
            ..base()
        };
        assert_eq!(
            render(&result),
            "tail\n[output truncated; full output: (unavailable)]"
        );
    }

    #[test]
    fn reports_sandbox_denials_before_exit_status_and_hints_only_when_escalation_is_advertised() {
        let result = ShellRunResult {
            exit_code: Some(1),
            stderr: stream("denied", false),
            sandbox: Some(ShellSandboxInfo {
                mode: SandboxMode::ReadOnly,
                denied: true,
                enforcement: None,
                runner_failed: None,
            }),
            ..base()
        };
        let plain = render(&result);
        assert!(
            plain.ends_with("denied under read-only mode]\n[exit code: 1]"),
            "{plain}"
        );
        let hinted = render_result(
            &RenderedRun {
                result: &result,
                stopped: None,
            },
            &[SandboxMode::WorkspaceWrite],
        );
        assert!(hinted.contains("[sandbox: escalation available"));
        let not_denied = ShellRunResult {
            sandbox: Some(ShellSandboxInfo {
                mode: SandboxMode::ReadOnly,
                denied: false,
                enforcement: None,
                runner_failed: None,
            }),
            ..result
        };
        assert!(
            !render_result(
                &RenderedRun {
                    result: &not_denied,
                    stopped: None
                },
                &[SandboxMode::WorkspaceWrite]
            )
            .contains("[sandbox:")
        );
    }
}

#[test]
fn render_promoted_pins_the_promoted_text_with_and_without_pre_promotion_output() {
    assert_eq!(
        render_promoted("bash-7", 120_000.0, ""),
        "[still running after 120000ms; moved to background job bash-7]"
    );
    // A partial line gains the separating newline exactly once.
    assert!(
        render_promoted("bash-7", 250.0, "partial")
            .contains("partial\n[still running after 250ms; moved to background job bash-7]")
    );
    assert!(
        render_promoted("bash-7", 250.0, "line\n").contains("line\n[still running after 250ms")
    );
}
