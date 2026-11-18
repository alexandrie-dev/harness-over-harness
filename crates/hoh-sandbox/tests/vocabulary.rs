use hoh_sandbox::*;
#[test]
fn carries_the_structured_name_code_identity_consumers_key_on() {
    let e = SandboxUnavailableError::new(ConfinedSandboxMode::ReadOnly, None);
    assert_eq!(e.name(), "SandboxUnavailableError");
    assert_eq!(e.code(), SANDBOX_UNAVAILABLE);
}
#[test]
fn names_the_refused_mode_and_the_operator_escape_hatches_in_its_message() {
    let e = SandboxUnavailableError::new(ConfinedSandboxMode::WorkspaceWrite, None);
    assert!(e.message().contains("\"workspace-write\""));
    assert!(e.message().contains("danger-full-access"));
    assert!(!e.message().contains("Runner failure"));
}
#[test]
fn carries_the_runner_detail_when_the_failure_is_discovered_at_execution_time() {
    let e = SandboxUnavailableError::new(
        ConfinedSandboxMode::ReadOnly,
        Some("landlock-run: landlock is not enforced by this kernel"),
    );
    assert_eq!(e.code(), SANDBOX_UNAVAILABLE);
    assert!(
        e.message()
            .contains("Runner failure: landlock-run: landlock is not enforced by this kernel")
    );
}
#[test]
fn carries_a_literal_name_a_closed_code_and_the_platform_failure_as_cause() {
    let e = SandboxPreparationError::new(
        "cannot grant the workspace",
        SandboxPreparationErrorCode::PreparationFailed,
    )
    .with_cause(std::io::Error::other(
        "SetNamedSecurityInfoW failed (Win32 5)",
    ));
    assert_eq!(e.name(), "SandboxPreparationError");
    assert_eq!(e.code(), "SANDBOX_PREPARATION_FAILED");
    assert_eq!(
        e.cause().unwrap().to_string(),
        "SetNamedSecurityInfoW failed (Win32 5)"
    );
    assert_eq!(
        SANDBOX_PREPARATION_ERROR_CODES,
        ["SANDBOX_PREPARATION_FAILED", "SANDBOX_INVALID_TEMP_ROOT"]
    );
}
