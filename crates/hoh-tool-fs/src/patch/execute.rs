//! Sequential provider commits with truthful partial results and uncertainty checks.

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_fs::{
    FsActor, FsError, FsErrorCode, FsInfo, FsMoveOptions, FsMoveOutcome, FsObservation, FsObserved,
    FsTarget,
};
use hoh_sandbox::SandboxExecutionPolicy;
use hoh_tools::ToolExecution;

use super::plan::{
    SourceKind, as_fs_error, destination_intent, mutation_targets, patch_failure, source_intent,
};
use super::types::{
    PatchExecutionReport, PatchFailure, PatchFileChange, PatchOperationOutcome, PlannedKind,
    PlannedOperation,
};
use crate::Env;
use crate::sandbox::FsSandboxController;

struct BeforeTarget {
    target: FsTarget,
    info: Option<FsInfo>,
}

async fn unchanged(env: &Env, before: &[BeforeTarget]) -> Result<bool, BoxError> {
    for entry in before {
        // An aborted tool signal must not cancel the independent outcome check.
        match env.fs.stat(&entry.target, None).await {
            Ok(after) => {
                if after.map(|info| info.version)
                    != entry.info.as_ref().map(|info| info.version.clone())
                {
                    return Ok(false);
                }
            }
            Err(error) => {
                return if as_fs_error(&error).is_some() {
                    Ok(false)
                } else {
                    Err(error)
                };
            }
        }
    }
    Ok(true)
}

fn observe(env: &Env, exec: &Arc<ToolExecution>, target: &FsTarget, observation: FsObservation) {
    env.ctx.emit(&FsObserved {
        target: target.clone(),
        observation,
        actor: Some(exec.clone() as FsActor),
    });
}

/// Mutable progress of one operation, kept outside the fallible body so the error path can tell
/// whether the provider call had started.
struct Progress {
    started: bool,
    before: Vec<BeforeTarget>,
    failure_path: String,
}

async fn run_operation(
    env: &Env,
    sandbox: &FsSandboxController,
    exec: &Arc<ToolExecution>,
    policy: Option<&SandboxExecutionPolicy>,
    operation: &PlannedOperation,
    progress: &mut Progress,
) -> Result<PatchOperationOutcome, BoxError> {
    let target = &operation.target;
    let record = operation.record.clone();
    let signal = exec.signal();
    for entry in mutation_targets(operation) {
        let info = env.fs.stat(entry, Some(signal.clone())).await?;
        progress.before.push(BeforeTarget {
            target: entry.clone(),
            info,
        });
    }
    let path = env.fs.process_path(target);
    match &operation.kind {
        PlannedKind::Add { content } => {
            let intent = destination_intent(env, exec, target).await?;
            progress.started = true;
            let result = env
                .fs
                .write_text(target, content, intent, Some(signal), policy.cloned())
                .await?;
            observe(
                env,
                exec,
                target,
                FsObservation::Present {
                    version: result.version.clone(),
                },
            );
            Ok(PatchOperationOutcome::Completed {
                operation: record,
                changes: vec![PatchFileChange {
                    path,
                    before: result.before,
                    after: Some(result.after),
                }],
            })
        }
        PlannedKind::Update { hunks } => {
            let intent = source_intent(env, exec, target, SourceKind::Edit).await?;
            progress.started = true;
            let result = env
                .fs
                .patch_text(target, hunks, intent, Some(signal), policy.cloned())
                .await?;
            observe(
                env,
                exec,
                target,
                FsObservation::Present {
                    version: result.version.clone(),
                },
            );
            Ok(PatchOperationOutcome::Completed {
                operation: record,
                changes: vec![PatchFileChange {
                    path,
                    before: Some(result.before),
                    after: Some(result.after),
                }],
            })
        }
        PlannedKind::Delete => {
            let intent = source_intent(env, exec, target, SourceKind::Delete).await?;
            progress.started = true;
            let result = env
                .fs
                .delete_file(target, intent, Some(signal), policy.cloned())
                .await?;
            observe(env, exec, target, FsObservation::Absent);
            Ok(PatchOperationOutcome::Completed {
                operation: record,
                changes: vec![PatchFileChange {
                    path,
                    before: result.before,
                    after: None,
                }],
            })
        }
        PlannedKind::Move { destination, hunks } => {
            let expected_source = source_intent(env, exec, target, SourceKind::Edit).await?;
            progress.failure_path = destination.display_path.clone();
            let expected_destination = destination_intent(env, exec, destination).await?;
            progress.started = true;
            let result = env
                .fs
                .move_file(
                    target,
                    destination,
                    FsMoveOptions {
                        hunks: Some(hunks.clone()),
                        expected_source,
                        expected_destination,
                    },
                    Some(signal),
                    policy.cloned(),
                )
                .await?;
            let (source_before, written, retained) = match result {
                FsMoveOutcome::Deleted {
                    source_before,
                    destination,
                } => (source_before, destination, None),
                FsMoveOutcome::Retained {
                    source_before,
                    destination,
                    source_failure,
                } => (source_before, destination, Some(source_failure)),
            };
            let mut changes = vec![PatchFileChange {
                path: env.fs.process_path(destination),
                before: written.before,
                after: Some(written.after),
            }];
            observe(
                env,
                exec,
                destination,
                FsObservation::Present {
                    version: written.version,
                },
            );
            match retained {
                None => {
                    observe(env, exec, target, FsObservation::Absent);
                    changes.push(PatchFileChange {
                        path,
                        before: source_before,
                        after: None,
                    });
                    Ok(PatchOperationOutcome::Completed {
                        operation: record,
                        changes,
                    })
                }
                Some(source_failure) => {
                    let error = FsError::new(source_failure.message, source_failure.code);
                    Ok(PatchOperationOutcome::Failed {
                        operation: record,
                        failure: patch_failure(sandbox, policy, &error, &target.display_path),
                        changes,
                    })
                }
            }
        }
    }
}

