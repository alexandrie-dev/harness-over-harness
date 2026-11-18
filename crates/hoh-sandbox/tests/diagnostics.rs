use hoh_sandbox::*;
use serde_json::{Value, json};
fn rule() -> RunnerFailureRule {
    RunnerFailureRule {
        allowed_exit_codes: Some(vec![125]),
        fatal_signatures: vec!["landlock-run: ".into()],
        informational_lines: Some(vec![
            "landlock-run: partial enforcement (older Landlock ABI)".into(),
        ]),
    }
}
#[tokio::test]
async fn attributes_executable_class_spawn_codes_once_cwd_ambiguity_is_eliminated() {
    let d = tempfile::tempdir().unwrap();
    for code in ["EACCES", "ENOENT"] {
        assert!(
            is_runner_spawn_failure(
                &json!({"code":code,"syscall":"spawn runner","path":"runner"}),
                Some("runner"),
                d.path().to_str().unwrap()
            )
            .await
        );
    }
}
#[tokio::test]
async fn keeps_unproven_executable_codes_ordinary_despite_synthetic_argv_fields() {
    let d = tempfile::tempdir().unwrap();
    for code in ["ENOEXEC", "ENOTDIR", "EPERM"] {
        assert!(
            !is_runner_spawn_failure(
                &json!({"code":code,"syscall":"spawn runner","path":"runner"}),
                Some("runner"),
                d.path().to_str().unwrap()
            )
            .await
        );
    }
}
#[tokio::test]
async fn requires_a_usable_caller_cwd_before_classifying_any_runner_form() {
    let d = tempfile::tempdir().unwrap();
    for runner in ["/absolute/runner", "node", "./runner"] {
        assert!(
            !is_runner_spawn_failure(
                &json!({"code":"ENOENT","syscall":format!("spawn {runner}"),"path":runner}),
                Some(runner),
                d.path().join("missing").to_str().unwrap()
            )
            .await
        );
    }
    tokio::fs::write(d.path().join("file"), "").await.unwrap();
    assert!(
        !is_runner_spawn_failure(
            &json!({"code":"ENOTDIR","syscall":"spawn node","path":"node"}),
            Some("node"),
            d.path().join("file").to_str().unwrap()
        )
        .await
    );
}
#[tokio::test]
async fn rejects_resource_non_spawn_mismatched_program_and_unstructured_failures() {
    let d = tempfile::tempdir().unwrap();
    let cwd = d.path().to_str().unwrap();
    for error in [
        json!({"code":"EMFILE","syscall":"spawn runner","path":"runner"}),
        json!({"code":"ENOMEM","syscall":"spawn runner","path":"runner"}),
        json!({"code":2,"syscall":"spawn runner","path":"runner"}),
        json!({"code":"ENOENT","syscall":"open","path":"runner"}),
        json!({"code":"ENOENT","syscall":1,"path":"runner"}),
        json!({"code":"ENOENT","syscall":"spawn","path":"other"}),
        json!({"code":"ENOENT","syscall":"spawn","path":1}),
        json!({"code":"ENOENT","syscall":"spawn","path":""}),
        json!({"code":"ENOENT","syscall":"spawn"}),
        json!({"code":"ENOENT","syscall":"spawn other"}),
        Value::Null,
    ] {
        assert!(!is_runner_spawn_failure(&error, Some("runner"), cwd).await);
    }
    assert!(
        !is_runner_spawn_failure(
            &json!({"code":"ENOENT","syscall":"spawn runner"}),
            None,
            cwd
        )
        .await
    );
}
#[tokio::test]
async fn accepts_only_syscall_and_error_path_facts_identifying_the_exact_program() {
    let d = tempfile::tempdir().unwrap();
    let cwd = d.path().to_str().unwrap();
    for error in [
        json!({"code":"ENOENT","syscall":"spawn","path":"runner with spaces"}),
        json!({"code":"ENOENT","syscall":"spawn runner with spaces","path":"runner with spaces"}),
        json!({"code":"ENOENT","syscall":"spawn runner with spaces"}),
    ] {
        assert!(is_runner_spawn_failure(&error, Some("runner with spaces"), cwd).await);
    }
    assert!(
        !is_runner_spawn_failure(
            &json!({"code":"ENOENT","syscall":"spawn other","path":"runner with spaces"}),
            Some("runner with spaces"),
            cwd
        )
        .await
    );
}
#[test]
fn ignores_empty_and_whitespace_only_fatal_signatures() {
    let r = RunnerFailureRule {
        allowed_exit_codes: Some(vec![125]),
        fatal_signatures: vec!["".into(), " ".into(), "\t".into(), "\u{feff}".into()],
        informational_lines: None,
    };
    for text in ["", "landlock-run: partial enforcement (older Landlock ABI)"] {
        assert!(classify_runner_failure(Some(125), text, std::slice::from_ref(&r)).is_none());
    }
}
#[test]
fn keeps_valid_fatal_signatures_active_beside_an_ignored_empty_entry() {
    let mut r = rule();
    r.fatal_signatures.insert(0, "".into());
    r.fatal_signatures.insert(1, " ".into());
    assert_eq!(classify_runner_failure(Some(125),"landlock-run: partial enforcement (older Landlock ABI)\nchild diagnostic\nlandlock-run: ruleset creation failed",&[r]).unwrap().detail,"landlock-run: ruleset creation failed");
}
#[test]
fn requires_landlock_exit_125_plus_a_non_notice_fatal_line_and_returns_the_original_line() {
    let notice = "landlock-run: partial enforcement (older Landlock ABI)";
    for code in [1, 2, 125] {
        assert!(classify_runner_failure(Some(code), notice, &[rule()]).is_none());
    }
    assert!(classify_runner_failure(Some(125), &notice.to_uppercase(), &[rule()]).is_none());
    for text in [
        format!("{notice}: extra detail"),
        "landlock-run: exec failed: No such file or directory".into(),
    ] {
        assert_eq!(
            classify_runner_failure(Some(125), &text, &[rule()])
                .unwrap()
                .detail,
            text
        );
    }
}
#[test]
fn keeps_known_and_future_landlock_fatal_diagnostics_fail_closed() {
    for fatal in [
        "usage error: missing `-- <argv>...` command",
        "landlock is not enforced by this kernel (ABI unsupported or disabled)",
        "cannot open rule path: /gone: No such file or directory",
        "landlock ruleset error: Invalid argument",
        "exec failed: Permission denied",
        "out of memory",
        "future fatal diagnostic",
    ] {
        let fatal = format!("landlock-run: {fatal}");
        assert_eq!(
            classify_runner_failure(Some(125), &fatal, &[rule()])
                .unwrap()
                .detail,
            fatal
        );
    }
}
#[test]
fn ignores_clean_or_signal_exits_and_accepts_an_ungated_fatal_rule() {
    let r = RunnerFailureRule {
        allowed_exit_codes: None,
        fatal_signatures: vec!["runner failed".into()],
        informational_lines: None,
    };
    for code in [Some(0), None] {
        assert!(classify_runner_failure(code, "runner failed", std::slice::from_ref(&r)).is_none());
    }
    assert!(classify_runner_failure(Some(1), "ordinary text", std::slice::from_ref(&r)).is_none());
    assert_eq!(
        classify_runner_failure(Some(1), "RUNNER FAILED: detail", &[r])
            .unwrap()
            .detail,
        "RUNNER FAILED: detail"
    );
}
#[test]
fn matches_selected_denial_signatures_only_on_failed_exits() {
    for (code, text, result) in [
        (None, "Permission denied", false),
        (Some(0), "Permission denied", false),
        (Some(1), "PERMISSION DENIED", true),
        (Some(1), "ordinary output", false),
    ] {
        assert_eq!(
            matches_signature(code, text, &["permission denied".into()]),
            result
        );
    }
}
