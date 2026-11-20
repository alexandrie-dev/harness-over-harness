use std::sync::Arc;

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_agent_loop_testkit::{create_inbox_stub, unsupported_model};
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Context, Kernel};
use hoh_llm::{ContentBlock, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId};
use hoh_subagent::{
    Config, ConfiguredMaxDepth, SubagentActivationSpec, SubagentCapabilities,
    SubagentDeliveryTarget, SubagentProvider, SubagentRuntime, SubagentStartRequest,
};
use hoh_timeout::AbortController;

struct Provider {
    name: &'static str,
    start: bool,
    continuable: bool,
}

impl SubagentProvider for Provider {
    fn name(&self) -> &str {
        self.name
    }

    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities::default()
    }

    fn inherits_parent_context(&self) -> bool {
        false
    }

    fn supports_start(&self) -> bool {
        self.start
    }

    fn supports_continuable(&self) -> bool {
        self.continuable
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

fn parent(ctx: &Context) -> Arc<dyn Agent> {
    Arc::new(StubAgent {
        id: SessionId::from("parent"),
        session: Session::create(SessionId::from("parent"), None, None, None, None).unwrap(),
        ctx: ctx.clone(),
        options: AgentOptions::default(),
        scope: ScopeKey::new(),
        model: unsupported_model(),
        inbox: create_inbox_stub(),
    })
}

fn harness(error: &BoxError) -> &HarnessError {
    HarnessError::of(error.as_ref()).expect("subagent errors are machine-routable")
}

#[test]
fn config_defaults_match_the_source_package() {
    assert_eq!(Config::default().max_active_subagents, 8);
    assert_eq!(Config::default().max_depth, 1);
    let empty: Config = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(empty.max_active_subagents, 8);
    assert_eq!(empty.max_depth, 1);
}

#[tokio::test]
async fn provider_must_implement_start_or_continuable() {
    let kernel = Kernel::start();
    let runtime = SubagentRuntime::new(kernel.root());
    let error = runtime
        .register_provider(Arc::new(Provider {
            name: "empty",
            start: false,
            continuable: false,
        }))
        .err()
        .unwrap();
    assert_eq!(harness(&error).code(), "UNSUPPORTED_CAPABILITY");
    assert_eq!(
        harness(&error).message(),
        "subagent provider \"empty\" must implement start or prepareContinuable"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn duplicate_provider_is_rejected_without_replacing_the_first() {
    let kernel = Kernel::start();
    let runtime = SubagentRuntime::new(kernel.root());
    let first: Arc<dyn SubagentProvider> = Arc::new(Provider {
        name: "provider",
        start: true,
        continuable: false,
    });
    let registration = runtime.register_provider(first.clone()).unwrap();
    let error = runtime
        .register_provider(Arc::new(Provider {
            name: "provider",
            start: false,
            continuable: true,
        }))
        .err()
        .unwrap();
    assert_eq!(harness(&error).code(), "DUPLICATE_PROVIDER");
    assert!(Arc::ptr_eq(
        &runtime.get_provider("provider").unwrap(),
        &first
    ));
    drop(registration);
    kernel.shutdown().await;
}

#[tokio::test]
async fn registration_drop_and_explicit_unregister_remove_the_provider() {
    let kernel = Kernel::start();
    let runtime = SubagentRuntime::new(kernel.root());
    let registration = runtime
        .register_provider(Arc::new(Provider {
            name: "drop-me",
            start: true,
            continuable: false,
        }))
        .unwrap();
    assert!(runtime.get_provider("drop-me").is_some());
    drop(registration);
    assert!(runtime.get_provider("drop-me").is_none());

    let registration = runtime
        .register_provider(Arc::new(Provider {
            name: "unregister-me",
            start: false,
            continuable: true,
        }))
        .unwrap();
    registration.unregister();
    assert!(runtime.get_provider("unregister-me").is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn max_depth_resolution_honors_default_override_and_provider_management() {
    let kernel = Kernel::start();
    let runtime = SubagentRuntime::new(kernel.root());
    assert_eq!(runtime.resolve_max_depth(None).unwrap(), Some(1));
    assert_eq!(
        runtime
            .resolve_max_depth(Some(ConfiguredMaxDepth::Value(7)))
            .unwrap(),
        Some(7)
    );
    assert_eq!(
        runtime
            .resolve_max_depth(Some(ConfiguredMaxDepth::ProviderManaged))
            .unwrap(),
        None
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn provider_resolution_precedes_max_depth_validation() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let runtime = SubagentRuntime::new(ctx.clone());
    let error = runtime
        .start_activation(SubagentActivationSpec {
            provider: "missing".into(),
            label: "task".into(),
            name: None,
            child_id: None,
            delivery: SubagentDeliveryTarget::Caller,
            request: SubagentStartRequest {
                cwd: None,
                agent_preset: None,
                label: None,
                prompt: vec![ContentBlock::text("task")],
                parent: parent(&ctx),
                signal: AbortController::new().signal(),
                agent_options: None,
                output_schema: None,
                max_depth: Some(u64::MAX),
            },
        })
        .await
        .err()
        .unwrap();
    assert_eq!(harness(&error).code(), "NO_PROVIDER");
    kernel.shutdown().await;
}
