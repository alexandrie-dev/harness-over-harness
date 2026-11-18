//! Filesystem capability seam for one execution world.
pub mod errno;
pub mod types;
pub use errno::{ErrnoError, fs_error_from_errno};
use hoh_error::BoxError;
use hoh_fs_patch::PatchHunk;
pub use hoh_sandbox::{SandboxExecutionPolicy, SandboxMode};
use hoh_timeout::AbortSignal;
use std::{any::Any, future::Future, pin::Pin, sync::Arc};
pub use types::*;
pub type FsFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BoxError>> + Send + 'a>>;
pub type FsTextStream = Pin<Box<dyn futures_util::Stream<Item = Result<String, BoxError>> + Send>>;
pub type FsChanged = Arc<dyn Fn(Option<BoxError>) + Send + Sync>;
pub trait FsWatcher: Send + Sync {
    fn close(&self) -> FsFuture<'_, ()>;
}
pub trait FileSystem: Send + Sync {
    fn watch(
        &self,
        _target: &FsTarget,
        _changed: FsChanged,
        signal: AbortSignal,
    ) -> FsFuture<'_, Arc<dyn FsWatcher>> {
        Box::pin(async move {
            signal.throw_if_aborted()?;
            Err(FsError::new(
                "Filesystem watching is not supported by this provider.",
                FsErrorCode::IoError,
            )
            .into())
        })
    }
    fn sandbox_mode(&self) -> Option<SandboxMode> {
        None
    }
    fn resolve<'a>(&'a self, path: &'a str, options: FsResolveOptions) -> FsFuture<'a, FsTarget>;
    fn process_path(&self, target: &FsTarget) -> String;
    fn process_path_from_host_path(&self, _host_path: &str) -> Option<String> {
        None
    }
    fn file_url(&self, target: &FsTarget) -> String;
    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool;
    fn stat<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsInfo>>;
    fn lstat<'a>(
        &'a self,
        path: &'a str,
        cwd: Option<&'a str>,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsPathInfo>>;
    fn read_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, String>;
    fn stream_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, FsTextStream>;
    fn read_bytes<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
        max_bytes: u64,
    ) -> FsFuture<'a, Vec<u8>>;
    fn read_byte_range<'a>(
        &'a self,
        target: &'a FsTarget,
        range: FsByteRange,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<u8>>;
    fn list_dir<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<FsDirEntry>>;
    fn write_text<'a>(
        &'a self,
        target: &'a FsTarget,
        content: &'a str,
        expected: Option<FsWriteIntent>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsWriteOutcome>;
    fn edit_text<'a>(
        &'a self,
        target: &'a FsTarget,
        edit: FsEditRequest,
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome>;
    fn patch_text<'a>(
        &'a self,
        target: &'a FsTarget,
        hunks: &'a [PatchHunk],
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome>;
    fn delete_file<'a>(
        &'a self,
        target: &'a FsTarget,
        expected: Option<FsVersionGuard>,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsDeleteOutcome>;
    fn move_file<'a>(
        &'a self,
        source: &'a FsTarget,
        destination: &'a FsTarget,
        options: FsMoveOptions,
        signal: Option<AbortSignal>,
        policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsMoveOutcome>;
}
hoh_kernel::service!(pub Fs:dyn FileSystem="fs");
pub type FsActor = Arc<dyn Any + Send + Sync>;
#[derive(Clone)]
pub struct FsWriteIntentEvent {
    pub target: FsTarget,
    pub actor: Option<FsActor>,
}
hoh_kernel::event!(
    FsWriteIntentEvent = "fs/write-intent",
    hoh_kernel::Waterfall<Option<FsWriteIntent>>
);
#[derive(Clone)]
pub struct FsEditIntent {
    pub target: FsTarget,
    pub actor: Option<FsActor>,
}
hoh_kernel::event!(
    FsEditIntent = "fs/edit-intent",
    hoh_kernel::Waterfall<Option<FsVersionGuard>>
);
#[derive(Clone)]
pub struct FsDeleteIntent {
    pub target: FsTarget,
    pub actor: Option<FsActor>,
}
hoh_kernel::event!(
    FsDeleteIntent = "fs/delete-intent",
    hoh_kernel::Waterfall<Option<FsVersionGuard>>
);
#[derive(Clone)]
pub struct FsObserved {
    pub target: FsTarget,
    pub observation: FsObservation,
    pub actor: Option<FsActor>,
}
hoh_kernel::event!(FsObserved = "fs/observed", hoh_kernel::Emit);
