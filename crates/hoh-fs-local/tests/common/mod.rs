#![allow(dead_code)]
use futures_util::StreamExt;
pub async fn poll_once<F: std::future::Future + ?Sized>(
    mut future: std::pin::Pin<&mut F>,
) -> std::task::Poll<F::Output> {
    std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx))).await
}
pub fn isolated_io_test(future: impl std::future::Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
        .block_on(future);
}
pub struct IoHold(Option<std::sync::mpsc::Sender<()>>);
impl Drop for IoHold {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}
pub async fn hold_io() -> IoHold {
    let (started, reached) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    tokio::task::spawn_blocking(move || {
        let _ = started.send(());
        let _ = blocked.recv();
    });
    reached.await.unwrap();
    IoHold(Some(release))
}
use hoh_error::{BoxError, HarnessError};
use hoh_fs::*;
use hoh_fs_local::{Config, LocalFileSystem};
use hoh_timeout::{AbortController, AbortSignal};
pub struct Env {
    pub dir: tempfile::TempDir,
    pub fs: LocalFileSystem,
}
impl Env {
    pub fn new() -> Self {
        Self::limit(1024)
    }
    pub fn limit(n: u64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let fs = LocalFileSystem::new(Config {
            cwd: Some(dir.path().to_str().unwrap().into()),
            diff_basis_max_bytes: n as f64,
        })
        .unwrap();
        Self { dir, fs }
    }
    pub fn path(&self, name: &str) -> String {
        if name.is_empty() {
            return self.dir.path().to_str().unwrap().into();
        }
        self.dir.path().join(name).to_str().unwrap().into()
    }
    pub async fn target(&self, name: &str) -> FsTarget {
        self.fs
            .resolve(name, FsResolveOptions::default())
            .await
            .unwrap()
    }
    pub async fn file(&self, name: &str, bytes: impl AsRef<[u8]>) -> FsTarget {
        tokio::fs::write(self.path(name), bytes).await.unwrap();
        self.target(name).await
    }
    pub async fn bytes(&self, name: &str) -> String {
        tokio::fs::read_to_string(self.path(name)).await.unwrap()
    }
    pub async fn version(&self, t: &FsTarget) -> FsVersion {
        self.fs.stat(t, None).await.unwrap().unwrap().version
    }
    pub fn hooks(&self) -> std::sync::MutexGuard<'_, hoh_fs_local::fsio::FsIoInternals> {
        self.fs.internals.lock().unwrap()
    }
}
pub fn error<T>(r: Result<T, BoxError>, code: &str) -> BoxError {
    let e = match r {
        Err(e) => e,
        Ok(_) => panic!("expected {code}"),
    };
    assert_code(e.as_ref(), code);
    e
}
pub fn assert_code(e: &(dyn std::error::Error + 'static), code: &str) {
    let e = HarnessError::of(e).unwrap_or_else(|| panic!("not an FsError: {e}"));
    assert!(e.is_named(FsError::NAME));
    assert_eq!(e.code(), code);
}
pub fn aborted() -> AbortSignal {
    let c = AbortController::new();
    c.abort();
    c.signal()
}
pub fn edit(old: &str, new: &str) -> FsEditRequest {
    FsEditRequest {
        old_string: old.into(),
        new_string: new.into(),
        replace_all: false,
    }
}
pub fn guard(v: FsVersion) -> Option<FsVersionGuard> {
    Some(FsVersionGuard { version: v })
}
pub fn replace(v: FsVersion) -> Option<FsWriteIntent> {
    Some(FsWriteIntent::ReplaceIfVersion { version: v })
}
pub async fn collect(mut s: FsTextStream) -> Result<String, BoxError> {
    let mut text = String::new();
    while let Some(chunk) = s.next().await {
        text.push_str(&chunk?);
    }
    Ok(text)
}
pub fn target(path: String) -> FsTarget {
    FsTarget {
        target_key: path.clone().into(),
        display_path: path,
    }
}
pub fn errno(code: &str, message: &str) -> BoxError {
    Box::new(ErrnoError::new(Some(code.into()), message))
}
