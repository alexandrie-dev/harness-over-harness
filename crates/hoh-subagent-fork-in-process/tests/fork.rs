use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, Agents, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_agent_loop_testkit::{create_inbox_stub, unsupported_model};
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_llm::UserMessage;
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId, SurfaceIntent};
use hoh_subagent::{
    ContinuableCreateRequest, ContinuableCreateSpec, SubagentCapabilities, SubagentProvider,
    SubagentRuntime, Subagents,
};
use hoh_subagent_fork_in_process::{
    Config, ForkInProcessProvider, INJECT, NAME, SubagentForkInProcess,
};
use hoh_timeout::AbortController;
use serde_json::json;
use std::sync::Arc;

struct StubAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    options: AgentOptions,
    scope: ScopeKey,
    model: Arc<dyn Model>,
    inbox: Arc<dyn Inbox>,
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
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        Ok(task(AbortController::new().signal()))
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        Ok(())
    }
    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
}

fn parent(ctx: &Context, id: &str) -> Arc<dyn Agent> {
    Arc::new(StubAgent {
        id: SessionId::from(id),
        session: Session::create(SessionId::from(id), None, None, None, None).unwrap(),
        ctx: ctx.clone(),
        options: AgentOptions::default(),
        scope: ScopeKey::new(),
        model: unsupported_model(),
        inbox: create_inbox_stub(),
    })
}

