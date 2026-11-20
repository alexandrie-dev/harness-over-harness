use hoh_agent::{
    Agent, AgentOptions, AgentRegistryPlugin, AgentStatus, Agents, CancelOptions, Inbox,
    InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection, SessionStartSource,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel, Service};
use hoh_llm::{MessageId, TokenUsage, UserMessage};
use hoh_loader_smoke::{FixtureEventObserver, FixtureTurnOptions, run_fixture_turn};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionEvent, SessionId, SessionPlugin,
    SessionStore, Sessions, SurfaceIntent,
};
use hoh_session_projection::SessionProjectionRegistry;
use serde_json::{Value, json};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

type Followup = Box<dyn FnMut(UserMessage) -> Result<(), BoxError> + Send>;

struct UnsupportedModel;

impl Model for UnsupportedModel {
    fn current(&self) -> Option<ModelSelection> {
        panic!("this test Agent does not support model selection")
    }

    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("this test Agent does not support model selection".into())
    }
}

struct UnsupportedInbox;

impl UnsupportedInbox {
    fn reject<T>() -> Result<T, BoxError> {
        Err("this test Agent does not support Inbox mutations".into())
    }
}

impl Inbox for UnsupportedInbox {
    fn next_turn(&self) -> Vec<UserMessage> {
        vec![]
    }

    fn next_step(&self) -> Vec<UserMessage> {
        vec![]
    }

    fn clear(&self) -> Result<(), BoxError> {
        Self::reject()
    }

    fn append(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Self::reject()
    }

    fn prepend(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Self::reject()
    }

    fn replace(&self, _: &MessageId, _: UserMessage) -> Result<bool, BoxError> {
        Self::reject()
    }

    fn remove(&self, _: &MessageId) -> Result<bool, BoxError> {
        Self::reject()
    }

    fn splice(
        &self,
        _: InboxTarget,
        _: f64,
        _: f64,
        _: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        Self::reject()
    }
}

struct StubAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    options: AgentOptions,
    scope: ScopeKey,
    model: Arc<dyn Model>,
    inbox: Arc<dyn Inbox>,
    idle_calls: Arc<AtomicUsize>,
    followup: Mutex<Followup>,
}

impl Agent for StubAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn scope_key(&self) -> ScopeKey {
        self.scope.clone()
    }
    fn options(&self) -> &AgentOptions {
        &self.options
    }
    fn session(&self) -> &Session {
        &self.session
    }
    fn model(&self) -> &dyn Model {
        self.model.as_ref()
    }
    fn inbox(&self) -> &dyn Inbox {
        self.inbox.as_ref()
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        Ok(())
    }
    fn when_idle(&self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.idle_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        Err("maintenance is unsupported in this fixture".into())
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        Ok(())
    }
    fn followup(&self, message: UserMessage) -> Result<(), BoxError> {
        self.followup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())(message)
    }
    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
}

struct Environment {
    kernel: Kernel,
    ctx: Context,
    sessions: Service<Sessions>,
    agents: Service<Agents>,
}

async fn environment() -> Environment {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!(null))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sessions = ctx.get::<Sessions>().unwrap();
    let agents = ctx.get::<Agents>().unwrap();
    Environment {
        kernel,
        ctx,
        sessions,
        agents,
    }
}

fn session(env: &Environment, id: &str, origin: Option<&str>) -> Session {
    let meta = origin.map(|origin| json!({ "origin": origin }));
    env.sessions
        .create(
            &env.ctx,
            Some(SessionId::from(id)),
            CreateSessionOptions {
                meta,
                ..Default::default()
            },
        )
        .unwrap()
}

fn agent(
    env: &Environment,
    session: Session,
    idle_calls: Arc<AtomicUsize>,
    followup: impl FnMut(UserMessage) -> Result<(), BoxError> + Send + 'static,
) -> Arc<dyn Agent> {
    Arc::new(StubAgent {
        id: session.id().clone(),
        session,
        ctx: env.ctx.clone(),
        options: AgentOptions::default(),
        scope: ScopeKey::new(),
        model: Arc::new(UnsupportedModel),
        inbox: Arc::new(UnsupportedInbox),
        idle_calls,
        followup: Mutex::new(Box::new(followup)),
    })
}

fn append(session: &Session, event_type: &str, data: Value) {
    session.append(event_type, data, None).unwrap();
}

fn receipt(session: &Session, message: &UserMessage) {
    append(
        session,
        "agent/inbox/spliced",
        json!({ "inserted": [message] }),
    );
}

