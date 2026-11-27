use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use hoh_external_harness::{
    AccessMode, AuthState, ExternalHarness, HarnessAction, HarnessEvent, HarnessPolicy,
    ItemOutcome, NetworkAccess, PromptImage, PromptParams, SandboxMode, StartParams, Started,
    VersionStatus, code,
};
use hoh_harness_kimi::KimiHarness;
use tempfile::TempDir;

const ID: &str = "kimi-test-session";

fn stub(dir: &Path, executable: &str) -> KimiHarness {
    let path = dir.join(executable);
    std::fs::write(&path, r#"#!/usr/bin/env python3
import json
from pathlib import Path
import sys

def send(**kwargs):
    print(json.dumps(dict(jsonrpc="2.0", **kwargs)), flush=True)

def update(event, **kwargs):
    send(method="session/update", params={"sessionId": session, "update": dict(sessionUpdate=event, **kwargs)})

assert sys.argv[1:] == ["acp"]
root = Path(sys.argv[0]).parent
(root / "started").touch()
session = "kimi-test-session"
catalog = {"configOptions": [
    {"id":"model","currentValue":"model-1","options":[{"value":"model-1","name":"Kimi Test"},{"value":"model-2","name":"Kimi Test 2"}]},
    {"id":"thinking","currentValue":"off","options":[{"value":"off"},{"value":"high"}]},
]}
mode = None
pending = None
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    params = request.get("params", {})
    result = {}
    if method == "initialize":
        assert params["protocolVersion"] == 1
        assert params["clientCapabilities"] == {}
        result = {"protocolVersion":1,"agentInfo":{"version":"2.5.3"}, "agentCapabilities":{
            "sessionCapabilities":{"resume":{}}, "promptCapabilities":{"image":True}
        }}
    elif method == "authenticate":
        assert params["methodId"] == "login"
        if Path(sys.argv[0]).name == "signed-out":
            send(id=request["id"], error={"code": -32000, "message": "Authentication required"})
            continue
    elif method == "session/new":
        (root / "created").touch()
        assert Path(params["cwd"]).is_absolute()
        assert params["mcpServers"] == []
        result = {"sessionId": session, **catalog}
    elif method == "session/resume":
        if params["sessionId"] != session or not (root / "history").exists():
            send(id=request["id"], error={"code":-32602, "message":"Unknown sessionId: " + params["sessionId"]})
            continue
        update("agent_message_chunk", content={"type":"text", "text":"history replay"})
        result = catalog
    elif method == "session/set_config_option":
        selected = next(item for item in catalog["configOptions"] if item["id"] == params["configId"])
        selected["currentValue"] = params["value"]
        result = catalog
    elif method == "session/set_mode":
        mode = params["modeId"]
    elif method == "session/prompt":
        assert mode == "auto"
        assert params["prompt"][1:] == [{"type":"image","mimeType":"image/png","data":"aW1n"}]
        assert catalog["configOptions"][0]["currentValue"] == "model-2"
        assert catalog["configOptions"][1]["currentValue"] == "high"
        if params["prompt"][0]["text"] == "wait":
            pending = request["id"]
            update("agent_message_chunk", content={"type":"text","text":"waiting"})
            continue
        (root / "history").touch()
        update("agent_thought_chunk", content={"type":"text","text":"thinking"})
        update("agent_message_chunk", content={"type":"text","text":"hello"})
        update("tool_call", toolCallId="tool-1", title="Shell", kind="execute",
               status="pending", rawInput={"command":"pwd"})
        update("tool_call_update", toolCallId="tool-1", status="completed",
               content=[{"type":"content","content":{"type":"text","text":"project"}}])
        result = {"stopReason":"end_turn"}
    elif method == "session/cancel":
        assert "id" not in request and params["sessionId"] == session
        assert pending is not None
        send(id=pending, result={"stopReason":"cancelled"})
        pending = None
        continue
    else:
        raise AssertionError(method)
    send(id=request["id"], result=result)
"#).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    KimiHarness::new(path)
}

fn params(cwd: &Path) -> StartParams {
    StartParams {
        session_id: "hoh-test".into(),
        cwd: cwd.to_str().unwrap().into(),
        cwd_identity: None,
        model: Some("model-2".into()),
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

fn prompt(text: &str) -> PromptParams {
    PromptParams {
        turn_id: text.into(),
        text: text.into(),
        images: vec![PromptImage {
            media_type: "image/png".into(),
            data: "aW1n".into(),
            name: None,
        }],
        ..Default::default()
    }
}

async fn next(started: &mut Started) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(5), started.events.recv())
        .await
        .expect("Kimi event timeout")
        .expect("Kimi stream closed")
}

#[tokio::test]
async fn discovers_installation_and_login_without_opening_a_native_session() {
    let dir = TempDir::new().unwrap();
    let missing = KimiHarness::new(dir.path().join("missing"));
    let found = missing.discover().await.unwrap();
    assert!(!found.installed);

    let signed_out = stub(dir.path(), "signed-out");
    let (found, version) = signed_out.discover_with_status().await.unwrap();
    assert_eq!(version, VersionStatus::Supported);
    assert!(found.installed);
    assert_eq!(found.auth, AuthState::Unauthenticated);
    assert!(found.detail.unwrap().contains("kimi login"));

    let kimi = stub(dir.path(), "kimi");
    let (found, version) = kimi.discover_with_status().await.unwrap();
    assert_eq!(version, VersionStatus::Supported);
    assert_eq!(found.auth, AuthState::Authenticated);
    assert_eq!(found.version.as_deref(), Some("2.5.3"));
    assert!(found.models.is_empty());
    assert!(!dir.path().join("created").exists());
}

#[tokio::test]
async fn requires_full_and_rejects_restricted_policy_before_starting_kimi() {
    let dir = TempDir::new().unwrap();
    let kimi = stub(dir.path(), "kimi");
    let mut params = params(dir.path());
    assert_eq!(
        kimi.start_or_resume(params.clone())
            .await
            .err()
            .unwrap()
            .code,
        code::POLICY_UNAVAILABLE
    );
    params.policy.sandbox = SandboxMode::ReadOnly;
    assert_eq!(
        kimi.start_with_access(params, AccessMode::Full)
            .await
            .err()
            .unwrap()
            .code,
        code::POLICY_UNAVAILABLE
    );
    assert!(!dir.path().join("started").exists());
}

#[tokio::test]
async fn streams_cancels_and_resumes_a_native_conversation() {
    let dir = TempDir::new().unwrap();
    let kimi = stub(dir.path(), "kimi");
    let mut params = params(dir.path());
    let mut started = kimi
        .start_with_access(params.clone(), AccessMode::Full)
        .await
        .unwrap();
    assert_eq!(started.result.native_session_id.as_str(), ID);
    assert!(!started.result.resumed);
    let found = kimi.discover().await.unwrap();
    assert_eq!(found.models[0].id, "model-1");
    assert!(found.models[0].is_default);
    assert_eq!(found.models[0].default_effort.as_deref(), Some("off"));
    assert_eq!(found.models[0].effort_levels[1].id, "high");
    assert!(found.models[1].effort_levels.is_empty());
    started.session.prompt(prompt("first")).await.unwrap();
    assert!(
        matches!(next(&mut started).await, HarnessEvent::TurnStarted { turn_id } if turn_id == "first")
    );
    assert!(
        matches!(next(&mut started).await, HarnessEvent::ReasoningDelta { text, .. } if text == "thinking")
    );
    assert!(
        matches!(next(&mut started).await, HarnessEvent::TextDelta { text, .. } if text == "hello")
    );
    assert!(
        matches!(next(&mut started).await, HarnessEvent::ItemStarted { action: HarnessAction::Command { command, .. }, .. } if command == "pwd")
    );
    assert!(
        matches!(next(&mut started).await, HarnessEvent::ItemCompleted { outcome: ItemOutcome::Completed, output: Some(text), .. } if text == "project")
    );
    assert!(
        matches!(next(&mut started).await, HarnessEvent::TurnCompleted { turn_id } if turn_id == "first")
    );

    started.session.prompt(prompt("wait")).await.unwrap();
    assert!(matches!(
        next(&mut started).await,
        HarnessEvent::TurnStarted { .. }
    ));
    assert!(
        matches!(next(&mut started).await, HarnessEvent::TextDelta { text, .. } if text == "waiting")
    );
    started.session.interrupt().await.unwrap();
    assert!(
        matches!(next(&mut started).await, HarnessEvent::TurnAborted { turn_id } if turn_id == "wait")
    );
    started.session.terminate(Duration::from_millis(100)).await;

    params.resume = Some(ID.to_owned().into());
    let mut resumed = kimi
        .start_with_access(params.clone(), AccessMode::Full)
        .await
        .unwrap();
    assert!(resumed.result.resumed);
    assert_eq!(resumed.result.native_session_id.as_str(), ID);
    resumed.session.prompt(prompt("again")).await.unwrap();
    assert!(
        matches!(next(&mut resumed).await, HarnessEvent::TurnStarted { turn_id } if turn_id == "again")
    );
    resumed.session.terminate(Duration::from_millis(100)).await;

    params.resume = Some("missing-native-id".to_owned().into());
    assert_eq!(
        kimi.start_with_access(params, AccessMode::Full)
            .await
            .err()
            .unwrap()
            .code,
        code::SESSION_NOT_FOUND
    );
}
