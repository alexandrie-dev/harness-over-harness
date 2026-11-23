use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hoh_external_harness::{
    AccessMode, ApprovalDecision, AuthState, ExternalHarness, HarnessEvent, HarnessPolicy,
    HarnessResponse, PromptParams, RespondParams, StartParams, VersionStatus,
};
use hoh_harness_codex::{CodexConfig, CodexHarness};
use serde_json::Value;
use tempfile::TempDir;

fn stub() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_codex-stub"))
}

fn harness(temp: &TempDir, scenario: &str) -> (CodexHarness, PathBuf) {
    let log = temp.path().join(format!("{scenario}.jsonl"));
    let mut config = CodexConfig::default();
    config.executable = stub();
    config.request_timeout = Duration::from_secs(3);
    config.extra_env = vec![
        ("CODEX_STUB_SCENARIO".into(), scenario.into()),
        ("CODEX_STUB_LOG".into(), log.to_string_lossy().into_owned()),
    ];
    (CodexHarness::new(config), log)
}

fn start_params(temp: &TempDir, resume: Option<String>) -> StartParams {
    StartParams {
        session_id: "hoh-session".into(),
        cwd: temp.path().to_string_lossy().into_owned(),
        model: Some("gpt-stub".into()),
        effort: Some("high".into()),
        policy: HarnessPolicy::default(),
        resume: resume.map(Into::into),
    }
}

fn lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn event(events: &mut tokio::sync::mpsc::Receiver<HarnessEvent>) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn discovery_uses_app_server_models_and_account() {
    let temp = TempDir::new().unwrap();
    let (harness, _) = harness(&temp, "discover");
    let (found, status) = harness.discover_with_status().await.unwrap();
    assert_eq!(status, VersionStatus::Supported);
    assert_eq!(found.auth, AuthState::Authenticated);
    assert_eq!(found.models.len(), 1);
    assert_eq!(found.models[0].id, "gpt-stub");
    assert_eq!(found.models[0].default_effort.as_deref(), Some("medium"));
    assert_eq!(found.models[0].effort_levels.len(), 3);
}

#[tokio::test]
async fn every_mode_is_present_on_thread_and_first_turn() {
    let expected = [
        (
            AccessMode::Confirm,
            "untrusted",
            "user",
            "read-only",
            "readOnly",
        ),
        (
            AccessMode::AutoAcceptEdits,
            "on-request",
            "user",
            "workspace-write",
            "workspaceWrite",
        ),
        (
            AccessMode::Auto,
            "on-request",
            "auto_review",
            "workspace-write",
            "workspaceWrite",
        ),
        (
            AccessMode::Full,
            "granular",
            "user",
            "danger-full-access",
            "dangerFullAccess",
        ),
    ];
    for (index, (mode, approval, reviewer, sandbox, turn_sandbox)) in
        expected.into_iter().enumerate()
    {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, &format!("mode-{index}"));
        let mut started = harness
            .start_with_access(start_params(&temp, None), mode)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-hoh".into(),
                text: "hello".into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        loop {
            if matches!(
                event(&mut started.events).await,
                HarnessEvent::ApprovalRequested { .. }
            ) {
                break;
            }
        }
        started.session.terminate(Duration::from_millis(100)).await;
        let wire = lines(&log);
        let thread = wire
            .iter()
            .find(|line| line.get("method") == Some(&Value::String("thread/start".into())))
            .unwrap();
        let turn = wire
            .iter()
            .find(|line| line.get("method") == Some(&Value::String("turn/start".into())))
            .unwrap();
        assert_eq!(
            thread
                .pointer("/params/approvalsReviewer")
                .and_then(Value::as_str),
            Some(reviewer)
        );
        assert_eq!(
            thread.pointer("/params/sandbox").and_then(Value::as_str),
            Some(sandbox)
        );
        assert_eq!(
            turn.pointer("/params/approvalsReviewer")
                .and_then(Value::as_str),
            Some(reviewer)
        );
        assert_eq!(
            turn.pointer("/params/sandboxPolicy/type")
                .and_then(Value::as_str),
            Some(turn_sandbox)
        );
        if approval == "granular" {
            assert_eq!(
                turn.pointer("/params/approvalPolicy/granular/sandbox_approval")
                    .and_then(Value::as_bool),
                Some(false)
            );
        } else {
            assert_eq!(
                turn.pointer("/params/approvalPolicy")
                    .and_then(Value::as_str),
                Some(approval)
            );
        }
    }
}