fn assistant_message(
    session: &Session,
    id: &str,
    turn: u64,
    step: u64,
    content: Value,
    usage: Option<Value>,
) {
    let mut data = json!({
        "turn": turn,
        "step": step,
        "stream": [],
        "message": {
            "role": "assistant",
            "id": id,
            "content": content,
            "source": { "kind": "model", "provider": "fixture", "model": "fixture" }
        }
    });
    if let Some(usage) = usage {
        data["usage"] = usage;
    }
    session
        .append("assistant/message", data, Some(SurfaceIntent::append()))
        .unwrap();
}

#[tokio::test]
async fn rejects_no_agent_registry() {
    let kernel = Kernel::start();
    let error = run_fixture_turn(
        &kernel.root(),
        FixtureTurnOptions {
            task: "ignored".into(),
            on_event: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "fixture turn requires exactly one top-level agent, found 0"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_multiple_roots() {
    let env = environment().await;
    let first = agent(
        &env,
        session(&env, "first", None),
        Arc::new(AtomicUsize::new(0)),
        |_| Ok(()),
    );
    let second = agent(
        &env,
        session(&env, "second", None),
        Arc::new(AtomicUsize::new(0)),
        |_| Ok(()),
    );
    let _first = env.agents.enter(&env.ctx, first).unwrap();
    let _second = env.agents.enter(&env.ctx, second).unwrap();
    let error = run_fixture_turn(
        &env.ctx,
        FixtureTurnOptions {
            task: "ignored".into(),
            on_event: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "fixture turn requires exactly one top-level agent, found 2"
    );
    env.kernel.shutdown().await;
}

fn is_root_agent(agent: &Arc<dyn Agent>) -> bool {
    agent.session().header()["origin"].as_str() != Some("subagent")
}

#[tokio::test]
async fn waits_for_publication_then_rechecks_the_exact_root_count() {
    let env = environment().await;
    let pending_ctx = env.ctx.clone();
    let pending = tokio::spawn(async move {
        run_fixture_turn(
            &pending_ctx,
            FixtureTurnOptions {
                task: "ignored".into(),
                on_event: None,
            },
        )
        .await
    });
    tokio::task::yield_now().await;
    let first = agent(
        &env,
        session(&env, "published-first", None),
        Arc::new(AtomicUsize::new(0)),
        |_| Ok(()),
    );
    let second = agent(
        &env,
        session(&env, "published-second", None),
        Arc::new(AtomicUsize::new(0)),
        |_| Ok(()),
    );
    let _first = env.agents.enter(&env.ctx, first.clone()).unwrap();
    let _second = env.agents.enter(&env.ctx, second).unwrap();
    env.agents
        .announce(&env.ctx, &first, SessionStartSource::Startup, None)
        .await
        .unwrap();
    let error = pending.await.unwrap().unwrap_err();
    assert_eq!(
        error.to_string(),
        "fixture turn requires exactly one top-level agent, found 2"
    );
    let third = agent(
        &env,
        session(&env, "published-after-selection", None),
        Arc::new(AtomicUsize::new(0)),
        |_| Ok(()),
    );
    let _third = env.agents.enter(&env.ctx, third.clone()).unwrap();
    env.agents
        .announce(&env.ctx, &third, SessionStartSource::Startup, None)
        .await
        .unwrap();
    assert_eq!(
        env.agents.list().into_iter().filter(is_root_agent).count(),
        3
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn selects_the_ordinary_agent_and_observes_only_its_owned_interval() {
    let env = environment().await;
    let owned = session(&env, "fixture-session", None);
    let foreign = session(&env, "foreign-session", None);
    let idle_calls = Arc::new(AtomicUsize::new(0));
    let callback_owned = owned.clone();
    let callback_foreign = foreign.clone();
    let ordinary = agent(&env, owned.clone(), idle_calls.clone(), move |message| {
        assistant_message(
            &callback_foreign,
            "foreign-assistant-1",
            9,
            1,
            json!([{ "type": "text", "text": "foreign" }]),
            None,
        );
        append(
            &callback_owned,
            "step/start",
            json!({ "turn": 1, "step": 1 }),
        );
        append(
            &callback_owned,
            "agent/inbox/spliced",
            json!({ "inserted": [{ "id": "other" }] }),
        );
        receipt(&callback_owned, &message);
        append(
            &callback_owned,
            "assistant/attempt",
            json!({
                "turn": 1,
                "step": 1,
                "stream": [
                    { "type": "text-chunks", "time0": 3, "index": 0, "dt": [], "texts": ["partial"] },
                    {
                        "type": "chunk",
                        "time": 4,
                        "chunk": { "type": "usage", "usage": {
                            "inputTokens": 2, "outputTokens": 3, "reasoningTokens": 1
                        }}
                    }
                ]
            }),
        );
        append(
            &callback_owned,
            "feedback/record",
            json!({ "text": "interleaved" }),
        );
        assistant_message(
            &callback_owned,
            "assistant-1",
            1,
            1,
            json!([{ "type": "text", "text": "final answer" }]),
            Some(json!({ "inputTokens": 4, "outputTokens": 5, "cacheReadTokens": 6 })),
        );
        append(
            &callback_owned,
            "assistant/attempt",
            json!({
                "turn": 1,
                "step": 2,
                "stream": [{
                    "type": "chunk",
                    "time": 6,
                    "chunk": { "type": "usage", "usage": {
                        "inputTokens": 1, "outputTokens": 2,
                        "cacheWriteTokens": 7, "reasoningTokens": 2
                    }}
                }]
            }),
        );
        assistant_message(
            &callback_owned,
            "assistant-2",
            1,
            2,
            json!([{ "type": "tool-call", "id": "call", "name": "tool", "arguments": "{}" }]),
            None,
        );
        append(
            &callback_owned,
            "assistant/attempt",
            json!({
                "turn": 1,
                "step": 3,
                "stream": [{ "type": "text-chunks", "time0": 8, "index": 0, "dt": [], "texts": ["no usage"] }]
            }),
        );
        assistant_message(
            &callback_foreign,
            "foreign-assistant-2",
            9,
            2,
            json!([{ "type": "text", "text": "foreign-2" }]),
            None,
        );
        Ok(())
    });
    let subagent = agent(
        &env,
        session(&env, "subagent", Some("subagent")),
        Arc::new(AtomicUsize::new(0)),
        |_| Ok(()),
    );
    let _ordinary = env.agents.enter(&env.ctx, ordinary).unwrap();
    let _subagent = env.agents.enter(&env.ctx, subagent).unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_out = observed.clone();
    let result = run_fixture_turn(
        &env.ctx,
        FixtureTurnOptions {
            task: "prove the fixture".into(),
            on_event: Some(
                Arc::new(move |session_id: &SessionId, event: &SessionEvent| {
                    assert_eq!(session_id.as_str(), "fixture-session");
                    observed_out
                        .lock()
                        .unwrap()
                        .push(event.event_type().to_owned());
                }) as FixtureEventObserver,
            ),
        },
    )
    .await
    .unwrap();
    assert_eq!(result.r#type, "result");
    assert_eq!(result.session_id.as_str(), "fixture-session");
    assert_eq!(result.output, "final answer");
    assert_eq!(
        result.usage,
        Some(TokenUsage {
            input_tokens: 5,
            output_tokens: 7,
            cache_read_tokens: Some(6),
            cache_write_tokens: Some(7),
            reasoning_tokens: Some(2),
            ..Default::default()
        })
    );
    assert_eq!(idle_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        *observed.lock().unwrap(),
        [
            "agent/inbox/spliced",
            "assistant/attempt",
            "feedback/record",
            "assistant/message",
            "assistant/attempt",
            "assistant/message",
            "assistant/attempt",
        ]
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn omits_usage_when_the_interval_records_none() {
    let env = environment().await;
    let owned = session(&env, "no-usage", None);
    let callback_session = owned.clone();
    let ordinary = agent(&env, owned, Arc::new(AtomicUsize::new(0)), move |message| {
        receipt(&callback_session, &message);
        Ok(())
    });
    let _ordinary = env.agents.enter(&env.ctx, ordinary).unwrap();
    let result = run_fixture_turn(
        &env.ctx,
        FixtureTurnOptions {
            task: "no model step".into(),
            on_event: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(result.output, "");
    assert_eq!(result.usage, None);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn always_removes_its_listener_when_the_turn_fails() {
    let env = environment().await;
    let owned = session(&env, "failed-turn", None);
    let callback_session = owned.clone();
    let ordinary = agent(
        &env,
        owned.clone(),
        Arc::new(AtomicUsize::new(0)),
        move |message| {
            receipt(&callback_session, &message);
            Err("turn failed".into())
        },
    );
    let _ordinary = env.agents.enter(&env.ctx, ordinary).unwrap();
    let flush_calls = Arc::new(AtomicUsize::new(0));
    let flush_out = flush_calls.clone();
    let _flush = SessionStore::on_flush(&env.ctx, move |_| {
        flush_out.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    })
    .unwrap();
    let observed = Arc::new(AtomicUsize::new(0));
    let observed_out = observed.clone();
    let error = run_fixture_turn(
        &env.ctx,
        FixtureTurnOptions {
            task: "fail".into(),
            on_event: Some(Arc::new(move |_, _| {
                observed_out.fetch_add(1, Ordering::SeqCst);
            })),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "turn failed");
    assert_eq!(observed.load(Ordering::SeqCst), 1);
    assert_eq!(flush_calls.load(Ordering::SeqCst), 0);
    append(
        &owned,
        "feedback/record",
        json!({ "text": "after failure" }),
    );
    assert_eq!(observed.load(Ordering::SeqCst), 1);
    env.kernel.shutdown().await;
}
