//! Semantic durability barriers before model requests, top-level tools and agent steps.

use hoh_agent::{AgentPreStep, PreStepDecision};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service};
use hoh_llm::{ContentBlock, Llm, LlmStream, LlmStreamEvent};
use hoh_session::Sessions;
use hoh_session_persistence::SessionPersistenceService;
use hoh_tools::{
    TOOL_ABORTED_BEFORE_DISPATCH, ToolErrorInfo, ToolExecutionFailure, ToolExecutionResult,
    ToolFailure, Tools, ToolsExecute,
};

fn aborted_before_dispatch_result() -> ToolExecutionResult {
    ToolExecutionResult::Failure(ToolExecutionFailure::new(
        ToolFailure {
            message: "tool call aborted before dispatch".into(),
            info: Some(ToolErrorInfo::new(
                "AbortError",
                TOOL_ABORTED_BEFORE_DISPATCH,
            )),
        },
        vec![ContentBlock::text(
            "Error: tool call aborted before dispatch",
        )],
    ))
}

/// Services whose request, tool, session and persistence boundaries are joined.
#[derive(Inject)]
pub struct SessionCheckpointPolicyDeps {
    llm: Service<Llm>,
    session_persistence: Service<SessionPersistenceService>,
    sessions: Service<Sessions>,
    tools: Service<Tools>,
}

/// Listener-only checkpoint policy. It owns no configuration or durable state.
pub struct SessionCheckpointPolicy;

impl Plugin for SessionCheckpointPolicy {
    const NAME: &'static str = "session-checkpoint-policy";
    type Config = NoConfig;
    type Deps = SessionCheckpointPolicyDeps;

    async fn apply(
        ctx: Context,
        deps: SessionCheckpointPolicyDeps,
        _config: Cfg<NoConfig>,
    ) -> Result<(), BoxError> {
        // Retaining the injected services makes the source's complete dependency boundary explicit.
        let _llm = deps.llm;
        let _persistence = deps.session_persistence;
        let sessions = deps.sessions;
        let _tools = deps.tools;

        let request_sessions = sessions.clone();
        ctx.on_waterfall::<LlmStreamEvent, LlmStream, _>(move |event, next| {
            let sessions = request_sessions.clone();
            async move {
                if let Some(session) = event
                    .options()
                    .session_id
                    .as_ref()
                    .and_then(|id| sessions.get(id))
                {
                    sessions.flush(&session).await?;
                }
                next.run().await
            }
        })?;

        let tool_sessions = sessions.clone();
        ctx.on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
            let sessions = tool_sessions.clone();
            async move {
                let Some(agent) = &event.exec.agent else {
                    return next.run().await;
                };
                if event.exec.parent.is_some() {
                    return next.run().await;
                }
                sessions.flush(agent.session()).await?;
                if event.exec.signal().aborted() {
                    return Ok(aborted_before_dispatch_result());
                }
                next.run().await
            }
        })?;

        ctx.on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            let sessions = sessions.clone();
            async move {
                sessions.flush(event.agent.session()).await?;
                next.run().await
            }
        })?;

        Ok(())
    }
}