#[tokio::test]
async fn a_future_model_effort_is_forwarded_without_a_hard_coded_allowlist() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "future-effort");
    let mut params = start_params(&temp, None);
    params.effort = Some("super-high".into());
    let mut started = harness
        .start_with_access(params, AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "hello".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    loop {
        if matches!(
            event(&mut started.events).await,
            HarnessEvent::ApprovalRequested { .. }
        ) {
            break;
        }
    }
    started.session.terminate(Duration::from_millis(100)).await;
    let wire = lines(&log);
    let turn = wire
        .iter()
        .find(|line| line.get("method") == Some(&Value::String("turn/start".into())))
        .unwrap();
    assert_eq!(
        turn.pointer("/params/effort").and_then(Value::as_str),
        Some("super-high")
    );
}

#[tokio::test]
async fn stream_steps_and_exact_approval_ids_round_trip() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "approvals");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "hello".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let mut saw_step = false;
    let first = loop {
        match event(&mut started.events).await {
            HarnessEvent::ItemStarted { .. } => saw_step = true,
            HarnessEvent::ApprovalRequested { request_id, .. } => break request_id,
            _ => {}
        }
    };
    assert!(saw_step);
    started
        .session
        .respond(RespondParams {
            session_id: "hoh-session".into(),
            request_id: first,
            response: HarnessResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        })
        .await
        .unwrap();
    let second = loop {
        if let HarnessEvent::ApprovalRequested { request_id, .. } = event(&mut started.events).await
        {
            break request_id;
        }
    };
    started
        .session
        .respond(RespondParams {
            session_id: "hoh-session".into(),
            request_id: second,
            response: HarnessResponse::Approval {
                decision: ApprovalDecision::Deny,
            },
        })
        .await
        .unwrap();
    let mut deltas = String::new();
    loop {
        match event(&mut started.events).await {
            HarnessEvent::TextDelta { text, .. } => deltas.push_str(&text),
            HarnessEvent::TurnCompleted { .. } => break,
            _ => {}
        }
    }
    assert_eq!(deltas, "part done");
    started.session.terminate(Duration::from_millis(100)).await;
    let wire = lines(&log);
    assert!(wire.iter().any(|line| line.get("id")
        == Some(&Value::String("approval-command".into()))
        && line.pointer("/result/decision").and_then(Value::as_str) == Some("accept")));
    assert!(
        wire.iter()
            .any(|line| line.get("id").and_then(Value::as_u64) == Some(71)
                && line.pointer("/result/decision").and_then(Value::as_str) == Some("decline"))
    );
}

#[tokio::test]
async fn resume_uses_the_same_native_thread() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "resume");
    let first = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    let native = String::from(first.result.native_session_id.clone());
    first.session.terminate(Duration::from_millis(100)).await;
    let second = harness
        .start_with_access(
            start_params(&temp, Some(native.clone())),
            AccessMode::Confirm,
        )
        .await
        .unwrap();
    assert!(second.result.resumed);
    assert_eq!(
        String::from(second.result.native_session_id.clone()),
        native
    );
    second.session.terminate(Duration::from_millis(100)).await;
    assert!(lines(&log).iter().any(|line| line.get("method")
        == Some(&Value::String("thread/resume".into()))
        && line.pointer("/params/threadId").and_then(Value::as_str) == Some("thread-stub")));
}

#[tokio::test]
async fn interrupt_then_terminate_removes_the_process_tree() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "interrupt");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "sleep".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    while !lines(&log)
        .iter()
        .any(|line| line.get("childPid").is_some())
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let child = lines(&log)
        .iter()
        .find_map(|line| line.get("childPid").and_then(Value::as_u64))
        .unwrap();
    let start = Instant::now();
    started.session.interrupt().await.unwrap();
    started.session.terminate(Duration::from_millis(200)).await;
    while Path::new(&format!("/proc/{child}")).exists() && start.elapsed() < Duration::from_secs(3)
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !Path::new(&format!("/proc/{child}")).exists(),
        "child {child} survived"
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    let _ = event(&mut started.events).await;
}

#[tokio::test]
async fn unsupported_and_unrecognized_versions_never_start_a_session() {
    for banner in ["codex-cli 0.167.0", "not a version"] {
        let temp = TempDir::new().unwrap();
        let mut config = CodexConfig::default();
        config.executable = stub();
        config.extra_env = vec![("CODEX_STUB_VERSION".into(), banner.into())];
        let harness = CodexHarness::new(config);
        let (_, status) = harness.discover_with_status().await.unwrap();
        assert_ne!(status, VersionStatus::Supported);
        assert!(
            harness
                .start_with_access(start_params(&temp, None), AccessMode::Confirm)
                .await
                .is_err()
        );
    }
}
