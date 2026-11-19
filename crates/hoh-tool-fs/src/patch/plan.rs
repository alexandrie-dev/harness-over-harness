//! Patch parsing and target identity shared by preflight and sequential commits.

use std::collections::HashMap;
use std::sync::Arc;

use hoh_error::{BoxError, HarnessError};
use hoh_fs::{
    FsActor, FsDeleteIntent, FsEditIntent, FsError, FsTarget, FsTargetKey, FsVersionGuard,
    FsWriteIntent, FsWriteIntentEvent,
};
use hoh_fs_patch::{PatchOperation, parse_patch};
use hoh_sandbox::SandboxExecutionPolicy;
use hoh_tools::ToolExecution;

use super::types::{
    PatchExecutionRequest, PatchFailure, PatchOperationKind, PatchOperationRecord, PatchPlan,
    PatchRefusal, PlannedKind, PlannedOperation,
};
use crate::Env;
use crate::error::remediate_fs_error;
use crate::sandbox::FsSandboxController;
use crate::session_cwd::session_resolve_options;

/// The filesystem error inside `error`, when it is one; any other error is a programming defect
/// the caller propagates.
pub fn as_fs_error(error: &BoxError) -> Option<&HarnessError> {
    HarnessError::of(error.as_ref()).filter(|found| found.is_named(FsError::NAME))
}

/// Resolve every operation without dropping repeated aliases or splitting moves.
///
/// `env` owns the filesystem service, `sandbox` maps filesystem diagnostics, `exec` supplies the
/// session cwd and execution signal, and `request` is the raw patch with its resolved per-call
/// policy. Returns resolved operations sharing one target per canonical key, plus resolution
/// refusals. A malformed patch fails with the parser's error.
pub async fn plan_patch(
    env: &Env,
    sandbox: &FsSandboxController,
    exec: &Arc<ToolExecution>,
    request: &PatchExecutionRequest,
) -> Result<PatchPlan, BoxError> {
    let parsed = parse_patch(&request.patch)?;
    let mut targets: HashMap<FsTargetKey, FsTarget> = HashMap::new();
    let mut result = PatchPlan::default();
    for (index, operation) in parsed.into_iter().enumerate() {
        let (path, line_number, move_to) = match &operation {
            PatchOperation::Add {
                path, line_number, ..
            }
            | PatchOperation::Delete { path, line_number } => (path.clone(), *line_number, None),
            PatchOperation::Update {
                path,
                line_number,
                move_to,
                ..
            } => (path.clone(), *line_number, move_to.clone()),
        };
        let mut record = PatchOperationRecord {
            index,
            kind: match (&operation, &move_to) {
                (PatchOperation::Update { .. }, Some(_)) => PatchOperationKind::Move,
                (PatchOperation::Update { .. }, None) => PatchOperationKind::Update,
                (PatchOperation::Add { .. }, _) => PatchOperationKind::Add,
                (PatchOperation::Delete { .. }, _) => PatchOperationKind::Delete,
            },
            path: path.clone(),
            destination: move_to.clone(),
            patch_line: line_number,
        };
        let mut failure_path = path.clone();
        let resolved: Result<PlannedOperation, BoxError> = async {
            let target = resolve(env, exec, &mut targets, &path).await?;
            record.path = target.display_path.clone();
            let kind = match operation {
                PatchOperation::Add { content, .. } => PlannedKind::Add { content },
                PatchOperation::Delete { .. } => PlannedKind::Delete,
                PatchOperation::Update {
                    hunks,
                    move_to: None,
                    ..
                } => PlannedKind::Update { hunks },
                PatchOperation::Update {
                    hunks,
                    move_to: Some(move_to),
                    ..
                } => {
                    failure_path = move_to.clone();
                    let destination = resolve(env, exec, &mut targets, &move_to).await?;
                    record.destination = Some(destination.display_path.clone());
                    PlannedKind::Move { destination, hunks }
                }
            };
            Ok(PlannedOperation {
                record: record.clone(),
                target,
                raw_path: path.clone(),
                kind,
            })
        }
        .await;
        match resolved {
            Ok(planned) => result.operations.push(planned),
            Err(error) => match as_fs_error(&error) {
                Some(fs_error) => result.refusals.push(PatchRefusal {
                    operation: record,
                    failure: patch_failure(
                        sandbox,
                        request.sandbox_policy.as_ref(),
                        fs_error,
                        &failure_path,
                    ),
                }),
                None => return Err(error),
            },
        }
    }
    Ok(result)
}

async fn resolve(
    env: &Env,
    exec: &Arc<ToolExecution>,
    targets: &mut HashMap<FsTargetKey, FsTarget>,
    path: &str,
) -> Result<FsTarget, BoxError> {
    let options = session_resolve_options(env, exec).await?;
    let target = env.fs.resolve(path, options).await?;
    if let Some(prior) = targets.get(&target.target_key) {
        return Ok(prior.clone());
    }
    targets.insert(target.target_key.clone(), target.clone());
    Ok(target)
}

/// Which source policy a source guard follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Delete,
    Edit,
}

/// Obtain a source guard without recording an observation. `None` for a bare provider.
pub async fn source_intent(
    env: &Env,
    exec: &Arc<ToolExecution>,
    target: &FsTarget,
    kind: SourceKind,
) -> Result<Option<FsVersionGuard>, BoxError> {
    let actor = Some(exec.clone() as FsActor);
    let target = target.clone();
    match kind {
        SourceKind::Delete => {
            env.ctx
                .waterfall(FsDeleteIntent { target, actor }, |_| async { Ok(None) })
                .await
        }
        SourceKind::Edit => {
            env.ctx
                .waterfall(FsEditIntent { target, actor }, |_| async { Ok(None) })
                .await
        }
    }
}

/// Obtain a write guard without recording an observation. `None` for a bare provider.
pub async fn destination_intent(
    env: &Env,
    exec: &Arc<ToolExecution>,
    target: &FsTarget,
) -> Result<Option<FsWriteIntent>, BoxError> {
    env.ctx
        .waterfall(
            FsWriteIntentEvent {
                target: target.clone(),
                actor: Some(exec.clone() as FsActor),
            },
            |_| async { Ok(None) },
        )
        .await
}

/// Preserve filesystem codes while applying the existing model-facing error policy.
///
/// `sandbox` is the shared denial mapper, `policy` the policy supplied by the request, `error` the
/// expected filesystem failure and `path` the display path whose operation failed.
pub fn patch_failure(
    sandbox: &FsSandboxController,
    policy: Option<&SandboxExecutionPolicy>,
    error: &HarnessError,
    path: &str,
) -> PatchFailure {
    let mapped = sandbox
        .map_fs_error(error, policy)
        .unwrap_or_else(|| error.clone());
    let remediated = remediate_fs_error(&mapped, path);
    PatchFailure::new(remediated.code(), remediated.message())
}

/// List all targets the operation can mutate, with destination first for a move.
pub fn mutation_targets(operation: &PlannedOperation) -> Vec<&FsTarget> {
    match &operation.kind {
        PlannedKind::Move { destination, .. } => vec![destination, &operation.target],
        _ => vec![&operation.target],
    }
}
