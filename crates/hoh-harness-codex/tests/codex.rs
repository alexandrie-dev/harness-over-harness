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
    let config = CodexConfig {
        executable: stub(),
        request_timeout: Duration::from_secs(3),
        extra_env: vec![
            ("CODEX_STUB_SCENARIO".into(), scenario.into()),
            ("CODEX_STUB_LOG".into(), log.to_string_lossy().into_owned()),
        ],
        ..CodexConfig::default()
    };
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

async fn protocol_end_without_a_request_card(
    events: &mut tokio::sync::mpsc::Receiver<HarnessEvent>,
    scenario: &str,
) {
    loop {
        match event(events).await {
            HarnessEvent::ApprovalRequested { .. } | HarnessEvent::QuestionAsked { .. } => {
                panic!("{scenario}: a stale or foreign request was shown to the user")
            }
            HarnessEvent::SessionEnded {
                reason, message, ..
            } => {
                assert_eq!(reason, hoh_external_harness::EndReason::ProtocolError);
                assert!(message.as_deref().is_some_and(|text| !text.is_empty()));
                return;
            }
            _ => {}
        }
    }
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
    let mut reasoning = String::new();
    let mut reasoning_final = None;
    let mut tool_started = false;
    let mut tool_output = None;
    let mut tool_completed = false;
    loop {
        match event(&mut started.events).await {
            HarnessEvent::TextDelta { text, .. } => deltas.push_str(&text),
            HarnessEvent::ReasoningDelta { text, .. } => reasoning.push_str(&text),
            HarnessEvent::ReasoningFinal { text, .. } => reasoning_final = Some(text),
            HarnessEvent::ItemStarted { item_id, action }
                if item_id == "tool-1"
                    && matches!(action, hoh_external_harness::HarnessAction::Tool { .. }) =>
            {
                tool_started = true;
            }
            HarnessEvent::ItemOutput { item_id, text } if item_id == "tool-1" => {
                tool_output = Some(text);
            }
            HarnessEvent::ItemCompleted { item_id, .. } if item_id == "tool-1" => {
                tool_completed = true;
            }
            HarnessEvent::TurnCompleted { .. } => break,
            _ => {}
        }
    }
    assert_eq!(deltas, "part done");
    assert_eq!(reasoning, "checking the result");
    assert_eq!(reasoning_final.as_deref(), Some("checking the result"));
    assert!(tool_started);
    assert_eq!(tool_output.as_deref(), Some("inspected src/lib.rs"));
    assert!(tool_completed);
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
        .start_with_access(start_params(&temp, None), AccessMode::AutoAcceptEdits)
        .await
        .unwrap();
    let native = String::from(first.result.native_session_id.clone());
    first.session.terminate(Duration::from_millis(100)).await;
    let mut second = harness
        .start_with_access(
            start_params(&temp, Some(native.clone())),
            AccessMode::AutoAcceptEdits,
        )
        .await
        .unwrap();
    assert!(second.result.resumed);
    assert_eq!(
        String::from(second.result.native_session_id.clone()),
        native
    );
    second
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-after-restart".into(),
            text: "continue".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let request_id = loop {
        if let HarnessEvent::ApprovalRequested { request_id, .. } = event(&mut second.events).await
        {
            break request_id;
        }
    };
    second
        .session
        .respond(RespondParams {
            session_id: "hoh-session".into(),
            request_id,
            response: HarnessResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        })
        .await
        .unwrap();
    loop {
        if matches!(
            event(&mut second.events).await,
            HarnessEvent::TurnCompleted { .. }
        ) {
            break;
        }
    }
    second.session.terminate(Duration::from_millis(100)).await;
    assert!(lines(&log).iter().any(|line| line.get("method")
        == Some(&Value::String("thread/resume".into()))
        && line.pointer("/params/threadId").and_then(Value::as_str) == Some("thread-stub")));
    let wire = lines(&log);
    let resumed_turn = wire
        .iter()
        .rev()
        .find(|line| line.get("method") == Some(&Value::String("turn/start".into())))
        .unwrap();
    assert_eq!(
        resumed_turn
            .pointer("/params/approvalPolicy")
            .and_then(Value::as_str),
        Some("on-request")
    );
    assert_eq!(
        resumed_turn
            .pointer("/params/sandboxPolicy/type")
            .and_then(Value::as_str),
        Some("workspaceWrite")
    );
}

