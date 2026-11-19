//! Read-only simulation of patch effects; no internal read establishes model observation.

use std::collections::HashMap;
use std::sync::Arc;

use hoh_error::BoxError;
use hoh_fs::{
    FsError, FsErrorCode, FsInfo, FsPathType, FsTarget, FsTargetKey, FsType, FsWriteIntent,
};
use hoh_fs_patch::{ApplyHunksResult, apply_hunks};
use hoh_sandbox::SandboxExecutionPolicy;
use hoh_tools::ToolExecution;

use super::plan::{SourceKind, as_fs_error, destination_intent, patch_failure, source_intent};
use super::types::{PatchFailure, PatchRefusal, PlannedKind, PlannedOperation};
use crate::Env;
use crate::sandbox::FsSandboxController;
use crate::session_cwd::session_resolve_options;

struct SimulatedFile {
    exists: bool,
    info: Option<FsInfo>,
    content: Option<String>,
    touched: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceOp {
    Delete,
    Edit,
    Move,
}

impl SourceOp {
    fn verb(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Edit => "edit",
            Self::Move => "move",
        }
    }
}

struct Preflight<'a> {
    env: &'a Env,
    sandbox: &'a FsSandboxController,
    exec: &'a Arc<ToolExecution>,
    policy: Option<&'a SandboxExecutionPolicy>,
    failure_path: String,
    states: HashMap<FsTargetKey, SimulatedFile>,
}

impl<'a> Preflight<'a> {
    async fn ensure_state(&mut self, target: &FsTarget) -> Result<(), BoxError> {
        if self.states.contains_key(&target.target_key) {
            return Ok(());
        }
        let info = self.env.fs.stat(target, Some(self.exec.signal())).await?;
        self.states.insert(
            target.target_key.clone(),
            SimulatedFile {
                exists: info.is_some(),
                info,
                content: None,
                touched: false,
            },
        );
        Ok(())
    }

