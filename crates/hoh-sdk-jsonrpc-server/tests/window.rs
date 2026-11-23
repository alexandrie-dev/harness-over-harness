//! Window protocol: host handshake, session list/events/cancel, `hoh/*` method table.
mod support;
use futures_util::{FutureExt, future::BoxFuture};
use hoh_error::BoxError;
use hoh_protocol::InitializeParams;
use hoh_sdk_jsonrpc_server::{
    HohPrivilegedApprover, HohRpcMethodRegistry, HohRpcMethods, PrivilegedApprover,
};
use hoh_session_persistence::SessionPersistenceService;
use hoh_user_approval::{Approval, ApprovalService};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::*;

const KEY: &str = "4242424242424242424242424242424242424242424242424242424242424242";

fn host_params(env: &Env, hoh: Value) -> InitializeParams {
    serde_json::from_value(json!({
        "cwd": env.dir.path(), "provider": "mock", "model": "mock",
        "capabilities": {"approvals": "host"}, "hoh": hoh,
    }))
    .unwrap()
}
async fn host_env() -> Env {
    let env = Env::new().await;
    mount::<ApprovalService>(&env.ctx, json!({})).await;
    env
}
async fn registry(env: &Env) -> Arc<HohRpcMethodRegistry> {
    let registry = Arc::new(HohRpcMethodRegistry::default());
    env.ctx
        .provide::<HohRpcMethods>(registry.clone())
        .await
        .unwrap();
    registry
}
fn message(error: BoxError) -> String {
    error.to_string()
}

#[tokio::test]
async fn host_initialize_confirms_host_approvals_and_installs_the_key_once() {
    let env = host_env().await;
    let result = env
        .server
        .initialize(host_params(
            &env,
            json!({"protocolVersion": 1, "approvalKey": KEY}),
        ))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({
            "serverInfo": {"name": "deepseek-harness-sdk-runtime", "version": "0.0.1"},
            "hoh": {"protocolVersion": 1, "approvals": "host"},
        })
    );
    assert!(env.ctx.get::<Approval>().unwrap().host_mode_enabled());
    env.close().await;
}

#[tokio::test]
async fn plain_initialize_keeps_the_sdk_result_and_host_mode_off() {
    let env = host_env().await;
    let result = env.server.initialize(env.params()).await.unwrap();
    assert!(result.hoh.is_none());
    assert!(!env.ctx.get::<Approval>().unwrap().host_mode_enabled());
    env.close().await;
}

#[tokio::test]
async fn repeated_initialize_is_an_error_and_keeps_the_first_key() {
    let env = host_env().await;
    let hoh = json!({"protocolVersion": 1, "approvalKey": KEY});
    env.server
        .initialize(host_params(&env, hoh.clone()))
        .await
        .unwrap();
    let other = json!({"protocolVersion": 1, "approvalKey": "43".repeat(32)});
    for params in [
        host_params(&env, hoh),
        host_params(&env, other),
        env.params(),
    ] {
        let error = message(env.server.initialize(params).await.unwrap_err());
        assert!(error.contains("already initialized"), "{error}");
    }
    env.close().await;
}

#[tokio::test]
async fn concurrent_initialize_lets_exactly_one_win() {
    let env = Env::new().await;
    let (a, b) = tokio::join!(
        env.server.initialize(env.params()),
        env.server.initialize(env.params())
    );
    assert_eq!(a.is_ok() as u8 + b.is_ok() as u8, 1);
    env.close().await;
}

#[tokio::test]
async fn rejected_host_handshakes_change_nothing_and_never_echo_the_key() {
    let env = host_env().await;
    let bad_keys = [
        "".to_string(),
        "42".repeat(31),
        "00".repeat(32),
        "AB".repeat(32),
    ];
    for key in &bad_keys {
        let params = host_params(&env, json!({"protocolVersion": 1, "approvalKey": key}));
        let error = message(env.server.initialize(params).await.unwrap_err());
        assert!(key.is_empty() || !error.contains(key.as_str()), "{error}");
    }
    let rejected = [
        json!({"protocolVersion": 1}),
        json!({"protocolVersion": 2, "approvalKey": KEY}),
    ];
    for hoh in rejected {
        assert!(env.server.initialize(host_params(&env, hoh)).await.is_err());
    }
    // approvals:"host" without any hoh block
    let no_hoh: InitializeParams = serde_json::from_value(json!({
        "cwd": env.dir.path(), "provider": "mock", "model": "mock",
        "capabilities": {"approvals": "host"},
    }))
    .unwrap();
    assert!(env.server.initialize(no_hoh).await.is_err());
    // a key without host approvals
    let key_only: InitializeParams = serde_json::from_value(json!({
        "cwd": env.dir.path(), "provider": "mock", "model": "mock",
        "hoh": {"protocolVersion": 1, "approvalKey": KEY},
    }))
    .unwrap();
    assert!(env.server.initialize(key_only).await.is_err());
    assert!(!env.ctx.get::<Approval>().unwrap().host_mode_enabled());
    // every failure above left the server ready for a proper handshake
    env.server
        .initialize(host_params(
            &env,
            json!({"protocolVersion": 1, "approvalKey": KEY}),
        ))
        .await
        .unwrap();
    env.close().await;
}