#[tokio::test]
async fn resume_rejects_a_different_returned_thread_without_rebinding() {
    let temp = TempDir::new().unwrap();
    let (harness, _) = harness(&temp, "resume-mismatch");
    let error = harness
        .start_with_access(
            start_params(&temp, Some("thread-stub".into())),
            AccessMode::Confirm,
        )
        .await
        .err()
        .expect("a mismatched resume id must fail");
    assert_eq!(error.code, hoh_external_harness::code::PROTOCOL);
    assert!(error.message.contains("different thread.id"), "{error:?}");
}

#[tokio::test]
async fn duplicate_native_request_ids_close_fail_closed_without_answering_another_action() {
    for scenario in ["duplicate-string", "duplicate-number"] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, scenario);
        let mut started = harness
            .start_with_access(start_params(&temp, None), AccessMode::Confirm)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-hoh".into(),
                text: "duplicate".into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        let first = loop {
            match event(&mut started.events).await {
                HarnessEvent::ApprovalRequested {
                    request_id, action, ..
                } => {
                    assert!(matches!(
                        action,
                        hoh_external_harness::HarnessAction::Command { .. }
                    ));
                    break request_id;
                }
                HarnessEvent::SessionEnded {
                    reason, message, ..
                } => {
                    panic!(
                        "duplicate closed before first request was observable: {reason:?} {message:?}"
                    )
                }
                _ => {}
            }
        };
        let response = started
            .session
            .respond(RespondParams {
                session_id: "hoh-session".into(),
                request_id: first,
                response: HarnessResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            })
            .await;
        assert!(
            response.is_ok(),
            "{scenario}: the first action's answer should win its race"
        );
        let ended = loop {
            if let HarnessEvent::SessionEnded {
                reason, message, ..
            } = event(&mut started.events).await
            {
                break (reason, message);
            }
        };
        assert_eq!(ended.0, hoh_external_harness::EndReason::ProtocolError);
        assert!(
            ended
                .1
                .as_deref()
                .is_some_and(|text| text.contains("reused JSON-RPC"))
        );
        let duplicate = if scenario == "duplicate-string" {
            Value::String("duplicate-approval".into())
        } else {
            Value::from(404)
        };
        let wire = lines(&log);
        assert_eq!(
            wire.iter()
                .filter(|line| line.get("id") == Some(&duplicate) && line.get("result").is_some())
                .count(),
            1,
            "{scenario}: a second action received the first action's approval"
        );
        assert!(
            wire.iter()
                .any(|line| line["stubEvent"] == "second-duplicate-action")
        );
    }
}

#[tokio::test]
async fn foreign_thread_requests_and_resolutions_fail_before_showing_a_card() {
    for (scenario, forbidden_id) in [
        ("foreign-thread-request", Some("foreign-approval")),
        ("foreign-resolved", Some("ghost")),
        ("unknown-resolved", Some("ghost")),
        ("untyped-resolved", None),
    ] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, scenario);
        let mut started = harness
            .start_with_access(start_params(&temp, None), AccessMode::Confirm)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-hoh".into(),
                text: scenario.into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        protocol_end_without_a_request_card(&mut started.events, scenario).await;
        if let Some(forbidden_id) = forbidden_id {
            assert!(
                !lines(&log).iter().any(|line| {
                    line.get("id") == Some(&Value::String(forbidden_id.into()))
                        && line.get("result").is_some()
                }),
                "{scenario}: a result escaped for an invalid request"
            );
        }
        if scenario == "foreign-thread-request" {
            let child = lines(&log)
                .iter()
                .find_map(|line| line.get("childPid").and_then(Value::as_u64))
                .expect("foreign-request fixture child");
            let start = Instant::now();
            while Path::new(&format!("/proc/{child}")).exists()
                && start.elapsed() < Duration::from_secs(3)
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(
                !Path::new(&format!("/proc/{child}")).exists(),
                "foreign request protocol failure left owned child {child} running"
            );
        }
    }
}