    fn regular(&self, target: &FsTarget) -> Result<(), BoxError> {
        let state = &self.states[&target.target_key];
        if state
            .info
            .as_ref()
            .is_some_and(|info| info.r#type != FsType::File)
        {
            return Err(FsError::new(
                format!(
                    "cannot modify \"{}\": not a regular file",
                    target.display_path
                ),
                FsErrorCode::NotRegularFile,
            )
            .into());
        }
        Ok(())
    }

    async fn source(&mut self, target: &FsTarget, op: SourceOp) -> Result<(), BoxError> {
        let touched = self.states[&target.target_key].touched;
        let intent = if touched {
            None
        } else {
            source_intent(
                self.env,
                self.exec,
                target,
                if op == SourceOp::Delete {
                    SourceKind::Delete
                } else {
                    SourceKind::Edit
                },
            )
            .await?
        };
        let state = &self.states[&target.target_key];
        if !state.exists {
            return Err(FsError::new(
                format!(
                    "cannot {} \"{}\": not found",
                    op.verb(),
                    target.display_path
                ),
                FsErrorCode::NotFound,
            )
            .into());
        }
        self.regular(target)?;
        let state = &self.states[&target.target_key];
        if let Some(intent) = intent
            && state.info.as_ref().map(|info| &info.version) != Some(&intent.version)
        {
            return Err(FsError::new(
                format!(
                    "cannot {} \"{}\": file changed since it was read",
                    op.verb(),
                    target.display_path
                ),
                FsErrorCode::StaleVersion,
            )
            .into());
        }
        Ok(())
    }

    async fn destination(&mut self, target: &FsTarget) -> Result<(), BoxError> {
        let touched = self.states[&target.target_key].touched;
        let intent = if touched {
            None
        } else {
            destination_intent(self.env, self.exec, target).await?
        };
        self.regular(target)?;
        let state = &self.states[&target.target_key];
        match intent {
            Some(FsWriteIntent::CreateIfAbsent) if state.exists => Err(FsError::new(
                format!(
                    "cannot overwrite existing \"{}\" without reading it first",
                    target.display_path
                ),
                FsErrorCode::NotObserved,
            )
            .into()),
            Some(FsWriteIntent::ReplaceIfVersion { version })
                if !state.exists
                    || state.info.as_ref().map(|info| &info.version) != Some(&version) =>
            {
                Err(FsError::new(
                    format!(
                        "cannot write \"{}\": file changed since it was read",
                        target.display_path
                    ),
                    FsErrorCode::StaleVersion,
                )
                .into())
            }
            _ => Ok(()),
        }
    }

    async fn content(&mut self, target: &FsTarget) -> Result<String, BoxError> {
        if self.states[&target.target_key].content.is_none() {
            let text = self
                .env
                .fs
                .read_text(target, Some(self.exec.signal()))
                .await?;
            // read_text samples only a prefix for NUL bytes; the mutation path rejects any NUL.
            if text.contains('\0') {
                return Err(FsError::new(
                    format!("cannot patch \"{}\": binary file", target.display_path),
                    FsErrorCode::NotText,
                )
                .into());
            }
            if let Some(state) = self.states.get_mut(&target.target_key) {
                state.content = Some(text.replace("\r\n", "\n"));
            }
        }
        Ok(self.states[&target.target_key]
            .content
            .clone()
            .unwrap_or_default())
    }

    fn publish(&mut self, target: &FsTarget, content: Option<String>) {
        self.states.insert(
            target.target_key.clone(),
            SimulatedFile {
                exists: content.is_some(),
                info: None,
                content,
                touched: true,
            },
        );
    }

    /// Simulate one operation; a filesystem refusal becomes the operation's failure.
    async fn check(
        &mut self,
        operation: &PlannedOperation,
    ) -> Result<Option<PatchFailure>, BoxError> {
        self.failure_path = operation.target.display_path.clone();
        match self.simulate(operation).await {
            Ok(failure) => Ok(failure),
            Err(error) => match as_fs_error(&error) {
                Some(fs_error) => Ok(Some(patch_failure(
                    self.sandbox,
                    self.policy,
                    fs_error,
                    &self.failure_path,
                ))),
                None => Err(error),
            },
        }
    }

    async fn simulate(
        &mut self,
        operation: &PlannedOperation,
    ) -> Result<Option<PatchFailure>, BoxError> {
        let target = &operation.target;
        if let PlannedKind::Move { destination, .. } = &operation.kind
            && target.target_key == destination.target_key
        {
            return Err(FsError::new(
                format!("cannot move \"{}\" to itself", target.display_path),
                FsErrorCode::SameTarget,
            )
            .into());
        }
        if matches!(
            operation.kind,
            PlannedKind::Delete | PlannedKind::Move { .. }
        ) {
            let options = session_resolve_options(self.env, self.exec).await?;
            let info = self
                .env
                .fs
                .lstat(
                    &operation.raw_path,
                    options.cwd.as_deref(),
                    options.signal.clone(),
                )
                .await?;
            if info.is_some_and(|info| info.r#type == FsPathType::Symlink) {
                return Ok(Some(PatchFailure::new(
                    "PATCH_SYMLINK_SOURCE",
                    format!(
                        "cannot {} \"{}\": path is a symbolic link — operate on its target path instead",
                        operation.record.kind.as_str(),
                        target.display_path
                    ),
                )));
            }
        }
        self.ensure_state(target).await?;
        match &operation.kind {
            PlannedKind::Add { content } => {
                self.destination(target).await?;
                self.publish(target, Some(content.clone()));
                Ok(None)
            }
            PlannedKind::Delete => {
                self.source(target, SourceOp::Delete).await?;
                self.publish(target, None);
                Ok(None)
            }
            PlannedKind::Update { hunks } => {
                self.source(target, SourceOp::Edit).await?;
                self.apply(operation, target, hunks).await
            }
            PlannedKind::Move { destination, hunks } => {
                self.source(target, SourceOp::Move).await?;
                self.failure_path = destination.display_path.clone();
                self.ensure_state(destination).await?;
                self.destination(destination).await?;
                self.failure_path = target.display_path.clone();
                self.apply(operation, target, hunks).await
            }
        }
    }

    async fn apply(
        &mut self,
        operation: &PlannedOperation,
        target: &FsTarget,
        hunks: &[hoh_fs_patch::PatchHunk],
    ) -> Result<Option<PatchFailure>, BoxError> {
        let before = self.content(target).await?;
        let after = match apply_hunks(&before, hunks) {
            ApplyHunksResult::Success { after } => after,
            ApplyHunksResult::Failure { failure } => {
                let first = failure.pattern.first().cloned().unwrap_or_default();
                let characters: Vec<char> = first.chars().collect();
                let preview = if characters.len() > 80 {
                    format!("{}…", characters[..80].iter().collect::<String>())
                } else {
                    first
                };
                return Ok(Some(PatchFailure {
                    code: FsErrorCode::PatchMismatch.as_str().to_owned(),
                    hunk_line: Some(failure.hunk_line_number),
                    message: format!(
                        "cannot patch \"{}\": hunk at patch line {} ({}) does not match — {}",
                        target.display_path,
                        failure.hunk_line_number,
                        stage_name(&failure.stage),
                        preview
                    ),
                }));
            }
        };
        if let PlannedKind::Move { destination, .. } = &operation.kind {
            self.publish(destination, Some(after));
            self.publish(target, None);
        } else {
            self.publish(target, Some(after));
        }
        Ok(None)
    }
}

fn stage_name(stage: &hoh_fs_patch::types::FailureStage) -> &'static str {
    match stage {
        hoh_fs_patch::types::FailureStage::Context => "context",
        hoh_fs_patch::types::FailureStage::OldLines => "oldLines",
    }
}

/// Collect every independent preflight refusal while simulating only admitted operations.
/// Sandbox containment is the provider's decision at commit, as for write and edit.
///
/// `env` owns the filesystem service (reads never emit observations), `sandbox` is the
/// model-facing denial mapper, `exec` supplies the observation actor and cancellation signal,
/// `policy` is the resolved confinement policy used to label refusals and `operations` the
/// complete resolved plan. Returns refused operations in patch order; any refusal forbids the
/// commit phase.
pub async fn preflight_patch(
    env: &Env,
    sandbox: &FsSandboxController,
    exec: &Arc<ToolExecution>,
    policy: Option<&SandboxExecutionPolicy>,
    operations: &[PlannedOperation],
) -> Result<Vec<PatchRefusal>, BoxError> {
    let mut preflight = Preflight {
        env,
        sandbox,
        exec,
        policy,
        failure_path: String::new(),
        states: HashMap::new(),
    };
    let mut refusals = Vec::new();
    for operation in operations {
        if let Some(failure) = preflight.check(operation).await? {
            refusals.push(PatchRefusal {
                operation: operation.record.clone(),
                failure,
            });
        }
    }
    Ok(refusals)
}
