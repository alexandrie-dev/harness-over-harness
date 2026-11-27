//! Sandbox-enforcing host filesystem provider.

pub mod containment;

use std::{
    os::fd::{AsFd, AsRawFd, OwnedFd},
    path::Path,
    sync::Arc,
};

use futures_util::StreamExt;
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

#[cfg(target_os = "linux")]
const DIRECTORY_ACCESS: rustix::fs::OFlags = rustix::fs::OFlags::PATH;
#[cfg(not(target_os = "linux"))]
const DIRECTORY_ACCESS: rustix::fs::OFlags = rustix::fs::OFlags::RDONLY;

struct AnchoredTarget {
    target: FsTarget,
    physical: String,
    _anchor: Option<OwnedFd>,
    _effect_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    #[cfg(unix)]
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn from_fd(fd: &impl AsFd) -> Result<Self, BoxError> {
        let stat = rustix::fs::fstat(fd)?;
        Ok(Self::from_stat(&stat))
    }

    fn from_stat(stat: &rustix::fs::Stat) -> Self {
        Self {
            #[cfg(target_os = "macos")]
            device: stat.st_dev as u64,
            #[cfg(not(target_os = "macos"))]
            device: stat.st_dev,
            inode: stat.st_ino,
        }
    }
}

/// The local filesystem backend with a per-mutation sandbox fence.
pub struct SandboxedFileSystem {
    local: LocalFileSystem,
    policy: SandboxPolicyService,
    default_mode: SandboxMode,
    protected_identities: Arc<[FileIdentity]>,
}

