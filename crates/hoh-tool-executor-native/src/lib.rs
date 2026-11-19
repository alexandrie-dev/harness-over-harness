//! Port of @deepseek-ai/dsh-tool-executor-native (packages/core/tool-executor-native).
//! Native execution plugin for the mounting Agent, preset, or deployment scope.

use std::sync::Arc;

use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Service, plugin};
use hoh_tools::shim::BoxFuture;
use hoh_tools::{
    ToolBatchInput, ToolBatchResult, ToolDefinition, ToolExecutionPipeline, ToolExecutor,
    ToolModelInput, Tools, execute_tool_batch, project_tool_schema,
};

/// Executes visible capabilities directly and owns their complete batch settlement.
pub struct NativeToolExecutor {
    pipeline: ToolExecutionPipeline,
    max_parallel: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl NativeToolExecutor {
    /// An executor with the default concurrency limit of 10.
    pub fn new(ctx: Context) -> Result<Self, HarnessError> {
        Self::with_max_parallel(ctx, || 10)
    }

    /// An executor whose concurrency limit is read from `max_parallel` at every batch.
    pub fn with_max_parallel(
        ctx: Context,
        max_parallel: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Result<Self, HarnessError> {
        Ok(Self {
            pipeline: ToolExecutionPipeline::new(ctx)?,
            max_parallel: Box::new(max_parallel),
        })
    }
}

impl ToolExecutor for NativeToolExecutor {
    fn describe(&self, tools: &[Arc<ToolDefinition>]) -> Result<ToolModelInput, BoxError> {
        let schemas = tools
            .iter()
            .map(|definition| project_tool_schema(definition, false))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ToolModelInput {
            schemas,
            known_names: None,
        })
    }

    fn execute(&self, input: ToolBatchInput) -> BoxFuture<'_, Result<ToolBatchResult, BoxError>> {
        Box::pin(execute_tool_batch(
            input,
            &self.pipeline,
            &*self.max_parallel,
        ))
    }
}

/// Tool registration and scoped dispatch.
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
}

/// Select native execution in the mounting scope.
#[plugin(NativeToolExecutorPlugin, name = "native-tool-executor")]
pub async fn apply(ctx: Context, deps: Deps, _config: Cfg<NoConfig>) -> Result<(), BoxError> {
    deps.tools
        .set_executor(&ctx, Arc::new(NativeToolExecutor::new(ctx.clone())?))?;
    Ok(())
}