#[tokio::test]
async fn canonical_resolved_without_a_turn_id_removes_the_exact_pending_request() {
    let temp = TempDir::new().unwrap();
    let (harness, _) = harness(&temp, "canonical-resolved");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "resolve canonically".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let request_id = loop {
        if let HarnessEvent::ApprovalRequested { request_id, .. } = event(&mut started.events).await
        {
            break request_id;
        }
    };
    assert_eq!(request_id, "codex:\"canonical-resolved\"");
    loop {
        match event(&mut started.events).await {
            HarnessEvent::RequestResolved {
                request_id: resolved,
            } => {
                assert_eq!(resolved, request_id);
                break;
            }
            HarnessEvent::SessionEnded {
                reason, message, ..
            } => {
                panic!("canonical resolved failed: {reason:?} {message:?}")
            }
            _ => {}
        }
    }
    started.session.terminate(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn resolved_ids_are_unambiguous_for_the_lifetime_of_the_app_server_process() {
    for scenario in ["stale-resolved", "cross-turn-request-id"] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, scenario);
        let mut started = harness
            .start_with_access(start_params(&temp, None), AccessMode::Confirm)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "hoh-first".into(),
                text: "first".into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        loop {
            if matches!(
                event(&mut started.events).await,
                HarnessEvent::TurnCompleted { .. }
            ) {
                break;
            }
        }
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "hoh-second".into(),
                text: "second".into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        protocol_end_without_a_request_card(&mut started.events, scenario).await;
        assert!(!lines(&log).iter().any(|line| {
            line.get("id") == Some(&Value::String("resolved-old".into()))
                && line.get("result").is_some()
        }));
    }
}

#[tokio::test]
async fn mcp_elicitation_accepts_schema_exact_omitted_and_null_turn_ids() {
    for (scenario, native_id) in [
        ("mcp-omitted", Value::String("mcp-omitted".into())),
        ("mcp-null", Value::from(909)),
    ] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, scenario);
        let mut started = harness
            .start_with_access(start_params(&temp, None), AccessMode::Confirm)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-hoh".into(),
                text: scenario.into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        let request_id = loop {
            if let HarnessEvent::ApprovalRequested {
                request_id,
                action: hoh_external_harness::HarnessAction::Tool { name, .. },
                ..
            } = event(&mut started.events).await
            {
                assert_eq!(name, "mcp-elicitation");
                break request_id;
            }
        };
        started
            .session
            .respond(RespondParams {
                session_id: "hoh-session".into(),
                request_id,
                response: HarnessResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            })
            .await
            .unwrap();
        loop {
            if matches!(
                event(&mut started.events).await,
                HarnessEvent::TurnCompleted { .. }
            ) {
                break;
            }
        }
        assert!(lines(&log).iter().any(|line| {
            line.get("id") == Some(&native_id)
                && line.pointer("/result/action").and_then(Value::as_str) == Some("accept")
        }));
    }
}

#[tokio::test]
async fn standalone_mcp_elicitation_without_an_active_turn_is_cancelled_on_its_exact_id() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "mcp-no-active");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "finish before standalone elicitation".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    loop {
        match event(&mut started.events).await {
            HarnessEvent::ApprovalRequested { request_id, .. }
                if request_id == "codex:\"mcp-no-active\"" =>
            {
                panic!("standalone elicitation without a turn was shown as a turn approval")
            }
            HarnessEvent::TurnCompleted { .. } => break,
            _ => {}
        }
    }
    let start = Instant::now();
    while !lines(&log).iter().any(|line| {
        line.get("id") == Some(&Value::String("mcp-no-active".into()))
            && line.pointer("/result/action").and_then(Value::as_str) == Some("cancel")
    }) && start.elapsed() < Duration::from_secs(3)
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(lines(&log).iter().any(|line| {
        line.get("id") == Some(&Value::String("mcp-no-active".into()))
            && line.pointer("/result/action").and_then(Value::as_str) == Some("cancel")
    }));
}

#[tokio::test]
async fn standalone_mcp_elicitation_rejects_foreign_thread_and_stale_supplied_turn() {
    for scenario in ["mcp-foreign", "mcp-stale-turn"] {
        let temp = TempDir::new().unwrap();
        let (harness, log) = harness(&temp, scenario);
        let mut started = harness
            .start_with_access(start_params(&temp, None), AccessMode::Confirm)
            .await
            .unwrap();
        started
            .session
            .prompt(PromptParams {
                session_id: "hoh-session".into(),
                turn_id: "turn-hoh".into(),
                text: scenario.into(),
                images: Vec::new(),
            })
            .await
            .unwrap();
        protocol_end_without_a_request_card(&mut started.events, scenario).await;
        assert!(!lines(&log).iter().any(|line| line.get("result").is_some()));
    }
}