#[tokio::test]
async fn host_initialize_without_the_approval_service_fails_closed() {
    let env = Env::new().await;
    let params = host_params(&env, json!({"protocolVersion": 1, "approvalKey": KEY}));
    let error = message(env.server.initialize(params).await.unwrap_err());
    assert!(error.contains("approval service"), "{error}");
    assert!(
        env.server
            .handle_request("session/list", json!({}))
            .await
            .is_err()
    );
    env.close().await;
}

#[tokio::test]
async fn host_mode_routes_tool_approvals_through_the_window_transport() {
    let env = host_env().await;
    env.server
        .initialize(host_params(
            &env,
            json!({"protocolVersion": 1, "approvalKey": KEY}),
        ))
        .await
        .unwrap();
    let agent = env.open("main").await;
    agent
        .session()
        .append("turn/start", json!({"turn": 1}), None)
        .unwrap();
    env.peer
        .answer(json!({"approvalId": "x", "outcome": "rejected", "mac": "0".repeat(64)}));
    let outcome = env
        .ctx
        .get::<Approval>()
        .unwrap()
        .request(hoh_user_approval::ApprovalRequest::new(agent, "bash"))
        .await
        .unwrap();
    assert_ne!(outcome, hoh_user_approval::ApprovalOutcome::AllowedOnce);
    assert_eq!(lock(&env.peer.requests)[0].0, "approval/request");
    assert_eq!(env.peer.events("approval.pending").len(), 1);
    env.close().await;
}

async fn chat(env: &Env, id: &str, text: &str) {
    env.prompt(id, text).await;
    env.wait(id).await;
}

#[tokio::test]
async fn session_events_pages_by_seq_and_matches_the_live_notifications() {
    let env = Env::new().await;
    env.initialize().await;
    chat(&env, "s1", "hello window").await;
    let streamed: Vec<Value> = env
        .peer
        .events("session.event")
        .iter()
        .map(|n| n["event"].clone())
        .collect();
    let mut all = Vec::new();
    let mut after = Value::Null;
    loop {
        let mut params = json!({"sessionId": "s1", "limit": 2});
        if !after.is_null() {
            params["afterSeq"] = after.clone();
        }
        let page = env
            .server
            .handle_request("session/events", params)
            .await
            .unwrap();
        let events = page["events"].as_array().unwrap().clone();
        if events.is_empty() {
            assert_eq!(page["nextSeq"], after.as_i64().unwrap_or(-1));
            break;
        }
        assert!(events.len() <= 2);
        assert_eq!(page["nextSeq"], events.last().unwrap()["seq"]);
        after = page["nextSeq"].clone();
        all.extend(events);
    }
    assert_eq!(all, streamed);
    assert_eq!(all[0]["seq"], 0);
    env.close().await;
}

#[tokio::test]
async fn session_events_rejects_bad_arguments_and_unknown_sessions() {
    let env = Env::new().await;
    assert!(
        env.server
            .handle_request("session/events", json!({"sessionId": "s", "limit": 1}))
            .await
            .unwrap_err()
            .to_string()
            .contains("not initialized")
    );
    env.initialize().await;
    chat(&env, "s1", "x").await;
    for params in [
        json!({"sessionId": "s1", "limit": 0}),
        json!({"sessionId": "s1", "limit": 1001}),
        json!({"sessionId": "s1", "limit": 5, "afterSeq": -2}),
        json!({"sessionId": "s1"}),
        json!({"limit": 5}),
    ] {
        let error = env.server.handle_request("session/events", params).await;
        assert!(error.is_err());
    }
    let error = env
        .server
        .handle_request("session/events", json!({"sessionId": "nope", "limit": 5}))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("unknown SDK session"), "{error}");
    env.close().await;
}

