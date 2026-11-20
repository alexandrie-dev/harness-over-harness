#![allow(dead_code)]

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use futures_util::future::BoxFuture;
use hoh_agent::{Agent, Agents, assemble_context_for};
use hoh_agent_loop_testkit::{
    AgentLoopTestDependenciesOptions, AgentLoopTestHarness, mount_agent_loop_test_dependencies,
    mount_agent_loop_test_harness,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::{
    ContentBlock, MessageSource, NewUserMessage, ToolCallId, UserMessage, create_user_message,
};
use hoh_session::{SessionId, Sessions};
use hoh_session_query::{
    Config as QueryConfig, SessionEventSearchPage, SessionEventSearchRequest, SessionQueryError,
    SessionQueryErrorCode, SessionSearchExecContext, SessionSearchHit, SessionSearchPage,
    SessionSearchProvider, SessionSearchRequest, mount_session_query,
};
use hoh_subagent::{SubagentPlugin, Subagents};
use hoh_system_prompt::{PromptAssembly, SystemPromptKey};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_subagent_control::ToolSubagentControl;
use hoh_tool_testkit::execute_tool_fixture;
use hoh_tools::{ToolExecutionInput, ToolExecutionResult, Tools};
use serde_json::Value;

struct SearchDisabled;

impl SessionSearchProvider for SearchDisabled {
    fn search_sessions<'a>(
        &'a self,
        _request: SessionSearchRequest,
        _exec: SessionSearchExecContext,
    ) -> BoxFuture<'a, Result<SessionSearchPage<SessionSearchHit>, BoxError>> {
        Box::pin(async {
            Err(Box::new(SessionQueryError::new(
                "disabled",
                SessionQueryErrorCode::SearchDisabled,
            )) as BoxError)
        })
    }

    fn search_events<'a>(
        &'a self,
        _request: SessionEventSearchRequest,
        _exec: SessionSearchExecContext,
    ) -> BoxFuture<'a, Result<SessionEventSearchPage, BoxError>> {
        Box::pin(async {
            Err(Box::new(SessionQueryError::new(
                "disabled",
                SessionQueryErrorCode::SearchDisabled,
            )) as BoxError)
        })
    }
}

static CALLS: AtomicU64 = AtomicU64::new(1);

pub struct Env {
    pub kernel: Kernel,
    pub ctx: Context,
    pub harness: AgentLoopTestHarness,
    pub parent: Arc<dyn Agent>,
    pub control: Fiber,
}

impl Env {
    pub async fn new(config: Value) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        mount_agent_loop_test_dependencies(
            &ctx,
            AgentLoopTestDependenciesOptions {
                working_directory: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let harness = mount_agent_loop_test_harness(&ctx).await.unwrap();
        mount_session_query(
            &ctx,
            ctx.get::<Sessions>().unwrap(),
            Arc::new(SearchDisabled),
            QueryConfig::default(),
        )
        .await
        .unwrap();
        ctx.plugin::<SubagentPlugin>(serde_json::json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let control = ctx.plugin::<ToolSubagentControl>(config).unwrap();
        control.settled().await.unwrap();
        let parent = harness.create(SessionId::from("parent")).await.unwrap();
        Self {
            kernel,
            ctx,
            harness,
            parent,
            control,
        }
    }

    pub fn tools(&self) -> hoh_kernel::Service<Tools> {
        self.ctx.get::<Tools>().unwrap()
    }

    pub fn agents(&self) -> hoh_kernel::Service<Agents> {
        self.ctx.get::<Agents>().unwrap()
    }

    pub fn subagents(&self) -> hoh_kernel::Service<Subagents> {
        self.ctx.get::<Subagents>().unwrap()
    }

    pub async fn call(
        &self,
        name: &str,
        arguments: Value,
        agent: Option<Arc<dyn Agent>>,
    ) -> ToolExecutionResult {
        self.call_with(name, arguments, agent, live_signal(), None)
            .await
    }

    pub async fn call_with(
        &self,
        name: &str,
        arguments: Value,
        agent: Option<Arc<dyn Agent>>,
        signal: AbortSignal,
        root_call_id: Option<ToolCallId>,
    ) -> ToolExecutionResult {
        let id = CALLS.fetch_add(1, Ordering::Relaxed);
        let mut input = ToolExecutionInput::new(
            ToolCallId::from(format!("call-{id}")),
            name,
            arguments,
            signal,
        );
        input.agent = agent;
        input.root_call_id = root_call_id;
        execute_tool_fixture(&self.ctx, input).await.unwrap()
    }

    pub async fn assembly(&self, agent: &Arc<dyn Agent>) -> PromptAssembly {
        self.ctx
            .get::<SystemPromptKey>()
            .unwrap()
            .assemble(assemble_context_for(agent, None))
            .await
            .unwrap()
    }

    pub async fn close(&self) {
        self.kernel.shutdown().await;
    }
}

pub fn live_signal() -> AbortSignal {
    AbortController::new().signal()
}

pub fn text(result: &ToolExecutionResult) -> String {
    result
        .content()
        .iter()
        .filter_map(ContentBlock::text_value)
        .collect::<Vec<_>>()
        .join("")
}

pub fn error_code(result: &ToolExecutionResult) -> Option<&str> {
    result.error()?.info.as_ref().map(|info| info.code.as_str())
}

pub fn message(value: &str) -> UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(value)],
        source: MessageSource::new("user"),
    })
}
