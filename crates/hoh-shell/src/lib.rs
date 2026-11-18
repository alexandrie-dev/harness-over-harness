//! Port of @deepseek-ai/dsh-shell (packages/shell/shell).

pub mod render;
pub mod types;

use std::sync::Arc;

use hoh_kernel::{Context, Disposer, KernelError};
use hoh_sandbox::SandboxMode;

pub use render::{ParsedExitStatus, parse_exit_status};
use types::ShellFuture;
pub use types::{
    CollectedOutput, DSH_ENV_PREFIX, DshEnvironment, DshEnvironmentKey, ShellExecRequest,
    ShellExecSpec, ShellExecution, ShellExpiryPolicy, ShellProcess, ShellProcessRead,
    ShellProcessStatus, ShellRunResult, ShellSandboxInfo,
};

/// Abstract shell execution service; one implementation is provided as `shell`.
pub trait ShellExecutor: Send + Sync + 'static {
    fn sandbox_mode(&self) -> Option<SandboxMode> {
        None
    }

    fn resolve(&self, request: &ShellExecRequest) -> ShellExecSpec;
    fn execute(&self, spec: ShellExecSpec) -> ShellFuture<'_, Arc<dyn ShellExecution>>;
}

hoh_kernel::service!(pub Shell: dyn ShellExecutor = "shell");

pub fn provide_shell(
    ctx: &Context,
    executor: Arc<dyn ShellExecutor>,
) -> impl Future<Output = Result<Disposer, KernelError>> + Send + 'static {
    ctx.provide::<Shell>(executor)
}