#[tokio::test]
async fn session_list_covers_live_and_stored_sessions_with_titles_and_filter() {
    let root = tempfile::tempdir().unwrap();
    let env = Env::persistent(root.path()).await;
    env.initialize().await;
    chat(&env, "older", "first question\nsecond line").await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    chat(&env, "newer", "another topic").await;
    let list = env
        .server
        .handle_request("session/list", json!({}))
        .await
        .unwrap();
    let sessions = list["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["id"], "newer");
    assert_eq!(sessions[0]["title"], "another topic");
    assert_eq!(sessions[1]["id"], "older");
    assert_eq!(sessions[1]["title"], "first question");
    assert!(
        sessions[0]["updatedAt"].as_u64().unwrap() >= sessions[1]["updatedAt"].as_u64().unwrap()
    );
    assert_eq!(sessions[0]["employeeId"], "default");
    let none = env
        .server
        .handle_request("session/list", json!({"employeeId": "someone"}))
        .await
        .unwrap();
    assert_eq!(none, json!({"sessions": []}));
    let some = env
        .server
        .handle_request("session/list", json!({"employeeId": "default"}))
        .await
        .unwrap();
    assert_eq!(some["sessions"].as_array().unwrap().len(), 2);
    assert!(
        env.server
            .handle_request("session/list", json!({"employeeId": null}))
            .await
            .is_err()
    );
    env.close().await;

    // A fresh process over the same store lists and replays sessions it has not opened.
    let again = Env::persistent(root.path()).await;
    again.initialize().await;
    let list = again
        .server
        .handle_request("session/list", json!({}))
        .await
        .unwrap();
    assert_eq!(list["sessions"].as_array().unwrap().len(), 2);
    let page = again
        .server
        .handle_request(
            "session/events",
            json!({"sessionId": "older", "limit": 1000}),
        )
        .await
        .unwrap();
    assert!(
        page["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "user/message")
    );
    again.close().await;
}

#[tokio::test]
async fn session_list_and_events_read_the_saved_log_while_another_process_holds_its_write_lease() {
    let root = tempfile::tempdir().unwrap();
    let holder = Env::persistent(root.path()).await;
    holder.initialize().await;
    chat(&holder, "stopped", "Count from 1 to 500").await;
    holder
        .ctx
        .get::<SessionPersistenceService>()
        .unwrap()
        .flush()
        .await
        .unwrap();

    let observer = Env::persistent(root.path()).await;
    observer.initialize().await;
    let resume = observer
        .server
        .handle_request("session/resume", json!({"sessionId": "stopped"}))
        .await
        .unwrap_err();
    assert!(resume.to_string().contains("already owned"), "{resume}");

    let list = observer
        .server
        .handle_request("session/list", json!({}))
        .await
        .unwrap();
    assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(list["sessions"][0]["id"], "stopped");
    assert_eq!(list["sessions"][0]["title"], "Count from 1 to 500");
    let page = observer
        .server
        .handle_request(
            "session/events",
            json!({"sessionId": "stopped", "limit": 1000}),
        )
        .await
        .unwrap();
    assert!(
        page["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "user/message")
    );

    holder.close().await;
    observer
        .server
        .handle_request("session/resume", json!({"sessionId": "stopped"}))
        .await
        .unwrap();
    observer.close().await;
}

#[tokio::test]
async fn session_cancel_ends_the_running_turn_and_reports_unknown_sessions() {
    let env = Env::new().await;
    env.initialize().await;
    *lock(&env.model.gate) = Some(Arc::new(tokio::sync::Semaphore::new(0)));
    env.prompt("busy", "long task").await;
    env.model.entered.notified().await;
    let result = env
        .server
        .handle_request("session/cancel", json!({"sessionId": "busy"}))
        .await
        .unwrap();
    assert_eq!(result, json!({}));
    let _ = env
        .server
        .handle_request("session/wait", json!({"sessionId": "busy"}))
        .await;
    let agent = env.agent("busy");
    let end = agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .rev()
        .find(|e| e.event_type() == "turn/end")
        .cloned()
        .expect("turn/end");
    assert_eq!(
        end.data()["reason"],
        json!({"kind": "aborted", "reason": {"kind": "user"}})
    );
    for params in [json!({"sessionId": "nope"}), json!({})] {
        assert!(
            env.server
                .handle_request("session/cancel", params)
                .await
                .is_err()
        );
    }
    env.close().await;
}

#[tokio::test]
async fn hoh_methods_dispatch_to_registered_handlers_only() {
    let env = Env::new().await;
    let registry = registry(&env).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let _guard = registry
        .register(&env.ctx, "hoh/employee/list", move |params| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(params);
                Ok(json!({"ok": true}))
            }
        })
        .unwrap();
    assert!(
        env.server
            .handle_request("hoh/employee/list", json!({}))
            .await
            .unwrap_err()
            .to_string()
            .contains("not initialized")
    );
    env.initialize().await;
    assert_eq!(
        env.server
            .handle_request("hoh/employee/list", json!({"x": 1}))
            .await
            .unwrap(),
        json!({"ok": true})
    );
    assert_eq!(*seen.lock().unwrap(), vec![json!({"x": 1})]);
    let unknown = env
        .server
        .handle_request("hoh/task/list", json!({}))
        .await
        .unwrap_err();
    assert!(unknown.to_string().contains("unknown HoH method"));
    env.close().await;
}

#[tokio::test]
async fn registry_refuses_undeclared_and_duplicate_methods_and_unregisters_on_dispose() {
    let env = Env::new().await;
    let registry = registry(&env).await;
    let ok = |_: Value| async { Ok::<_, BoxError>(json!({})) };
    assert!(registry.register(&env.ctx, "hoh/not/declared", ok).is_err());
    assert!(registry.register(&env.ctx, "session/list", ok).is_err());
    let guard = registry.register(&env.ctx, "hoh/task/list", ok).unwrap();
    assert!(registry.register(&env.ctx, "hoh/task/list", ok).is_err());
    env.initialize().await;
    env.server
        .handle_request("hoh/task/list", json!({}))
        .await
        .unwrap();
    guard.dispose().await;
    assert!(
        env.server
            .handle_request("hoh/task/list", json!({}))
            .await
            .is_err()
    );
    env.close().await;
}

/// HOH-40: the one `session/prompt` listener hears each prompt's session, the id of the message
/// it queued and the whole params (the window's `effort`, which the server itself does not read),
/// until it is disposed; `session/query` says nothing to it.
#[tokio::test]
async fn a_prompt_listener_hears_the_params_of_every_prompt() {
    let env = Env::new().await;
    let registry = registry(&env).await;
    let heard = Arc::new(Mutex::new(Vec::new()));
    let log = heard.clone();
    let guard = registry
        .on_prompt(&env.ctx, move |session, message, params| {
            log.lock().unwrap().push((
                session.to_owned(),
                message.to_owned(),
                params["effort"].clone(),
            ));
        })
        .unwrap();
    assert!(registry.on_prompt(&env.ctx, |_, _, _| {}).is_err());
    env.initialize().await;
    let prompt = |effort: &str| {
        json!({
            "sessionId": "s1",
            "contentBlocks": [{"type": "text", "text": "hi"}],
            "effort": effort,
        })
    };
    let receipt = env
        .server
        .handle_request("session/prompt", prompt("high"))
        .await
        .unwrap();
    env.wait("s1").await;
    let message = receipt["messageId"].as_str().unwrap().to_owned();
    assert_eq!(
        *heard.lock().unwrap(),
        vec![("s1".to_owned(), message, json!("high"))]
    );
    env.server
        .handle_request(
            "session/query",
            json!({"sessionId": "s1", "contentBlocks": [{"type": "text", "text": "hi"}]}),
        )
        .await
        .unwrap();
    env.wait("s1").await;
    assert_eq!(heard.lock().unwrap().len(), 1);
    guard.dispose().await;
    env.server
        .handle_request("session/prompt", prompt("max"))
        .await
        .unwrap();
    env.wait("s1").await;
    assert_eq!(heard.lock().unwrap().len(), 1);
    env.close().await;
}

#[tokio::test]
async fn invalid_interaction_mode_is_rejected_before_session_message_and_listener() {
    let env = Env::new().await;
    let registry = registry(&env).await;
    let heard = Arc::new(Mutex::new(Vec::new()));
    let log = heard.clone();
    let _guard = registry
        .on_prompt(&env.ctx, move |session, message, _| {
            log.lock()
                .unwrap()
                .push((session.to_owned(), message.to_owned()));
        })
        .unwrap();
    env.initialize().await;
    for interaction_mode in [json!("future-mode"), Value::Null] {
        let result = env
            .server
            .handle_request(
                "session/prompt",
                json!({
                    "sessionId":"invalid-mode-session",
                    "contentBlocks":[{"type":"text","text":"must not be stored"}],
                    "interactionMode":interaction_mode,
                }),
            )
            .await;
        assert!(result.is_err());
    }
    assert!(heard.lock().unwrap().is_empty());
    let listed = env
        .server
        .handle_request("session/list", json!({}))
        .await
        .unwrap();
    assert_eq!(listed["sessions"], json!([]));
    env.close().await;
}

struct Approver(Mutex<Vec<String>>, bool);
impl PrivilegedApprover for Approver {
    fn approve(&self, method: &str, _: &Value) -> BoxFuture<'static, Result<(), BoxError>> {
        self.0.lock().unwrap().push(method.into());
        let allow = self.1;
        async move {
            if allow {
                Ok(())
            } else {
                Err("rejected by the user".into())
            }
        }
        .boxed()
    }
}

