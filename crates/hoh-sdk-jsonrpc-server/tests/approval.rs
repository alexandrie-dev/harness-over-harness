mod support;
use futures_util::future::BoxFuture;
use hoh_agent::{Agent, AgentSessionMeta, Agents, CreateAgentOptions};
use hoh_error::{BoxError, HarnessError};
use hoh_sdk_jsonrpc_server::{
    HarnessSdkJsonRpcServer,
    integration::{Subagents, SubagentsApi},
    transport::JsonRpcLineTransport,
};
use hoh_timeout::AbortController;
use hoh_user_approval::{
    Approval, ApprovalOutcome, ApprovalPolicy, ApprovalRequest, ApprovalService,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use support::*;
use tokio::io::BufReader;
async fn setup(enabled: Option<bool>) -> Env {
    let h = Env::new().await;
    mount::<ApprovalService>(&h.ctx, json!({})).await;
    let mut p = h.params();
    p.approval_requests = enabled;
    h.server.initialize(p).await.unwrap();
    let a = h.open("main").await;
    a.session()
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    h
}
fn req(agent: Arc<dyn Agent>) -> ApprovalRequest {
    ApprovalRequest::new(agent, "bash")
}
async fn decide(h: &Env, request: ApprovalRequest) -> ApprovalOutcome {
    h.ctx
        .get::<Approval>()
        .unwrap()
        .request(request)
        .await
        .unwrap()
}
#[tokio::test]
async fn applies_and_audits_allowed_once_rejected_cancelled() {
    let h = setup(Some(true)).await;
    for (text, outcome) in [
        ("allowed-once", ApprovalOutcome::AllowedOnce),
        ("rejected", ApprovalOutcome::Rejected),
        ("cancelled", ApprovalOutcome::Cancelled),
    ] {
        h.peer.answer(json!({"outcome":text}));
        let mut r = req(h.agent("main"));
        r.call_id = Some("call-1".into());
        r.reason = Some("write outside workspace".into());
        assert_eq!(decide(&h, r).await, outcome);
        assert_eq!(
            lock(&h.peer.requests).last().unwrap().clone(),
            (
                "approval/request".into(),
                json!({"sessionId":"main","toolName":"bash","callId":"call-1","reason":"write outside workspace"})
            )
        );
        let events = h.agent("main").session().snapshot_events(None, None);
        let last = &events[events.len() - 2..];
        assert_eq!(last[0].event_type(), "approval/asked");
        assert_eq!(last[0].data()["toolName"], "bash");
        assert_eq!(last[1].event_type(), "approval/decided");
        assert_eq!(last[1].data()["outcome"], text);
    }
    h.close().await;
}
#[tokio::test]
async fn delegates_when_approval_requests_is_omitted_or_false() {
    for enabled in [None, Some(false)] {
        let h = setup(enabled).await;
        h.ctx
            .on_waterfall::<ApprovalRequest, Value, _>(|_, _| async { Ok(json!("rejected")) })
            .unwrap();
        assert_eq!(
            decide(&h, req(h.agent("main"))).await,
            ApprovalOutcome::Rejected
        );
        assert!(lock(&h.peer.requests).is_empty());
        h.close().await;
    }
}
#[tokio::test]
async fn fails_closed_for_invalid_response() {
    let h = setup(Some(true)).await;
    for value in [
        Value::Null,
        json!({}),
        json!({"outcome":"allow-always"}),
        json!({"outcome":true}),
        json!({"outcome":"unavailable"}),
    ] {
        h.peer.answer(value);
        assert_eq!(
            decide(&h, req(h.agent("main"))).await,
            ApprovalOutcome::Unavailable
        );
    }
    h.close().await;
}
#[tokio::test]
async fn fails_closed_on_client_errors_and_a_disconnected_transport() {
    let mut h = setup(Some(true)).await;
    assert_eq!(
        decide(&h, req(h.agent("main"))).await,
        ApprovalOutcome::Unavailable
    );
    h.server.shutdown().await.unwrap();
    let (a, b) = tokio::io::duplex(4096);
    let (r, w) = tokio::io::split(a);
    let wire = JsonRpcLineTransport::start(&h.ctx, BufReader::new(r), w).unwrap();
    let _other = b;
    h.server =
        HarnessSdkJsonRpcServer::new(h.ctx.clone(), Arc::new(wire.clone()), Default::default())
            .unwrap();
    let mut p = h.params();
    p.approval_requests = Some(true);
    h.server.initialize(p).await.unwrap();
    let a = h.open("wire").await;
    a.session()
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    wire.close();
    assert_eq!(decide(&h, req(a)).await, ApprovalOutcome::Unavailable);
    h.close().await;
}
#[tokio::test]
async fn correlates_concurrent_requests_while_another_client_request_is_in_flight() {
    let h = setup(Some(true)).await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let (g, e) = (gate.clone(), entered.clone());
    *lock(&h.peer.host) = Some(Arc::new(move |_, p, _| {
        let (g, e) = (g.clone(), e.clone());
        Box::pin(async move {
            if p["toolName"] == "first" {
                e.notify_one();
                g.acquire().await.unwrap().forget();
                Ok(json!({"outcome":"allowed-once"}))
            } else {
                Ok(json!({"outcome":"rejected"}))
            }
        })
    }));
    let mut r = req(h.agent("main"));
    r.tool_name = "first".into();
    let first = h.ctx.get::<Approval>().unwrap().request(r);
    entered.notified().await;
    let mut r = req(h.agent("main"));
    r.tool_name = "second".into();
    assert_eq!(decide(&h, r).await, ApprovalOutcome::Rejected);
    h.open("main").await;
    gate.add_permits(1);
    assert_eq!(first.await.unwrap(), ApprovalOutcome::AllowedOnce);
    h.close().await;
}
async fn pending(action: &str) {
    let h = setup(Some(true)).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let (g, e) = (gate.clone(), entered.clone());
    *lock(&h.peer.host) = Some(Arc::new(move |_, _, signal| {
        let (g, e) = (g.clone(), e.clone());
        Box::pin(async move {
            e.notify_one();
            let signal = signal.unwrap();
            tokio::select! {_=g.acquire()=>Ok(json!({"outcome":"allowed-once"})),reason=signal.cancelled()=>Err(reason.into())}
        })
    }));
    let control = AbortController::new();
    let mut r = req(h.agent("main"));
    r.signal = Some(control.signal());
    let agent = r.agent.clone();
    let pending = h.ctx.get::<Approval>().unwrap().request(r);
    entered.notified().await;
    if action == "cancel" {
        control.abort();
    } else {
        h.server.shutdown().await.unwrap();
    }
    let expected = if action == "cancel" {
        ApprovalOutcome::Cancelled
    } else {
        ApprovalOutcome::Unavailable
    };
    assert_eq!(pending.await.unwrap(), expected);
    gate.add_permits(1);
    tokio::task::yield_now().await;
    assert_eq!(
        agent
            .session()
            .snapshot_events(None, None)
            .iter()
            .filter(|e| e.event_type() == "approval/decided")
            .count(),
        1
    );
    h.close().await;
}
#[tokio::test]
async fn settles_a_pending_approval_on_cancel_and_ignores_a_late_grant() {
    pending("cancel").await;
}
#[tokio::test]
async fn settles_a_pending_approval_on_shutdown_and_ignores_a_late_grant() {
    pending("shutdown").await;
}
#[derive(Clone)]
enum TreeFailure {
    Named(&'static str, &'static str),
    Foreign(Value),
}
struct Tree {
    roots: Mutex<HashMap<String, Arc<dyn Agent>>>,
    failure: Mutex<Option<TreeFailure>>,
}
impl SubagentsApi for Tree {
    fn root_of(&self, a: &Arc<dyn Agent>) -> Result<Arc<dyn Agent>, BoxError> {
        if let Some(failure) = lock(&self.failure).clone() {
            return Err(match failure {
                TreeFailure::Named(name, code) => {
                    HarnessError::named(name, "root lookup failed", code).into()
                }
                TreeFailure::Foreign(value) => {
                    Box::new(hoh_sdk_jsonrpc_server::error::ForeignErrorValue::new(value))
                }
            });
        }
        lock(&self.roots)
            .get(a.id().as_str())
            .cloned()
            .ok_or_else(|| {
                HarnessError::named("SubagentError", "not a managed subagent", "UNAUTHORIZED")
                    .into()
            })
    }
    fn wait_for_tree_work(&self, _: Arc<dyn Agent>) -> BoxFuture<'static, Result<bool, BoxError>> {
        Box::pin(async { Ok(false) })
    }
    fn drain_descendants(
        &self,
        _: Vec<Arc<dyn Agent>>,
    ) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }
}
async fn outside(h: &Env, id: &str, lineage: bool) -> hoh_agent::AgentHandle {
    let a = h
        .ctx
        .get::<Agents>()
        .unwrap()
        .create(
            &h.ctx,
            CreateAgentOptions {
                session_id: Some(id.into()),
                meta: lineage.then(|| AgentSessionMeta {
                    parent_session: Some("main".into()),
                    origin: Some("subagent".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    a.agent
        .session()
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    a
}
#[tokio::test]
async fn does_not_claim_an_unrelated_live_agent_with_or_without_subagents_or_sdk_lineage() {
    for subagents in [false, true] {
        for lineage in [false, true] {
            let h = setup(Some(true)).await;
            if subagents {
                h.ctx
                    .provide::<Subagents>(Arc::new(Tree {
                        roots: Mutex::new(HashMap::new()),
                        failure: Mutex::new(None),
                    }))
                    .await
                    .unwrap();
            }
            let a = outside(&h, "foreign", lineage).await;
            h.peer.answer(json!({"outcome":"allowed-once"}));
            h.ctx
                .on_waterfall::<ApprovalRequest, Value, _>(|_, _| async { Ok(json!("rejected")) })
                .unwrap();
            assert_eq!(
                decide(&h, req(a.agent.clone())).await,
                ApprovalOutcome::Rejected
            );
            assert!(lock(&h.peer.requests).is_empty());
            a.dispose().await.unwrap();
            h.close().await;
        }
    }
}
#[tokio::test]
async fn serves_runtime_descendants_with_sdk_session_lineage_and_preserves_never_policy() {
    for lineage in [false, true] {
        let h = setup(Some(true)).await;
        let child = outside(&h, "child", lineage).await;
        let grandchild = outside(&h, "grandchild", false).await;
        let roots = [
            ("child".into(), h.agent("main")),
            ("grandchild".into(), h.agent("main")),
        ]
        .into_iter()
        .collect();
        h.ctx
            .provide::<Subagents>(Arc::new(Tree {
                roots: Mutex::new(roots),
                failure: Mutex::new(None),
            }))
            .await
            .unwrap();
        h.peer.answer(json!({"outcome":"allowed-once"}));
        assert_eq!(
            decide(&h, req(child.agent.clone())).await,
            ApprovalOutcome::AllowedOnce
        );
        assert_eq!(
            decide(&h, req(grandchild.agent.clone())).await,
            ApprovalOutcome::AllowedOnce
        );
        h.ctx
            .get::<Approval>()
            .unwrap()
            .set_policy(&*child.agent, ApprovalPolicy::Never)
            .unwrap();
        assert_eq!(
            decide(&h, req(child.agent.clone())).await,
            ApprovalOutcome::Rejected
        );
        assert_eq!(lock(&h.peer.requests).len(), 2);
        child.dispose().await.unwrap();
        grandchild.dispose().await.unwrap();
        h.close().await;
    }
}
#[tokio::test]
async fn routes_approval_from_a_managed_descendant_after_its_creator_leaves() {
    let h = setup(Some(true)).await;
    let creator = outside(&h, "creator", true).await;
    let child = outside(&h, "grandchild", true).await;
    let tree = Arc::new(Tree {
        roots: Mutex::new(
            [("grandchild".into(), h.agent("main"))]
                .into_iter()
                .collect(),
        ),
        failure: Mutex::new(None),
    });
    h.ctx.provide::<Subagents>(tree).await.unwrap();
    creator.dispose().await.unwrap();
    h.peer.answer(json!({"outcome":"allowed-once"}));
    assert_eq!(
        decide(&h, req(child.agent.clone())).await,
        ApprovalOutcome::AllowedOnce
    );
    child.dispose().await.unwrap();
    h.close().await;
}
#[tokio::test]
async fn does_not_claim_another_runtime_owners_child_despite_sdk_session_lineage() {
    let h = setup(Some(true)).await;
    let owner = outside(&h, "foreign-owner", false).await;
    let child = outside(&h, "foreign-child", true).await;
    h.ctx
        .provide::<Subagents>(Arc::new(Tree {
            roots: Mutex::new(
                [("foreign-child".into(), owner.agent.clone())]
                    .into_iter()
                    .collect(),
            ),
            failure: Mutex::new(None),
        }))
        .await
        .unwrap();
    h.ctx
        .on_waterfall::<ApprovalRequest, Value, _>(|_, _| async { Ok(json!("rejected")) })
        .unwrap();
    assert_eq!(
        decide(&h, req(child.agent.clone())).await,
        ApprovalOutcome::Rejected
    );
    assert!(lock(&h.peer.requests).is_empty());
    child.dispose().await.unwrap();
    owner.dispose().await.unwrap();
    h.close().await;
}
#[tokio::test]
async fn delegates_approval_for_an_unauthorized_ownership_lookup_by_error_fields() {
    for failure in [
        TreeFailure::Named("SubagentError", "UNAUTHORIZED"),
        TreeFailure::Foreign(
            json!({"name":"SubagentError","code":"UNAUTHORIZED","message":"foreign owner"}),
        ),
    ] {
        let h = setup(Some(true)).await;
        let foreign = outside(&h, "foreign", false).await;
        h.ctx
            .provide::<Subagents>(Arc::new(Tree {
                roots: Mutex::new(HashMap::new()),
                failure: Mutex::new(Some(failure)),
            }))
            .await
            .unwrap();
        h.ctx
            .on_waterfall::<ApprovalRequest, Value, _>(|_, _| async { Ok(json!("rejected")) })
            .unwrap();
        assert_eq!(
            decide(&h, req(foreign.agent.clone())).await,
            ApprovalOutcome::Rejected
        );
        foreign.dispose().await.unwrap();
        h.close().await;
    }
}
#[tokio::test]
async fn does_not_pass_a_failed_ownership_lookup_to_another_approval_answerer() {
    for (name, code) in [
        ("Error", "UNKNOWN"),
        ("SubagentError", "NOT_RESUMABLE"),
        ("OtherError", "UNAUTHORIZED"),
        ("SubagentError", ""),
    ] {
        let h = setup(Some(true)).await;
        let foreign = outside(&h, "foreign", false).await;
        h.ctx
            .provide::<Subagents>(Arc::new(Tree {
                roots: Mutex::new(HashMap::new()),
                failure: Mutex::new(Some(TreeFailure::Named(name, code))),
            }))
            .await
            .unwrap();
        let fallback = Arc::new(AtomicUsize::new(0));
        let c = fallback.clone();
        h.ctx
            .on_waterfall::<ApprovalRequest, Value, _>(move |_, _| {
                c.fetch_add(1, Ordering::SeqCst);
                async { Ok(json!("rejected")) }
            })
            .unwrap();
        assert_eq!(
            decide(&h, req(foreign.agent.clone())).await,
            ApprovalOutcome::Unavailable
        );
        assert_eq!(fallback.load(Ordering::SeqCst), 0);
        foreign.dispose().await.unwrap();
        h.close().await;
    }
}
#[tokio::test]
async fn does_not_claim_a_disposed_agent_when_an_owned_replacement_reuses_its_id() {
    let h = setup(Some(true)).await;
    let old = outside(&h, "reused", true).await;
    old.dispose().await.unwrap();
    let replacement = outside(&h, "reused", true).await;
    h.ctx
        .provide::<Subagents>(Arc::new(Tree {
            roots: Mutex::new([("reused".into(), h.agent("main"))].into_iter().collect()),
            failure: Mutex::new(None),
        }))
        .await
        .unwrap();
    let result = h
        .ctx
        .waterfall(req(old.agent.clone()), |_| async { Ok(json!("rejected")) })
        .await
        .unwrap();
    assert_eq!(result, "rejected");
    assert!(lock(&h.peer.requests).is_empty());
    replacement.dispose().await.unwrap();
    h.close().await;
}
#[tokio::test]
async fn rejects_an_invalid_wire_capability_flag() {
    let h = setup(None).await;
    assert_eq!(
        h.server
            .handle_request(
                "initialize",
                json!({"cwd":".","provider":"mock","model":"mock","approvalRequests":"true"})
            )
            .await
            .unwrap_err()
            .to_string(),
        "initialize approvalRequests must be a boolean"
    );
    h.close().await;
}

/// approval.spec.ts:110 on a real wire: the request frame reaches the client, the pending
/// approval settles, and the client's late grant frame changes nothing.
async fn pending_on_wire(action: &str) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let mut h = setup(Some(true)).await;
    h.server.shutdown().await.unwrap();
    let (a, b) = tokio::io::duplex(4096);
    let (r, w) = tokio::io::split(a);
    let wire = JsonRpcLineTransport::start(&h.ctx, BufReader::new(r), w).unwrap();
    h.server =
        HarnessSdkJsonRpcServer::new(h.ctx.clone(), Arc::new(wire.clone()), Default::default())
            .unwrap();
    let mut p = h.params();
    p.approval_requests = Some(true);
    h.server.initialize(p).await.unwrap();
    let agent = h.open("wire").await;
    agent
        .session()
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    let (client_in, mut client_out) = tokio::io::split(b);
    let mut client_in = BufReader::new(client_in);
    let control = AbortController::new();
    let mut request = req(agent.clone());
    request.signal = Some(control.signal());
    let pending = h.ctx.get::<Approval>().unwrap().request(request);
    let frame = loop {
        let mut line = String::new();
        client_in.read_line(&mut line).await.unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["method"] != "session.event" {
            break frame;
        }
    };
    assert_eq!(frame["method"], "approval/request");
    assert_eq!(frame["params"]["toolName"], "bash");
    match action {
        "cancel" => control.abort(),
        "shutdown" => {
            h.server.shutdown().await.unwrap();
        }
        _ => wire.close(),
    }
    let expected = if action == "cancel" {
        ApprovalOutcome::Cancelled
    } else {
        ApprovalOutcome::Unavailable
    };
    assert_eq!(pending.await.unwrap(), expected);
    let grant = json!({"jsonrpc":"2.0","id":frame["id"],"result":{"outcome":"allowed-once"}});
    client_out
        .write_all(format!("{grant}\n").as_bytes())
        .await
        .unwrap();
    client_out.flush().await.unwrap();
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    let decided: Vec<_> = agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|e| e.event_type() == "approval/decided")
        .map(|e| e.data()["outcome"].clone())
        .collect();
    assert_eq!(
        decided,
        [json!(if action == "cancel" {
            "cancelled"
        } else {
            "unavailable"
        })]
    );
    h.close().await;
}
#[tokio::test]
async fn settles_a_wire_approval_on_cancel_and_ignores_the_late_grant_frame() {
    pending_on_wire("cancel").await;
}
#[tokio::test]
async fn settles_a_wire_approval_on_shutdown_and_ignores_the_late_grant_frame() {
    pending_on_wire("shutdown").await;
}
#[tokio::test]
async fn settles_a_wire_approval_on_disconnect_and_ignores_the_late_grant_frame() {
    pending_on_wire("disconnect").await;
}