impl SandboxedFileSystem {
    /// Construct a standalone provider over the local backend.
    pub fn new(config: Config, policy: SandboxPolicyService) -> Result<Self, BoxError> {
        let default_mode = policy.resolve(SandboxPolicyRequest::default())?.mode;
        let mut protected_identities = Vec::new();
        for root in policy.protected_roots() {
            match std::fs::metadata(root) {
                Ok(metadata) => {
                    let identity = FileIdentity::from_metadata(&metadata);
                    if !protected_identities.contains(&identity) {
                        protected_identities.push(identity);
                    }
                }
                Err(error) if matches!(error.raw_os_error(), Some(2 | 20)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(Self {
            local: LocalFileSystem::new(config)?,
            policy,
            default_mode,
            protected_identities: protected_identities.into(),
        })
    }

    fn protected_error(target: &FsTarget) -> BoxError {
        FsError::new(
            format!(
                "cannot access \"{}\": protected engine storage is unavailable to model tools",
                target.display_path
            ),
            FsErrorCode::SandboxDenied,
        )
        .into()
    }

    fn is_protected(&self, path: &str) -> bool {
        self.policy
            .protected_roots()
            .iter()
            .any(|root| Path::new(path).starts_with(root))
            || self
                .path_reaches_protected_identity(Path::new(path))
                .unwrap_or(true)
    }

    fn is_protected_fd(&self, fd: &impl AsFd) -> Result<bool, BoxError> {
        let identity = FileIdentity::from_fd(fd)?;
        Ok(self.protected_identities.contains(&identity))
    }

    fn directory_chain_reaches_protected(&self, mut directory: OwnedFd) -> Result<bool, BoxError> {
        loop {
            let identity = FileIdentity::from_fd(&directory)?;
            if self.protected_identities.contains(&identity) {
                return Ok(true);
            }
            let parent = rustix::fs::openat(
                &directory,
                "..",
                DIRECTORY_ACCESS | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )?;
            if FileIdentity::from_fd(&parent)? == identity {
                return Ok(false);
            }
            directory = parent;
        }
    }

    /// Inspect an existing target (or its nearest existing directory ancestor) without reopening
    /// any already selected component by name. Opening the parent first and the basename through
    /// `openat` keeps target/parent identity coherent; walking `..` from directory fds survives
    /// ancestor renames and therefore still finds a protected root by `(dev, ino)`.
    fn path_reaches_protected_identity(&self, path: &Path) -> Result<bool, BoxError> {
        if self.protected_identities.is_empty() {
            return Ok(false);
        }
        let mut candidate = path;
        loop {
            let Some(parent_path) = candidate.parent() else {
                let root = rustix::fs::open(
                    candidate,
                    DIRECTORY_ACCESS | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )?;
                return self.directory_chain_reaches_protected(root);
            };
            let parent = match rustix::fs::open(
                parent_path,
                DIRECTORY_ACCESS | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            ) {
                Ok(parent) => parent,
                Err(rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR) => {
                    if parent_path == candidate {
                        return Ok(false);
                    }
                    candidate = parent_path;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            #[cfg(target_os = "linux")]
            let target_is_protected = match candidate.file_name() {
                Some(name) => match rustix::fs::openat(
                    &parent,
                    name,
                    rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                ) {
                    Ok(target) => self.is_protected_fd(&target)?,
                    Err(rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR) => false,
                    Err(error) => return Err(error.into()),
                },
                None => self.is_protected_fd(&parent)?,
            };
            #[cfg(not(target_os = "linux"))]
            let target_is_protected = match candidate.file_name() {
                Some(name) => match rustix::fs::statat(&parent, name, rustix::fs::AtFlags::empty())
                {
                    Ok(stat) => self
                        .protected_identities
                        .contains(&FileIdentity::from_stat(&stat)),
                    Err(rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR) => false,
                    Err(error) => return Err(error.into()),
                },
                None => self.is_protected_fd(&parent)?,
            };
            return Ok(target_is_protected || self.directory_chain_reaches_protected(parent)?);
        }
    }

    async fn checked_resolved_target(&self, target: FsTarget) -> Result<FsTarget, BoxError> {
        for root in self.policy.protected_roots() {
            if is_path_under(target.target_key.as_str(), root, None).await? {
                return Err(Self::protected_error(&target));
            }
        }
        if self.path_reaches_protected_identity(Path::new(target.target_key.as_str()))? {
            return Err(Self::protected_error(&target));
        }
        Ok(target)
    }

    /// Checking a project's working directory only needs metadata. Bind that access to the
    /// opened parent and never follow a symlink substituted after resolving the original path.
    #[cfg(target_os = "macos")]
    async fn stat_protected(
        &self,
        target: &FsTarget,
        signal: Option<AbortSignal>,
    ) -> Result<Option<FsInfo>, BoxError> {
        use hoh_fs::FsType;
        use rustix::fs::{AtFlags, FileType, Mode, OFlags, fstat, open, statat};
        use rustix::io::Errno;

        let _effect_guard = self.policy.lock_protected_effect().await;
        let fresh = self
            .local
            .resolve(
                &target.display_path,
                FsResolveOptions {
                    signal: signal.clone(),
                    ..FsResolveOptions::default()
                },
            )
            .await?;
        self.checked_resolved_target(fresh.clone()).await?;
        let path = Path::new(fresh.target_key.as_str());
        let parent = match open(
            path.parent().unwrap_or(path),
            DIRECTORY_ACCESS | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(parent) => parent,
            Err(Errno::NOENT | Errno::NOTDIR) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let stat = match path.file_name() {
            Some(name) => statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW),
            None => fstat(&parent),
        };
        if self.directory_chain_reaches_protected(parent)? {
            return Err(Self::protected_error(&fresh));
        }
        let stat = match stat {
            Ok(stat) => stat,
            Err(Errno::NOENT | Errno::NOTDIR) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let identity = FileIdentity::from_stat(&stat);
        if self.protected_identities.contains(&identity) {
            return Err(Self::protected_error(&fresh));
        }
        let r#type = match FileType::from_raw_mode(stat.st_mode.into()) {
            FileType::RegularFile => FsType::File,
            FileType::Directory => FsType::Directory,
            FileType::Symlink => return Err(Errno::LOOP.into()),
            _ => FsType::Other,
        };
        if let Some(signal) = signal {
            signal.throw_if_aborted()?;
        }
        let size = stat.st_size as u64;
        let mtime = i128::from(stat.st_mtime) * 1_000_000_000 + i128::from(stat.st_mtime_nsec);
        let ctime = i128::from(stat.st_ctime) * 1_000_000_000 + i128::from(stat.st_ctime_nsec);
        Ok(Some(FsInfo {
            version: format!(
                "{}:{}:{size}:{mtime}:{ctime}",
                identity.device, identity.inode
            )
            .into(),
            r#type,
            size: Some(size),
        }))
    }

    #[cfg(target_os = "linux")]
    async fn open_anchor(&self, path: &str) -> Result<(OwnedFd, String), BoxError> {
        let fd = rustix::fs::open(
            path,
            rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )?;
        let process_path = format!("/proc/self/fd/{}", fd.as_raw_fd());
        let physical = tokio::fs::read_link(&process_path)
            .await?
            .to_string_lossy()
            .into_owned();
        Ok((fd, physical))
    }

    #[cfg(not(target_os = "linux"))]
    async fn open_anchor(&self, _path: &str) -> Result<(OwnedFd, String), BoxError> {
        // A path reopen cannot provide the descriptor binding required for protected storage.
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "HoH filesystem tools with protected storage require descriptor-bound access on this platform",
        )
        .into())
    }

    async fn check_physical_path(&self, physical: &str, target: &FsTarget) -> Result<(), BoxError> {
        for root in self.policy.protected_roots() {
            if is_path_under(physical, root, None).await? {
                return Err(Self::protected_error(target));
            }
        }
        if self.path_reaches_protected_identity(Path::new(physical))? {
            return Err(Self::protected_error(target));
        }
        Ok(())
    }

    /// A target handed through unchanged: with no protected storage there is nothing to anchor,
    /// so reads and watches take the source's plain path (no procfs, no descriptor).
    fn plain_target(target: &FsTarget) -> AnchoredTarget {
        AnchoredTarget {
            physical: target.target_key.to_string(),
            target: target.clone(),
            _anchor: None,
            _effect_guard: None,
        }
    }

    async fn checked_access_target(&self, target: &FsTarget) -> Result<AnchoredTarget, BoxError> {
        if self.policy.protected_roots().is_empty() {
            return Ok(Self::plain_target(target));
        }
        let effect_guard = self.policy.lock_protected_effect().await;
        let fresh = self
            .local
            .resolve(&target.display_path, FsResolveOptions::default())
            .await?;
        self.checked_resolved_target(fresh.clone()).await?;
        match self.open_anchor(fresh.target_key.as_str()).await {
            Ok((anchor, physical)) => {
                self.check_physical_path(&physical, &fresh).await?;
                Ok(AnchoredTarget {
                    target: FsTarget {
                        target_key: format!("/proc/self/fd/{}", anchor.as_raw_fd()).into(),
                        display_path: fresh.display_path,
                    },
                    physical,
                    _anchor: Some(anchor),
                    _effect_guard: effect_guard,
                })
            }
            Err(error)
                if error
                    .downcast_ref::<rustix::io::Errno>()
                    .is_some_and(|errno| {
                        matches!(*errno, rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR)
                    }) =>
            {
                // `stat`/preflight must preserve LocalFileSystem's ordinary missing-target
                // contract. Anchor the nearest existing parent for the protection check, but
                // let the delegated operation observe absence at the physical target spelling.
                let mut anchored = self.checked_mutation_target(&fresh).await?;
                anchored._effect_guard = effect_guard;
                Ok(anchored)
            }
            Err(error) => Err(error),
        }
    }

    async fn checked_mutation_target(&self, target: &FsTarget) -> Result<AnchoredTarget, BoxError> {
        let fresh = self
            .local
            .resolve(&target.display_path, FsResolveOptions::default())
            .await?;
        let physical = Path::new(fresh.target_key.as_str());
        let Some(mut ancestor) = physical.parent() else {
            // The filesystem root has no parent to anchor. Without protected storage nothing is
            // there to guard, so the mode check and the delegate answer as in the source.
            if self.policy.protected_roots().is_empty() {
                return Ok(AnchoredTarget {
                    physical: fresh.target_key.to_string(),
                    target: fresh,
                    _anchor: None,
                    _effect_guard: None,
                });
            }
            return Err(Self::protected_error(&fresh));
        };
        let (anchor, anchored_parent) = loop {
            match self.open_anchor(&ancestor.to_string_lossy()).await {
                Ok(opened) => break opened,
                Err(error)
                    if error
                        .downcast_ref::<rustix::io::Errno>()
                        .is_some_and(|errno| {
                            matches!(*errno, rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR)
                        }) =>
                {
                    ancestor = ancestor
                        .parent()
                        .ok_or_else(|| Self::protected_error(&fresh))?;
                }
                Err(error) => return Err(error),
            }
        };
        self.check_physical_path(&anchored_parent, &fresh).await?;
        let suffix = physical
            .strip_prefix(ancestor)
            .map_err(|_| Self::protected_error(&fresh))?;
        let physical_target = Path::new(&anchored_parent)
            .join(suffix)
            .to_string_lossy()
            .into_owned();
        self.check_physical_path(&physical_target, &fresh).await?;
        Ok(AnchoredTarget {
            target: FsTarget {
                // `hoh-fs-local` implements atomic replacement with sibling temp files and
                // rename. Linux does not preserve those semantics through a `/proc/self/fd`
                // magic-link pathname, so mutation must retain the freshly resolved physical
                // spelling. The parent descriptor remains alive through the call and the shared
                // protected-effect lock prevents another model tool from swapping the suffix.
                target_key: physical_target.clone().into(),
                display_path: fresh.display_path,
            },
            physical: physical_target,
            _anchor: Some(anchor),
            _effect_guard: None,
        })
    }

    async fn checked_target_unlocked(
        &self,
        target: &FsTarget,
        policy: Option<SandboxExecutionPolicy>,
    ) -> Result<AnchoredTarget, BoxError> {
        let policy = match policy {
            Some(policy) => policy,
            None => self.policy.resolve(SandboxPolicyRequest::default())?,
        };
        // The mode decides first, as in the source: a read-only call is refused and a
        // danger-full-access call is delegated before any path is opened or anchored.
        if policy.mode == SandboxMode::ReadOnly {
            return Err(FsError::new(
                format!(
                    "cannot write \"{}\": file access denied under read-only mode",
                    target.display_path
                ),
                FsErrorCode::SandboxDenied,
            )
            .into());
        }
        let protecting = !self.policy.protected_roots().is_empty();
        if policy.mode == SandboxMode::DangerFullAccess && !protecting {
            return Ok(Self::plain_target(target));
        }
        let fresh = if protecting {
            self.checked_mutation_target(target).await?
        } else {
            let fresh = self
                .local
                .resolve(&target.display_path, FsResolveOptions::default())
                .await?;
            Self::plain_target(&fresh)
        };
        if policy.mode == SandboxMode::DangerFullAccess {
            return Ok(fresh);
        }

        for root in writable_roots(&policy).await? {
            if is_path_under(&fresh.physical, &root, None).await? {
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

    async fn checked_target(
        &self,
        target: &FsTarget,
        policy: Option<SandboxExecutionPolicy>,
    ) -> Result<AnchoredTarget, BoxError> {
        let effect_guard = self.policy.lock_protected_effect().await;
        let mut target = self.checked_target_unlocked(target, policy).await?;
        target._effect_guard = effect_guard;
        Ok(target)
    }
}

impl FileSystem for SandboxedFileSystem {
    fn watch(
        &self,
        target: &FsTarget,
        changed: FsChanged,
        signal: AbortSignal,
    ) -> FsFuture<'_, Arc<dyn FsWatcher>> {
        let target = target.clone();
        Box::pin(async move {
            let checked = self.checked_access_target(&target).await?;
            // The watcher belongs to the real path (as in the source), never to a descriptor
            // number that is closed, and may be reused, once this call returns.
            let real = FsTarget {
                target_key: checked.physical.clone().into(),
                display_path: checked.target.display_path.clone(),
            };
            self.local.watch(&real, changed, signal).await
        })
    }

    fn sandbox_mode(&self) -> Option<SandboxMode> {
        Some(self.default_mode)
    }

    fn resolve<'a>(&'a self, path: &'a str, options: FsResolveOptions) -> FsFuture<'a, FsTarget> {
        Box::pin(async move {
            let _effect_guard = self.policy.lock_protected_effect().await;
            let target = self.local.resolve(path, options).await?;
            self.checked_resolved_target(target).await
        })
    }

    fn process_path(&self, target: &FsTarget) -> String {
        if self.is_protected(target.target_key.as_str()) {
            String::new()
        } else {
            self.local.process_path(target)
        }
    }

    fn process_path_from_host_path(&self, host_path: &str) -> Option<String> {
        if self.is_protected(host_path) {
            return None;
        }
        self.local
            .process_path_from_host_path(host_path)
            .filter(|path| !self.is_protected(path))
    }

    fn file_url(&self, target: &FsTarget) -> String {
        if self.is_protected(target.target_key.as_str()) {
            String::new()
        } else {
            self.local.file_url(target)
        }
    }

    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        self.local.contains(parent, child)
    }

    fn stat<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsInfo>> {
        Box::pin(async move {
            #[cfg(target_os = "macos")]
            if !self.policy.protected_roots().is_empty() {
                return self.stat_protected(target, signal).await;
            }
            let target = self.checked_access_target(target).await?;
            self.local.stat(&target.target, signal).await
        })
    }

    fn lstat<'a>(
        &'a self,
        path: &'a str,
        cwd: Option<&'a str>,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsPathInfo>> {
        Box::pin(async move {
            let target = self
                .local
                .resolve(
                    path,
                    FsResolveOptions {
                        cwd: cwd.map(str::to_owned),
                        signal: signal.clone(),
                    },
                )
                .await?;
            if self.policy.protected_roots().is_empty() {
                return self
                    .local
                    .lstat(target.target_key.as_str(), None, signal)
                    .await;
            }
            self.checked_resolved_target(target.clone()).await?;
            let effect_guard = self.policy.lock_protected_effect().await;
            let mut anchored = self.checked_mutation_target(&target).await?;
            anchored._effect_guard = effect_guard;
            self.local
                .lstat(anchored.target.target_key.as_str(), None, signal)
                .await
        })
    }

    fn read_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, String> {
        Box::pin(async move {
            let target = self.checked_access_target(target).await?;
            self.local.read_text(&target.target, signal).await
        })
    }

    fn stream_text<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, FsTextStream> {
        Box::pin(async move {
            let target = self.checked_access_target(target).await?;
            let stream = self.local.stream_text(&target.target, signal).await?;
            Ok(Box::pin(stream.map(move |item| {
                let _keep_anchor_open = &target;
                item
            })) as FsTextStream)
        })
    }

    fn read_bytes<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
        max_bytes: u64,
    ) -> FsFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let target = self.checked_access_target(target).await?;
            self.local
                .read_bytes(&target.target, signal, max_bytes)
                .await
        })
    }

    fn read_byte_range<'a>(
        &'a self,
        target: &'a FsTarget,
        range: FsByteRange,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let target = self.checked_access_target(target).await?;
            self.local
                .read_byte_range(&target.target, range, signal)
                .await
        })
    }

    fn list_dir<'a>(
        &'a self,
        target: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<FsDirEntry>> {
        Box::pin(async move {
            let target = self.checked_access_target(target).await?;
            self.local.list_dir(&target.target, signal).await
        })
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
                .write_text(&target.target, content, expected, signal, None)
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
                .edit_text(&target.target, edit, expected, signal, None)
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
                .patch_text(&target.target, hunks, expected, signal, None)
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
                .delete_file(&target.target, expected, signal, None)
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
            let _effect_guard = self.policy.lock_protected_effect().await;
            let source = self.checked_target_unlocked(source, policy.clone()).await?;
            let destination = self.checked_target_unlocked(destination, policy).await?;
            self.local
                .move_file(&source.target, &destination.target, options, signal, None)
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
