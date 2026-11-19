//! Shared path resolution and regular-file validation for model-facing read tools.

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_fs::{FsActor, FsError, FsErrorCode, FsInfo, FsObservation, FsObserved, FsTarget, FsType};
use hoh_tools::ToolExecution;

use crate::Env;
use crate::session_cwd::session_resolve_options;

/// Resolve a model-supplied path, observe absence, and require a regular file.
///
/// `exec` is the current tool execution, including the owning Agent and cancellation;
/// `requested_path` is the raw path supplied to the tool. Returns the resolved target and its
/// single stat result.
pub async fn resolve_regular_read_target(
    env: &Env,
    exec: &Arc<ToolExecution>,
    requested_path: &str,
) -> Result<(FsTarget, FsInfo), BoxError> {
    let options = session_resolve_options(env, exec).await?;
    let target = env.fs.resolve(requested_path, options).await?;
    let info = env.fs.stat(&target, Some(exec.signal())).await?;
    let Some(info) = info else {
        env.ctx.emit(&FsObserved {
            target: target.clone(),
            observation: FsObservation::Absent,
            actor: Some(exec.clone() as FsActor),
        });
        return Err(FsError::new(
            format!("cannot read \"{}\": not found", target.display_path),
            FsErrorCode::NotFound,
        )
        .into());
    };
    if info.r#type != FsType::File {
        return Err(FsError::new(
            format!(
                "cannot read \"{}\": not a regular file",
                target.display_path
            ),
            FsErrorCode::NotRegularFile,
        )
        .into());
    }
    Ok((target, info))
}
