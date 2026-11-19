use super::*;
use hoh_brand::ids::SessionId;
use serde_json::json;

fn pending_service() -> PermissionPresetService {
    PermissionPresetService::build(
        default_presets(),
        Arc::new(|| None),
        Arc::new(|| Some(SandboxMode::WorkspaceWrite)),
        Arc::new(|| ApprovalPolicy::Ask),
        StateSource::Pending,
    )
    .unwrap()
}

fn fresh(id: &str) -> Session {
    Session::create(SessionId::from(id), None, None, None, None).unwrap()
}

#[test]
fn keeps_session_creation_read_only_while_permission_registration_is_pending() {
    let service = pending_service();
    let session = fresh("pending-session");
    assert!(
        service
            .current(&session)
            .unwrap_err()
            .to_string()
            .contains("not ready")
    );
    assert!(session.snapshot_events(None, None).is_empty());

    *lock(&service.inner.state_source) = StateSource::Direct;
    assert_eq!(service.current(&session).unwrap(), "workspace-write");
    assert!(session.snapshot_events(None, None).is_empty());
}

#[test]
fn rolls_back_fresh_and_auto_agent_creation_while_registration_is_pending() {
    let service = pending_service();
    let fresh_session = fresh("pending-fresh");
    let before = fresh_session.snapshot_events(None, None);
    assert!(
        service
            .pin_initial_permission(&fresh_session)
            .unwrap_err()
            .to_string()
            .contains("not ready")
    );
    assert_eq!(
        fresh_session.snapshot_events(None, None).as_ref(),
        before.as_ref()
    );

    let auto = fresh("pending-auto-source");
    auto.append("permission/preset", json!({"preset":"auto"}), None)
        .unwrap();
    auto.append("sandbox/mode", json!({"mode":"danger-full-access"}), None)
        .unwrap();
    auto.append("approval/policy", json!({"policy":"never"}), None)
        .unwrap();
    let before = auto.snapshot_events(None, None);
    assert!(
        service
            .pin_initial_permission(&auto)
            .unwrap_err()
            .to_string()
            .contains("not ready")
    );
    assert_eq!(auto.snapshot_events(None, None).as_ref(), before.as_ref());

    *lock(&service.inner.state_source) = StateSource::Direct;
    service.pin_initial_permission(&fresh_session).unwrap();
    assert_eq!(service.current(&fresh_session).unwrap(), "workspace-write");
    let error = service.pin_initial_permission(&auto).unwrap_err();
    assert!(error.to_string().contains("cannot restore preset \"auto\""));
}
