//! Shared test support: a kernel with the tools registry and a prepared `ToolExecution`.
use std::sync::Arc;

use hoh_kernel::{Context, Kernel};
use hoh_scope::ScopeKey;
use hoh_session::Session;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::AbortController;
use hoh_tools::{
    Agent, DefineToolOptions, DefineToolOutput, ToolExecution, ToolExecutionInput,
    ToolExecutionPipeline, ToolRegistrationOptions, ToolRuntime, Tools, define_tool,
};
use serde_json::json;

pub struct TestAgent {
    key: ScopeKey,
    session: Session,
}

impl Agent for TestAgent {
    fn scope_key(&self) -> ScopeKey {
        self.key.clone()
    }
    fn session(&self) -> Session {
        self.session.clone()
    }
}

pub struct Env {
    /// Keeps the kernel (and the registered plugins) alive for the test's duration.
    pub _kernel: Kernel,
    pub ctx: Context,
    pipeline: ToolExecutionPipeline,
}

pub async fn setup() -> Env {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    for plugin in [
        ctx.plugin::<SystemPrompt>(json!({})).unwrap(),
        ctx.plugin::<ToolRuntime>(json!({})).unwrap(),
    ] {
        plugin.settled().await.unwrap();
    }
    let tools = ctx.get::<Tools>().unwrap();
    let bash = define_tool(DefineToolOptions::new(
        "bash",
        "test bash",
        json!({ "command": { "type": "string" } }),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, value| vec![hoh_llm::ContentBlock::text(value.as_str().unwrap())]),
            presentation_meta: None,
        },
        |_, _| Box::pin(async { Ok(json!("ok")) }),
    ))
    .unwrap();
    std::mem::forget(
        tools
            .register(&ctx, bash, ToolRegistrationOptions::default())
            .unwrap(),
    );
    let pipeline = ToolExecutionPipeline::new(ctx.clone()).unwrap();
    Env {
        _kernel: kernel,
        ctx,
        pipeline,
    }
}

impl Env {
    /// The `execution(sessionId?)` fixture of the source.
    pub async fn execution(&self, session_id: Option<&str>) -> Arc<ToolExecution> {
        let mut input = ToolExecutionInput::new(
            "bash-env-call".into(),
            "bash",
            json!({ "command": "true" }),
            AbortController::new().signal(),
        );
        if let Some(id) = session_id {
            input.agent = Some(Arc::new(TestAgent {
                key: ScopeKey::new(),
                session: Session::create(id.into(), None, None, None, None).unwrap(),
            }));
        }
        self.pipeline.prepare(input).await.exec().clone()
    }
}
