use std::sync::{Arc, Mutex};

use hoh_deepseek_llm_api_extensions::{
    DeepSeekLlmApiExtensionRegistry, DeepSeekLlmApiExtensionRequest, DeepSeekLlmApiExtensions,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Fiber, Inject, Kernel, NoConfig, Plugin, Service};
use hoh_session::{
    CreateSessionOptions, Session, SessionEvent, SessionId, SessionLogOffset, SessionPlugin,
    SessionSeq, SessionSeqCursor, Sessions, SurfaceIntent,
};
use hoh_session_log_deepseek::{
    Config, DeepSeekSessionLogExtension, SessionLogDeepSeek, SessionLogDeepSeekPlugin,
    accepted_through,
};
use hoh_timeout::AbortController;
use serde_json::{Value, json};

struct Env {
    kernel: Kernel,
    ctx: Context,
    sessions: hoh_session::SessionStore,
    extensions: Service<DeepSeekLlmApiExtensions>,
    upload: Fiber,
}

#[derive(Inject)]
struct ExclusionDeps {
    upload: Service<SessionLogDeepSeek>,
}

struct ExclusionConsumer;

impl Plugin for ExclusionConsumer {
    const NAME: &'static str = "SessionLogExclusionConsumer";
    type Config = NoConfig;
    type Deps = ExclusionDeps;

    async fn apply(ctx: Context, deps: ExclusionDeps, _: Cfg<NoConfig>) -> Result<(), BoxError> {
        deps.upload.register_exclusion(&ctx, |request| {
            Ok(request.purpose.as_deref() == Some("private"))
        })?;
        Ok(())
    }
}

impl Env {
    async fn new(config: Value) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let sessions = ctx.plugin::<SessionPlugin>(Value::Null).unwrap();
        sessions.settled().await.unwrap();
        let extensions = ctx
            .plugin::<DeepSeekLlmApiExtensionRegistry>(Value::Null)
            .unwrap();
        extensions.settled().await.unwrap();
        let upload = ctx.plugin::<SessionLogDeepSeekPlugin>(config).unwrap();
        upload.settled().await.unwrap();
        Self {
            sessions: (*ctx.get::<Sessions>().unwrap()).clone(),
            extensions: ctx.get::<DeepSeekLlmApiExtensions>().unwrap(),
            kernel,
            ctx,
            upload,
        }
    }

    fn session(&self, id: &str) -> Session {
        self.sessions
            .create(&self.ctx, Some(id.into()), CreateSessionOptions::default())
            .unwrap()
    }

    async fn prepare(
        &self,
        id: Option<&SessionId>,
        purpose: Option<&str>,
    ) -> hoh_deepseek_llm_api_extensions::PreparedDeepSeekLlmApiExtensions {
        self.extensions
            .prepare(DeepSeekLlmApiExtensionRequest {
                body: Default::default(),
                session_id: id.map(ToString::to_string),
                purpose: purpose.map(str::to_owned),
                signal: AbortController::new().signal(),
            })
            .await
            .unwrap()
    }

    async fn field(&self, id: &SessionId) -> Option<DeepSeekSessionLogExtension> {
        self.prepare(Some(id), None)
            .await
            .fields()
            .get("dsh_session_log")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .unwrap()
    }

    async fn shutdown(self) {
        self.kernel.shutdown().await;
    }
}

fn turn(session: &Session, number: u64) {
    session
        .append("turn/start", json!({"turn": number}), None)
        .unwrap();
}

