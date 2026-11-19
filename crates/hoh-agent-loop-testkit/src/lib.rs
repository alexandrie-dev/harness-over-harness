//! Shared prerequisite mounting, production loop drivers, and structural Agent stubs for tests.

mod inbox;
mod model;
mod working_directory_fixture;

pub use inbox::{create_inbox_stub, unsupported_inbox};
pub use model::unsupported_model;
pub use working_directory_fixture::provide_working_directory_fixture;

use hoh_agent::{
    Agent, AgentInboxClaimed, AgentOptions, AgentSessionMeta, InboxTarget, agent_events,
};
use hoh_agent_loop::{AgentLoopKey, AgentLoopPlugin};
use hoh_error::BoxError;
use hoh_fs::Fs;
use hoh_fs_local::FsLocalPlugin;
use hoh_kernel::{Context, Plugin};
use hoh_llm::LlmPlugin;
use hoh_session::{SessionId, SessionPlugin};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::{Config as SystemPromptConfig, SystemPrompt};
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tools::ToolRuntime;
use hoh_working_directory::WorkingDirectory;
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// Executor selected for the test topology.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolMode {
    #[default]
    Native,
    Ptc,
}

/// Configuration forwarded to the selected tool executor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolOptions {
    pub mode: ToolMode,
    pub max_parallel_sub_calls: Option<u64>,
}

/// Configuration forwarded to prerequisite service plugins.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentLoopTestDependenciesOptions {
    pub working_directory: bool,
    pub system_prompt: Option<SystemPromptConfig>,
    pub tools: Option<ToolOptions>,
}

async fn mount<P: Plugin>(ctx: &Context, config: Value) -> Result<(), BoxError> {
    let fiber = ctx.plugin::<P>(config)?;
    fiber.settled().await?;
    Ok(())
}

/// Mount the standard prerequisite services, stopping before [`AgentLoopPlugin`].
///
/// PTC execution is intentionally unavailable until the workspace's redesign entries
/// `tool-executor-ptc` and `ptc-runtime` acquire Rust crates.
pub async fn mount_agent_loop_test_dependencies(
    ctx: &Context,
    options: AgentLoopTestDependenciesOptions,
) -> Result<(), BoxError> {
    mount::<LlmPlugin>(ctx, json!({})).await?;
    mount::<SessionPlugin>(ctx, json!({})).await?;
    mount::<SessionProjectionRegistry>(ctx, json!({})).await?;
    let system_prompt = match options.system_prompt {
        Some(config) => serde_json::to_value(config)?,
        None => json!({}),
    };
    mount::<SystemPrompt>(ctx, system_prompt).await?;
    if options.working_directory {
        if ctx.get::<Fs>().is_none() {
            mount::<FsLocalPlugin>(ctx, json!({})).await?;
        }
        mount::<WorkingDirectory>(ctx, json!({})).await?;
    }
    mount::<ToolRuntime>(ctx, json!({})).await?;
    match options.tools.unwrap_or_default() {
        ToolOptions {
            mode: ToolMode::Native,
            ..
        } => mount::<NativeToolExecutorPlugin>(ctx, json!({})).await?,
        ToolOptions {
            mode: ToolMode::Ptc,
            max_parallel_sub_calls: Some(0),
        } => return Err("maxParallelSubCalls must be greater than or equal to 1".into()),
        ToolOptions {
            mode: ToolMode::Ptc,
            ..
        } => {
            return Err(
                "PTC tool executor is unavailable: its Rust redesign is not in the workspace"
                    .into(),
            );
        }
    }
    mount::<hoh_agent::AgentRegistryPlugin>(ctx, json!({})).await?;
    Ok(())
}

/// Narrow driver over production Agents created by a mounted loop.
#[derive(Clone)]
pub struct AgentLoopTestHarness {
    ctx: Context,
}

impl AgentLoopTestHarness {
    /// Create a production Agent with default loop options and metadata.
    pub async fn create(&self, id: SessionId) -> Result<Arc<dyn Agent>, BoxError> {
        self.create_with(id, AgentOptions::default(), AgentSessionMeta::default())
            .await
    }

    /// Create a production Agent with explicit options and fresh-session metadata.
    pub async fn create_with(
        &self,
        id: SessionId,
        options: AgentOptions,
        meta: AgentSessionMeta,
    ) -> Result<Arc<dyn Agent>, BoxError> {
        self.ctx
            .get::<AgentLoopKey>()
            .ok_or_else(|| -> BoxError { "agentLoop service is unavailable".into() })?
            .create(id, options, meta)
            .await
    }

    /// Admit all next-step input and, for a turn claim, one next-turn message.
    pub fn claim(
        &self,
        agent: &Arc<dyn Agent>,
        target: InboxTarget,
        turn: u64,
    ) -> Result<Vec<hoh_llm::UserMessage>, BoxError> {
        let inbox = agent.inbox();
        let mut claimed = claim_messages(agent, InboxTarget::NextStep, inbox.next_step())?;
        if target == InboxTarget::NextTurn {
            claimed.extend(claim_messages(
                agent,
                InboxTarget::NextTurn,
                inbox.next_turn().into_iter().take(1).collect(),
            )?);
        }
        let events = agent_events(agent.ctx(), agent);
        for message in &claimed {
            let message = message.clone();
            events.emit(|agent| AgentInboxClaimed {
                agent,
                message,
                turn,
            });
        }
        Ok(claimed)
    }
}

fn claim_messages(
    agent: &Arc<dyn Agent>,
    target: InboxTarget,
    messages: Vec<hoh_llm::UserMessage>,
) -> Result<Vec<hoh_llm::UserMessage>, BoxError> {
    if messages.is_empty() {
        return Ok(messages);
    }
    let mut data = Map::new();
    data.insert("target".into(), target.as_str().into());
    data.insert("start".into(), 0.into());
    data.insert("removedCount".into(), messages.len().into());
    data.insert("inserted".into(), Value::Array(vec![]));
    agent
        .session()
        .append("agent/inbox/spliced", Value::Object(data), None)?;
    Ok(messages)
}

/// Mount the production AgentLoop after its prerequisite services and return its driver.
pub async fn mount_agent_loop_test_harness(
    ctx: &Context,
) -> Result<AgentLoopTestHarness, BoxError> {
    mount::<AgentLoopPlugin>(ctx, json!({"agents":[]})).await?;
    Ok(AgentLoopTestHarness { ctx: ctx.clone() })
}
