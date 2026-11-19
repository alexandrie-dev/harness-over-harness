//! Port of credentials-local/tests/watcher.spec.ts: the watcher pipeline.
//!
//! The file watcher is the nondeterministic OS boundary: a fake watcher drives the event pipeline
//! (error events, races with unreadable files) deterministically, exactly as the source drives a
//! fake `chokidar`. Real end-to-end watching stays covered by `local.rs`.
mod support;

use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use hoh_credentials_local::{
    Seams,
    watch::{WatchRequest, WatchSink, Watcher, WatcherFactory},
};
use hoh_error::BoxError;
use support::*;

struct Instance {
    path: PathBuf,
    stability_threshold_ms: u64,
    poll_interval_ms: u64,
    sink: WatchSink,
    closed: Arc<AtomicBool>,
}

struct FakeWatcher {
    closed: Arc<AtomicBool>,
}

impl Watcher for FakeWatcher {
    fn close(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        self.closed.store(true, Ordering::SeqCst);
        Box::pin(async {})
    }
}

type Instances = Arc<Mutex<Vec<Instance>>>;

fn fake_factory(instances: Instances) -> WatcherFactory {
    Arc::new(
        move |request: WatchRequest| -> Result<Box<dyn Watcher>, BoxError> {
            let closed = Arc::new(AtomicBool::new(false));
            instances.lock().unwrap().push(Instance {
                path: request.path,
                stability_threshold_ms: request.stability_threshold_ms,
                poll_interval_ms: request.poll_interval_ms,
                sink: request.sink,
                closed: closed.clone(),
            });
            Ok(Box::new(FakeWatcher { closed }))
        },
    )
}

async fn boot_fake(path: &std::path::Path, debounce_ms: u64) -> (Booted, Instances) {
    let instances = Instances::default();
    let booted = boot_seamed(
        watching(path, debounce_ms),
        Seams {
            writer: None,
            watcher: Some(fake_factory(instances.clone())),
        },
    )
    .await;
    (booted, instances)
}

fn sink_of(instances: &Instances) -> WatchSink {
    instances.lock().unwrap()[0].sink.clone()
}

fn good_value(info: Option<(String, String)>) -> Option<String> {
    info.map(|(value, _)| value)
}

#[tokio::test]
async fn clamps_the_write_settle_poll_interval_for_a_zero_debounce() {
    let dir = temp_dir();
    let path = doc(&dir);
    let (booted, instances) = boot_fake(&path, 0).await;
    {
        let instances = instances.lock().unwrap();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].stability_threshold_ms, 0);
        assert_eq!(instances[0].poll_interval_ms, 1);
        // The watcher is asked for the document's canonical path.
        assert_eq!(instances[0].path.file_name().unwrap(), ".credentials.yaml");
    }
    booted.shutdown().await;
}

#[tokio::test]
async fn survives_a_watcher_error_and_keeps_publishing_later_edits() {
    let dir = temp_dir();
    let path = doc(&dir);
    let (booted, instances) = boot_fake(&path, 5).await;
    let sink = sink_of(&instances);
    let credentials = booted.credentials();

    sink.error("watch backend failure".into());
    assert_eq!(resolved(&*credentials, "DSH_CRED_PIPE").await, None);

    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_PIPE: arrived\n");
    sink.all();
    wait_for(|| async {
        good_value(resolved(&*credentials, "DSH_CRED_PIPE").await).as_deref() == Some("arrived")
    })
    .await;
    booted.shutdown().await;
}

