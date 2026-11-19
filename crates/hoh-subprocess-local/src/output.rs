//! Byte-exact bounded tails and best-effort retained spill files.
use crate::{error_text, lock};
use hoh_error::BoxError;
use hoh_kernel::Logger;
use hoh_subprocess::{CollectedOutput, SubprocessOutputRead, SubprocessOutputReader};
use std::{
    collections::VecDeque,
    fs::{File, OpenOptions},
    io::Write,
    os::{
        fd::IntoRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub type SpillFailureReporter = Arc<dyn Fn(BoxError, &str) -> Result<(), BoxError> + Send + Sync>;
#[derive(Debug, thiserror::Error)]
pub enum SpillCloseFailure {
    #[error("{error}")]
    Retained { file: File, error: BoxError },
    #[error("{error}")]
    Descriptor {
        descriptor: std::os::fd::RawFd,
        error: BoxError,
    },
}
impl SpillCloseFailure {
    fn retry(self, io: &dyn SpillIo) -> Result<(), Self> {
        match self {
            Self::Retained { file, .. } => io.close(file),
            Self::Descriptor { descriptor, .. } => close_descriptor(descriptor),
        }
    }
    fn leave_after_final_failure(self) {
        // Source seal forgets the descriptor after its final failed close.
        if let Self::Retained { file, .. } = self {
            let _ = file.into_raw_fd();
        }
    }
}
fn close_descriptor(descriptor: std::os::fd::RawFd) -> Result<(), SpillCloseFailure> {
    nix::unistd::close(descriptor).map_err(|error| SpillCloseFailure::Descriptor {
        descriptor,
        error: crate::linux_execve::system_error(error.into(), "close", None),
    })
}
/// Explicit equivalent of the source's filesystem test adapters.
pub trait SpillIo: Send + Sync {
    fn reporter_failure(&self, line: &str) {
        write_diagnostic(line);
    }
    fn open(&self, path: &std::path::Path) -> Result<File, BoxError>;
    fn write(&self, file: &mut File, bytes: &[u8]) -> Result<(), BoxError>;
    fn close(&self, file: File) -> Result<(), SpillCloseFailure>;
    fn unlink(&self, path: &std::path::Path) -> Result<(), BoxError>;
}
pub struct NativeSpillIo;
impl SpillIo for NativeSpillIo {
    fn open(&self, path: &std::path::Path) -> Result<File, BoxError> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| {
                crate::linux_execve::system_error(e, "open", Some(&path.to_string_lossy()))
            })
    }
    fn write(&self, file: &mut File, bytes: &[u8]) -> Result<(), BoxError> {
        file.write_all(bytes)
            .map_err(|e| crate::linux_execve::system_error(e, "write", None))
    }
    fn close(&self, file: File) -> Result<(), SpillCloseFailure> {
        close_descriptor(file.into_raw_fd())
    }
    fn unlink(&self, path: &std::path::Path) -> Result<(), BoxError> {
        std::fs::remove_file(path).map_err(|e| {
            crate::linux_execve::system_error(e, "unlink", Some(&path.to_string_lossy()))
        })
    }
}
#[derive(Clone)]
pub struct SpillOptions {
    pub max_bytes: u64,
    pub dir: PathBuf,
    pub on_failure: SpillFailureReporter,
}
#[derive(Clone)]
pub struct ManagedProcessBinding {
    pub spill_dir: PathBuf,
    pub on_spill_failure: SpillFailureReporter,
    pub storage: Arc<SpillStorage>,
}
#[derive(Default)]
pub struct SpillStorage {
    directory: Mutex<Option<PathBuf>>,
}
impl SpillStorage {
    pub fn directory(&self) -> std::io::Result<PathBuf> {
        let mut directory = lock(&self.directory);
        if directory.is_none() {
            *directory = Some(crate::shim::normalize_temporary_path(
                &tempfile::Builder::new()
                    .prefix("dsh-subprocess-")
                    .permissions(std::fs::Permissions::from_mode(0o700))
                    .tempdir_in(crate::shim::temporary_directory())?
                    .keep(),
            ));
        }
        // A directory is stored on every successful return above.
        Ok(directory.as_ref().expect("created spill directory").clone())
    }
}
impl Drop for SpillStorage {
    fn drop(&mut self) {
        if let Some(path) = lock(&self.directory).take() {
            let _ = std::fs::remove_dir(path);
        }
    }
}
pub fn log_spill_failure(logger: Logger, owner: String) -> SpillFailureReporter {
    Arc::new(move |error, label| {
        let removed = error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            || error
                .downcast_ref::<crate::runner_protocol::RunnerFailure>()
                .is_some_and(|e| e.diagnostic().code.as_deref() == Some("ENOENT"));
        logger.error(format!("{owner} could not write the complete {label} stream to its spill file; the result keeps only the in-memory tail and reports no full-output path.{} {}",if removed {" The spill directory no longer exists; a temporary-file cleaner removing it while empty is the usual cause."} else {""},error_text(error.as_ref())));
        Ok(())
    })
}
pub(crate) fn stderr_spill_failure() -> SpillFailureReporter {
    Arc::new(|error, label| {
        write_diagnostic(&format!(
            "dsh-subprocess-local: {label} spill failed; only the in-memory tail is retained: {}\n",
            error_text(error.as_ref())
        ));
        Ok(())
    })
}
fn write_diagnostic(line: &str) {
    let _ = std::io::stderr().write_all(line.as_bytes());
}
pub fn prepare_managed_process_binding(
    storage: &Arc<SpillStorage>,
    spill_dir: Option<PathBuf>,
    on_spill_failure: SpillFailureReporter,
) -> Result<ManagedProcessBinding, BoxError> {
    Ok(ManagedProcessBinding {
        spill_dir: match spill_dir {
            Some(dir) => dir,
            None => storage.directory()?,
        },
        on_spill_failure,
        storage: storage.clone(),
    })
}
struct State {
    chunks: VecDeque<Vec<u8>>,
    bytes: usize,
    total: u64,
    dropped: bool,
    file: Option<File>,
    pending_close: Option<SpillCloseFailure>,
    path: Option<PathBuf>,
    disabled: bool,
}
pub struct OutputCollector {
    max_bytes: usize,
    label: String,
    spill: Option<SpillOptions>,
    state: Mutex<State>,
    io: Arc<dyn SpillIo>,
}
pub struct OutputSnapshot {
    pub bytes: Vec<u8>,
    pub total_bytes: u64,
}
static SPILL_COUNTER: AtomicU64 = AtomicU64::new(0);
impl OutputCollector {
    pub fn new(max_bytes: u64, label: impl Into<String>, spill: Option<SpillOptions>) -> Self {
        Self::new_with_io(max_bytes, label, spill, Arc::new(NativeSpillIo))
    }
    pub fn new_with_io(
        max_bytes: u64,
        label: impl Into<String>,
        spill: Option<SpillOptions>,
        io: Arc<dyn SpillIo>,
    ) -> Self {
        Self {
            max_bytes: usize::try_from(max_bytes).unwrap_or(usize::MAX),
            label: label.into(),
            state: Mutex::new(State {
                chunks: VecDeque::new(),
                bytes: 0,
                total: 0,
                dropped: false,
                file: None,
                pending_close: None,
                path: None,
                disabled: spill.is_none(),
            }),
            spill,
            io,
        }
    }
    /// Synchronous source helper; process readers invoke it on a blocking worker.
    pub fn push(&self, chunk: &[u8]) {
        let mut state = lock(&self.state);
        state.total += chunk.len() as u64;
        let overflow = state.bytes.saturating_add(chunk.len()) > self.max_bytes;
        let mut failure = None;
        if let Some(spill) = &self.spill
            && !state.disabled
            && (overflow || state.file.is_some())
        {
            if state.total > spill.max_bytes {
                self.discard(&mut state);
            } else if let Err(error) = self.spill_all(&mut state, spill, chunk) {
                self.discard(&mut state);
                failure = Some(error);
            }
        }
        state.chunks.push_back(chunk.to_vec());
        state.bytes += chunk.len();
        while state.bytes > self.max_bytes {
            let excess = state.bytes - self.max_bytes;
            // A positive retained byte count always has a nonempty head chunk.
            let mut head = state.chunks.pop_front().expect("tail has a head");
            let remove = head.len().min(excess);
            state.bytes -= remove;
            if remove < head.len() {
                head.drain(..remove);
                state.chunks.push_front(head);
            }
            state.dropped = true;
        }
        drop(state);
        if let (Some(error), Some(spill)) = (failure, &self.spill) {
            let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (spill.on_failure)(error, &self.label)
            }));
            match report {
                Ok(Ok(())) => {}
                Ok(Err(error)) => self.io.reporter_failure(&format!(
                    "dsh-subprocess-local: spill failure reporter threw: {}\n",
                    error_text(error.as_ref())
                )),
                Err(payload) => {
                    let text = payload
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| payload.downcast_ref::<&str>().copied())
                        .unwrap_or("reporter panicked");
                    self.io.reporter_failure(&format!(
                        "dsh-subprocess-local: spill failure reporter threw: {text}\n"
                    ));
                }
            }
        }
    }
    fn spill_all(
        &self,
        state: &mut State,
        spill: &SpillOptions,
        chunk: &[u8],
    ) -> Result<(), BoxError> {
        if state.file.is_none() {
            let bytes = crate::shim::random_bytes::<6>()?;
            let random = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let filename = format!(
                "dsh-subprocess-{}-{}-{random}-{}.log",
                std::process::id(),
                SPILL_COUNTER.fetch_add(1, Ordering::Relaxed) + 1,
                self.label
            );
            let path = PathBuf::from(hoh_util_workspace_path::node_path::join(&[
                &spill.dir.to_string_lossy(),
                &filename,
            ]));
            let file = self.io.open(&path)?;
            state.file = Some(file);
            state.path = Some(path);
            for prior in &state.chunks {
                if let Some(file) = &mut state.file {
                    self.io.write(file, prior)?;
                }
            }
        }
        if let Some(file) = &mut state.file {
            self.io.write(file, chunk)?;
        }
        Ok(())
    }
    fn discard(&self, state: &mut State) {
        if let Some(file) = state.file.take() {
            state.pending_close = self.io.close(file).err();
        }
        if let Some(path) = state.path.take() {
            let _ = self.io.unlink(&path);
        }
        state.disabled = true;
    }
    pub fn snapshot(&self) -> OutputSnapshot {
        let state = lock(&self.state);
        OutputSnapshot {
            bytes: state.chunks.iter().flatten().copied().collect(),
            total_bytes: state.total,
        }
    }
    pub fn seal(&self) {
        self.seal_state(&mut lock(&self.state));
    }
    fn seal_state(&self, state: &mut State) {
        let file = state.file.take();
        if let Some(file) = file
            && let Err(error) = self.io.close(file)
        {
            state.path = None;
            error.leave_after_final_failure();
        }
        if let Some(pending) = state.pending_close.take()
            && let Err(error) = pending.retry(self.io.as_ref())
        {
            state.path = None;
            error.leave_after_final_failure();
        }
    }
    pub fn finalize(&self) -> CollectedOutput {
        let mut state = lock(&self.state);
        self.seal_state(&mut state);
        CollectedOutput {
            text: String::from_utf8_lossy(
                &state.chunks.iter().flatten().copied().collect::<Vec<_>>(),
            )
            .into_owned(),
            truncated: state.dropped,
            spill_path: state
                .path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
        }
    }
}
impl SubprocessOutputReader for OutputCollector {
    fn read_from(&self, from_byte: u64) -> SubprocessOutputRead {
        let state = lock(&self.state);
        let start = state.total - state.bytes as u64;
        let lossy = from_byte < start;
        let bytes = state.chunks.iter().flatten().copied().collect::<Vec<_>>();
        let offset = if lossy {
            0
        } else {
            usize::try_from(from_byte - start)
                .unwrap_or(usize::MAX)
                .min(bytes.len())
        };
        SubprocessOutputRead {
            text: String::from_utf8_lossy(&bytes[offset..]).into_owned(),
            next_offset: state.total,
            lossy,
            spill_path: state
                .path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
        }
    }
}
