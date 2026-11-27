use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use hoh_external_harness::{
    AccessMode, ExternalHarness, HarnessAction, HarnessEvent, HarnessPolicy, NetworkAccess,
    PromptImage, PromptParams, SandboxMode, StartParams, TitleRequest, VersionStatus, code,
};
use hoh_harness_pi::PiHarness;
use tempfile::TempDir;

const ID: &str = "30ef5cde-f3d8-4a0e-a933-1308bf671102";

fn params(cwd: &Path) -> StartParams {
    StartParams {
        session_id: "test".into(),
        cwd: cwd.to_str().unwrap().into(),
        cwd_identity: None,
        model: Some("mock/model".into()),
        effort: Some("high".into()),
        interaction_mode: Default::default(),
        policy: HarnessPolicy {
            sandbox: SandboxMode::FullAccess,
            network: NetworkAccess::Allowed,
            ..Default::default()
        },
        resume: None,
    }
}

fn stub(dir: &Path) -> PiHarness {
    let path = dir.join("pi");
    std::fs::write(&path, r#"#!/usr/bin/env python3
import json
import pathlib
import sys

def emit(record):
    print(json.dumps(record), flush=True)

args = sys.argv[1:]
if args == ["--version"]:
    print("1.6.1")
    sys.exit(0)
assert {"--no-approve", "--no-extensions", "--no-mcp", "--offline"} <= set(args)
session_id = "30ef5cde-f3d8-4a0e-a933-1308bf671102"
resume = "--session-id" in args
title_mode = "--system-prompt" in args
if title_mode:
    assert {"--no-session", "--no-skills", "--no-prompt-templates", "--no-context-files"} <= set(args)
    assert "--session-dir" not in args and "--session-id" not in args
    assert not list(pathlib.Path.cwd().iterdir())
if "--no-session" in args:
    assert "--no-tools" in args
    saved = None
else:
    saved = pathlib.Path(args[args.index("--session-dir") + 1]) / "history"
    if resume:
        assert args[args.index("--session-id") + 1] == session_id
model = {"provider": "mock", "id": "model", "name": "Test Model"}
title_model_selected = False
for line in sys.stdin:
    request = json.loads(line)
    method = request["type"]
    data = None
    if method == "get_state":
        data = {"sessionId": session_id, "messageCount": int(resume and saved.exists()), "model": model, "thinkingLevel": "high"}
    elif method == "get_available_models":
        data = {"models": [model]}
    elif method == "get_available_thinking_levels":
        data = {"levels": ["off", "high"]}
    elif method == "get_commands":
        assert saved is None and "--no-tools" in args
        assert pathlib.Path("workspace-marker").is_file()
        data = {"commands": [
            {"name": "summary", "source": "extension"},
            {"name": "summary", "source": "prompt", "description": "Summarize the release"},
            {"name": "skill:ship", "source": "skill", "description": "Prepare release"},
            {"name": "skill:ship", "source": "skill"},
            {"name": "run-extension", "source": "extension"},
        ]}
    elif method == "set_model":
        if title_mode:
            assert request["provider"] == "another"
            if request["modelId"] == "nested/model":
                data = {"provider": request["provider"], "id": request["modelId"]}
                title_model_selected = True
            else:
                data = model
        else:
            assert request["provider"] == "mock" and request["modelId"] == "model"
    elif method == "set_thinking_level":
        assert request["level"] == ("off" if title_mode else "high")
    elif method == "prompt":
        if title_mode:
            assert title_model_selected and request["images"] == []
        else:
            assert request["images"] == [{"type":"image", "mimeType":"image/png", "data":"aW1n"}]
        data = {"disposition": "handled" if request["message"] == "/handled" else "started"}
    emit({"id": request["id"], "type": "response", "command": method, "success": True, "data": data})
    if method == "prompt" and data["disposition"] == "started":
        if title_mode:
            answer = json.dumps({"title": "Доступ к кабинету CEO"}) if request["message"] == "title" else "Title: Кабинет CEO"
            message = {"role": "assistant", "content": [{"type": "text", "text": answer}]}
            emit({"type": "message_end", "message": message})
            emit({"type": "agent_settled", "aborted": False})
            continue
        saved.touch()
        message = {"role": "assistant", "content": [{"type": "text", "text": "Hello"}, {"type": "thinking", "thinking": "Plan"}]}
        for kind, delta in [("text_delta", "Hello"), ("thinking_delta", "Plan")]:
            emit({"type": "message_update", "assistantMessageEvent": {"type": kind, "delta": delta}})
        emit({"type": "message_end", "message": message})
        emit({"type": "tool_execution_start", "toolCallId": "tool-1", "toolName": "bash", "args": {"command": "pwd"}})
        for output in ["/pro", "/project"]:
            emit({"type": "tool_execution_update", "toolCallId": "tool-1", "partialResult": {"content": [{"type": "text", "text": output}]}})
        emit({"type": "tool_execution_end", "toolCallId": "tool-1", "result": {"content": [{"type": "text", "text": "/project"}]}, "isError": False})
        messages = [message]
        if request["message"] == "hello":
            # Pi can put images from both the user and a tool in this one record.
            for role, megabytes in [("user", 5), ("toolResult", 4)]:
                messages.append({"role": role, "content": [
                    {"type": "image", "mimeType": "image/png", "data": "A" * (megabytes * 1024 * 1024)}
                ]})
        emit({"type": "agent_end", "willRetry": True, "messages": messages})
        emit({"type": "message_end", "message": {"role": "assistant", "stopReason": "error", "errorMessage": "provider error"}})
        if request["message"] != "fail":
            emit({"type": "message_end", "message": message})
        emit({"type": "agent_settled", "aborted": False})
"#).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    PiHarness::new(path, dir.join("sessions"))
}

#[tokio::test]
async fn workspace_get_commands_uses_no_session_or_extensions() {
    let dir = TempDir::new().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    std::fs::write(cwd.join("workspace-marker"), "probe cwd").unwrap();
    let pi = stub(&cwd);

    let catalog = pi.commands(cwd.clone()).await.unwrap();
    assert_eq!(catalog.commands.len(), 1);
    assert_eq!(catalog.commands[0].name, "summary");
    assert_eq!(catalog.skills.len(), 1);
    assert_eq!(catalog.skills[0].name, "ship");
    assert_eq!(
        catalog.skills[0].description.as_deref(),
        Some("Prepare release")
    );
    assert!(!cwd.join("sessions").exists());
}

#[tokio::test]
async fn titles_use_an_isolated_session_and_the_bound_model() {
    let dir = TempDir::new().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(
        project.join("AGENTS.md"),
        "Do not load this in the title process.",
    )
    .unwrap();
    let pi = stub(dir.path());
    let request = |prompt: &str, model: Option<&str>| TitleRequest {
        prompt: prompt.into(),
        model: model.map(str::to_owned),
        cwd: Some(project.clone()),
        trusted: true,
        native_session_id: None,
    };

    assert_eq!(
        pi.generate_title(request("title", Some("another/nested/model")))
            .await
            .unwrap(),
        "Доступ к кабинету CEO"
    );
    assert_eq!(
        pi.generate_title(request("invalid", Some("another/nested/model")))
            .await
            .unwrap_err()
            .code,
        code::PROTOCOL,
    );
    assert!(pi.generate_title(request("title", None)).await.is_err());
    assert_eq!(
        pi.generate_title(request("title", Some("another/unavailable")))
            .await
            .unwrap_err()
            .code,
        code::PROTOCOL,
    );
    assert!(!dir.path().join("sessions").exists());
}

#[tokio::test]
async fn discovers_streams_and_resumes_only_an_existing_native_history() {
    let dir = TempDir::new().unwrap();
    let pi = stub(dir.path());
    let (found, version) = pi.discover_with_status().await.unwrap();
    assert_eq!(version, VersionStatus::Supported);
    assert!(found.installed);
    assert_eq!(found.models[0].id, "mock/model");
    assert_eq!(found.models[0].default_effort.as_deref(), Some("high"));

    let mut params = params(dir.path());
    let mut started = pi
        .start_with_access(params.clone(), AccessMode::Full)
        .await
        .unwrap();
    assert_eq!(started.result.native_session_id.as_str(), ID);
    let events_for = |text: &str| PromptParams {
        text: text.into(),
        turn_id: text.into(),
        images: vec![PromptImage {
            media_type: "image/png".into(),
            data: "aW1n".into(),
            name: None,
        }],
        ..Default::default()
    };
    for text in ["hello", "/handled", "fail"] {
        started.session.prompt(events_for(text)).await.unwrap();
        // Read until actual settlement, including any work after agent_end.
        let mut events = Vec::new();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), started.events.recv())
                .await
                .unwrap()
                .unwrap();
            let finished = matches!(
                event,
                HarnessEvent::TurnCompleted { .. } | HarnessEvent::TurnFailed { .. }
            );
            events.push(event);
            if finished {
                break;
            }
        }
        assert!(matches!(&events[0], HarnessEvent::TurnStarted { turn_id } if turn_id == text));
        if text == "hello" {
            let output: String = events
                .iter()
                .filter_map(|event| match event {
                    HarnessEvent::ItemOutput { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(output, "/project");
            assert!(events.iter().any(
                |event| matches!(event, HarnessEvent::TextDelta { text, .. } if text == "Hello")
            ));
            assert!(events.iter().any(
                |event| matches!(event, HarnessEvent::ReasoningDelta { text, .. } if text == "Plan")
            ));
            assert!(events.iter().any(|event| matches!(event, HarnessEvent::ItemStarted { action: HarnessAction::Command { command, .. }, .. } if command == "pwd")));
            assert!(events.iter().any(|event| matches!(event, HarnessEvent::ItemCompleted { output: Some(output), .. } if output == "/project")));
        }
        if text == "fail" {
            assert!(
                matches!(events.last(), Some(HarnessEvent::TurnFailed { error, .. }) if error.message == "provider error")
            );
        } else {
            assert!(
                matches!(events.last(), Some(HarnessEvent::TurnCompleted { turn_id }) if turn_id == text)
            );
        }
    }

    started.session.terminate(Duration::from_millis(200)).await;
    params.resume = Some(started.result.native_session_id);
    let resumed = pi
        .start_with_access(params.clone(), AccessMode::Full)
        .await
        .unwrap();
    assert!(resumed.result.resumed);
    resumed.session.terminate(Duration::from_millis(200)).await;
    std::fs::remove_file(dir.path().join("sessions/history")).unwrap();
    let error = pi
        .start_with_access(params, AccessMode::Full)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, code::SESSION_NOT_FOUND);
}

#[tokio::test]
async fn requires_full_access_before_starting_pi() {
    let dir = TempDir::new().unwrap();
    let pi = PiHarness::new(dir.path().join("missing"), dir.path().join("sessions"));
    let mut params = params(dir.path());
    for access in [
        AccessMode::Confirm,
        AccessMode::AutoAcceptEdits,
        AccessMode::Auto,
        AccessMode::AcceptEdits,
    ] {
        let error = pi
            .start_with_access(params.clone(), access)
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, code::POLICY_UNAVAILABLE);
    }
    params.policy.sandbox = SandboxMode::ReadOnly;
    let error = pi
        .start_with_access(params, AccessMode::Full)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, code::POLICY_UNAVAILABLE);
    assert!(!dir.path().join("sessions").exists());
}