#[tokio::test]
async fn keeps_the_last_good_snapshot_when_the_file_turns_unreadable_at_runtime() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_PIPE: good\n");
    let (booted, instances) = boot_fake(&path, 5).await;

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    sink_of(&instances).all();
    // The warn-and-keep path is asynchronous; give the serialized refresh a turn.
    pause(100).await;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        good_value(resolved(&*booted.credentials(), "DSH_CRED_PIPE").await).as_deref(),
        Some("good")
    );
    let warned = warnings(&booted.ctx);
    assert!(
        warned
            .iter()
            .any(|message| message.contains("reload failed")),
        "{warned:?}"
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn keeps_the_last_good_snapshot_when_the_read_fails_after_its_permission_check() {
    use std::os::unix::fs::DirBuilderExt;
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_PIPE: good\n");
    let (booted, instances) = boot_fake(&path, 5).await;

    // An owner-only directory in the document's place passes the permission check and then fails
    // the read (the source injects an EACCES into `readFile`; a native read cannot be injected).
    std::fs::remove_file(&path).unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    sink_of(&instances).all();
    wait_for(|| async {
        warnings(&booted.ctx)
            .iter()
            .any(|message| message.contains("reload failed"))
    })
    .await;
    assert_eq!(
        good_value(resolved(&*booted.credentials(), "DSH_CRED_PIPE").await).as_deref(),
        Some("good")
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn contains_a_watcher_error_emitted_after_close() {
    let dir = temp_dir();
    let path = doc(&dir);
    let (booted, instances) = boot_fake(&path, 5).await;
    let sink = sink_of(&instances);
    let closed = instances.lock().unwrap()[0].closed.clone();
    let before = warnings(&booted.ctx).len();
    booted.fiber.dispose().await;
    assert!(closed.load(Ordering::SeqCst), "the watcher was closed");

    // A backend error still in flight after close is dropped, not reported or thrown.
    sink.error("EPERM: operation not permitted, stat 'C:\\Temp\\dsh\\.credentials.yaml'".into());
    assert_eq!(warnings(&booted.ctx).len(), before);
    booted.kernel.shutdown().await;
}

#[tokio::test]
async fn quiesces_the_refresh_pipeline_before_dispose_completes() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_PIPE: initial\n");
    let (booted, instances) = boot_fake(&path, 5).await;
    let disposed = Arc::new(AtomicBool::new(false));
    let post_dispose_commits = Arc::new(Mutex::new(0_u32));
    {
        let (disposed, commits) = (disposed.clone(), post_dispose_commits.clone());
        booted
            .ctx
            .on_contained::<hoh_credentials::CredentialsReferenceUpdated>(move |_| {
                if disposed.load(Ordering::SeqCst) {
                    *commits.lock().unwrap() += 1;
                }
                Ok(())
            })
            .unwrap();
    }

    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_PIPE: changed\n");
    let sink = sink_of(&instances);
    // Two queued refreshes: dispose interrupts one mid-flight and the other before it starts, so
    // both closed guards must hold.
    sink.all();
    sink.all();
    booted.fiber.dispose().await;
    disposed.store(true, Ordering::SeqCst);
    sink.all();
    sink.ready();
    pause(200).await;
    assert_eq!(*post_dispose_commits.lock().unwrap(), 0);
    booted.kernel.shutdown().await;
}

#[tokio::test]
async fn empties_the_snapshot_when_the_document_is_deleted_and_emits_the_removals() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_PIPE: doomed\n");
    let (booted, instances) = boot_fake(&path, 5).await;
    let seen = updates(&booted.ctx);
    let credentials = booted.credentials();

    std::fs::remove_file(&path).unwrap();
    sink_of(&instances).all();
    wait_for(|| async { resolved(&*credentials, "DSH_CRED_PIPE").await.is_none() }).await;
    assert_eq!(snapshot(&seen), vec!["DSH_CRED_PIPE"]);
    booted.shutdown().await;
}

#[tokio::test]
async fn keeps_the_last_good_snapshot_when_an_external_edit_makes_the_document_invalid() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_PIPE: a\n");
    let (booted, instances) = boot_fake(&path, 5).await;
    let seen = updates(&booted.ctx);
    let credentials = booted.credentials();
    let sink = sink_of(&instances);

    // A key the seam cannot address is a rejection, not preserved content: a live reload must warn
    // and keep serving the last good snapshot rather than take the process down or silently drop
    // the entry it could not validate.
    write_credentials(
        &path,
        "version: 1\nrefs:\n  BAD-KEY: 2\n  DSH_CRED_PIPE: b\n",
    );
    sink.all();
    pause(100).await;
    assert_eq!(
        good_value(resolved(&*credentials, "DSH_CRED_PIPE").await).as_deref(),
        Some("a")
    );
    assert!(snapshot(&seen).is_empty());

    // Repairing the document resumes publishing.
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_PIPE: b\n");
    sink.all();
    wait_for(|| async {
        good_value(resolved(&*credentials, "DSH_CRED_PIPE").await).as_deref() == Some("b")
    })
    .await;
    assert_eq!(snapshot(&seen), vec!["DSH_CRED_PIPE"]);
    booted.shutdown().await;
}

