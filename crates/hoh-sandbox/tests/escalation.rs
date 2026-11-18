use hoh_error::{HarnessError, SharedError};
use hoh_sandbox::*;
use std::sync::{Arc, Mutex};
struct Approver {
    outcome: EscalationOutcome,
    seen: Arc<Mutex<Vec<EscalationAsk>>>,
}
impl EscalationApprover for Approver {
    fn request(&self, r: EscalationAsk) -> SandboxFuture<'_, EscalationOutcome> {
        self.seen.lock().unwrap().push(r);
        Box::pin(async { Ok(self.outcome) })
    }
}
fn req() -> EscalationRequest {
    EscalationRequest {
        requested_mode: "workspace-write".into(),
        justification: "the user asked to write in the workspace".into(),
        effective_mode: SandboxMode::ReadOnly,
        subject: "command".into(),
    }
}
fn ingredients(outcome: EscalationOutcome) -> (EscalationApproval, Arc<Mutex<Vec<EscalationAsk>>>) {
    let seen = Arc::new(Mutex::new(vec![]));
    (
        EscalationApproval {
            approver: Some(Arc::new(Approver {
                outcome,
                seen: seen.clone(),
            })),
            agent: Some(serde_json::json!({})),
            call_id: "call-1".into(),
            tool_name: "bash".into(),
            signal: None,
        },
        seen,
    )
}
fn error(e: &(dyn std::error::Error + 'static), code: &str) {
    let e = HarnessError::of(e).unwrap();
    assert_eq!(e.name(), "SandboxEscalationError");
    assert_eq!(e.code(), code);
}
#[test]
fn read_only_escalates_to_either_wider_mode_workspace_write_only_to_full_access() {
    assert_eq!(WIDER_MODES["read-only"], ESCALATION_TARGETS);
    assert_eq!(
        WIDER_MODES["workspace-write"],
        [SandboxMode::DangerFullAccess]
    );
    assert!(!WIDER_MODES.contains_key("danger-full-access"));
}
#[test]
fn the_target_enum_is_the_closed_set_every_session_could_escalate_to() {
    assert_eq!(
        ESCALATION_TARGETS,
        [SandboxMode::WorkspaceWrite, SandboxMode::DangerFullAccess]
    );
}
#[test]
fn accepts_neither_field_or_both_with_a_non_empty_justification() {
    validate_escalation_args(None, None).unwrap();
    validate_escalation_args(
        Some("workspace-write"),
        Some("because the workspace needs it"),
    )
    .unwrap();
}
#[test]
fn rejects_one_field_without_the_other_and_a_blank_justification() {
    for (a, b, text) in [
        (Some("workspace-write"), None, "requires a justification"),
        (
            None,
            Some("orphan reason"),
            "only valid together with sandbox_permissions",
        ),
        (Some("workspace-write"), Some("   "), "non-empty sentence"),
        (
            Some("workspace-write"),
            Some("\u{feff}"),
            "non-empty sentence",
        ),
    ] {
        let e = validate_escalation_args(a, b).unwrap_err();
        assert_eq!(e.name(), "SandboxEscalationError");
        assert_eq!(e.code(), "SANDBOX_ESCALATION_INVALID_ARGUMENT");
        assert!(e.message().contains(text));
    }
    assert!(validate_escalation_args(Some("x"), Some("\u{85}")).is_ok());
}
#[test]
fn the_denial_marker_names_the_mode() {
    for mode in [SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite] {
        assert_eq!(
            sandbox_denial_marker(mode),
            format!("[sandbox: file access denied under {mode} mode]")
        );
    }
}
#[test]
fn the_sandbox_permissions_description_names_the_family_subject() {
    for subject in ["command", "operation"] {
        assert_eq!(
            sandbox_permissions_description(subject),
            format!(
                "The narrowest wider sandbox mode for a one-shot retry of the exact {subject} the sandbox just denied; the retry asks the user for approval."
            )
        );
    }
}
#[test]
fn the_hint_marker_names_the_family_subject() {
    for subject in ["command", "operation"] {
        assert!(escalation_hint_marker(subject).contains(&format!(
            "retry this exact {subject} once with sandbox_permissions"
        )));
    }
}
#[tokio::test]
async fn grants_returns_the_requested_mode_asking_with_the_audit_reason() {
    let (a, seen) = ingredients(EscalationOutcome::AllowedOnce);
    assert_eq!(
        approve_escalation(req(), a).await.unwrap(),
        SandboxMode::WorkspaceWrite
    );
    let seen = seen.lock().unwrap();
    let r = &seen[0];
    assert_eq!(
        r.reason,
        "escalate sandbox to workspace-write: the user asked to write in the workspace"
    );
    assert_eq!(
        r.display_reason["en"],
        "Allow this operation with workspace-write permissions: the user asked to write in the workspace"
    );
    assert_eq!(
        r.display_reason["zh"],
        "允许本次操作使用 workspace-write 权限：the user asked to write in the workspace"
    );
    assert_eq!(
        r.display_reason["ja"],
        "workspace-write 権限でこの操作を許可します：the user asked to write in the workspace"
    );
    assert_eq!(r.call_id, "call-1");
    assert_eq!(r.tool_name, "bash");
}
#[tokio::test]
async fn repeating_the_current_mode_succeeds_without_asking_for_approval() {
    for mode in ESCALATION_TARGETS {
        let (a, seen) = ingredients(EscalationOutcome::Rejected);
        let mut r = req();
        r.requested_mode = mode.to_string();
        r.effective_mode = mode;
        assert_eq!(approve_escalation(r.clone(), a).await.unwrap(), mode);
        assert!(seen.lock().unwrap().is_empty());
        let (mut a, _) = ingredients(EscalationOutcome::Rejected);
        a.agent = None;
        a.approver = None;
        assert_eq!(approve_escalation(r, a).await.unwrap(), mode);
    }
}
#[tokio::test]
async fn a_narrower_or_unsupported_target_fails_closed_without_asking() {
    for (mode, current) in [
        ("read-only", SandboxMode::WorkspaceWrite),
        ("workspace-write", SandboxMode::DangerFullAccess),
        ("unknown-mode", SandboxMode::ReadOnly),
    ] {
        let (a, seen) = ingredients(EscalationOutcome::AllowedOnce);
        let mut r = req();
        r.requested_mode = mode.into();
        r.effective_mode = current;
        let e = approve_escalation(r, a).await.unwrap_err();
        error(e.as_ref(), "SANDBOX_ESCALATION_INVALID_ARGUMENT");
        assert!(e.to_string().contains("not strictly wider"));
        assert!(seen.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn a_missing_approval_service_and_agent_less_call_fail_with_distinct_text() {
    for no_service in [false, true] {
        let (mut a, _) = ingredients(EscalationOutcome::AllowedOnce);
        if no_service {
            a.approver = None;
        } else {
            a.agent = None;
        }
        let e = approve_escalation(req(), a).await.unwrap_err();
        error(e.as_ref(), "SANDBOX_ESCALATION_UNAVAILABLE");
        assert!(e.to_string().contains(if no_service {
            "no approval service is composed"
        } else {
            "no agent to route it through"
        }));
    }
}
#[tokio::test]
async fn maps_each_non_grant_outcome_to_its_distinct_verbatim_text() {
    for (outcome, text) in [
        (
            EscalationOutcome::Rejected,
            "the user rejected escalating this operation to \"workspace-write\"; it stays denied, so stop and explain instead of working around it",
        ),
        (
            EscalationOutcome::Cancelled,
            "approval for escalating to \"workspace-write\" was cancelled",
        ),
        (
            EscalationOutcome::Unavailable,
            "sandbox escalation to \"workspace-write\" requires approval, but no approval channel is available",
        ),
    ] {
        let (a, _) = ingredients(outcome);
        let mut r = req();
        r.subject = "operation".into();
        assert_eq!(
            approve_escalation(r, a).await.unwrap_err().to_string(),
            text
        );
    }
}
#[tokio::test]
async fn codes_each_non_grant_outcome() {
    for (outcome, code) in [
        (EscalationOutcome::Rejected, "SANDBOX_ESCALATION_REJECTED"),
        (EscalationOutcome::Cancelled, "SANDBOX_ESCALATION_CANCELLED"),
        (
            EscalationOutcome::Unavailable,
            "SANDBOX_ESCALATION_UNAVAILABLE",
        ),
    ] {
        let (a, _) = ingredients(outcome);
        error(
            approve_escalation(req(), a).await.unwrap_err().as_ref(),
            code,
        );
    }
}
#[tokio::test]
async fn a_cancelled_approval_for_an_aborted_call_rethrows_the_calls_cancellation() {
    let c = hoh_timeout::AbortController::new();
    let reason = SharedError::new(std::io::Error::other("caller stopped"));
    c.abort_with(reason.clone());
    let (mut a, _) = ingredients(EscalationOutcome::Cancelled);
    a.signal = Some(c.signal());
    let e = approve_escalation(req(), a).await.unwrap_err();
    assert!(e.downcast_ref::<SharedError>().unwrap().ptr_eq(&reason));
}
#[test]
fn an_outcome_outside_the_closed_union_trips_the_exhaustiveness_guard() {
    assert!(serde_json::from_value::<EscalationOutcome>(serde_json::json!("bogus")).is_err());
}