#[tokio::test]
async fn privileged_methods_fail_closed_without_host_and_wait_for_the_approver_with_it() {
    // Without host approvals the new hoh/* surface refuses privileged calls outright.
    let plain = Env::new().await;
    let table = registry(&plain).await;
    let calls = Arc::new(Mutex::new(0));
    let counter = calls.clone();
    let _guard = table
        .register(&plain.ctx, "hoh/model/key/set", move |_| {
            let counter = counter.clone();
            async move {
                *counter.lock().unwrap() += 1;
                Ok(json!({}))
            }
        })
        .unwrap();
    plain.initialize().await;
    let refused = plain
        .server
        .handle_request("hoh/model/key/set", json!({"apiKey": "k", "baseUrl": "u"}))
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("host approvals"), "{refused}");
    assert_eq!(*calls.lock().unwrap(), 0);
    plain.close().await;

    // Host mode: no approver mounted -> refused; approver says no -> refused; yes -> runs.
    let env = host_env().await;
    let table = registry(&env).await;
    let counter = calls.clone();
    let _guard = table
        .register(&env.ctx, "hoh/model/key/set", move |_| {
            let counter = counter.clone();
            async move {
                *counter.lock().unwrap() += 1;
                Ok(json!({}))
            }
        })
        .unwrap();
    env.server
        .initialize(host_params(
            &env,
            json!({"protocolVersion": 1, "approvalKey": KEY}),
        ))
        .await
        .unwrap();
    let args = json!({"apiKey": "k", "baseUrl": "u"});
    assert!(
        env.server
            .handle_request("hoh/model/key/set", args.clone())
            .await
            .is_err()
    );
    let no = Arc::new(Approver(Mutex::default(), false));
    let denied = env
        .ctx
        .provide::<HohPrivilegedApprover>(no.clone())
        .await
        .unwrap();
    assert!(
        env.server
            .handle_request("hoh/model/key/set", args.clone())
            .await
            .is_err()
    );
    assert_eq!(*calls.lock().unwrap(), 0);
    drop(denied);
    env.close().await;

    let env = host_env().await;
    let table = registry(&env).await;
    let counter = calls.clone();
    let _guard = table
        .register(&env.ctx, "hoh/model/key/set", move |_| {
            let counter = counter.clone();
            async move {
                *counter.lock().unwrap() += 1;
                Ok(json!({}))
            }
        })
        .unwrap();
    let yes = Arc::new(Approver(Mutex::default(), true));
    env.ctx
        .provide::<HohPrivilegedApprover>(yes.clone())
        .await
        .unwrap();
    env.server
        .initialize(host_params(
            &env,
            json!({"protocolVersion": 1, "approvalKey": KEY}),
        ))
        .await
        .unwrap();
    env.server
        .handle_request("hoh/model/key/set", args)
        .await
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);
    // The legacy SDK method is gated the same way in host mode.
    env.server
        .handle_request(
            "session/working-directory/set",
            json!({"sessionId": "w", "path": "."}),
        )
        .await
        .unwrap();
    assert_eq!(
        *yes.0.lock().unwrap(),
        vec!["hoh/model/key/set", "session/working-directory/set"]
    );
    env.close().await;
}

/// `session/cancel` abandons the model request in flight; the next prompt is a turn of its own
/// with its own request and answer (the first request's late text is never delivered).
#[tokio::test]
async fn the_prompt_after_a_cancel_makes_its_own_model_request() {
    let env = Env::new().await;
    env.initialize().await;
    lock(&env.model.texts).extend(["late text".to_owned(), "fresh text".to_owned()]);
    *lock(&env.model.gate) = Some(Arc::new(tokio::sync::Semaphore::new(0)));
    env.prompt("c", "slow").await;
    env.model.entered.notified().await;
    env.server
        .handle_request("session/cancel", json!({"sessionId": "c"}))
        .await
        .unwrap();
    let _ = env
        .server
        .handle_request("session/wait", json!({"sessionId": "c"}))
        .await;
    chat(&env, "c", "again").await;
    assert_eq!(lock(&env.model.requests).len(), 2);
    let texts: Vec<String> = env
        .agent("c")
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|e| e.event_type() == "assistant/message")
        .map(|e| {
            e.data()["message"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(texts, ["fresh text"]);
    env.close().await;
}