fn user(session: &Session, text: &str) {
    session
        .append(
            "user/message",
            json!({
                "content": [{"type":"text","text":text}],
                "source": {"kind":"user"},
                "role": "user",
                "id": "message",
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
}

fn message(role: &str, text: &str) -> Value {
    let source = match role {
        "system" => json!({"kind":"system-prompt"}),
        "tool" => json!({"kind":"tool","callId":"call"}),
        _ => json!({"kind":"user"}),
    };
    let mut message = json!({
        "id":format!("{role}-{text}"),
        "role":role,
        "content":[{"type":"text","text":text}],
        "source":source,
    });
    if role == "tool" {
        message["toolCallId"] = json!("call");
        message["isError"] = json!(false);
    }
    message
}

fn message_data(role: &str, text: &str) -> Value {
    if role == "user" {
        message(role, text)
    } else {
        json!({"turn":1,"step":1,"message":message(role,text)})
    }
}

fn raw_event(seq: u64, event_type: &str, data: Value) -> Arc<SessionEvent> {
    Arc::new(SessionEvent::from_value(json!({
        "type": event_type,
        "seq": seq,
        "time": seq + 1,
        "data": data,
    })))
}

fn cursor(value: SessionSeqCursor) -> i64 {
    match value {
        SessionSeqCursor::BeforeFirst => -1,
        SessionSeqCursor::Seq(value) => u64::from(value) as i64,
    }
}

#[test]
fn config_defaults_are_environment_independent() {
    let config: Config = serde_json::from_value(json!({})).unwrap();
    assert!(*config.enabled);
    assert_eq!(config.max_bytes, (8 * 1024 * 1024) as f64);
    let disabled: Config = serde_json::from_value(json!({"enabled":false})).unwrap();
    assert!(!*disabled.enabled);
    let minimum: Config = serde_json::from_value(json!({"maxBytes":1})).unwrap();
    assert_eq!(minimum.max_bytes, 1.0);
}

#[tokio::test]
async fn omits_direct_stale_empty_and_disabled_requests() {
    let env = Env::new(json!({"enabled":true})).await;
    let session = env.session("edges");
    assert!(env.prepare(None, None).await.fields().is_empty());
    let missing: SessionId = "missing".into();
    assert!(env.prepare(Some(&missing), None).await.fields().is_empty());
    assert!(env.field(session.id()).await.is_none());
    env.shutdown().await;

    let env = Env::new(json!({"enabled":false})).await;
    let session = env.session("disabled");
    turn(&session, 1);
    assert!(env.field(session.id()).await.is_none());
    env.shutdown().await;
}

#[tokio::test]
async fn uploads_full_prefix_accepts_and_then_uploads_suffix() {
    let env = Env::new(json!({"enabled":true})).await;
    let session = env.session("incremental");
    turn(&session, 1);
    session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    let first = env.prepare(Some(session.id()), None).await;
    let field: DeepSeekSessionLogExtension =
        serde_json::from_value(first.fields()["dsh_session_log"].clone()).unwrap();
    assert_eq!(
        (field.after_seq, field.through_seq, field.events.len()),
        (-1, 1, 2)
    );
    first.accept().await.unwrap();
    assert_eq!(cursor(accepted_through(&session).unwrap()), 1);
    assert_eq!(
        session.snapshot_events(None, None).last().unwrap().data(),
        &json!({
            "sessionId":session.id(),
            "sessionFormatVersion":4,
            "throughSeq":1,
        })
    );
    session
        .append("step/end", json!({"turn":1,"step":1}), None)
        .unwrap();
    let second = env.field(session.id()).await.unwrap();
    assert_eq!((second.after_seq, second.through_seq), (1, 3));
    assert_eq!(
        second.events[0].r#type,
        "session-log-deepseek/delivery-accepted"
    );
    env.shutdown().await;
}

#[tokio::test]
async fn acceptance_is_idempotent_through_the_registry() {
    let env = Env::new(json!({})).await;
    let session = env.session("idempotent");
    turn(&session, 1);
    let prepared = env.prepare(Some(session.id()), None).await;
    prepared.accept().await.unwrap();
    prepared.accept().await.unwrap();
    assert_eq!(session.seq().get(), 2);
    env.shutdown().await;
}

#[tokio::test]
async fn takes_maximum_of_out_of_order_acceptances() {
    let env = Env::new(json!({})).await;
    let session = env.session("concurrent");
    turn(&session, 1);
    let early = env.prepare(Some(session.id()), None).await;
    session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    let late = env.prepare(Some(session.id()), None).await;
    late.accept().await.unwrap();
    early.accept().await.unwrap();
    assert_eq!(cursor(accepted_through(&session).unwrap()), 1);
    env.shutdown().await;
}

#[tokio::test]
async fn reconstructs_persisted_cursor_and_ignores_parent_cursor_in_child() {
    let env = Env::new(json!({})).await;
    let parent = env.session("parent");
    turn(&parent, 1);
    let prepared = env.prepare(Some(parent.id()), None).await;
    prepared.accept().await.unwrap();
    let seed = parent.snapshot_events(None, None).to_vec();

    let restored = Session::from_restore(
        "parent".into(),
        &seed,
        (*parent.header()).clone(),
        SessionLogOffset::new(0).unwrap(),
        hoh_session::SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert_eq!(cursor(accepted_through(&restored).unwrap()), 0);
    let resumed_env = Env::new(json!({})).await;
    let _resumed_detach = resumed_env
        .sessions
        .enter(&resumed_env.ctx, restored.clone())
        .unwrap();
    assert_eq!(resumed_env.field(restored.id()).await.unwrap().after_seq, 0);

    let child = Session::from_restore(
        "child".into(),
        &seed,
        json!({
            "version":4,"id":"child","createdAt":1,"isSeeded":true,
            "parentSession":"parent"
        }),
        SessionLogOffset::new(seed.len() as u64).unwrap(),
        hoh_session::SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert_eq!(
        accepted_through(&child).unwrap(),
        SessionSeqCursor::BeforeFirst
    );
    let _child_detach = env.sessions.enter(&env.ctx, child.clone()).unwrap();
    let child_field = env.field(child.id()).await.unwrap();
    assert_eq!(child_field.after_seq, -1);
    assert_eq!(child_field.through_seq, child.seq().get() - 1);
    resumed_env.shutdown().await;
    env.shutdown().await;
}

#[tokio::test]
async fn uploads_all_request_purposes_by_default() {
    let env = Env::new(json!({})).await;
    for (index, purpose) in [
        None,
        Some("compaction"),
        Some("session-title"),
        Some("auto-review"),
    ]
    .into_iter()
    .enumerate()
    {
        let session = env.session(&format!("purpose-{index}"));
        turn(&session, 1);
        assert!(
            env.prepare(Some(session.id()), purpose)
                .await
                .fields()
                .contains_key("dsh_session_log")
        );
    }
    env.shutdown().await;
}

#[tokio::test]
async fn exclusions_are_disposable_and_duplicate_registrations_are_independent() {
    let env = Env::new(json!({})).await;
    let session = env.session("excluded");
    turn(&session, 1);
    let service = env.ctx.get::<SessionLogDeepSeek>().unwrap();
    let first = service.register_exclusion(&env.ctx, |_| Ok(true)).unwrap();
    let second = service.register_exclusion(&env.ctx, |_| Ok(true)).unwrap();
    assert!(env.field(session.id()).await.is_none());
    first.dispose().await;
    assert!(env.field(session.id()).await.is_none());
    second.dispose().await;
    assert!(env.field(session.id()).await.is_some());
    env.shutdown().await;
}

#[tokio::test]
async fn matching_exclusion_is_order_independent_and_accept_is_a_noop() {
    for reverse in [false, true] {
        let env = Env::new(json!({})).await;
        let session = env.session(if reverse {
            "excluded-reverse"
        } else {
            "excluded-forward"
        });
        turn(&session, 1);
        let service = env.ctx.get::<SessionLogDeepSeek>().unwrap();
        let false_rule: Arc<hoh_session_log_deepseek::SessionLogUploadExclusion> =
            Arc::new(|_| Ok(false));
        let matching_rule: Arc<hoh_session_log_deepseek::SessionLogUploadExclusion> =
            Arc::new(|request| Ok(request.purpose.as_deref() == Some("private")));
        let rules = if reverse {
            vec![matching_rule, false_rule]
        } else {
            vec![false_rule, matching_rule]
        };
        let mut disposers = Vec::new();
        for rule in rules {
            disposers.push(
                service
                    .register_exclusion(&env.ctx, move |request| rule(request))
                    .unwrap(),
            );
        }
        let excluded = env.prepare(Some(session.id()), Some("private")).await;
        assert!(!excluded.fields().contains_key("dsh_session_log"));
        excluded.accept().await.unwrap();
        assert_eq!(session.seq().get(), 1);
        assert_eq!(
            accepted_through(&session).unwrap(),
            SessionSeqCursor::BeforeFirst
        );
        assert!(env.field(session.id()).await.is_some());
        for disposer in disposers {
            disposer.dispose().await;
        }
        env.shutdown().await;
    }
}

#[tokio::test]
async fn exclusion_predicate_can_register_another_exclusion_reentrantly() {
    let env = Env::new(json!({})).await;
    let session = env.session("reentrant-exclusion");
    turn(&session, 1);
    let service = env.ctx.get::<SessionLogDeepSeek>().unwrap();
    let nested = Arc::new(Mutex::new(Vec::new()));
    let nested_from_rule = nested.clone();
    let nested_service = service.clone();
    let nested_ctx = env.ctx.clone();
    let first = service
        .register_exclusion(&env.ctx, move |_| {
            let mut nested = nested_from_rule
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if nested.is_empty() {
                nested.push(nested_service.register_exclusion(&nested_ctx, |_| Ok(false))?);
            }
            Ok(false)
        })
        .unwrap();
    assert!(env.field(session.id()).await.is_some());
    assert_eq!(
        nested
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1
    );
    first.dispose().await;
    let nested = nested
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop()
        .unwrap();
    nested.dispose().await;
    env.shutdown().await;
}

#[tokio::test]
async fn throwing_exclusion_fails_only_its_contribution() {
    let env = Env::new(json!({})).await;
    let session = env.session("throwing-exclusion");
    turn(&session, 1);
    let service = env.ctx.get::<SessionLogDeepSeek>().unwrap();
    let disposer = service
        .register_exclusion(&env.ctx, |_| Err("exclusion failed".into()))
        .unwrap();
    assert!(env.field(session.id()).await.is_none());
    assert_eq!(cursor(accepted_through(&session).unwrap()), -1);
    assert!(env.ctx.logger().buffer().iter().any(|record| {
        record
            .message
            .contains("omitting field \"dsh_session_log\"")
            && record.message.contains("exclusion failed")
    }));
    disposer.dispose().await;
    assert!(env.field(session.id()).await.is_some());
    env.shutdown().await;
}

#[tokio::test]
async fn prepared_contribution_survives_later_exclusion() {
    let env = Env::new(json!({})).await;
    let session = env.session("prepared");
    turn(&session, 1);
    let prepared = env.prepare(Some(session.id()), None).await;
    env.ctx
        .get::<SessionLogDeepSeek>()
        .unwrap()
        .register_exclusion(&env.ctx, |_| Ok(true))
        .unwrap();
    prepared.accept().await.unwrap();
    assert_eq!(cursor(accepted_through(&session).unwrap()), 0);
    assert!(env.field(session.id()).await.is_none());
    env.shutdown().await;
}

#[tokio::test]
async fn caller_exclusion_rebinds_after_service_replacement() {
    let env = Env::new(json!({})).await;
    let session = env.session("rebind");
    turn(&session, 1);
    let consumer = env.ctx.plugin::<ExclusionConsumer>(Value::Null).unwrap();
    consumer.settled().await.unwrap();
    assert!(
        env.prepare(Some(session.id()), Some("private"))
            .await
            .fields()
            .is_empty()
    );
    env.upload.disposer().unwrap().dispose().await;
    let replacement = env
        .ctx
        .plugin::<SessionLogDeepSeekPlugin>(json!({}))
        .unwrap();
    replacement.settled().await.unwrap();
    consumer.settled().await.unwrap();
    assert!(
        env.prepare(Some(session.id()), Some("private"))
            .await
            .fields()
            .is_empty()
    );
    consumer.disposer().unwrap().dispose().await;
    assert!(env.field(session.id()).await.is_some());
    env.shutdown().await;
}

#[tokio::test]
async fn uploads_feedback_events_without_changing_the_message_surface() {
    let env = Env::new(json!({})).await;
    let session = env.session("feedback");
    user(&session, "question");
    let before = session.derive_messages().unwrap();
    session
        .append(
            "feedback/record",
            json!({"text":"The session needs a clearer explanation."}),
            None,
        )
        .unwrap();
    session
        .append(
            "feedback/message-put",
            json!({"sessionId":session.id(),"item":{"rating":"negative","version":1}}),
            None,
        )
        .unwrap();
    let initial = session.snapshot_events(None, None).to_vec();
    let first = env.prepare(Some(session.id()), None).await;
    let retry = env.prepare(Some(session.id()), None).await;
    assert_eq!(
        first.fields()["dsh_session_log"],
        retry.fields()["dsh_session_log"]
    );
    let field: DeepSeekSessionLogExtension =
        serde_json::from_value(first.fields()["dsh_session_log"].clone()).unwrap();
    assert_eq!(
        field
            .events
            .iter()
            .map(|event| event.r#type.as_str())
            .collect::<Vec<_>>(),
        vec!["user/message", "feedback/record", "feedback/message-put"]
    );
    assert_eq!(
        field
            .events
            .iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect::<Vec<_>>(),
        initial
            .iter()
            .map(|event| event.as_value().clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(session.derive_messages().unwrap(), before);
    first.accept().await.unwrap();
    assert_eq!(cursor(accepted_through(&session).unwrap()), 2);
    env.shutdown().await;
}

#[tokio::test]
async fn volatile_enabled_changes_without_replacing_fiber() {
    let env = Env::new(json!({"enabled":true,"maxBytes":4096})).await;
    let session = env.session("volatile");
    turn(&session, 1);
    let first = env.prepare(Some(session.id()), None).await;
    first.accept().await.unwrap();
    let accepted = cursor(accepted_through(&session).unwrap());
    assert_eq!(accepted, 0);
    let uid = env.upload.uid();
    env.upload
        .update(json!({"enabled":false,"maxBytes":4096}))
        .await
        .unwrap();
    assert_eq!(env.upload.uid(), uid);
    session
        .append(
            "feedback/record",
            json!({"text":"recorded while upload is disabled"}),
            None,
        )
        .unwrap();
    assert!(env.field(session.id()).await.is_none());
    assert_eq!(cursor(accepted_through(&session).unwrap()), accepted);
    env.upload
        .update(json!({"enabled":true,"maxBytes":4096}))
        .await
        .unwrap();
    assert_eq!(env.upload.uid(), uid);
    let resumed = env.field(session.id()).await.unwrap();
    assert_eq!(resumed.after_seq, accepted);
    assert!(
        resumed
            .events
            .iter()
            .any(|event| event.r#type == "feedback/record")
    );
    env.shutdown().await;
}

#[tokio::test]
async fn plugin_unload_withdraws_field() {
    let env = Env::new(json!({})).await;
    let session = env.session("unload");
    turn(&session, 1);
    assert!(env.field(session.id()).await.is_some());
    env.upload.disposer().unwrap().dispose().await;
    assert!(env.field(session.id()).await.is_none());
    env.shutdown().await;
}

#[tokio::test]
async fn uploads_developer_system_and_tool_surface_changes_verbatim() {
    let env = Env::new(json!({})).await;
    let session = env.session("wire-surface-kinds");
    let header = session
        .append(
            "request/header",
            json!({
                "reason":"initial",
                "header":{
                    "config":{"provider":"test","model":"test"},
                    "tools":[{"name":"search","description":"Search","parameters":{}}]
                }
            }),
            None,
        )
        .unwrap();
    let first_developer = session
        .append(
            "developer/message",
            json!({
                "turn":1,"step":1,"headerSeq":header.seq(),
                "message":{
                    "id":"developer-add","role":"developer",
                    "content":[{"type":"tool-addition","toolName":"search"}],
                    "source":{"kind":"tool-registry"}
                }
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append(
            "developer/message",
            json!({
                "turn":1,"step":1,
                "message":{
                    "id":"developer-remove","role":"developer",
                    "content":[{"type":"tool-removal","toolName":"search"}],
                    "source":{"kind":"tool-registry"}
                }
            }),
            Some(SurfaceIntent::replace(
                first_developer.seq(),
                first_developer.seq(),
                vec![first_developer.seq()],
            )),
        )
        .unwrap();
    let first_system = session
        .append(
            "system/message",
            {
                let mut data = message_data("system", "head");
                data["extra"] = json!({"retained":true});
                data
            },
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append(
            "system/message",
            message_data("system", "new head"),
            Some(SurfaceIntent::replace(
                first_system.seq(),
                first_system.seq(),
                vec![first_system.seq()],
            )),
        )
        .unwrap();
    session
        .append(
            "tool/result",
            message_data("tool", "result"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();

    let events = session.snapshot_events(None, None);
    let field = env.field(session.id()).await.unwrap();
    assert_eq!(field.events.len(), events.len());
    for (wire, event) in field.events.iter().zip(events.iter()) {
        assert_eq!(
            serde_json::to_value(wire).unwrap(),
            event.as_value().clone()
        );
    }
    env.shutdown().await;
}

#[tokio::test]
async fn translates_ignorable_and_surface_envelopes_to_raw_wire_values() {
    let env = Env::new(json!({})).await;
    let events = vec![
        raw_event(0, "turn/start", json!({"turn":1})),
        Arc::new(SessionEvent::from_value(json!({
            "type":"user/message","seq":1,"time":2,"data":message("user","first"),
            "ignorable":true,"sourceEventSeqs":[0],"surfaceOp":"append"
        }))),
        Arc::new(SessionEvent::from_value(json!({
            "type":"user/message","seq":2,"time":3,"data":message("user","replacement"),
            "sourceEventSeqs":[1],
            "surfaceOp":{"op":"replace","startSeq":1,"endSeq":1}
        }))),
    ];
    let session = env
        .sessions
        .create(
            &env.ctx,
            Some("wire-envelopes".into()),
            CreateSessionOptions {
                seed: Some(events),
                ..Default::default()
            },
        )
        .unwrap();
    let field = env.field(session.id()).await.unwrap();
    assert_eq!(field.events[0].surface_op, None);
    assert_eq!(field.events[0].source_event_seqs, None);
    assert_eq!(field.events[1].ignorable, Some(true));
    assert_eq!(field.events[1].surface_op, Some(json!("append")));
    assert_eq!(field.events[1].source_event_seqs, Some(json!([0])));
    assert_eq!(
        field.events[2].surface_op,
        Some(json!({"op":"replace","startSeq":1,"endSeq":1}))
    );
    assert_eq!(field.events[2].source_event_seqs, Some(json!([1])));
    env.shutdown().await;
}

#[tokio::test]
async fn translates_header_and_surface_values() {
    let env = Env::new(json!({})).await;
    let first = raw_event(
        0,
        "user/message",
        json!({"content":[{"type":"text","text":"first"}],"source":{"kind":"user"},"role":"user","id":"first"}),
    );
    let mut first_value = first.as_value().clone();
    first_value["ignorable"] = json!(true);
    first_value["surfaceOp"] = json!("append");
    let first = Arc::new(SessionEvent::from_value(first_value));
    let options = CreateSessionOptions {
        seed: Some(vec![first]),
        inherited_event_count: Some(SessionLogOffset::new(1).unwrap()),
        meta: Some(json!({
            "createdAt": 5,
            "cwd":"/work",
            "parentSession":"parent",
            "isSeeded":true,
            "origin":"subagent",
            "delegationDepth":1,
            "agentPreset":"minimal"
        })),
    };
    let session = env
        .sessions
        .create(&env.ctx, Some("child".into()), options)
        .unwrap();
    let field = env.field(session.id()).await.unwrap();
    assert_eq!(field.session.id, "child");
    assert_eq!(field.session.cwd.as_deref(), Some("/work"));
    assert_eq!(field.session.parent_session.as_deref(), Some("parent"));
    assert_eq!(field.session.seed_length, Some(1));
    assert_eq!(field.session.origin.as_deref(), Some("subagent"));
    assert_eq!(field.session.delegation_depth, Some(1));
    assert_eq!(field.session.agent_preset.as_deref(), Some("minimal"));
    assert_eq!(field.events[0].ignorable, Some(true));
    assert_eq!(field.events[0].surface_op, Some(json!("append")));
    assert_eq!(field.events[0].source_event_seqs, None);
    let wire = serde_json::to_value(&field).unwrap();
    assert!(wire["session"].get("isSeeded").is_none());

    let ordinary = env.session("not-seeded");
    turn(&ordinary, 1);
    let ordinary = env.field(ordinary.id()).await.unwrap();
    assert_eq!(ordinary.session.seed_length, None);
    assert!(
        serde_json::to_value(&ordinary).unwrap()["session"]
            .get("seedLength")
            .is_none()
    );
    env.shutdown().await;
}

#[tokio::test]
async fn strips_surface_metadata_from_known_log_only_events() {
    for event_type in [
        "turn/start",
        "assistant/attempt",
        "request/context",
        "tool/ptc-dispatch",
    ] {
        for (key, metadata) in [
            ("surfaceOp", json!("append")),
            ("sourceEventSeqs", json!([0])),
        ] {
            let mut value = json!({
                "type":event_type,"seq":0,"time":1,
                "data":{"turn":1,"step":1,"stream":[]},"ignorable":true,
            });
            value[key] = metadata;
            let event = Arc::new(SessionEvent::from_value(value));
            let error = Session::from_restore(
                "known-log-only".into(),
                &[event],
                json!({"version":4,"id":"known-log-only","createdAt":1,"isSeeded":false}),
                SessionLogOffset::new(0).unwrap(),
                hoh_session::SessionSeedEventState::SharedFrozen,
                None,
            )
            .expect_err("known log-only metadata is rejected");
            assert!(
                error.to_string().contains("not surface-eligible"),
                "{error}"
            );
        }
    }
}

#[tokio::test]
async fn preserves_opaque_metadata_for_unknown_ignorable_events() {
    let env = Env::new(json!({})).await;
    for (type_index, event_type) in [
        "extension/event",
        "tool/code-dispatch",
        "tool/code-dispatch-start",
    ]
    .into_iter()
    .enumerate()
    {
        for (metadata_index, metadata) in [
            json!({}),
            json!({"surfaceOp":null}),
            json!({"sourceEventSeqs":null}),
            json!({
                "surfaceOp":{"opaque":["retained"]},
                "sourceEventSeqs":{"opaque":[null]}
            }),
        ]
        .into_iter()
        .enumerate()
        {
            let mut value = json!({
                "type":event_type,"seq":0,"time":1,"data":{"nested":[null,true]},
                "ignorable":true,
            });
            value
                .as_object_mut()
                .unwrap()
                .extend(metadata.as_object().unwrap().clone());
            let event = Arc::new(SessionEvent::from_value(value.clone()));
            let id = format!("opaque-{type_index}-{metadata_index}");
            let session = Session::from_restore(
                id.clone().into(),
                &[event],
                json!({"version":4,"id":id,"createdAt":1,"isSeeded":false}),
                SessionLogOffset::new(0).unwrap(),
                hoh_session::SessionSeedEventState::Detached,
                None,
            )
            .unwrap();
            let _detach = env.sessions.enter(&env.ctx, session.clone()).unwrap();
            let prepared = env.prepare(Some(session.id()), None).await;
            assert_eq!(prepared.fields()["dsh_session_log"]["events"][0], value);
            assert!(session.derive_messages().unwrap().is_empty());
        }
    }
    env.shutdown().await;
}

#[tokio::test]
async fn assistant_omits_source_event_seqs() {
    let env = Env::new(json!({})).await;
    let session = env.session("assistant");
    session
        .append(
            "assistant/message",
            json!({
                "turn":1,"step":1,"stream":[],
                "message":{"content":[{"type":"text","text":"answer"}],"role":"assistant","id":"a","source":{"kind":"model","provider":"deepseek-official","model":"v4"}}
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let wire = &env.field(session.id()).await.unwrap().events[0];
    assert_eq!(wire.surface_op, Some(json!("append")));
    assert_eq!(wire.source_event_seqs, None);
    env.shutdown().await;
}

#[tokio::test]
async fn replacement_surface_operation_is_raw_numeric_json() {
    let env = Env::new(json!({})).await;
    let session = env.session("replace");
    user(&session, "first");
    session
        .append(
            "user/message",
            json!({"content":[{"type":"text","text":"second"}],"source":{"kind":"user"},"role":"user","id":"second"}),
            Some(SurfaceIntent::replace(
                SessionSeq::new(0).unwrap(),
                SessionSeq::new(0).unwrap(),
                vec![SessionSeq::new(0).unwrap()],
            )),
        )
        .unwrap();
    let field = env.field(session.id()).await.unwrap();
    assert_eq!(
        field.events[1].surface_op,
        Some(json!({"op":"replace","startSeq":0,"endSeq":0}))
    );
    assert_eq!(field.events[1].source_event_seqs, Some(json!([0])));
    env.shutdown().await;
}

#[tokio::test]
async fn exact_byte_limit_selects_longest_prefix() {
    let env = Env::new(json!({})).await;
    let session = env.session("byte-exact");
    user(&session, &"a".repeat(200));
    user(&session, &"b".repeat(200));
    user(&session, &"c".repeat(200));
    let full = env.field(session.id()).await.unwrap();
    let mut two = full.clone();
    two.events.truncate(2);
    two.through_seq = 1;
    let limit = serde_json::to_vec(&two).unwrap().len();
    env.upload.disposer().unwrap().dispose().await;
    let bounded = env
        .ctx
        .plugin::<SessionLogDeepSeekPlugin>(json!({"maxBytes":limit}))
        .unwrap();
    bounded.settled().await.unwrap();
    let field = env.field(session.id()).await.unwrap();
    assert_eq!(field.events.len(), 2);
    assert_eq!(serde_json::to_vec(&field).unwrap().len(), limit);
    bounded.dispose().await;
    let smaller = env
        .ctx
        .plugin::<SessionLogDeepSeekPlugin>(json!({"maxBytes":limit - 1}))
        .unwrap();
    smaller.settled().await.unwrap();
    let field = env.field(session.id()).await.unwrap();
    assert_eq!(field.events.len(), 1);
    env.shutdown().await;
}

#[tokio::test]
async fn default_limit_is_eight_mibibytes() {
    let env = Env::new(json!({})).await;
    let session = env.session("default-limit");
    user(&session, &"a".repeat(3 * 1024 * 1024));
    user(&session, &"b".repeat(3 * 1024 * 1024));
    user(&session, &"c".repeat(3 * 1024 * 1024));
    let field = env.field(session.id()).await.unwrap();
    assert_eq!(field.events.len(), 2);
    assert!(serde_json::to_vec(&field).unwrap().len() <= 8 * 1024 * 1024);
    env.shutdown().await;
}

#[tokio::test]
async fn byte_limit_counts_utf8_bytes() {
    let env = Env::new(json!({})).await;
    let session = env.session("utf8");
    user(&session, &"界".repeat(100));
    user(&session, &"界".repeat(100));
    let full = env.field(session.id()).await.unwrap();
    let limit = serde_json::to_vec(&full).unwrap().len() - 1;
    assert!(serde_json::to_string(&full).unwrap().chars().count() < limit);
    env.upload.disposer().unwrap().dispose().await;
    let bounded = env
        .ctx
        .plugin::<SessionLogDeepSeekPlugin>(json!({"maxBytes":limit}))
        .unwrap();
    bounded.settled().await.unwrap();
    assert_eq!(env.field(session.id()).await.unwrap().events.len(), 1);
    env.shutdown().await;
}

#[tokio::test]
async fn oversized_first_event_blocks_later_events_without_advancing() {
    let env = Env::new(json!({})).await;
    let session = env.session("oversized");
    user(&session, &"x".repeat(1000));
    let full = env.field(session.id()).await.unwrap();
    let needed = serde_json::to_vec(&full).unwrap().len();
    let limit = needed - 1;
    env.upload.disposer().unwrap().dispose().await;
    let bounded = env
        .ctx
        .plugin::<SessionLogDeepSeekPlugin>(json!({"maxBytes":limit}))
        .unwrap();
    bounded.settled().await.unwrap();
    turn(&session, 1);
    let prepared = env.prepare(Some(session.id()), None).await;
    assert!(!prepared.fields().contains_key("dsh_session_log"));
    prepared.accept().await.unwrap();
    assert!(env.field(session.id()).await.is_none());
    assert_eq!(cursor(accepted_through(&session).unwrap()), -1);
    assert_eq!(session.seq().get(), 2);
    let warnings: Vec<_> = env
        .ctx
        .logger()
        .buffer()
        .iter()
        .filter(|record| record.message.contains("upload stays at event 0"))
        .map(|record| record.message.clone())
        .collect();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0],
        format!(
            "session-log-deepseek: event 0 of session \"oversized\" needs a {needed}-byte dsh_session_log field, above maxBytes {limit}; this session's upload stays at event 0 until maxBytes admits it"
        )
    );
    env.shutdown().await;
}

#[tokio::test]
async fn backlog_drains_across_accepted_requests() {
    let env = Env::new(json!({"maxBytes":1200})).await;
    let session = env.session("drain");
    for digit in 0..6 {
        user(&session, &digit.to_string().repeat(200));
    }
    let mut uploaded = Vec::new();
    let mut batches: Vec<DeepSeekSessionLogExtension> = Vec::new();
    for _ in 0..10 {
        let prepared = env.prepare(Some(session.id()), None).await;
        let Some(value) = prepared.fields().get("dsh_session_log") else {
            break;
        };
        let field: DeepSeekSessionLogExtension = serde_json::from_value(value.clone()).unwrap();
        assert!(serde_json::to_vec(&field).unwrap().len() <= 1200);
        if let Some(previous) = batches.last() {
            assert_eq!(field.after_seq, previous.through_seq as i64);
        } else {
            assert_eq!(field.after_seq, -1);
        }
        uploaded.extend(
            field
                .events
                .iter()
                .filter(|event| event.r#type == "user/message")
                .map(|event| event.seq),
        );
        batches.push(field);
        prepared.accept().await.unwrap();
        if uploaded.len() == 6 {
            break;
        }
    }
    assert_eq!(uploaded, vec![0, 1, 2, 3, 4, 5]);
    assert!(batches.len() > 2);
    env.shutdown().await;
}

#[test]
fn ignores_old_format_before_interpreting_frozen_watermark() {
    let id = "migrated";
    let events = vec![
        raw_event(0, "turn/start", json!({"turn":1})),
        raw_event(
            1,
            "session-log-deepseek/delivery-accepted",
            json!({
                "sessionId":id,"throughSeq":99
            }),
        ),
        raw_event(2, "step/start", json!({"turn":1,"step":1})),
        raw_event(
            3,
            "session-log-deepseek/delivery-accepted",
            json!({
                "sessionId":id,"throughSeq":2,"sessionFormatVersion":4
            }),
        ),
    ];
    let session = Session::from_restore(
        id.into(),
        &events,
        json!({"version":4,"id":id,"createdAt":1,"isSeeded":false}),
        SessionLogOffset::new(0).unwrap(),
        hoh_session::SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert_eq!(cursor(accepted_through(&session).unwrap()), 2);
}

#[tokio::test]
async fn uploads_migrated_v3_marker_before_current_generation_acknowledgement() {
    let env = Env::new(json!({})).await;
    let id = "migrated-v3-delivery";
    let events = vec![
        raw_event(0, "feedback/record", json!({"text":"retained"})),
        raw_event(
            1,
            "session-log-deepseek/delivery-accepted",
            json!({
                "sessionId":id,"throughSeq":0,"sessionFormatVersion":3
            }),
        ),
    ];
    let session = Session::from_restore(
        id.into(),
        &events,
        json!({
            "version":4,"id":id,"createdAt":1,"isSeeded":false,
            "delegationDepth":0
        }),
        SessionLogOffset::new(0).unwrap(),
        hoh_session::SessionSeedEventState::Detached,
        None,
    )
    .unwrap();
    let _detach = env.sessions.enter(&env.ctx, session.clone()).unwrap();
    env.sessions.announce(&session).unwrap();
    let first = env.prepare(Some(session.id()), None).await;
    let field: DeepSeekSessionLogExtension =
        serde_json::from_value(first.fields()["dsh_session_log"].clone()).unwrap();
    assert_eq!(field.session_format_version, 4);
    assert_eq!(field.after_seq, -1);
    assert_eq!(field.events[0].seq, 0);
    assert_eq!(field.events[1].seq, 1);
    assert_eq!(field.events[1].data["sessionFormatVersion"], 3);
    assert_eq!(field.events[1].data["throughSeq"], 0);
    let through = field.through_seq;
    first.accept().await.unwrap();
    assert_eq!(
        session.snapshot_events(None, None).last().unwrap().data(),
        &json!({
            "sessionId":id,"sessionFormatVersion":4,"throughSeq":through
        })
    );
    let second = env.field(session.id()).await.unwrap();
    assert_eq!(second.after_seq, through as i64);
    assert_eq!(second.events.len(), 1);
    assert_eq!(second.events[0].seq, through + 1);
    assert_eq!(
        session
            .event_at(SessionSeq::new(1).unwrap())
            .unwrap()
            .data()["sessionFormatVersion"],
        3
    );
    env.shutdown().await;
}

#[test]
fn cached_acceptance_fold_picks_up_only_newly_appended_tail() {
    let session = Session::create(
        "incremental-fold".into(),
        None,
        Some(json!({
            "version":4,"id":"incremental-fold","createdAt":1,"isSeeded":false
        })),
        None,
        None,
    )
    .unwrap();
    turn(&session, 1);
    session
        .append(
            "session-log-deepseek/delivery-accepted",
            json!({
                "sessionId":"incremental-fold","throughSeq":0,"sessionFormatVersion":4
            }),
            None,
        )
        .unwrap();
    assert_eq!(cursor(accepted_through(&session).unwrap()), 0);
    assert_eq!(cursor(accepted_through(&session).unwrap()), 0);
    session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    session
        .append(
            "session-log-deepseek/delivery-accepted",
            json!({
                "sessionId":"incremental-fold","throughSeq":2,"sessionFormatVersion":4
            }),
            None,
        )
        .unwrap();
    assert_eq!(cursor(accepted_through(&session).unwrap()), 2);
}

#[test]
fn null_acceptance_format_version_defaults_to_zero_generation() {
    let id = "null-format";
    let events = vec![
        raw_event(0, "turn/start", json!({"turn":1})),
        raw_event(
            1,
            "session-log-deepseek/delivery-accepted",
            json!({
                "sessionId":id,"throughSeq":0,"sessionFormatVersion":null
            }),
        ),
    ];
    let session = Session::from_restore(
        id.into(),
        &events,
        json!({"version":4,"id":id,"createdAt":1,"isSeeded":false}),
        SessionLogOffset::new(0).unwrap(),
        hoh_session::SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert_eq!(
        accepted_through(&session).unwrap(),
        SessionSeqCursor::BeforeFirst
    );
}

#[test]
fn rejects_malformed_acceptance_format_versions() {
    for format in [json!(-1), json!(-0.0), json!(0.5)] {
        let event = raw_event(
            0,
            "session-log-deepseek/delivery-accepted",
            json!({
                "sessionId":"bad-format","throughSeq":0,"sessionFormatVersion":format
            }),
        );
        let session = Session::from_restore(
            "bad-format".into(),
            &[event],
            json!({"version":4,"id":"bad-format","createdAt":1,"isSeeded":false}),
            SessionLogOffset::new(0).unwrap(),
            hoh_session::SessionSeedEventState::SharedFrozen,
            None,
        )
        .unwrap();
        assert!(
            accepted_through(&session)
                .unwrap_err()
                .to_string()
                .contains("malformed acceptance format version")
        );
    }
}

#[test]
fn rejects_malformed_or_negative_current_watermarks() {
    for through in [json!(-1), json!(0), json!(0.5)] {
        let event = raw_event(
            0,
            "session-log-deepseek/delivery-accepted",
            json!({
                "sessionId":"bad-watermark","throughSeq":through,"sessionFormatVersion":4
            }),
        );
        let session = Session::from_restore(
            "bad-watermark".into(),
            &[event],
            json!({"version":4,"id":"bad-watermark","createdAt":1,"isSeeded":false}),
            SessionLogOffset::new(0).unwrap(),
            hoh_session::SessionSeedEventState::SharedFrozen,
            None,
        )
        .unwrap();
        assert!(
            accepted_through(&session)
                .unwrap_err()
                .to_string()
                .contains("malformed acceptance watermark")
        );
    }
}

#[test]
fn ignores_matching_format_acceptance_for_another_session() {
    let events = vec![
        raw_event(0, "turn/start", json!({"turn":1})),
        raw_event(
            1,
            "session-log-deepseek/delivery-accepted",
            json!({
                "sessionId":"parent","throughSeq":0,"sessionFormatVersion":4
            }),
        ),
    ];
    let session = Session::from_restore(
        "child".into(),
        &events,
        json!({"version":4,"id":"child","createdAt":1,"isSeeded":false}),
        SessionLogOffset::new(0).unwrap(),
        hoh_session::SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert_eq!(
        accepted_through(&session).unwrap(),
        SessionSeqCursor::BeforeFirst
    );
}

#[tokio::test]
async fn rejects_invalid_max_bytes_configs() {
    for value in [json!(0), json!(-1), json!(1.5)] {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let session = ctx.plugin::<SessionPlugin>(Value::Null).unwrap();
        session.settled().await.unwrap();
        let extension = ctx
            .plugin::<DeepSeekLlmApiExtensionRegistry>(Value::Null)
            .unwrap();
        extension.settled().await.unwrap();
        let upload = ctx
            .plugin::<SessionLogDeepSeekPlugin>(json!({"maxBytes":value}))
            .unwrap();
        assert!(upload.settled().await.is_err());
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn max_bytes_update_reloads_service_and_rejects_invalid_value() {
    let env = Env::new(json!({"maxBytes":4096})).await;
    let session = env.session("max-bytes-update");
    user(&session, &"x".repeat(1000));
    let full = env.field(session.id()).await.unwrap();
    let needed = serde_json::to_vec(&full).unwrap().len();
    let uid = env.upload.uid();

    env.upload
        .update(json!({"maxBytes":needed - 1}))
        .await
        .unwrap();
    assert_eq!(env.upload.uid(), uid);
    assert!(env.field(session.id()).await.is_none());

    let error = env.upload.update(json!({"maxBytes":0})).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("maxBytes must be a positive integer"),
        "{error}"
    );
    env.shutdown().await;
}
