use futures_util::StreamExt;
use hoh_error::BoxError;
use hoh_fs::*;
use hoh_fs_patch::PatchHunk;
use hoh_kernel::{Context, Fiber, Kernel, plugin, service};
use hoh_timeout::AbortSignal;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
#[derive(Default)]
struct Fake {
    files: Mutex<HashMap<String, String>>,
}
fn result<T: Send + 'static>(value: Result<T, BoxError>) -> FsFuture<'static, T> {
    Box::pin(std::future::ready(value))
}
fn unsupported<T: Send + 'static>(verb: &str) -> FsFuture<'static, T> {
    result(Err(FsError::new(
        format!("{verb} unsupported by test provider"),
        FsErrorCode::IoError,
    )
    .into()))
}
impl FileSystem for Fake {
    fn resolve<'a>(&'a self, path: &'a str, _opts: FsResolveOptions) -> FsFuture<'a, FsTarget> {
        result(Ok(FsTarget {
            target_key: path.into(),
            display_path: path.into(),
        }))
    }
    fn process_path(&self, t: &FsTarget) -> String {
        t.target_key.as_str().into()
    }
    fn file_url(&self, t: &FsTarget) -> String {
        let mut encoded = String::new();
        for b in t.target_key.as_str().bytes() {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                encoded.push(b as char);
            } else {
                encoded.push_str(&format!("%{b:02X}"));
            }
        }
        format!("file:///{encoded}")
    }
    fn contains(&self, p: &FsTarget, c: &FsTarget) -> bool {
        p.target_key == c.target_key
            || c.target_key
                .as_str()
                .starts_with(&format!("{}/", p.target_key.as_str()))
    }
    fn stat<'a>(
        &'a self,
        t: &'a FsTarget,
        _signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsInfo>> {
        result(Ok(self
            .files
            .lock()
            .unwrap()
            .get(t.target_key.as_str())
            .map(|s| FsInfo {
                version: "v1".into(),
                r#type: FsType::File,
                size: Some(s.encode_utf16().count() as u64),
            })))
    }
    fn lstat<'a>(
        &'a self,
        p: &'a str,
        _cwd: Option<&'a str>,
        _signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Option<FsPathInfo>> {
        result(Ok(self.files.lock().unwrap().get(p).map(|s| FsPathInfo {
            version: "v1".into(),
            r#type: FsPathType::File,
            size: Some(s.encode_utf16().count() as u64),
        })))
    }
    fn read_text<'a>(
        &'a self,
        t: &'a FsTarget,
        _signal: Option<AbortSignal>,
    ) -> FsFuture<'a, String> {
        result(
            self.files
                .lock()
                .unwrap()
                .get(t.target_key.as_str())
                .cloned()
                .ok_or_else(|| {
                    FsError::new(
                        format!("not found: {}", t.display_path),
                        FsErrorCode::NotFound,
                    )
                    .into()
                }),
        )
    }
    fn stream_text<'a>(
        &'a self,
        t: &'a FsTarget,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, FsTextStream> {
        Box::pin(async move {
            let text = self.read_text(t, signal).await?;
            Ok(Box::pin(futures_util::stream::iter([Ok(text)])) as FsTextStream)
        })
    }
    fn read_bytes<'a>(
        &'a self,
        t: &'a FsTarget,
        signal: Option<AbortSignal>,
        max: u64,
    ) -> FsFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let bytes = self.read_text(t, signal).await?.into_bytes();
            if bytes.len() as u64 > max {
                return Err(FsError::new(
                    format!("too large: {}", t.display_path),
                    FsErrorCode::TooLarge,
                )
                .into());
            }
            Ok(bytes)
        })
    }
    fn read_byte_range<'a>(
        &'a self,
        t: &'a FsTarget,
        range: FsByteRange,
        signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let b = self.read_text(t, signal).await?.into_bytes();
            let start = (range.offset as usize).min(b.len());
            let end = start.saturating_add(range.length as usize).min(b.len());
            Ok(b[start..end].into())
        })
    }
    fn list_dir<'a>(
        &'a self,
        t: &'a FsTarget,
        _signal: Option<AbortSignal>,
    ) -> FsFuture<'a, Vec<FsDirEntry>> {
        if t.target_key.as_str() != "skills" {
            return result(Err(FsError::new(
                format!("not a directory: {}", t.display_path),
                FsErrorCode::NotDirectory,
            )
            .into()));
        }
        result(Ok(vec![FsDirEntry {
            name: "alpha.md".into(),
            r#type: FsType::File,
            target: FsTarget {
                target_key: "skills/alpha.md".into(),
                display_path: "skills/alpha.md".into(),
            },
            version: Some("v1".into()),
            size: Some(2),
        }]))
    }
    fn write_text<'a>(
        &'a self,
        t: &'a FsTarget,
        content: &'a str,
        _expected: Option<FsWriteIntent>,
        _signal: Option<AbortSignal>,
        _policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsWriteOutcome> {
        let before = self
            .files
            .lock()
            .unwrap()
            .insert(t.target_key.as_str().into(), content.into());
        result(Ok(FsWriteOutcome {
            operation: if before.is_some() {
                FsWriteOperation::Update
            } else {
                FsWriteOperation::Create
            },
            version: "v2".into(),
            before,
            after: content.into(),
        }))
    }
    fn edit_text<'a>(
        &'a self,
        t: &'a FsTarget,
        edit: FsEditRequest,
        _expected: Option<FsVersionGuard>,
        _signal: Option<AbortSignal>,
        _policy: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        let mut files = self.files.lock().unwrap();
        let before = files
            .get(t.target_key.as_str())
            .cloned()
            .unwrap_or_default();
        let after = before.replace(&edit.old_string, &edit.new_string);
        files.insert(t.target_key.as_str().into(), after.clone());
        result(Ok(FsEditOutcome {
            version: "v3".into(),
            before,
            after,
        }))
    }
    fn patch_text<'a>(
        &'a self,
        _t: &'a FsTarget,
        _hunks: &'a [PatchHunk],
        _e: Option<FsVersionGuard>,
        _s: Option<AbortSignal>,
        _p: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsEditOutcome> {
        unsupported("patch")
    }
    fn delete_file<'a>(
        &'a self,
        _t: &'a FsTarget,
        _e: Option<FsVersionGuard>,
        _s: Option<AbortSignal>,
        _p: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsDeleteOutcome> {
        unsupported("delete")
    }
    fn move_file<'a>(
        &'a self,
        _t: &'a FsTarget,
        _d: &'a FsTarget,
        _o: FsMoveOptions,
        _s: Option<AbortSignal>,
        _p: Option<SandboxExecutionPolicy>,
    ) -> FsFuture<'a, FsMoveOutcome> {
        unsupported("move")
    }
}
service!(FakeKey:Fake="fakeFs");
#[plugin(FakeProvider, name = "FakeFileSystem")]
async fn apply(ctx: Context) -> Result<(), BoxError> {
    let f = Arc::new(Fake::default());
    ctx.provide::<Fs>(f.clone()).await?;
    ctx.provide::<FakeKey>(f).await?;
    Ok(())
}
async fn setup() -> (Kernel, Context, Fiber) {
    let k = Kernel::start();
    let ctx = k.root();
    let f = ctx.plugin::<FakeProvider>(serde_json::json!({})).unwrap();
    f.settled().await.unwrap();
    (k, ctx, f)
}
async fn target(ctx: &Context, p: &str) -> FsTarget {
    ctx.get::<Fs>()
        .unwrap()
        .resolve(p, FsResolveOptions::default())
        .await
        .unwrap()
}
fn seed(ctx: &Context, p: &str, s: &str) {
    ctx.get::<FakeKey>()
        .unwrap()
        .files
        .lock()
        .unwrap()
        .insert(p.into(), s.into());
}
#[tokio::test]
async fn registers_as_ctx_fs_and_serves_the_primitives() {
    let (k, c, _) = setup().await;
    let fs = c.get::<Fs>().unwrap();
    assert!(fs.sandbox_mode().is_none());
    assert!(fs.process_path_from_host_path("/host/file").is_none());
    seed(&c, "a.txt", "hi");
    let t = target(&c, "a.txt").await;
    assert_eq!(
        fs.stat(&t, None).await.unwrap().unwrap().r#type,
        FsType::File
    );
    assert_eq!(fs.read_text(&t, None).await.unwrap(), "hi");
    k.shutdown().await;
}
#[tokio::test]
async fn throws_when_a_second_implementation_is_loaded() {
    let (k, c, _) = setup().await;
    let second = c.plugin::<FakeProvider>(serde_json::json!({})).unwrap();
    assert!(second.settled().await.is_err());
    k.shutdown().await;
}
#[tokio::test]
async fn removes_the_service_when_the_providing_fiber_is_disposed() {
    let (k, c, f) = setup().await;
    assert!(c.get::<Fs>().is_some());
    f.dispose().await;
    assert!(c.get::<Fs>().is_none());
    k.shutdown().await;
}
#[tokio::test]
async fn stream_text_yields_the_same_text_read_text_returns() {
    let (k, c, _) = setup().await;
    seed(&c, "a.txt", "one\ntwo");
    let fs = c.get::<Fs>().unwrap();
    let t = target(&c, "a.txt").await;
    let mut s = fs.stream_text(&t, None).await.unwrap();
    let mut text = String::new();
    while let Some(chunk) = s.next().await {
        text.push_str(&chunk.unwrap());
    }
    assert_eq!(text, fs.read_text(&t, None).await.unwrap());
    k.shutdown().await;
}
#[tokio::test]
async fn read_byte_range_returns_the_window_shorter_at_end_and_empty_past_it() {
    let (k, c, _) = setup().await;
    seed(&c, "a.bin", "hello");
    let fs = c.get::<Fs>().unwrap();
    let t = target(&c, "a.bin").await;
    for (offset, length, expected) in [(1, 3, "ell"), (3, 10, "lo"), (9, 2, "")] {
        assert_eq!(
            fs.read_byte_range(&t, FsByteRange { offset, length }, None)
                .await
                .unwrap(),
            expected.as_bytes()
        );
    }
    k.shutdown().await;
}
#[tokio::test]
async fn read_bytes_returns_raw_content_and_enforces_the_byte_cap() {
    let (k, c, _) = setup().await;
    seed(&c, "a.bin", "hi");
    let fs = c.get::<Fs>().unwrap();
    let t = target(&c, "a.bin").await;
    assert_eq!(fs.read_bytes(&t, None, 2).await.unwrap(), b"hi");
    assert_eq!(
        hoh_error::HarnessError::of(fs.read_bytes(&t, None, 1).await.unwrap_err().as_ref())
            .unwrap()
            .code(),
        "FS_TOO_LARGE"
    );
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_watching_when_a_provider_does_not_implement_observation() {
    let (k, c, _) = setup().await;
    let t = target(&c, "a.txt").await;
    let error = c
        .get::<Fs>()
        .unwrap()
        .watch(
            &t,
            Arc::new(|_| {}),
            hoh_timeout::AbortController::new().signal(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        "FS_IO_ERROR"
    );
    k.shutdown().await;
}
#[tokio::test]
async fn list_dir_returns_child_entry_targets_without_reading_content() {
    let (k, c, _) = setup().await;
    let entries = c
        .get::<Fs>()
        .unwrap()
        .list_dir(&target(&c, "skills").await, None)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(entries).unwrap(),
        serde_json::json!([{"name":"alpha.md","type":"file","target":{"targetKey":"skills/alpha.md","displayPath":"skills/alpha.md"},"version":"v1","size":2}])
    );
    k.shutdown().await;
}
#[tokio::test]
async fn stat_returns_undefined_for_an_absent_target() {
    let (k, c, _) = setup().await;
    assert!(
        c.get::<Fs>()
            .unwrap()
            .stat(&target(&c, "missing.txt").await, None)
            .await
            .unwrap()
            .is_none()
    );
    k.shutdown().await;
}
#[tokio::test]
async fn lstat_returns_path_metadata_before_resolving_a_target() {
    let (k, c, _) = setup().await;
    seed(&c, "a.txt", "hi");
    let fs = c.get::<Fs>().unwrap();
    assert_eq!(
        serde_json::to_value(fs.lstat("a.txt", None, None).await.unwrap()).unwrap(),
        serde_json::json!({"version":"v1","type":"file","size":2})
    );
    assert!(fs.lstat("missing", None, None).await.unwrap().is_none());
    k.shutdown().await;
}
#[test]
fn carries_a_stable_code_and_harness_error_name() {
    let e = FsError::new("nope", FsErrorCode::NotFound);
    assert_eq!(e.code(), "FS_NOT_FOUND");
    assert_eq!(e.name(), "FsError");
}
#[test]
fn chains_an_underlying_cause_through_error_options() {
    let e = FsError::new("cannot read", FsErrorCode::Aborted)
        .with_cause(std::io::Error::other("EACCES"));
    assert_eq!(e.cause().unwrap().to_string(), "EACCES");
    assert_eq!(e.code(), "FS_ABORTED");
}
#[tokio::test]
async fn exposes_guarded_patch_delete_and_move_on_the_provider_interface() {
    let (k, c, _) = setup().await;
    let fs = c.get::<Fs>().unwrap();
    let t = target(&c, "a").await;
    let a = fs.patch_text(&t, &[], None, None, None).await.unwrap_err();
    let b = fs.delete_file(&t, None, None, None).await.unwrap_err();
    let d = fs
        .move_file(&t, &t, FsMoveOptions::default(), None, None)
        .await
        .unwrap_err();
    for e in [a, b, d] {
        assert_eq!(
            hoh_error::HarnessError::of(e.as_ref()).unwrap().code(),
            "FS_IO_ERROR"
        );
    }
    assert_eq!(
        FsError::new("mismatch", FsErrorCode::PatchMismatch).code(),
        "FS_PATCH_MISMATCH"
    );
    assert_eq!(
        FsError::new("same", FsErrorCode::SameTarget).code(),
        "FS_SAME_TARGET"
    );
    k.shutdown().await;
}