/// Commit a fully admitted plan; the first failure stops later operations without rollback.
///
/// `env` is the provider and observation dispatcher, `sandbox` the existing model-facing denial
/// mapper, `exec` the session actor and cancellation signal, `policy` the request's resolved
/// confinement policy and `operations` the operations that all passed preflight. Returns known
/// changes, failed or unknown effects, and skipped successors in patch order.
pub async fn commit_patch(
    env: &Env,
    sandbox: &FsSandboxController,
    exec: &Arc<ToolExecution>,
    policy: Option<&SandboxExecutionPolicy>,
    operations: &[PlannedOperation],
) -> Result<PatchExecutionReport, BoxError> {
    let mut outcomes: Vec<PatchOperationOutcome> = operations
        .iter()
        .map(|operation| PatchOperationOutcome::Skipped {
            operation: operation.record.clone(),
        })
        .collect();
    let mut cancelled = false;
    for (index, operation) in operations.iter().enumerate() {
        if exec.signal().aborted() {
            cancelled = true;
            break;
        }
        let mut progress = Progress {
            started: false,
            before: Vec::new(),
            failure_path: operation.target.display_path.clone(),
        };
        let outcome =
            match run_operation(env, sandbox, exec, policy, operation, &mut progress).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    let Some(fs_error) = as_fs_error(&error) else {
                        return Err(error);
                    };
                    let failure = patch_failure(sandbox, policy, fs_error, &progress.failure_path);
                    // A cancelled remote request rejects while the provider may still be writing, so
                    // no stat can settle an aborted mutation; an I/O failure is settled by unchanged
                    // versions.
                    let code = fs_error.code();
                    let uncertain = progress.started
                        && (code == FsErrorCode::Aborted.as_str()
                            || (code == FsErrorCode::IoError.as_str()
                                && !unchanged(env, &progress.before).await?));
                    if uncertain {
                        PatchOperationOutcome::Unknown {
                            operation: operation.record.clone(),
                            failure: PatchFailure {
                                message: format!(
                                    "{} — re-read the affected files before retrying",
                                    failure.message
                                ),
                                ..failure
                            },
                        }
                    } else {
                        PatchOperationOutcome::Failed {
                            operation: operation.record.clone(),
                            failure,
                            changes: Vec::new(),
                        }
                    }
                }
            };
        let completed = matches!(outcome, PatchOperationOutcome::Completed { .. });
        outcomes[index] = outcome;
        if !completed {
            cancelled = exec.signal().aborted();
            break;
        }
    }
    let mut affected_paths: Vec<String> = Vec::new();
    for outcome in &outcomes {
        for change in outcome.changes().unwrap_or_default() {
            if !affected_paths.contains(&change.path) {
                affected_paths.push(change.path.clone());
            }
        }
    }
    Ok(PatchExecutionReport::Commit {
        outcomes,
        affected_paths,
        cancelled,
    })
}
