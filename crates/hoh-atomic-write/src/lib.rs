//! Atomic replacement and exclusive-create writer coordination.
use hoh_error::{CodedError, HarnessError};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, sync::watch, time::Instant};

pub struct WriteFileAtomicOptions {
    pub mode: u32,
    pub dir_mode: Option<u32>,
}

#[derive(Clone, Default)]
pub struct FileLockOptions {
    pub signal: Option<watch::Receiver<bool>>,
    pub wait_ms: Option<u64>,
    /// Reports true only when a signal-zero probe proves ESRCH.
    /// Overrides the native Unix probe; Windows has no default probe until stage 5.
    pub process_exited: Option<Arc<dyn Fn(u32) -> bool + Send + Sync>>,
}

#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct FileLockError {
    pub lock_path: PathBuf,
    pub error: HarnessError,
}
impl FileLockError {
    pub fn new(lock_path: PathBuf) -> Self {
        Self {
            error: HarnessError::new(
                format!(
                    "atomic-write: timed out waiting for the writer lock at {}",
                    lock_path.display()
                ),
                "FILE_LOCK_TIMEOUT",
            )
            .with_name("FileLockError"),
            lock_path,
        }
    }
}
impl std::ops::Deref for FileLockError {
    type Target = HarnessError;
    fn deref(&self) -> &HarnessError {
        &self.error
    }
}
impl CodedError for FileLockError {
    fn harness_error(&self) -> &HarnessError {
        &self.error
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AtomicWriteError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Lock(#[from] FileLockError),
    #[error("The operation was aborted")]
    Aborted,
    /// Preserve an operation's crate-local error and its standard cause chain.
    #[error("{0}")]
    Operation(#[source] Box<dyn std::error::Error + Send + Sync>),
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
    async fn write_new(&self, path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
        write_new(path, data, mode).await
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
async fn remove(path: &Path) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
async fn write_new(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(mode);
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options.open(path).await?;
    file.write_all(data).await?;
    // Node writeFile closes the descriptor before the rename/exclusive-create call returns.
    file.flush().await?;
    drop(file);
    Ok(())
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
    error.kind() == io::ErrorKind::PermissionDenied
        || error.kind() == io::ErrorKind::ResourceBusy
        || matches!(error.raw_os_error(), Some(5 | 32 | 33)) && cfg!(windows)
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
    let result = async {
        runtime.write_new(&temp, content, options.mode).await?;
        rename_atomic_temp(runtime, &temp, filename).await
    }
    .await;
    if let Err(error) = result {
        runtime.remove(&temp).await?;
        return Err(error);
    }
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
    match runtime
        .write_new(
            &claim,
            format!("{}\n", std::process::id()).as_bytes(),
            0o600,
        )
        .await
    {
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
    let _ = runtime.remove(&claim).await;
    result
}
fn check_signal(options: &FileLockOptions) -> Result<(), AtomicWriteError> {
    if options.signal.as_ref().is_some_and(|s| *s.borrow()) {
        Err(AtomicWriteError::Aborted)
    } else {
        Ok(())
    }
}
async fn wait(delay: u64, options: &FileLockOptions) -> Result<(), AtomicWriteError> {
    let cancelled = async {
        if let Some(mut signal) = options.signal.clone()
            && signal.wait_for(|aborted| *aborted).await.is_ok()
        {
            return;
        }
        std::future::pending::<()>().await;
    };
    tokio::select! {
        () = tokio::time::sleep(Duration::from_millis(delay)) => Ok(()),
        () = cancelled => Err(AtomicWriteError::Aborted),
    }
}
pub async fn with_file_lock<T, F: Future<Output = Result<T, AtomicWriteError>>>(
    filename: impl AsRef<Path>,
    operation: impl FnOnce() -> F,
    options: FileLockOptions,
) -> Result<T, AtomicWriteError> {
    with_file_lock_with(&SystemRuntime, filename.as_ref(), operation, options).await
}
async fn with_file_lock_with<T, F: Future<Output = Result<T, AtomicWriteError>>>(
    runtime: &impl Runtime,
    filename: &Path,
    operation: impl FnOnce() -> F,
    options: FileLockOptions,
) -> Result<T, AtomicWriteError> {
    let lock_path = sibling(filename, ".lock");
    let deadline = Instant::now() + Duration::from_millis(options.wait_ms.unwrap_or(2000));
    let mut delay = 20;
    let mut retried_permission = false;
    loop {
        check_signal(&options)?;
        match runtime
            .write_new(
                &lock_path,
                format!("{}\n", std::process::id()).as_bytes(),
                0o600,
            )
            .await
        {
            Ok(()) => break,
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
    }
    let result = match check_signal(&options) {
        Ok(()) => operation().await,
        Err(error) => Err(error),
    };
    runtime.remove(&lock_path).await?;
    result
}

#[cfg(test)]
#[path = "../tests/atomic-write.rs"]
mod tests;
