//! Cooperative per-tool timeout policy.
//!
//! Tools opt in with [`hoh_tools::ToolDefinition::timeout_ms`]. The policy swaps a fused deadline
//! onto the shared execution, waits for downstream cancellation to settle, and maps only its own
//! deadline to [`TOOL_TIMEOUT`]. It never abandons a running tool future.

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service};
use hoh_llm::ContentBlock;
use hoh_timeout::{AbortSignal, deadline, timeout_of};
use hoh_tools::{
    ToolErrorInfo, ToolExecution, ToolExecutionFailure, ToolExecutionResult, ToolFailure, Tools,
    ToolsExecute,
};
use hoh_util_values::js_number_string;

/// Error code owned by this policy and used for both deadline classification and tool results.
pub const TOOL_TIMEOUT: &str = "TOOL_TIMEOUT";

/// The timeout policy's required service boundary.
#[derive(Inject)]
pub struct ToolCallTimeoutPolicyDeps {
    tools: Service<Tools>,
}

/// Listener-only policy; timeout budgets live on individual tool definitions.
pub struct ToolCallTimeoutPolicy;

/// Restores the caller-visible signal when the wrapper settles, errors, or is cancelled.
struct SignalRestorer {
    exec: Arc<ToolExecution>,
    upstream: AbortSignal,
}

impl Drop for SignalRestorer {
    fn drop(&mut self) {
        self.exec.set_signal(self.upstream.clone());
    }
}

fn tool_timeout_result(timeout_ms: f64) -> ToolExecutionResult {
    let message = format!(
        "tool call timed out after {}ms",
        js_number_string(timeout_ms)
    );
    ToolExecutionResult::Failure(ToolExecutionFailure::new(
        ToolFailure {
            message: message.clone(),
            info: Some(ToolErrorInfo::new("ToolTimeoutError", TOOL_TIMEOUT)),
        },
        vec![ContentBlock::text(format!("Error: {message}"))],
    ))
}

impl Plugin for ToolCallTimeoutPolicy {
    const NAME: &'static str = "timeout-policy";
    type Config = NoConfig;
    type Deps = ToolCallTimeoutPolicyDeps;

    async fn apply(
        ctx: Context,
        deps: ToolCallTimeoutPolicyDeps,
        _config: Cfg<NoConfig>,
    ) -> Result<(), BoxError> {
        let tools = deps.tools;
        ctx.on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
            let tools = tools.clone();
            async move {
                let scope = event.exec.agent.as_ref().map(|agent| agent.scope_key());
                let timeout_ms = tools
                    .get(
                        &event.exec.name,
                        scope.as_ref(),
                        event.exec.tool_set.as_ref(),
                    )
                    .and_then(|definition| definition.timeout_ms);
                let Some(timeout_ms) = timeout_ms else {
                    return next.run().await;
                };

                let upstream = event.exec.signal();
                let owned_deadline = deadline(Some(&upstream), timeout_ms, TOOL_TIMEOUT)?;
                event.exec.set_signal(owned_deadline.signal.clone());
                let restorer = SignalRestorer {
                    exec: event.exec.clone(),
                    upstream,
                };

                let result = next.run().await;
                drop(restorer);
                let result = result?;
                if timeout_of(owned_deadline.signal.reason().as_ref(), Some(TOOL_TIMEOUT)).is_some()
                {
                    Ok(tool_timeout_result(timeout_ms))
                } else {
                    Ok(result)
                }
            }
        })?;
        Ok(())
    }
}