fn append_user(session: &Session, text: &str) {
    session
        .append(
            "user/message",
            json!({
                "id": format!("user-{text}"),
                "role": "user",
                "content": [{"type":"text","text":text}],
                "source": {"kind":"user"}
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
}

fn complete_turn(session: &Session, turn: u64, text: &str) {
    session
        .append("turn/start", json!({"turn":turn}), None)
        .unwrap();
    append_user(session, text);
    session
        .append(
            "turn/end",
            json!({"turn":turn,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
}

fn request(parent: Arc<dyn Agent>, id: &str) -> ContinuableCreateRequest {
    ContinuableCreateRequest {
        cwd: "/workspace".into(),
        session_id: SessionId::from(id),
        parent,
        signal: AbortController::new().signal(),
    }
}

async fn prepare(provider: &dyn SubagentProvider, parent: Arc<dyn Agent>) -> ContinuableCreateSpec {
    provider
        .prepare_continuable(request(parent, "child"))
        .expect("fork is continuable")
        .await
        .unwrap()
}

async fn mount_runtime() -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.provide::<Subagents>(Arc::new(SubagentRuntime::new(ctx.clone())))
        .await
        .unwrap();
    (kernel, ctx)
}

struct FreshProvider(&'static str);

impl SubagentProvider for FreshProvider {
    fn name(&self) -> &str {
        self.0
    }
    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities::default()
    }
    fn inherits_parent_context(&self) -> bool {
        false
    }
    fn supports_continuable(&self) -> bool {
        true
    }
    fn prepare_continuable(
        &self,
        _: ContinuableCreateRequest,
    ) -> Option<BoxFuture<'static, Result<ContinuableCreateSpec, BoxError>>> {
        Some(Box::pin(async { Ok(ContinuableCreateSpec::default()) }))
    }
}

#[tokio::test]
async fn provider_is_published_when_plugin_activation_settles() {
    let (kernel, ctx) = mount_runtime().await;
    assert!(ctx.get::<Agents>().is_none());
    let fiber = ctx
        .plugin::<SubagentForkInProcess>(json!({"providerName":"fork"}))
        .unwrap();
    assert!(
        ctx.get::<Subagents>()
            .unwrap()
            .get_provider("fork")
            .is_none()
    );
    fiber.settled().await.unwrap();
    assert!(
        ctx.get::<Subagents>()
            .unwrap()
            .get_provider("fork")
            .is_some()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn no_completed_parent_turn_produces_no_seed() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let provider = ForkInProcessProvider::new("fork");
    assert!(
        prepare(&provider, parent(&ctx, "fresh"))
            .await
            .seed
            .is_none()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn every_completed_turn_through_the_last_turn_end_is_seeded() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let parent = parent(&ctx, "two-turns");
    complete_turn(parent.session(), 1, "one");
    complete_turn(parent.session(), 2, "two");
    let expected = parent.session().snapshot_events(None, None).len();
    let seed = prepare(&ForkInProcessProvider::new("fork"), parent)
        .await
        .seed
        .unwrap();
    assert_eq!(seed.len(), expected);
    assert_eq!(seed.last().unwrap().event_type(), "turn/end");
    assert_eq!(
        seed.iter()
            .filter(|event| event.event_type() == "turn/end")
            .count(),
        2
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn seeded_prefix_contains_parent_conversation_context() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let parent = parent(&ctx, "context");
    complete_turn(parent.session(), 1, "parent question");
    let provider = ForkInProcessProvider::new("fork");
    let seed = prepare(&provider, parent).await.seed.unwrap();
    assert!(provider.inherits_parent_context());
    assert!(seed.iter().any(|event| {
        event.event_type() == "user/message"
            && event.data()["content"][0]["text"] == "parent question"
    }));
    kernel.shutdown().await;
}

#[tokio::test]
async fn open_parent_turn_is_excluded_from_balanced_seed() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let parent = parent(&ctx, "open");
    complete_turn(parent.session(), 1, "done");
    parent
        .session()
        .append("turn/start", json!({"turn":2}), None)
        .unwrap();
    append_user(parent.session(), "in flight");
    let seed = prepare(&ForkInProcessProvider::new("fork"), parent)
        .await
        .seed
        .unwrap();
    assert_eq!(seed.last().unwrap().event_type(), "turn/end");
    assert!(seed.iter().all(|event| event.data()["turn"] != 2));
    kernel.shutdown().await;
}

#[test]
fn structured_output_is_advertised_through_shared_in_process_runtime() {
    let capabilities = ForkInProcessProvider::new("fork").capabilities();
    assert!(capabilities.output_schema);
}

#[tokio::test]
async fn seed_is_captured_once_before_the_preparation_future_is_polled() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let parent = parent(&ctx, "snapshot");
    complete_turn(parent.session(), 1, "first");
    let expected = parent.session().snapshot_events(None, None).len();
    let future = ForkInProcessProvider::new("fork")
        .prepare_continuable(request(parent.clone(), "child"))
        .unwrap();
    complete_turn(parent.session(), 2, "later");
    let seed = future.await.unwrap().seed.unwrap();
    assert_eq!(seed.len(), expected);
    assert!(seed.iter().all(|event| event.data()["turn"] != 2));
    kernel.shutdown().await;
}

#[test]
fn advertises_every_start_time_capability() {
    assert_eq!(
        ForkInProcessProvider::new("fork").capabilities(),
        SubagentCapabilities {
            agent_options: true,
            agent_preset: false,
            output_schema: true,
            depth_limit: true,
        }
    );
}

#[tokio::test]
async fn disposing_plugin_unregisters_provider() {
    let (kernel, ctx) = mount_runtime().await;
    let fiber = ctx
        .plugin::<SubagentForkInProcess>(json!({"providerName":"fork"}))
        .unwrap();
    fiber.settled().await.unwrap();
    assert!(
        ctx.get::<Subagents>()
            .unwrap()
            .get_provider("fork")
            .is_some()
    );
    fiber.dispose().await;
    assert!(
        ctx.get::<Subagents>()
            .unwrap()
            .get_provider("fork")
            .is_none()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn continuable_seed_is_contiguous_from_zero_and_ends_balanced() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let parent = parent(&ctx, "seq");
    complete_turn(parent.session(), 1, "one");
    let seed = prepare(&ForkInProcessProvider::new("fork"), parent)
        .await
        .seed
        .unwrap();
    assert_eq!(seed.last().unwrap().event_type(), "turn/end");
    for (index, event) in seed.iter().enumerate() {
        assert_eq!(u64::from(event.seq()), index as u64);
    }
    kernel.shutdown().await;
}

#[test]
fn exports_namespace_plugin_contract_and_default_config() {
    assert_eq!(NAME, "subagent-fork-in-process");
    assert_eq!(INJECT, ["subagents"]);
    assert_eq!(Config::default().provider_name, "fork");
}

#[tokio::test]
async fn fresh_and_fork_providers_register_and_coexist() {
    let (kernel, ctx) = mount_runtime().await;
    let runtime = ctx.get::<Subagents>().unwrap();
    let _spawn = runtime
        .register_provider(Arc::new(FreshProvider("spawn")))
        .unwrap();
    let fiber = ctx
        .plugin::<SubagentForkInProcess>(json!({"providerName":"fork"}))
        .unwrap();
    fiber.settled().await.unwrap();
    assert!(runtime.get_provider("spawn").is_some());
    assert!(runtime.get_provider("fork").is_some());
    kernel.shutdown().await;
}

#[tokio::test]
async fn same_parent_yields_fresh_spawn_and_seeded_fork_specs() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let parent = parent(&ctx, "shared-parent");
    complete_turn(parent.session(), 1, "parent turn");
    let fresh = prepare(&FreshProvider("spawn"), parent.clone()).await;
    let fork = prepare(&ForkInProcessProvider::new("fork"), parent).await;
    assert!(fresh.seed.is_none());
    assert!(fork.seed.is_some());
    kernel.shutdown().await;
}