#[tokio::test]
async fn a_request_from_the_prior_turn_cannot_rebind_a_reused_item_id() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "stale-prior-turn");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "hoh-first".into(),
            text: "first".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    loop {
        if matches!(
            event(&mut started.events).await,
            HarnessEvent::TurnCompleted { .. }
        ) {
            break;
        }
    }
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "hoh-second".into(),
            text: "second".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    protocol_end_without_a_request_card(&mut started.events, "stale-prior-turn").await;
    assert!(!lines(&log).iter().any(|line| {
        line.get("id") == Some(&Value::String("stale-approval".into()))
            && line.get("result").is_some()
    }));
}

#[tokio::test]
async fn reusing_an_item_id_for_a_different_action_fails_before_approval() {
    let temp = TempDir::new().unwrap();
    let (harness, log) = harness(&temp, "reused-item");
    let mut started = harness
        .start_with_access(start_params(&temp, None), AccessMode::Confirm)
        .await
        .unwrap();
    started
        .session
        .prompt(PromptParams {
            session_id: "hoh-session".into(),
            turn_id: "turn-hoh".into(),
            text: "reuse item".into(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    protocol_end_without_a_request_card(&mut started.events, "reused-item").await;
    assert!(!lines(&log).iter().any(|line| line.get("result").is_some()));
}

#[tokio::test]
async fn parallel_sessions_keep_their_first_turn_access_isolated() {
    let temp = TempDir::new().unwrap();
    let (harness_a, log_a) = harness(&temp, "parallel-auto-accept");
    let (harness_b, log_b) = harness(&temp, "parallel-auto");
    let mut a = harness_a
        .start_with_access(start_params(&temp, None), AccessMode::AutoAcceptEdits)
        .await
        .unwrap();
    let mut b = harness_b
        .start_with_access(start_params(&temp, None), AccessMode::Auto)
        .await
        .unwrap();

    let prompt_a = a.session.prompt(PromptParams {
        session_id: "chat-a".into(),
        turn_id: "turn-a".into(),
        text: "a".into(),
        images: Vec::new(),
    });
    let prompt_b = b.session.prompt(PromptParams {
        session_id: "chat-b".into(),
        turn_id: "turn-b".into(),
        text: "b".into(),
        images: Vec::new(),
    });
    let (result_a, result_b) = tokio::join!(prompt_a, prompt_b);
    result_a.unwrap();
    result_b.unwrap();

    for (started, session_id) in [(&mut a, "chat-a"), (&mut b, "chat-b")] {
        let request_id = loop {
            if let HarnessEvent::ApprovalRequested { request_id, .. } =
                event(&mut started.events).await
            {
                break request_id;
            }
        };
        started
            .session
            .respond(RespondParams {
                session_id: session_id.into(),
                request_id,
                response: HarnessResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            })
            .await
            .unwrap();
        loop {
            if matches!(
                event(&mut started.events).await,
                HarnessEvent::TurnCompleted { .. }
            ) {
                break;
            }
        }
        started.session.terminate(Duration::from_millis(100)).await;
    }

    let a_turn = lines(&log_a)
        .into_iter()
        .find(|line| line.get("method") == Some(&Value::String("turn/start".into())))
        .unwrap();
    let b_turn = lines(&log_b)
        .into_iter()
        .find(|line| line.get("method") == Some(&Value::String("turn/start".into())))
        .unwrap();
    assert_eq!(
        a_turn
            .pointer("/params/approvalsReviewer")
            .and_then(Value::as_str),
        Some("user")
    );
    assert_eq!(
        b_turn
            .pointer("/params/approvalsReviewer")
            .and_then(Value::as_str),
        Some("auto_review")
    );
    assert_eq!(
        a_turn
            .pointer("/params/sandboxPolicy/type")
            .and_then(Value::as_str),
        Some("workspaceWrite")
    );
    assert_eq!(
        b_turn
            .pointer("/params/sandboxPolicy/type")
            .and_then(Value::as_str),
        Some("workspaceWrite")
    );
}

#[tokio::test]
async fn terminate_removes_the_process_tree_without_a_native_interrupt() {
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
        let config = CodexConfig {
            executable: stub(),
            extra_env: vec![("CODEX_STUB_VERSION".into(), banner.into())],
            ..CodexConfig::default()
        };
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
