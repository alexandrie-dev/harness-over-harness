//! Async filesystem operations and explicit test seams; no process-global mocks.
use crate::{Result, error::error};
use futures_util::future::BoxFuture;
use std::os::unix::fs::MetadataExt;
use std::{
    fs::Metadata,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoOperation {
    Open,
    Read,
    ReadFile,
    Write,
    Flush,
    Sync,
    Close,
    Stat,
    Lstat,
    ReadDir,
    Mkdir,
    Link,
    Remove,
    Truncate,
    Canonicalize,
    Flock,
}
#[derive(Debug, Clone)]
pub struct IoRequest {
    pub operation: IoOperation,
    pub path: String,
    pub dest: Option<String>,
    pub after: bool,
    pub bytes: Option<usize>,
}
pub type IoHook = Arc<dyn Fn(IoRequest) -> BoxFuture<'static, Result<()>> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct NodeFileError {
    message: String,
    pub code: Option<&'static str>,
    pub path: String,
    pub dest: Option<String>,
    #[source]
    pub source: std::io::Error,
}
pub(crate) fn fs_error(
    source: std::io::Error,
    syscall: &str,
    path: &str,
    dest: Option<&str>,
) -> hoh_error::BoxError {
    Box::new(NodeFileError {
        message: hoh_error::node_fs_error_message(&source, syscall, Some(path), dest),
        code: hoh_error::node_error_code(&source),
        path: path.into(),
        dest: dest.map(str::to_owned),
        source,
    })
}
fn fd_error(source: std::io::Error, syscall: &str, path: &str) -> hoh_error::BoxError {
    // Node FileHandle errors describe the fd syscall without a pathname.
    Box::new(NodeFileError {
        message: hoh_error::node_fs_error_message(&source, syscall, None, None),
        code: hoh_error::node_error_code(&source),
        path: path.into(),
        dest: None,
        source,
    })
}
impl NodeFileError {
    pub fn new(source: std::io::Error, syscall: &str, path: &str, dest: Option<&str>) -> Self {
        Self {
            message: hoh_error::node_fs_error_message(&source, syscall, Some(path), dest),
            code: hoh_error::node_error_code(&source),
            path: path.into(),
            dest: dest.map(str::to_owned),
            source,
        }
    }
}
pub(crate) fn is_not_found(error: &(dyn std::error::Error + 'static)) -> bool {
    error
        .downcast_ref::<NodeFileError>()
        .is_some_and(|e| e.code == Some("ENOENT"))
        || error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}
pub(crate) fn is_io(error: &(dyn std::error::Error + 'static)) -> bool {
    error.is::<NodeFileError>() || error.is::<std::io::Error>()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonlPhysicalIdentity {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_ns: i128,
    pub ctime_ns: i128,
}
impl JsonlPhysicalIdentity {
    pub(crate) fn from_metadata(value: &Metadata) -> Self {
        Self {
            dev: value.dev(),
            ino: value.ino(),
            size: value.len(),
            mtime_ns: i128::from(value.mtime()) * 1_000_000_000 + i128::from(value.mtime_nsec()),
            ctime_ns: i128::from(value.ctime()) * 1_000_000_000 + i128::from(value.ctime_nsec()),
        }
    }
    pub fn revision(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.dev, self.ino, self.size, self.mtime_ns, self.ctime_ns
        )
    }
}
#[derive(Clone, Default)]
pub(crate) struct FileSystem {
    pub hook: Option<IoHook>,
}
impl FileSystem {
    pub(crate) async fn hook(
        &self,
        operation: IoOperation,
        path: &str,
        dest: Option<&str>,
        after: bool,
        bytes: Option<usize>,
    ) -> Result<()> {
        if let Some(hook) = &self.hook {
            hook(IoRequest {
                operation,
                path: path.into(),
                dest: dest.map(str::to_owned),
                after,
                bytes,
            })
            .await?;
        }
        Ok(())
    }
    pub(crate) async fn metadata(&self, path: &str) -> Result<Metadata> {
        self.hook(IoOperation::Stat, path, None, false, None)
            .await?;
        let info = tokio::fs::metadata(path)
            .await
            .map_err(|e| fs_error(e, "stat", path, None))?;
        self.hook(IoOperation::Stat, path, None, true, None).await?;
        Ok(info)
    }
    pub(crate) async fn identity(&self, path: &str) -> Result<JsonlPhysicalIdentity> {
        Ok(JsonlPhysicalIdentity::from_metadata(
            &self.metadata(path).await?,
        ))
    }
    pub(crate) async fn symlink_metadata(&self, path: &str) -> Result<Metadata> {
        self.hook(IoOperation::Lstat, path, None, false, None)
            .await?;
        let info = tokio::fs::symlink_metadata(path)
            .await
            .map_err(|e| fs_error(e, "lstat", path, None))?;
        self.hook(IoOperation::Lstat, path, None, true, None)
            .await?;
        Ok(info)
    }
    pub(crate) async fn read_dir(&self, path: &str) -> Result<Vec<(String, bool, bool)>> {
        self.hook(IoOperation::ReadDir, path, None, false, None)
            .await?;
        let mut dir = tokio::fs::read_dir(path)
            .await
            .map_err(|e| fs_error(e, "scandir", path, None))?;
        let mut entries = vec![];
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| fs_error(e, "scandir", path, None))?
        {
            let kind = entry
                .file_type()
                .await
                .map_err(|e| fs_error(e, "scandir", path, None))?;
            entries.push((
                entry.file_name().to_string_lossy().into_owned(),
                kind.is_dir(),
                kind.is_file(),
            ));
        }
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        self.hook(IoOperation::ReadDir, path, None, true, None)
            .await?;
        Ok(entries)
    }
    pub(crate) async fn mkdir(&self, path: &str) -> Result<()> {
        self.hook(IoOperation::Mkdir, path, None, false, None)
            .await?;
        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .await
            .map_err(|e| fs_error(e, "mkdir", path, None))?;
        self.hook(IoOperation::Mkdir, path, None, true, None).await
    }
    pub(crate) async fn link(&self, source: &str, target: &str) -> Result<()> {
        self.hook(IoOperation::Link, source, Some(target), false, None)
            .await?;
        tokio::fs::hard_link(source, target)
            .await
            .map_err(|e| fs_error(e, "link", source, Some(target)))?;
        self.hook(IoOperation::Link, source, Some(target), true, None)
            .await
    }
    pub(crate) async fn remove(&self, path: &str) -> Result<()> {
        self.hook(IoOperation::Remove, path, None, false, None)
            .await?;
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(fs_error(e, "unlink", path, None)),
        }
        self.hook(IoOperation::Remove, path, None, true, None).await
    }
    pub(crate) async fn canonicalize(&self, path: &str) -> Result<String> {
        self.hook(IoOperation::Canonicalize, path, None, false, None)
            .await?;
        let resolved = tokio::fs::canonicalize(path)
            .await
            .map_err(|e| fs_error(e, "realpath", path, None))?;
        self.hook(IoOperation::Canonicalize, path, None, true, None)
            .await?;
        Ok(resolved.to_string_lossy().into_owned())
    }
    pub(crate) async fn open(&self, path: &str, mode: &str) -> Result<OpenFile> {
        self.hook(IoOperation::Open, path, None, false, None)
            .await?;
        let mut options = tokio::fs::OpenOptions::new();
        match mode {
            "r" => {
                options.read(true);
            }
            "r+" => {
                options.read(true).write(true);
            }
            "a" => {
                options.append(true).create(true);
            }
            "wx" => {
                options.write(true).create_new(true).mode(0o600);
            }
            _ => return Err(error("invalid JSONL file open mode")),
        };
        let file = options
            .open(path)
            .await
            .map_err(|e| fs_error(e, "open", path, None))?;
        self.hook(IoOperation::Open, path, None, true, None).await?;
        Ok(OpenFile {
            file: Some(file),
            path: path.into(),
            fs: self.clone(),
        })
    }
    pub(crate) async fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.hook(IoOperation::ReadFile, path, None, false, None)
            .await?;
        let mut file = self.open(path, "r").await?;
        let mut output = vec![];
        let mut scratch = vec![0; 1024 * 1024];
        loop {
            let count = file.read(&mut scratch).await?;
            if count == 0 {
                break;
            }
            output.extend_from_slice(&scratch[..count]);
        }
        file.close().await?;
        self.hook(IoOperation::ReadFile, path, None, true, Some(output.len()))
            .await?;
        Ok(output)
    }
    pub(crate) async fn sync_dir(&self, path: &str) -> Result<()> {
        let mut handle = self.open(path, "r").await?;
        handle.sync().await?;
        handle.close().await
    }
}
pub(crate) struct OpenFile {
    file: Option<tokio::fs::File>,
    path: String,
    fs: FileSystem,
}
impl OpenFile {
    fn file(&mut self) -> Result<&mut tokio::fs::File> {
        self.file
            .as_mut()
            .ok_or_else(|| error("JSONL file is closed"))
    }
    pub(crate) async fn read(&mut self, output: &mut [u8]) -> Result<usize> {
        self.fs
            .hook(
                IoOperation::Read,
                &self.path,
                None,
                false,
                Some(output.len()),
            )
            .await?;
        let result = self
            .file()?
            .read(output)
            .await
            .map_err(|e| fd_error(e, "read", &self.path))?;
        self.fs
            .hook(IoOperation::Read, &self.path, None, true, Some(result))
            .await?;
        Ok(result)
    }
    pub(crate) async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.fs
            .hook(
                IoOperation::Write,
                &self.path,
                None,
                false,
                Some(bytes.len()),
            )
            .await?;
        self.file()?
            .write_all(bytes)
            .await
            .map_err(|e| fd_error(e, "write", &self.path))?;
        self.fs
            .hook(
                IoOperation::Write,
                &self.path,
                None,
                true,
                Some(bytes.len()),
            )
            .await
    }
    pub(crate) async fn flush(&mut self) -> Result<()> {
        self.fs
            .hook(IoOperation::Flush, &self.path, None, false, None)
            .await?;
        self.file()?
            .flush()
            .await
            .map_err(|e| fd_error(e, "write", &self.path))?;
        self.fs
            .hook(IoOperation::Flush, &self.path, None, true, None)
            .await
    }
    pub(crate) async fn sync(&mut self) -> Result<()> {
        // Tokio's sync_all discards an outstanding write error: flush must expose it first.
        self.flush().await?;
        self.fs
            .hook(IoOperation::Sync, &self.path, None, false, None)
            .await?;
        self.file()?
            .sync_all()
            .await
            .map_err(|e| fd_error(e, "fsync", &self.path))?;
        self.fs
            .hook(IoOperation::Sync, &self.path, None, true, None)
            .await
    }
    pub(crate) async fn truncate(&mut self, size: u64) -> Result<()> {
        self.fs
            .hook(IoOperation::Truncate, &self.path, None, false, None)
            .await?;
        self.file()?
            .set_len(size)
            .await
            .map_err(|e| fd_error(e, "ftruncate", &self.path))?;
        self.fs
            .hook(IoOperation::Truncate, &self.path, None, true, None)
            .await
    }
    pub(crate) async fn size(&mut self) -> Result<u64> {
        let path = self.path.clone();
        Ok(self
            .file()?
            .metadata()
            .await
            .map_err(|e| fd_error(e, "fstat", &path))?
            .len())
    }
    pub(crate) async fn close(mut self) -> Result<()> {
        let check = self
            .fs
            .hook(IoOperation::Close, &self.path, None, false, None)
            .await;
        // Drain deferred I/O even if the test seam injects close failure; ownership is released.
        let flush = self.flush().await;
        drop(self.file.take());
        check?;
        flush?;
        self.fs
            .hook(IoOperation::Close, &self.path, None, true, None)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fd_read_errors_match_node_without_a_path_suffix() {
        // hoh-error/tests/fixtures/node_system_errors.json: fsMessages.readFile directory.
        let directory = tempfile::tempdir().unwrap();
        let error = FileSystem::default()
            .read(directory.path().to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "EISDIR: illegal operation on a directory, read"
        );
        assert_eq!(
            error.downcast_ref::<NodeFileError>().unwrap().code,
            Some("EISDIR")
        );
    }
}
