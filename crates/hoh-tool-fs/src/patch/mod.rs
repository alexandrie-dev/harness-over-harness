//! Internal patch executor; tool registration and persisted result metadata are owned by its caller.

pub mod execute;
pub mod plan;
pub mod preflight;
pub mod types;

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_fs_patch::PatchParseError;
use hoh_tools::ToolExecution;

use crate::Env;
use crate::sandbox::FsSandboxController;
use execute::commit_patch;
use plan::plan_patch;
use preflight::preflight_patch;
pub use types::{
    PatchExecutionReport, PatchExecutionRequest, PatchFailure, PatchFileChange, PatchOperationKind,
    PatchOperationOutcome, PatchOperationRecord, PatchRefusal,
};

/// Preflight a whole patch before sequential guarded mutations; business failures become records.
///
/// `env` carries the mounted filesystem and observation-policy events, `sandbox` is the denial
/// mapper shared with the other filesystem tools, `exec` supplies the session cwd, observation
/// actor and cancellation signal, and `request` is the raw patch with its resolved policy.
/// Returns the syntax failure, the preflight refusals, or the ordered commit outcomes. An
/// unexpected defect (an error that is not a filesystem error) is returned as `Err` without
/// starting further operations.
pub async fn execute_patch(
    env: &Env,
    sandbox: &FsSandboxController,
    exec: &Arc<ToolExecution>,
    request: &PatchExecutionRequest,
) -> Result<PatchExecutionReport, BoxError> {
    let plan = match plan_patch(env, sandbox, exec, request).await {
        Ok(plan) => plan,
        Err(error) => {
            return match error.downcast_ref::<PatchParseError>() {
                Some(parse) => Ok(PatchExecutionReport::Syntax {
                    failure: PatchFailure::new("PATCH_SYNTAX", parse.to_string()),
                }),
                None => Err(error),
            };
        }
    };
    let mut refusals = plan.refusals.clone();
    refusals.extend(
        preflight_patch(
            env,
            sandbox,
            exec,
            request.sandbox_policy.as_ref(),
            &plan.operations,
        )
        .await?,
    );
    refusals.sort_by_key(|refusal| refusal.operation.index);
    if !refusals.is_empty() {
        return Ok(PatchExecutionReport::Preflight { refusals });
    }
    commit_patch(
        env,
        sandbox,
        exec,
        request.sandbox_policy.as_ref(),
        &plan.operations,
    )
    .await
}
