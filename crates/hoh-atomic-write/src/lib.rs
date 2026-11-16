//! Atomic replacement and exclusive-create writer coordination.
use hoh_error::{BoxError, HarnessError};
use hoh_timeout::{AbortError, AbortSignal};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

pub struct WriteFileAtomicOptions {
    pub mode: u32,
    pub dir_mode: Option<u32>,
}

#[derive(Clone, Default)]
pub struct FileLockOptions {
    pub signal: Option<AbortSignal>,
    pub wait_ms: Option<u64>,
    /// Reports true only when a signal-zero probe proves ESRCH.
    /// Overrides the native Unix probe; Windows has no default probe until stage 5.
    pub process_exited: Option<Arc<dyn Fn(u32) -> bool + Send + Sync>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileLockErrorCode {
    FileLockTimeout,
}
impl FileLockErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FileLockTimeout => "FILE_LOCK_TIMEOUT",
        }
    }
}
#[derive(Clone, Debug)]
pub struct FileLockErrorDetails {
    pub lock_path: PathBuf,
}
pub struct FileLockError;
impl FileLockError {
    pub const NAME: &str = "FileLockError";
    // PORTING.md §7 requires subclass constructor namespaces returning HarnessError.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(lock_path: PathBuf) -> HarnessError {
        HarnessError::named(
            Self::NAME,
            format!(
                "atomic-write: timed out waiting for the writer lock at {}",
                lock_path.display()
            ),
            FileLockErrorCode::FileLockTimeout.as_str(),
        )
        .with_detail(FileLockErrorDetails { lock_path })
    }
}

trait Runtime {
    fn windows(&self) -> bool {
        cfg!(windows)
    }
    async fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        tokio::fs::rename(from, to).await
    }
    async fn remove(&self, path: &Path) -> io::Result<()> {
        remove(path).await
    }
    async fn write_new(&self, owner: &FileGuard, mode: u32) -> io::Result<()> {
        write_new(owner, mode, false).await
    }
    fn exited(&self, pid: u32, options: &FileLockOptions) -> io::Result<bool> {
        if let Some(probe) = &options.process_exited {
            return Ok(probe(pid));
        }
        default_process_exited(pid)
    }
}
struct SystemRuntime;
impl Runtime for SystemRuntime {}

#[cfg(unix)]
fn default_process_exited(pid: u32) -> io::Result<bool> {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return Ok(false);
    };
    // Like process.kill(pid, 0), only ESRCH proves exit; EPERM and all other errors do not.
    Ok(matches!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH)
    ))
}

#[cfg(not(unix))]
fn default_process_exited(_pid: u32) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic-write: a safe signal-zero process probe is required for external lock holders",
    ))
}

fn sibling(filename: &Path, suffix: &str) -> PathBuf {
    let mut name = filename.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}
fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
async fn remove(path: &Path) -> io::Result<()> {
    remove_file(path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileKind {
    Lock,
    Claim,
    Temp,
}

struct CreatedFile {
    file: Option<Arc<std::fs::File>>,
    metadata: Option<std::fs::Metadata>,
    writing: bool,
}
#[derive(Default)]
struct OwnershipState {
    cancelled: bool,
    created: Option<CreatedFile>,
}
struct FileOwnership {
    path: PathBuf,
    data: Arc<[u8]>,
    kind: FileKind,
    state: Mutex<OwnershipState>,
}
fn ownership_lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn same_file(original: &std::fs::Metadata, current: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        original.dev() == current.dev() && original.ino() == current.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (original, current);
        true
    }
}
impl FileOwnership {
    fn cleanup(&self, state: &mut OwnershipState) {
        let Some(created) = &mut state.created else {
            return;
        };
        let mut proven_inode = false;
        if let Some(original) = &created.metadata {
            match std::fs::symlink_metadata(&self.path) {
                Ok(current) if !same_file(original, &current) => return,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    state.created = None;
                    return;
                }
                Err(_) => {}
                Ok(_) => proven_inode = cfg!(unix),
            }
        } else {
            // No identity: free the creator descriptor before the record read (including EMFILE).
            created.file.take();
        }
        let may_remove = proven_inode && (created.writing || self.kind == FileKind::Temp)
            || std::fs::read(&self.path).ok().is_some_and(|data| {
                data.as_slice() == self.data.as_ref()
                    || created.writing && self.data.starts_with(&data)
            });
        if may_remove && remove_file(&self.path).is_ok() {
            state.created = None;
        }
    }
    fn created(&self, file: Arc<std::fs::File>, metadata: Option<std::fs::Metadata>) -> bool {
        let mut state = ownership_lock(&self.state);
        state.created = Some(CreatedFile {
            file: Some(file),
            metadata,
            writing: true,
        });
        if state.cancelled {
            self.cleanup(&mut state);
            return false;
        }
        true
    }
    fn finished_write(&self, succeeded: bool) {
        let mut state = ownership_lock(&self.state);
        if succeeded && let Some(created) = &mut state.created {
            created.writing = false;
        }
        if state.cancelled {
            self.cleanup(&mut state);
        }
    }
}
struct FileGuard {
    shared: Arc<FileOwnership>,
    armed: bool,
}
impl FileGuard {
    // Exists before any file-creating await; the worker registers ownership at exclusive creation.
    fn new(path: &Path, data: &[u8], kind: FileKind) -> Self {
        Self {
            shared: Arc::new(FileOwnership {
                path: path.into(),
                data: data.into(),
                kind,
                state: Mutex::default(),
            }),
            armed: true,
        }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for FileGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut state = ownership_lock(&self.shared.state);
        state.cancelled = true;
        // Drop cannot await. A still-running creator observes cancellation and repeats cleanup.
        self.shared.cleanup(&mut state);
    }
}
fn create_and_write(owner: Arc<FileOwnership>, mode: u32, fail_identity: bool) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let file = Arc::new(options.open(&owner.path)?);
    let metadata = if fail_identity {
        None
    } else {
        file.metadata().ok()
    };
    if !owner.created(file.clone(), metadata) {
        drop(file);
        owner.finished_write(false);
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "file creation was cancelled",
        ));
    }
    let result = {
        let mut writer = file.as_ref();
        writer.write_all(&owner.data).and_then(|()| writer.flush())
    };
    drop(file);
    owner.finished_write(result.is_ok());
    result
}
async fn write_new(owner: &FileGuard, mode: u32, fail_identity: bool) -> io::Result<()> {
    let shared = owner.shared.clone();
    tokio::task::spawn_blocking(move || create_and_write(shared, mode, fail_identity))
        .await
        .map_err(|error| io::Error::other(error.to_string()))?
}
fn permission(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(1)
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(5))
    }
    #[cfg(not(any(unix, windows)))]
    {
        error.kind() == io::ErrorKind::PermissionDenied
    }
}
fn transient(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error().is_some_and(|code| {
            [
                rustix::io::Errno::ACCESS,
                rustix::io::Errno::BUSY,
                rustix::io::Errno::PERM,
            ]
            .iter()
            .any(|errno| errno.raw_os_error() == code)
        })
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(5 | 32 | 33 | 170 | 1314))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = error;
        false
    }
}
async fn rename_atomic_temp(
    runtime: &impl Runtime,
    temp: &Path,
    filename: &Path,
) -> io::Result<()> {
    let mut delay = 20;
    let mut retries = 0;
    loop {
        match runtime.rename(temp, filename).await {
            Ok(()) => return Ok(()),
            Err(e) if !runtime.windows() || !transient(&e) || retries >= 8 => return Err(e),
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(delay)).await;
        delay = (delay * 2).min(200);
        retries += 1;
    }
}
pub async fn write_file_atomic(
    filename: impl AsRef<Path>,
    content: impl AsRef<[u8]>,
    options: WriteFileAtomicOptions,
) -> io::Result<()> {
    write_file_atomic_with(&SystemRuntime, filename.as_ref(), content.as_ref(), options).await
}
async fn write_file_atomic_with(
    runtime: &impl Runtime,
    filename: &Path,
    content: &[u8],
    options: WriteFileAtomicOptions,
) -> io::Result<()> {
    let parent = filename
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    if let Some(mode) = options.dir_mode {
        builder.mode(mode);
    }
    builder.create(parent).await?;
    let mut bytes = [0; 6];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    let temp = sibling(
        filename,
        &format!(
            ".{}.tmp",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
    );
    let mut guard = FileGuard::new(&temp, content, FileKind::Temp);
    let result = async {
        runtime.write_new(&guard, options.mode).await?;
        rename_atomic_temp(runtime, &temp, filename).await
    }
    .await;
    if let Err(error) = result {
        runtime.remove(&temp).await?;
        return Err(error);
    }
    guard.disarm();
    Ok(())
}
fn holder_exited(
    runtime: &impl Runtime,
    record: &str,
    options: &FileLockOptions,
) -> io::Result<bool> {
    let Some(digits) = record.strip_suffix('\n') else {
        return Ok(false);
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(false);
    }
    let Ok(pid) = digits.parse::<u32>() else {
        return Ok(false);
    };
    if pid == 0 || pid > i32::MAX as u32 || pid == std::process::id() {
        return Ok(false);
    }
    runtime.exited(pid, options)
}
async fn read_lock_record(path: &Path) -> Option<String> {
    tokio::fs::read(path)
        .await
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}
async fn take_over_exited_lock(
    runtime: &impl Runtime,
    path: &Path,
    options: &FileLockOptions,
) -> io::Result<bool> {
    let Some(record) = read_lock_record(path).await else {
        return Ok(false);
    };
    if !holder_exited(runtime, &record, options)? {
        return Ok(false);
    }
    let hash = Sha256::digest(record.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let claim = sibling(path, &format!(".takeover-{}", &hash[..16]));
    let mut guard = FileGuard::new(
        &claim,
        format!("{}\n", std::process::id()).as_bytes(),
        FileKind::Claim,
    );
    match runtime.write_new(&guard, 0o600).await {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists || permission(&error) => {
            return Ok(false);
        }
        Err(error) => return Err(error),
        Ok(()) => {}
    }
    let result = async {
        if read_lock_record(path).await.as_deref() != Some(&record)
            || !holder_exited(runtime, &record, options)?
        {
            return Ok(false);
        }
        Ok(runtime.remove(path).await.is_ok())
    }
    .await;
    if runtime.remove(&claim).await.is_ok() {
        guard.disarm();
    }
    result
}
fn check_signal(options: &FileLockOptions) -> Result<(), BoxError> {
    if let Some(signal) = &options.signal {
        signal.throw_if_aborted()?;
    }
    Ok(())
}
async fn wait(delay: u64, options: &FileLockOptions) -> Result<(), BoxError> {
    let cancelled = async {
        match &options.signal {
            Some(signal) => signal.cancelled().await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        biased;
        reason = cancelled => Err(AbortError::new("The operation was aborted").with_cause(reason).into()),
        () = tokio::time::sleep(Duration::from_millis(delay)) => Ok(()),
    }
}
pub async fn with_file_lock<T, F: Future<Output = Result<T, BoxError>>>(
    filename: impl AsRef<Path>,
    operation: impl FnOnce() -> F,
    options: FileLockOptions,
) -> Result<T, BoxError> {
    with_file_lock_with(&SystemRuntime, filename.as_ref(), operation, options).await
}

async fn with_file_lock_with<T, F: Future<Output = Result<T, BoxError>>>(
    runtime: &impl Runtime,
    filename: &Path,
    operation: impl FnOnce() -> F,
    options: FileLockOptions,
) -> Result<T, BoxError> {
    let lock_path = sibling(filename, ".lock");
    let deadline = Instant::now() + Duration::from_millis(options.wait_ms.unwrap_or(2000));
    let mut delay = 20;
    let mut retried_permission = false;
    let mut guard = loop {
        check_signal(&options)?;
        let candidate = FileGuard::new(
            &lock_path,
            format!("{}\n", std::process::id()).as_bytes(),
            FileKind::Lock,
        );
        match runtime.write_new(&candidate, 0o600).await {
            Ok(()) => break candidate,
            Err(error) => {
                let contention = error.kind() == io::ErrorKind::AlreadyExists
                    || permission(&error) && tokio::fs::symlink_metadata(&lock_path).await.is_ok();
                if !contention {
                    if !runtime.windows() || !permission(&error) || retried_permission {
                        return Err(error.into());
                    }
                    retried_permission = true;
                } else if take_over_exited_lock(runtime, &lock_path, &options).await? {
                    continue;
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(FileLockError::new(lock_path).into());
        }
        wait(delay, &options).await?;
        delay = (delay * 2).min(200);
    };
    let result = match check_signal(&options) {
        Ok(()) => operation().await,
        Err(error) => Err(error),
    };
    runtime.remove(&lock_path).await?;
    guard.disarm();
    result
}

#[cfg(test)]
#[path = "../tests/atomic-write.rs"]
mod tests;
