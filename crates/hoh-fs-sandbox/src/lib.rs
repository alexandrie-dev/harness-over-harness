//! Sandbox-enforcing host filesystem provider.

pub mod containment;

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_fs::{
    FileSystem, Fs, FsByteRange, FsChanged, FsDeleteOutcome, FsDirEntry, FsEditOutcome,
    FsEditRequest, FsError, FsErrorCode, FsFuture, FsInfo, FsMoveOptions, FsMoveOutcome,
    FsPathInfo, FsResolveOptions, FsTarget, FsTextStream, FsVersionGuard, FsWatcher, FsWriteIntent,
    FsWriteOutcome,
};
pub use hoh_fs_local::Config;
use hoh_fs_local::LocalFileSystem;
use hoh_fs_patch::PatchHunk;
use hoh_kernel::{Cfg, Context, Inject, Service, plugin};
use hoh_sandbox::{SandboxExecutionPolicy, SandboxMode, writable_roots};
use hoh_sandbox_policy::{SandboxPolicy, SandboxPolicyRequest, SandboxPolicyService};
use hoh_timeout::AbortSignal;

use crate::containment::is_path_under;

/// The local filesystem backend with a per-mutation sandbox fence.
pub struct SandboxedFileSystem {
    local: LocalFileSystem,
    policy: SandboxPolicyService,
    default_mode: SandboxMode,
}

impl SandboxedFileSystem {
    /// Construct a standalone provider over the local backend.
    pub fn new(config: Config, policy: SandboxPolicyService) -> Result<Self, BoxError> {
        let default_mode = policy.resolve(SandboxPolicyRequest::default())?.mode;
        Ok(Self {
            local: LocalFileSystem::new(config)?,
            policy,
            default_mode,
        })
    }

    async fn checked_target(
        &self,
        target: &FsTarget,
        policy: Option<SandboxExecutionPolicy>,
    ) -> Result<FsTarget, BoxError> {
        let policy = match policy {
            Some(policy) => policy,
            None => self.policy.resolve(SandboxPolicyRequest::default())?,
        };
        match policy.mode {
            SandboxMode::DangerFullAccess => return Ok(target.clone()),
            SandboxMode::ReadOnly => {
                return Err(FsError::new(
                    format!(
                        "cannot write \"{}\": file access denied under read-only mode",
                        target.display_path
                    ),
                    FsErrorCode::SandboxDenied,
                )
                .into());
            }
            SandboxMode::WorkspaceWrite => {}
        }

        // Re-resolve immediately before delegation and mutate this fresh identity,
        // never the possibly stale key supplied by the caller.
        let fresh = self
            .local
            .resolve(&target.display_path, FsResolveOptions::default())
            .await?;
        for root in writable_roots(&policy).await? {
            if is_path_under(fresh.target_key.as_str(), &root, None).await? {
                return Ok(fresh);
            }
        }
        Err(FsError::new(
            format!(
                "cannot write \"{}\": file access denied under workspace-write mode",
                target.display_path
            ),
            FsErrorCode::SandboxDenied,
        )
        .into())
    }
}

impl FileSystem for SandboxedFileSystem {
    fn watch(
        &self,
        target: &FsTarget,
        changed: FsChanged,
        signal: AbortSignal,
    ) -> FsFuture<'_, Arc<dyn FsWatcher>> {
        self.local.watch(target, changed, signal)
    }

    fn sandbox_mode(&self) -> Option<SandboxMode> {
        Some(self.default_mode)
    }

    fn resolve<'a>(&'a self, path: &'a str, options: FsResolveOptions) -> FsFuture<'a, FsTarget> {
        self.local.resolve(path, options)
    }

    fn process_path(&self, target: &FsTarget) -> String {
        self.local.process_path(target)
    }

    fn process_path_from_host_path(&self, host_path: &str) -> Option<String> {
        self.local.process_path_from_host_path(host_path)
    }

    fn file_url(&self, target: &FsTarget) -> String {
        self.local.file_url(target)
    }

    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        self.local.contains(parent, child)
    }

    fn stat<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsInfo>> {
        self.local.stat(target, signal)
    }

    fn lstat<'a>(
        &'a self,
        path: &'a str,
        cwd: Option<&'a str>,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsPathInfo>> {
        self.local.lstat(path, cwd, signal)
    }

    fn read_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, String> {
        self.local.read_text(target, signal)
    }

    fn stream_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, FsTextStream> {
        self.local.stream_text(target, signal)
    }

    fn read_bytes<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
        max_bytes: u64,
    ) -> FsFuture<'a, Vec<u8>> {
        self.local.read_bytes(target, signal, max_bytes)
    }

    fn read_byte_range<'a>(
        &'a self,
        target: &'a FsTarget,
        range: FsByteRange,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<u8>> {
        self.local.read_byte_range(target, range, signal)
    }

    fn list_dir<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<FsDirEntry>> {
        self.local.list_dir(target, signal)
    }

    fn write_text<'a>(
        &'a self,
        target: &'a FsTarget,
        content: &'a str,
        expected: Option<FsWriteIntent>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsWriteOutcome> {
        Box::pin(async move {
            let target = self.checked_target(target, policy).await?;
            self.local
                .write_text(&target, content, expected, signal, None)
                .await
        })
    }

    fn edit_text<'a>(
        &'a self,
        target: &'a FsTarget,
        edit: FsEditRequest,
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        Box::pin(async move {
            let target = self.checked_target(target, policy).await?;
            self.local
                .edit_text(&target, edit, expected, signal, None)
                .await
        })
    }

    fn patch_text<'a>(
        &'a self,
        target: &'a FsTarget,
        hunks: &'a [PatchHunk],
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        Box::pin(async move {
            let target = self.checked_target(target, policy).await?;
            self.local
                .patch_text(&target, hunks, expected, signal, None)
                .await
        })
    }

    fn delete_file<'a>(
        &'a self,
        target: &'a FsTarget,
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsDeleteOutcome> {
        Box::pin(async move {
            let target = self.checked_target(target, policy).await?;
            self.local
                .delete_file(&target, expected, signal, None)
                .await
        })
    }

    fn move_file<'a>(
        &'a self,
        source: &'a FsTarget,
        destination: &'a FsTarget,
        options: FsMoveOptions,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsMoveOutcome> {
        Box::pin(async move {
            let source = self.checked_target(source, policy.clone()).await?;
            let destination = self.checked_target(destination, policy).await?;
            self.local
                .move_file(&source, &destination, options, signal, None)
                .await
        })
    }
}

/// Services required by the sandboxing filesystem provider.
#[derive(Inject)]
pub struct Deps {
    sandbox_policy: Service<SandboxPolicy>,
}

/// Mount [`SandboxedFileSystem`] as the `fs` service.
#[plugin(FsSandboxPlugin, name = "SandboxedFileSystem")]
pub async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
    let provider = Arc::new(SandboxedFileSystem::new(
        (*config.get()).clone(),
        (*deps.sandbox_policy).clone(),
    )?);
    ctx.provide::<Fs>(provider).await?;
    Ok(())
}
