//! POSIX kernel write ownership. The stable lock inode is never unlinked.
use crate::{
    Result,
    io::{FileSystem, IoHook, IoOperation, NodeFileError, fs_error, is_not_found, lock},
};
use hoh_session::SessionId;
use hoh_session_persistence::SessionAlreadyOwnedError;
use std::os::unix::fs::MetadataExt;
use std::{fs::File, sync::Mutex};
pub const LEASE_FILENAME: &str = "session.lock";
pub struct SessionWriteLease {
    file: Mutex<Option<File>>,
    fs: FileSystem,
    path: String,
}
impl SessionWriteLease {
    pub async fn acquire(dir: &str, id: &SessionId) -> Result<Self> {
        Self::acquire_with_io_hook(dir, id, None).await
    }
    pub async fn acquire_with_io_hook(
        dir: &str,
        id: &SessionId,
        hook: Option<IoHook>,
    ) -> Result<Self> {
        let fs = FileSystem { hook };
        let path = hoh_util_workspace_path::node_path::join(&[dir, LEASE_FILENAME]);
        fs.mkdir(dir).await?;
        for _ in 0..3 {
            fs.hook(IoOperation::Open, &path, None, false, None).await?;
            let file = tokio::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .await
                .map_err(|e| fs_error(e, "open", &path, None))?
                .into_std()
                .await;
            fs.hook(IoOperation::Open, &path, None, true, None).await?;
            let checked = async {
                fs.hook(IoOperation::Flock, &path, None, false, None)
                    .await?;
                let (file, result) = tokio::task::spawn_blocking(move || {
                    let result = hoh_node_addon_system::try_lock_exclusive(&file)
                        .and_then(|()| file.metadata());
                    (file, result)
                })
                .await?;
                let held = result.map_err(|e| fs_error(e, "flock", &path, None))?;
                fs.hook(IoOperation::Flock, &path, None, true, None).await?;
                let current = match fs.metadata(&path).await {
                    Ok(current) => Some(current),
                    Err(e) if is_not_found(e.as_ref()) => None,
                    Err(e) => return Err(e),
                };
                if current.is_some_and(|current| {
                    current.dev() == held.dev() && current.ino() == held.ino()
                }) {
                    Ok(Some(file))
                } else {
                    drop(file);
                    fs.hook(IoOperation::Close, &path, None, false, None)
                        .await?;
                    fs.hook(IoOperation::Close, &path, None, true, None).await?;
                    Ok(None)
                }
            }
            .await;
            match checked {
                Ok(Some(file)) => {
                    return Ok(Self {
                        file: Mutex::new(Some(file)),
                        fs,
                        path,
                    });
                }
                Ok(None) => {}
                Err(failure) => {
                    // The descriptor has left the failed attempt; source awaits close before rethrow.
                    fs.hook(IoOperation::Close, &path, None, false, None)
                        .await?;
                    fs.hook(IoOperation::Close, &path, None, true, None).await?;
                    let contention = failure
                        .downcast_ref::<NodeFileError>()
                        .is_some_and(|e| matches!(e.code, Some("EAGAIN" | "EWOULDBLOCK")))
                        || failure
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|e| e.kind() == std::io::ErrorKind::WouldBlock);
                    if contention {
                        return Err(Box::new(SessionAlreadyOwnedError::new(id.clone())));
                    }
                    return Err(failure);
                }
            }
        }
        Err(Box::new(SessionAlreadyOwnedError::new(id.clone())))
    }
    pub async fn release(&self) -> Result<()> {
        let file = lock(&self.file).take();
        if file.is_none() {
            return Ok(());
        }
        drop(file);
        self.fs
            .hook(IoOperation::Close, &self.path, None, false, None)
            .await?;
        self.fs
            .hook(IoOperation::Close, &self.path, None, true, None)
            .await
    }
}
