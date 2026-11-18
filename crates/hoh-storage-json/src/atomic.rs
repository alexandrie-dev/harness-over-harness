//! Source-local durable replace protocol (not the separate atomic-write package).
use hoh_error::{BoxError, node_error_code, node_fs_error_message};
use hoh_util_workspace_path::node_path::{dirname, join};
use std::{io, sync::Arc};
use tokio::{fs, io::AsyncWriteExt};

/// Node filesystem error with the original OS failure available as its cause.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct NodeFsError {
    pub code: Option<&'static str>,
    message: String,
    #[source]
    cause: Arc<io::Error>,
}
impl NodeFsError {
    pub(crate) fn new(
        error: io::Error,
        syscall: &str,
        path: Option<&str>,
        dest: Option<&str>,
    ) -> Self {
        Self {
            code: node_error_code(&error),
            message: node_fs_error_message(&error, syscall, path, dest),
            cause: Arc::new(error),
        }
    }
}
pub(crate) fn fs_error(
    error: io::Error,
    syscall: &str,
    path: Option<&str>,
    dest: Option<&str>,
) -> BoxError {
    Box::new(NodeFsError::new(error, syscall, path, dest))
}

pub(crate) async fn mkdir(path: &str) -> Result<(), BoxError> {
    // create_dir_all("") succeeds in Rust; Node recursive mkdir("") rejects.
    // Use the native mkdir error so the shared formatter keeps its errno/text.
    if path.is_empty() {
        return fs::create_dir(path)
            .await
            .map_err(|error| fs_error(error, "mkdir", Some(path), None));
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(path)
        .await
        .map_err(|error| fs_error(error, "mkdir", Some(path), None))
}
pub(crate) async fn read_file(path: &str) -> Result<Option<String>, BoxError> {
    match fs::read(path).await {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            let syscall = if error.kind() == io::ErrorKind::IsADirectory {
                "read"
            } else {
                "open"
            };
            Err(fs_error(
                error,
                syscall,
                (syscall == "open").then_some(path),
                None,
            ))
        }
    }
}
pub(crate) async fn rename(from: &str, to: &str) -> Result<(), BoxError> {
    fs::rename(from, to)
        .await
        .map_err(|error| fs_error(error, "rename", Some(from), Some(to)))
}
pub(crate) fn missing(error: &BoxError) -> bool {
    error
        .downcast_ref::<NodeFsError>()
        .is_some_and(|error| error.cause.kind() == io::ErrorKind::NotFound)
}

/// Persist directory-entry changes on POSIX; Windows does not support it.
pub async fn fsync_directory(path: &str) -> Result<(), BoxError> {
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let file = fs::File::open(path)
            .await
            .map_err(|error| fs_error(error, "open", Some(path), None))?;
        file.sync_all()
            .await
            .map_err(|error| fs_error(error, "fsync", None, None))
    }
}

/// Exclusive same-directory temporary file, file fsync, replace, directory fsync.
pub async fn write_atomic(path: &str, data: &str) -> Result<(), BoxError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| super::runtime::plain(error.to_string()))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let filename = format!(
        ".{}-{}-{}-{}-{}.tmp",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    );
    let dir = dirname(path);
    let tmp = join(&[&dir, &filename]);
    let result = replace(&tmp, path, data, &dir).await;
    if result.is_err() {
        match fs::remove_file(&tmp).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(fs_error(error, "unlink", Some(&tmp), None)),
        }
    }
    result
}

async fn replace(tmp: &str, path: &str, data: &str, dir: &str) -> Result<(), BoxError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(tmp)
        .await
        .map_err(|error| fs_error(error, "open", Some(tmp), None))?;
    let result = async {
        file.write_all(data.as_bytes())
            .await
            .map_err(|error| fs_error(error, "write", None, None))?;
        // Tokio queues write_all; flush observes its error before sync_all can
        // swallow it and before a short temporary file can replace the target.
        file.flush()
            .await
            .map_err(|error| fs_error(error, "write", None, None))?;
        file.sync_all()
            .await
            .map_err(|error| fs_error(error, "fsync", None, None))
    }
    .await;
    // The queued write has completed before syncing or dropping the handle.
    drop(file);
    result?;
    rename(tmp, path).await?;
    fsync_directory(dir).await
}
