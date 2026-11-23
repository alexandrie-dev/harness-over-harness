mod support;
use hoh_agent::{
    AgentError, AgentSessionMeta, AgentStatus, AgentStatusChanged, Agents, CreateAgentOptions,
};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::Event;
use hoh_llm::{
    GenerateOptions, Llm, LlmAdapter, LlmFuture, LlmModelInfo, LlmModelReasoningInfo,
    LlmReasoningEffortInfo, LlmResolvedModelInfo, LlmStream, ReasoningEffortId,
};
use hoh_protocol::{InitializeParams, SessionWaitParams};
use hoh_sdk_jsonrpc_server::{
    HarnessSdkJsonRpcServer, HarnessSdkJsonRpcServerOptions, integration::SubagentRunEndInfo,
};
use hoh_session::{CreateSessionOptions, SessionCreated, Sessions};
use hoh_timeout::AbortSignal;
use serde_json::{Value, json};
use std::sync::Arc;
use support::*;

#[tokio::test]
async fn queues_overlapping_prompts_for_one_session_without_blocking_other_sessions() {
    let h = Env::new().await;
    h.initialize().await;
    let (a, b, c) = tokio::join!(
        h.prompt("main", "first"),
        h.prompt("main", "overlap"),
        h.prompt("other", "independent")
    );
    assert_ne!(a["messageId"], b["messageId"]);
    assert!(c["messageId"].is_string());
    h.wait("main").await;
    h.wait("other").await;
    assert_eq!(h.ctx.get::<Agents>().unwrap().list().len(), 2);
    assert_eq!(
        types(&h.agent("main"))
            .iter()
            .filter(|t| t.as_str() == "turn/end")
            .count(),
        2
    );
    h.close().await;
}
#[tokio::test]
async fn rejects_a_prompt_for_a_session_whose_agent_was_disposed_outside_the_server() {
    let h = Env::new().await;
    h.initialize().await;
    h.prompt("zombie", "live").await;
    h.wait("zombie").await;
    h.loop_fiber.dispose().await;
    let e = h
        .server
        .handle_request(
            "session/prompt",
            json!({"sessionId":"zombie","contentBlocks":[{"type":"text","text":"after"}]}),
        )
        .await
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "session agent was disposed outside the server: zombie"
    );
    h.close().await;
}
#[tokio::test]
async fn forwards_whole_agent_status_without_attributing_a_turn_outcome() {
    let h = Env::new().await;
    h.initialize().await;
    let a = h.open("status").await;
    for status in [AgentStatus::Running, AgentStatus::Idle] {
        h.ctx
            .dispatch(AgentStatusChanged {
                agent: a.clone(),
                status,
            })
            .invoke(|_| panic!("observer"));
    }
    assert_eq!(
        h.peer.events("session.status"),
        vec![
            json!({"sessionId":"status","status":"running"}),
            json!({"sessionId":"status","status":"idle"})
        ]
    );
    h.close().await;
}
#[tokio::test]
async fn notifies_the_host_only_for_subagent_sessions_with_a_direct_creator() {
    let h = Env::new().await;
    let sessions = h.ctx.get::<Sessions>().unwrap();
    for (id, meta) in [
        ("root", json!({})),
        ("child", json!({"origin":"subagent","parentSession":"main"})),
        ("legacy", json!({"parentSession":"main"})),
        ("unparented", json!({"origin":"subagent"})),
    ] {
        sessions
            .create(
                &h.ctx,
                Some(id.into()),
                CreateSessionOptions {
                    meta: Some(meta),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    assert_eq!(
        h.peer.events("subagent.started"),
        vec![json!({"parentSessionId":"main","childSessionId":"child"})]
    );
    h.close().await;
}
fn end(
    h: &Env,
    provider: &str,
    id: &str,
    parent: &str,
    local: bool,
    reason: &str,
    text: Option<&str>,
) {
    h.ctx
        .dispatch(SubagentRunEndInfo {
            provider: provider.into(),
            id: id.into(),
            parent_session_id: parent.into(),
            local,
            stop_reason: reason.into(),
            last_assistant_message: text.map(|t| vec![hoh_llm::ContentBlock::text(t)]),
        })
        .invoke(|_| panic!("observer"));
}
#[tokio::test]
async fn notifies_the_host_when_a_subagent_run_settles() {
    let h = Env::new().await;
    end(
        &h,
        "spawn",
        "child",
        "main",
        true,
        "completed",
        Some("child done"),
    );
    end(&h, "spawn", "parentless", "main", true, "error", None);
    assert_eq!(
        h.peer.events("subagent.finished"),
        vec![
            json!({"provider":"spawn","agentId":"child","parentSessionId":"main","childSessionId":"child","status":"ok","stopReason":"completed","lastAssistantMessage":[{"type":"text","text":"child done"}]}),
            json!({"provider":"spawn","agentId":"parentless","parentSessionId":"main","childSessionId":"parentless","status":"error","stopReason":"error"})
        ]
    );
    h.close().await;
}
#[tokio::test]
async fn reports_a_nested_child_creator_after_disposal_when_lifecycle_dispatch_belongs_to_the_team_root()
 {
    let h = Env::new().await;
    h.initialize().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let creator = agents
        .create(
            &h.ctx,
            CreateAgentOptions {
                session_id: Some("creator".into()),
                meta: Some(AgentSessionMeta {
                    origin: Some("subagent".into()),
                    parent_session: Some("root".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let child = agents
        .create(
            &h.ctx,
            CreateAgentOptions {
                session_id: Some("leaf".into()),
                meta: Some(AgentSessionMeta {
                    origin: Some("subagent".into()),
                    parent_session: Some("creator".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    creator.dispose().await.unwrap();
    child.dispose().await.unwrap();
    end(&h, "spawn", "leaf", "creator", true, "completed", None);
    assert_eq!(
        h.peer.events("subagent.finished")[0]["parentSessionId"],
        "creator"
    );
    assert_eq!(
        h.peer.events("subagent.started")[1]["parentSessionId"],
        "creator"
    );
    h.close().await;
}
#[tokio::test]
async fn ignores_a_remote_run_id_that_collides_with_a_local_child_of_the_same_parent() {
    let h = Env::new().await;
    h.initialize().await;
    h.open("remote-run-id").await;
    end(
        &h,
        "remote",
        "remote-run-id",
        "main",
        false,
        "completed",
        Some("remote"),
    );
    assert!(h.peer.events("subagent.finished").is_empty());
    h.close().await;
}
#[tokio::test]
async fn retains_locality_across_continuation_runs_on_one_live_child() {
    let h = Env::new().await;
    for text in ["first", "second"] {
        end(
            &h,
            "continuation",
            "child",
            "parent",
            true,
            "completed",
            Some(text),
        );
    }
    assert_eq!(h.peer.events("subagent.finished").len(), 2);
    h.close().await;
}
#[tokio::test]
async fn correlates_reused_local_ids_by_parent_scope_when_runs_settle_out_of_order() {
    let h = Env::new().await;
    for (p, t) in [
        ("old-parent", "same lifetime"),
        ("new-parent", "new lifetime"),
        ("old-parent", "old lifetime"),
    ] {
        end(&h, "reused", "reused-child", p, true, "completed", Some(t));
    }
    let events = h.peer.events("subagent.finished");
    assert_eq!(
        events
            .iter()
            .map(|e| e["parentSessionId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["old-parent", "new-parent", "old-parent"]
    );
    assert_eq!(
        events
            .iter()
            .map(|e| e["lastAssistantMessage"][0]["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["same lifetime", "new lifetime", "old lifetime"]
    );
    h.close().await;
}
#[tokio::test]
async fn keeps_locality_bound_to_the_accepted_run_across_provider_reregistration() {
    let h = Env::new().await;
    end(
        &h,
        "reused-provider",
        "child",
        "parent",
        false,
        "completed",
        Some("remote"),
    );
    end(
        &h,
        "reused-provider",
        "child",
        "parent",
        true,
        "completed",
        Some("local"),
    );
    assert_eq!(h.peer.events("subagent.finished").len(), 1);
    assert_eq!(
        h.peer.events("subagent.finished")[0]["lastAssistantMessage"][0]["text"],
        "local"
    );
    h.close().await;
}
#[tokio::test]
async fn uses_the_recorded_local_flag_when_start_was_missed_and_ignores_remote_runs() {
    let h = Env::new().await;
    let peer = Arc::new(Peer::default());
    let server = HarnessSdkJsonRpcServer::new(
        h.ctx.clone(),
        peer.clone(),
        HarnessSdkJsonRpcServerOptions {
            max_tokens_as_success: true,
        },
    )
    .unwrap();
    end(&h, "fork", "child", "parent", true, "max-tokens", None);
    end(&h, "fork", "missing", "parent", false, "error", None);
    assert_eq!(
        peer.events("subagent.finished")[0],
        json!({"provider":"fork","agentId":"child","parentSessionId":"parent","childSessionId":"child","status":"ok","stopReason":"max-tokens"})
    );
    assert_eq!(peer.events("subagent.finished").len(), 1);
    server.shutdown().await.unwrap();
    h.close().await;
}
#[tokio::test]
async fn does_not_reregister_an_llm_adapter_whose_provider_already_has_an_owner() {
    let h = Env::new().await;
    h.ctx
        .get::<Llm>()
        .unwrap()
        .register_adapter(&h.ctx, vec!["deepseek-official".into()], h.model.clone())
        .unwrap();
    let mut params = h.params();
    params.provider = "deepseek-official".into();
    h.server.initialize(params).await.unwrap();
    assert_eq!(
        h.ctx
            .get::<Llm>()
            .unwrap()
            .list_providers()
            .iter()
            .filter(|p| p.id == "deepseek-official")
            .count(),
        1
    );
    h.close().await;
}
#[tokio::test]
async fn rejects_a_missing_non_deepseek_provider_when_an_llm_service_already_exists() {
    let h = Env::new().await;
    let mut p = h.params();
    p.provider = "private".into();
    assert_eq!(
        h.server.initialize(p).await.unwrap_err().to_string(),
        "no adapter registered for provider \"private\""
    );
    assert_eq!(h.ctx.get::<Llm>().unwrap().list_providers().len(), 1);
    h.close().await;
}
#[tokio::test]
async fn rejects_invalid_initialize_max_tokens_at_the_wire_boundary() {
    let h = Env::new().await;
    for n in [
        json!(0),
        json!(-1),
        json!(1.5),
        Value::Null,
        json!(9_007_199_254_740_992_u64),
    ] {
        let mut params = serde_json::to_value(h.params()).unwrap();
        params["maxTokens"] = n;
        assert_eq!(
            h.server
                .handle_request("initialize", params)
                .await
                .unwrap_err()
                .to_string(),
            "initialize maxTokens must be a positive safe integer"
        );
    }
    h.close().await;
}
#[tokio::test]
async fn rejects_malformed_initialize_reasoning_effort_values_at_the_wire_boundary() {
    let h = Env::new().await;
    for effort in [json!(""), json!(42)] {
        let mut p = serde_json::to_value(h.params()).unwrap();
        p["reasoningEffort"] = effort;
        assert_eq!(
            h.server
                .handle_request("initialize", p)
                .await
                .unwrap_err()
                .to_string(),
            "initialize reasoningEffort must be a non-empty string"
        );
    }
    h.close().await;
}
struct Rejecting;
impl LlmAdapter for Rejecting {
    fn resolve_model<'a>(
        &'a self,
        p: &'a str,
        m: &'a str,
        _: Option<&'a AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        Box::pin(async move { Err(format!("model unavailable: {p}/{m}").into()) })
    }
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        panic!("must not stream")
    }
}
#[tokio::test]
async fn rejects_an_unavailable_exact_model_during_initialize() {
    let h = Env::new().await;
    h.ctx
        .get::<Llm>()
        .unwrap()
        .register_adapter(&h.ctx, vec!["private".into()], Arc::new(Rejecting))
        .unwrap();
    let mut p = h.params();
    p.provider = "private".into();
    p.model = "missing".into();
    assert_eq!(
        h.server.initialize(p).await.unwrap_err().to_string(),
        "model unavailable: private/missing"
    );
    assert!(
        h.query("invalid", "not admitted")
            .await
            .unwrap_err()
            .to_string()
            .contains("not initialized")
    );
    assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    h.close().await;
}

#[tokio::test]
async fn checks_prompt_initialization_before_validating_its_wire_shape() {
    let h = Env::new().await;
    for params in [Value::Null, json!({}), json!({"contentBlocks":[]})] {
        assert_eq!(
            h.server
                .handle_request("session/prompt", params)
                .await
                .unwrap_err()
                .to_string(),
            "SDK server is not initialized"
        );
    }
    assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    h.initialize().await;
    assert_eq!(
        h.server
            .handle_request("session/prompt", json!({}))
            .await
            .unwrap_err()
            .to_string(),
        "session/prompt requires a sessionId string"
    );
    h.close().await;
}

#[tokio::test]
async fn reproduces_the_node_path_resolve_error_for_a_missing_cwd() {
    let h = Env::new().await;
    assert_eq!(
        h.server
            .handle_request(
                "initialize",
                json!({"provider":"mock","model":"apply-model"}),
            )
            .await
            .unwrap_err()
            .to_string(),
        "The \"paths[0]\" argument must be of type string. Received undefined"
    );
    h.close().await;
}

#[tokio::test]
async fn rejects_unsupported_prompt_image_types_at_the_strict_sdk_wire_boundary() {
    let h = Env::new().await;
    h.initialize().await;
    support::images::Store::mount(&h).await;
    let error = h
        .server
        .handle_request(
            "session/prompt",
            json!({"sessionId":"invalid-image-type","contentBlocks":[{
                "type":"image","data":"AQ==","mimeType":"image/x-invalid"
            }]}),
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("data did not match any variant of untagged enum SdkPromptContentBlock")
    );
    assert!(hoh_error::HarnessError::of(error.as_ref()).is_none());
    assert!(lock(&h.model.requests).is_empty());
    h.close().await;
}

#[tokio::test]
async fn replays_the_configured_route_and_multiple_sdk_prompts_without_a_live_provider() {
    let h = Env::new().await;
    let _system = h
        .ctx
        .get::<hoh_system_prompt::SystemPromptKey>()
        .unwrap()
        .section(
            &h.ctx,
            hoh_system_prompt::PromptSection {
                name: "sdk-live-provider-replay".into(),
                order: 0.0,
                text: "You are helpful.".into(),
                ..Default::default()
            },
        )
        .unwrap();
    *lock(&h.model.reasoning) = Some(LlmModelReasoningInfo {
        efforts: vec![LlmReasoningEffortInfo {
            id: ReasoningEffortId::from("max"),
            name: "Maximum".into(),
            description: None,
        }],
        default_effort: None,
    });
    let mut params = h.params();
    params.model = "configured-model".into();
    params.reasoning_effort = Some("max".into());
    params.max_tokens = Some(serde_json::from_value(json!(321)).unwrap());
    h.server.initialize(params).await.unwrap();

    for text in ["fix it", "again"] {
        h.prompt("configured", text).await;
        h.wait("configured").await;
    }
    {
        let requests = lock(&h.model.requests);
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| {
            request.provider == "mock"
                && request.model == "configured-model"
                && request.reasoning_effort.as_ref().map(|v| v.as_str()) == Some("max")
                && request.max_tokens == Some(321)
        }));
        let first_messages = hoh_util_values::js_json_stringify(
            &serde_json::to_value(&requests[0].messages).unwrap(),
        );
        assert!(first_messages.contains("You are helpful."));
        assert!(first_messages.contains("fix it"));
        assert!(
            hoh_util_values::js_json_stringify(
                &serde_json::to_value(&requests[1].messages).unwrap()
            )
            .contains("again")
        );
    }
    assert!(h.peer.events("session.event").len() > 1);
    assert_eq!(
        h.peer.events("session.status").last().unwrap()["status"],
        "idle"
    );
    h.close().await;
}
struct Pending {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Semaphore>,
}
impl LlmAdapter for Pending {
    fn resolve_model<'a>(
        &'a self,
        p: &'a str,
        m: &'a str,
        _: Option<&'a AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            Ok(LlmResolvedModelInfo {
                model: LlmModelInfo {
                    provider: p.into(),
                    id: m.into(),
                    name: m.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
        })
    }
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        panic!("must not stream")
    }
}
#[tokio::test]
async fn rejects_prompts_while_exact_route_initialization_is_pending() {
    let h = Env::new().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    h.ctx
        .get::<Llm>()
        .unwrap()
        .register_adapter(
            &h.ctx,
            vec!["private".into()],
            Arc::new(Pending {
                entered: entered.clone(),
                release: release.clone(),
            }),
        )
        .unwrap();
    let mut p = h.params();
    p.provider = "private".into();
    let server = h.server.clone();
    let init = tokio::spawn(async move { server.initialize(p).await });
    entered.notified().await;
    assert!(
        h.query("too-early", "no")
            .await
            .unwrap_err()
            .to_string()
            .contains("not initialized")
    );
    assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    release.add_permits(1);
    init.await.unwrap().unwrap();
    h.close().await;
}
#[tokio::test]
async fn rejects_an_unsupported_reasoning_effort_during_initialize() {
    let h = Env::new().await;
    let mut p = h.params();
    p.reasoning_effort = Some("impossible".into());
    assert!(
        h.server
            .initialize(p)
            .await
            .unwrap_err()
            .to_string()
            .contains("does not support reasoning effort \"impossible\"")
    );
    h.close().await;
}

#[tokio::test]
async fn uses_the_initialized_cwd_as_the_fallback_for_sessions_without_an_origin() {
    let h = Env::new().await;
    let child = h.dir.path().join("initialized-project");
    tokio::fs::create_dir(&child).await.unwrap();
    let mut params = h.params();
    params.cwd = child.to_string_lossy().into_owned();
    h.server.initialize(params).await.unwrap();
    assert_eq!(
        h.ctx
            .get::<hoh_working_directory::WorkingDirectoryKey>()
            .unwrap()
            .default_directory(),
        child.to_string_lossy()
    );
    h.open("no-origin").await;
    assert_eq!(
        h.ctx
            .get::<hoh_working_directory::WorkingDirectoryKey>()
            .unwrap()
            .get(h.agent("no-origin").session()),
        child.to_string_lossy()
    );
    h.close().await;
}

#[tokio::test]
async fn reports_no_adapter_when_the_llm_service_is_absent() {
    let k = hoh_kernel::Kernel::start();
    let server =
        HarnessSdkJsonRpcServer::new(k.root(), Arc::new(Peer::default()), Default::default())
            .unwrap();
    let p: InitializeParams =
        serde_json::from_value(json!({"cwd":".","provider":"missing","model":"m"})).unwrap();
    assert_eq!(
        server.initialize(p).await.unwrap_err().to_string(),
        "no adapter registered for provider \"missing\""
    );
    server.shutdown().await.unwrap();
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_unknown_jsonrpc_runtime_methods() {
    let h = Env::new().await;
    assert_eq!(
        h.server
            .handle_request("does/not/exist", json!({}))
            .await
            .unwrap_err()
            .to_string(),
        "unknown DeepSeek Harness SDK runtime method: does/not/exist"
    );
    h.close().await;
}
#[tokio::test]
async fn coalesces_concurrent_session_creation_and_retries_a_failed_creation() {
    let h = Env::new().await;
    h.initialize().await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (g, e, c) = (gate.clone(), entered.clone(), count.clone());
    let off = h
        .ctx
        .on_serial::<hoh_agent::AgentCreated, (), _>(move |event| {
            let (g, e, c) = (g.clone(), e.clone(), c.clone());
            async move {
                if event.agent.id().as_str() == "shared" {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    e.notify_one();
                    g.acquire().await.unwrap().forget();
                }
                Ok(None)
            }
        })
        .unwrap();
    let server = h.server.clone();
    let one = tokio::spawn(async move {
        server
            .handle_request(
                "session/working-directory/get",
                json!({"sessionId":"shared"}),
            )
            .await
    });
    entered.notified().await;
    let server = h.server.clone();
    let two = tokio::spawn(async move {
        server
            .handle_request(
                "session/working-directory/get",
                json!({"sessionId":"shared"}),
            )
            .await
    });
    tokio::task::yield_now().await;
    gate.add_permits(1);
    one.await.unwrap().unwrap();
    two.await.unwrap().unwrap();
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    off.dispose().await;
    let fail = h
        .ctx
        .on_dispatch(move |_, name, payload| {
            if name == SessionCreated::NAME
                && let Some(event) = payload.downcast_ref::<SessionCreated>()
                && event.session.id().as_str() == "retry"
            {
                event.validation.reject("creation failed".into());
            }
        })
        .unwrap();
    assert!(
        h.server
            .handle_request(
                "session/working-directory/get",
                json!({"sessionId":"retry"})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("creation failed")
    );
    fail.dispose().await;
    h.open("retry").await;
    h.close().await;
}
#[tokio::test]
async fn resolves_a_relative_cwd_before_creating_the_session() {
    let h = Env::new().await;
    let mut p = h.params();
    p.cwd = ".".into();
    p.max_tokens = Some(serde_json::from_value(json!(123)).unwrap());
    h.server.initialize(p).await.unwrap();
    h.prompt("relative", "test").await;
    h.wait("relative").await;
    assert_eq!(
        h.agent("relative").session().header()["cwd"],
        std::env::current_dir().unwrap().to_str().unwrap()
    );
    assert_eq!(lock(&h.model.requests)[0].max_tokens, Some(123));
    h.close().await;
}
#[tokio::test]
async fn rejects_inline_sdk_images_when_the_composition_has_no_attachment_store() {
    let h = Env::new().await;
    h.initialize().await;
    let error=h.server.handle_request("session/prompt",json!({"sessionId":"image","contentBlocks":[{"type":"image","data":"AQ==","mimeType":"image/png"}]})).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "SDK image prompt requires an attachment store"
    );
    assert!(lock(&h.model.requests).is_empty());
    h.close().await;
}
#[tokio::test]
async fn changes_effective_directories_through_rpc_while_preserving_session_origins_and_replayable_context()
 {
    let h = Env::new().await;
    let child = h.dir.path().join("child");
    tokio::fs::create_dir(&child).await.unwrap();
    assert!(
        h.server
            .handle_request("session/working-directory/get", json!({"sessionId":"a"}))
            .await
            .unwrap_err()
            .to_string()
            .contains("not initialized")
    );
    h.initialize().await;
    h.open("a").await;
    assert_eq!(
        h.server
            .handle_request(
                "session/working-directory/set",
                json!({"sessionId":"a","path":"child"})
            )
            .await
            .unwrap(),
        json!({"cwd":child})
    );
    assert_eq!(
        h.agent("a").session().header()["cwd"],
        h.dir.path().to_str().unwrap()
    );
    h.open("b").await;
    assert_eq!(
        h.ctx
            .get::<hoh_working_directory::WorkingDirectoryKey>()
            .unwrap()
            .get(h.agent("b").session()),
        h.dir.path().to_str().unwrap()
    );
    h.prompt("a", "where").await;
    h.wait("a").await;
    assert!(
        hoh_util_values::js_json_stringify(
            &serde_json::to_value(&lock(&h.model.requests)[0].messages).unwrap()
        )
        .contains(child.to_str().unwrap())
    );
    assert!(
        h.server
            .handle_request("session/working-directory/get", json!({}))
            .await
            .is_err()
    );
    assert!(
        h.server
            .handle_request(
                "session/working-directory/set",
                json!({"sessionId":"a","path":3})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("path string")
    );
    h.close().await;
}
#[tokio::test]
async fn wait_rejects_unknown_without_creating_session() {
    let h = Env::new().await;
    assert!(
        h.server
            .wait(SessionWaitParams {
                session_id: "missing".into()
            })
            .await
            .unwrap_err()
            .to_string()
            .contains("not initialized")
    );
    h.initialize().await;
    assert!(
        h.server
            .wait(SessionWaitParams {
                session_id: "missing".into()
            })
            .await
            .unwrap_err()
            .to_string()
            .contains("unknown SDK session")
    );
    assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    h.close().await;
}
#[tokio::test]
async fn wait_retains_live_failure_and_clears_only_on_later_durable_terminal() {
    let h = Env::new().await;
    h.initialize().await;
    h.prompt("a", "first").await;
    h.wait("a").await;
    let agent = h.agent("a");
    let sentinel = SharedError::new("live failure");
    h.ctx
        .dispatch(AgentError {
            agent: agent.clone(),
            turn: 1,
            step: 1,
            error: sentinel.clone(),
        })
        .invoke(|_| panic!("observer"));
    let e = h
        .server
        .wait(SessionWaitParams {
            session_id: "a".into(),
        })
        .await
        .unwrap_err();
    assert!(e.downcast_ref::<SharedError>().unwrap().ptr_eq(&sentinel));
    h.prompt("a", "recovery").await;
    h.wait("a").await;
    h.close().await;
}
#[tokio::test]
async fn admits_sdk_images_before_prompt_query_image_only_or_whitespace_message_enters_session() {
    for entry in ["prompt", "query", "image-only", "whitespace"] {
        let h = Env::new().await;
        h.initialize().await;
        let store = support::images::Store::mount(&h).await;
        let text = if entry == "whitespace" {
            "  "
        } else {
            "inspect"
        };
        let mut blocks = Vec::new();
        if entry != "image-only" {
            blocks.push(json!({"type":"text","text":text}));
        }
        blocks.push(
            json!({"type":"image","data":"AQ==","mimeType":"image/png","name":"diagram.png"}),
        );
        let response = h
            .server
            .handle_request(
                if entry == "prompt" {
                    "session/prompt"
                } else {
                    "session/query"
                },
                json!({"sessionId":"image","contentBlocks":blocks}),
            )
            .await
            .unwrap();
        assert!(response["messageId"].is_string());
        h.wait("image").await;
        {
            let saved = lock(&store.saved);
            assert_eq!(saved[0][0].data, vec![1]);
            assert_eq!(saved[0][0].name.as_deref(), Some("diagram.png"));
        }
        let a = h.agent("image");
        let events = a.session().snapshot_events(None, None);
        let message = events
            .iter()
            .find(|e| e.event_type() == "user/message" && e.data()["source"]["kind"] == "user")
            .unwrap();
        let content = message.data()["content"].as_array().unwrap();
        assert_eq!(content.last().unwrap()["type"], "image");
        assert_eq!(
            content.last().unwrap()["attachment"]["attachmentId"],
            "sha256:image-0"
        );
        assert!(content.last().unwrap().get("data").is_none());
        if entry != "image-only" {
            assert_eq!(content[0]["text"], text);
        }
        h.close().await;
    }
}
#[tokio::test]
async fn rechecks_agent_liveness_after_asynchronous_image_admission() {
    let h = Env::new().await;
    h.initialize().await;
    let store = support::images::Store::mount(&h).await;
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    *lock(&store.gate) = Some(release.clone());
    let server = h.server.clone();
    let request = tokio::spawn(async move {
        server.handle_request("session/prompt",json!({"sessionId":"image-race","contentBlocks":[{"type":"image","data":"AQ==","mimeType":"image/png"}]})).await
    });
    store.entered.notified().await;
    h.loop_fiber.dispose().await;
    release.add_permits(1);
    assert_eq!(
        request.await.unwrap().unwrap_err().to_string(),
        "session agent was disposed outside the server: image-race"
    );
    assert!(lock(&h.model.requests).is_empty());
    h.close().await;
}
#[tokio::test]
async fn creates_an_sdk_session_without_an_optional_system_prompt() {
    let h = Env::new().await;
    h.initialize().await;
    h.prompt("plain", "hello").await;
    h.wait("plain").await;
    assert_eq!(lock(&h.model.requests).len(), 1);
    h.close().await;
}

/// server.ts:233-234 names `${params.provider}` for any provider that is not a registered id.
#[tokio::test]
async fn names_a_missing_or_non_string_provider_like_the_source() {
    let h = Env::new().await;
    for (provider, shown) in [
        (None, "undefined"),
        (Some(json!(5)), "5"),
        (Some(json!(null)), "null"),
        (Some(json!(true)), "true"),
        (Some(json!(["a", "b"])), "a,b"),
        (Some(json!({"x": 1})), "[object Object]"),
        (Some(json!("unregistered")), "unregistered"),
    ] {
        let mut params = json!({"cwd": ".", "model": "m"});
        if let Some(provider) = provider {
            params["provider"] = provider;
        }
        let error = h
            .server
            .handle_request("initialize", params)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("no adapter registered for provider \"{shown}\"")
        );
    }
    // none of these attempts counted as the one initialize
    h.initialize().await;
    h.close().await;
}

#[tokio::test]
async fn reads_a_non_object_or_non_boolean_user_questions_capability_as_disabled() {
    let h = Env::new().await;
    for capabilities in [Value::Null, json!({"userQuestions": "yes"})] {
        let h = Env::new().await;
        let mut params = serde_json::to_value(h.params()).unwrap();
        params["capabilities"] = capabilities.clone();
        let result = h.server.handle_request("initialize", params).await;
        assert!(result.is_ok(), "{capabilities}: {:?}", result.err());
        h.close().await;
    }
    h.close().await;
}
