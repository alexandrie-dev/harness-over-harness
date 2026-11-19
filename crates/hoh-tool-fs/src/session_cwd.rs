//! Resolution options for filesystem calls in the Session's current directory. Non-agent calls
//! leave directory defaults to the filesystem provider.

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_fs::FsResolveOptions;
use hoh_tools::{ToolContextUnavailableError, ToolExecution};

use crate::Env;
use crate::shim::live_agent;

/// Resolve the current directory and cancellation for a filesystem call.
///
/// `exec` is the tool execution supplying the agent and cancellation. Returns the filesystem
/// resolution options for this call.
pub async fn session_resolve_options(
    env: &Env,
    exec: &Arc<ToolExecution>,
) -> Result<FsResolveOptions, BoxError> {
    let signal = exec.signal();
    let cwd = match &exec.agent {
        None => None,
        Some(agent) => {
            let live = live_agent(&env.ctx, agent).ok_or_else(|| {
                ToolContextUnavailableError::new(
                    "tool-fs: the calling agent is not registered in the agents registry",
                )
            })?;
            Some(env.working_directory.ensure(&live, Some(&signal)).await?)
        }
    };
    Ok(FsResolveOptions {
        cwd,
        signal: Some(signal),
    })
}