#[tokio::test]
async fn warns_about_an_unknown_top_level_key_a_live_reload_brings_in_once_per_change() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_PIPE: a\n");
    let (booted, instances) = boot_fake(&path, 5).await;
    let credentials = booted.credentials();
    assert!(warnings(&booted.ctx).is_empty());

    write_credentials(&path, "version: 1\nFOREIGN: x\nrefs:\n  DSH_CRED_PIPE: b\n");
    let sink = sink_of(&instances);
    sink.all();
    wait_for(|| async {
        good_value(resolved(&*credentials, "DSH_CRED_PIPE").await).as_deref() == Some("b")
    })
    .await;
    let warned = warnings(&booted.ctx);
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(
        warned[0].contains("ignoring unknown top-level"),
        "{warned:?}"
    );
    assert!(warned[0].contains("\"FOREIGN\""), "{warned:?}");
    assert!(
        warned[0].contains(&path.to_string_lossy().into_owned()),
        "{warned:?}"
    );

    // An event that finds the same text publishes and reports nothing again.
    sink.all();
    pause(100).await;
    assert_eq!(warnings(&booted.ctx).len(), 1);
    booted.shutdown().await;
}

#[tokio::test]
async fn treats_an_event_for_a_still_absent_file_as_a_no_op() {
    let dir = temp_dir();
    let path = doc(&dir);
    let (booted, instances) = boot_fake(&path, 5).await;
    sink_of(&instances).all();
    pause(100).await;
    assert_eq!(
        resolved(&*booted.credentials(), "DSH_CRED_PIPE").await,
        None
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn reconciles_at_watcher_ready_so_a_change_during_setup_is_not_missed() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_PIPE: a\n");
    let (booted, instances) = boot_fake(&path, 5).await;
    let credentials = booted.credentials();
    // Written after the initial load but before the watcher became active: no 'all' event will ever
    // fire for it.
    write_credentials(
        &path,
        "version: 1\nrefs:\n  DSH_CRED_PIPE: written-before-ready\n",
    );
    sink_of(&instances).ready();
    wait_for(|| async {
        good_value(resolved(&*credentials, "DSH_CRED_PIPE").await).as_deref()
            == Some("written-before-ready")
    })
    .await;
    booted.shutdown().await;
}

#[tokio::test]
async fn never_puts_an_unknown_version_in_a_reload_warning() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(&path, "version: 1\nrefs:\n  DSH_CRED_PIPE: a\n");
    let (booted, instances) = boot_fake(&path, 5).await;
    write_credentials(
        &path,
        "version: sk-founder-secret\nrefs:\n  DSH_CRED_PIPE: b\n",
    );
    sink_of(&instances).all();
    wait_for(|| async {
        warnings(&booted.ctx)
            .iter()
            .any(|message| message.contains("reload failed"))
    })
    .await;
    let warned = warnings(&booted.ctx);
    assert!(
        warned
            .iter()
            .any(|message| message.contains("unsupported version")),
        "{warned:?}"
    );
    assert!(
        warned
            .iter()
            .all(|message| !message.contains("sk-founder-secret")),
        "{warned:?}"
    );
    booted.shutdown().await;
}
