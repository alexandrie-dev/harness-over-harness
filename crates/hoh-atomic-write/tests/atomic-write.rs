use super::*;
use hoh_error::SharedError;
use hoh_timeout::{AbortController, is_abort_error};
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

const EXITED_PID: u32 = 2_000_000_000;
#[derive(Default)]
struct State {
    windows: bool,
    rename_failures: VecDeque<io::ErrorKind>,
    rename_attempts: usize,
    claim_failure: Option<io::ErrorKind>,
    claim_removal_fails: bool,
    lock_removal_fails: bool,
    permission_failures: usize,
    release_before_probe: bool,
    after_claim: Option<String>,
    probe_responses: Option<usize>,
    other_user: bool,
    probe_all_exited: bool,
    abort_after_lock: Option<(Arc<AbortController>, SharedError)>,
    identity_failure: bool,
    pause_write: Option<FileKind>,
    pause_remove: Option<FileKind>,
    paused: Option<tokio::sync::oneshot::Sender<PathBuf>>,
}
#[derive(Default)]
struct Fake(Mutex<State>);
fn denied() -> io::Error {
    io::Error::from_raw_os_error(if cfg!(windows) { 5 } else { 1 })
}
fn claim(path: &Path) -> bool {
    path.to_string_lossy().contains(".lock.takeover-")
}
fn lock(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".lock")
}
impl Runtime for Fake {
    fn windows(&self) -> bool {
        self.0.lock().unwrap().windows
    }
    async fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let fail = {
            let mut s = self.0.lock().unwrap();
            s.rename_attempts += 1;
            s.rename_failures.pop_front()
        };
        if let Some(kind) = fail {
            return Err(match kind {
                io::ErrorKind::PermissionDenied => {
                    #[cfg(unix)]
                    {
                        io::Error::from_raw_os_error(rustix::io::Errno::ACCESS.raw_os_error())
                    }
                    #[cfg(windows)]
                    {
                        io::Error::from_raw_os_error(5)
                    }
                }
                io::ErrorKind::ResourceBusy => {
                    #[cfg(unix)]
                    {
                        io::Error::from_raw_os_error(rustix::io::Errno::BUSY.raw_os_error())
                    }
                    #[cfg(windows)]
                    {
                        io::Error::from_raw_os_error(170)
                    }
                }
                kind => io::Error::new(kind, "injected rename failure without a code"),
            });
        }
        tokio::fs::rename(from, to).await
    }
    async fn remove(&self, path: &Path) -> io::Result<()> {
        let kind = if claim(path) {
            FileKind::Claim
        } else if lock(path) {
            FileKind::Lock
        } else {
            FileKind::Temp
        };
        let paused = {
            let mut state = self.0.lock().unwrap();
            if state.pause_remove == Some(kind) {
                state.pause_remove = None;
                Some(state.paused.take())
            } else {
                None
            }
        };
        if let Some(notify) = paused {
            if let Some(notify) = notify {
                let _ = notify.send(path.into());
            }
            std::future::pending::<()>().await;
        }
        let fail = {
            let s = self.0.lock().unwrap();
            claim(path) && s.claim_removal_fails || lock(path) && s.lock_removal_fails
        };
        if fail {
            return Err(io::ErrorKind::ResourceBusy.into());
        }
        remove(path).await
    }
    async fn write_new(&self, owner: &FileGuard, mode: u32) -> io::Result<()> {
        let path = &owner.shared.path;
        let (failure, release) = {
            let mut s = self.0.lock().unwrap();
            if claim(path) {
                (s.claim_failure, false)
            } else if lock(path) && s.permission_failures > 0 {
                s.permission_failures -= 1;
                (
                    Some(io::ErrorKind::PermissionDenied),
                    s.release_before_probe,
                )
            } else {
                (None, false)
            }
        };
        if release {
            remove(path).await?;
        }
        if let Some(kind) = failure {
            return Err(if kind == io::ErrorKind::PermissionDenied {
                denied()
            } else {
                kind.into()
            });
        }
        let identity_failure = self.0.lock().unwrap().identity_failure;
        write_new(owner, mode, identity_failure).await?;
        if lock(path) {
            let abort = self.0.lock().unwrap().abort_after_lock.take();
            if let Some((controller, reason)) = abort {
                controller.abort_with(reason);
            }
        }
        if claim(path) {
            let replacement = self.0.lock().unwrap().after_claim.take();
            if let Some(record) = replacement {
                let name = path.to_string_lossy();
                let (lock_name, _) = name.split_once(".takeover-").unwrap();
                tokio::fs::write(lock_name, record).await?;
            }
        }
        let paused = {
            let mut state = self.0.lock().unwrap();
            if state.pause_write == Some(owner.shared.kind) {
                state.pause_write = None;
                Some(state.paused.take())
            } else {
                None
            }
        };
        if let Some(notify) = paused {
            if let Some(notify) = notify {
                let _ = notify.send(path.clone());
            }
            std::future::pending::<()>().await;
        }
        Ok(())
    }
    fn exited(&self, pid: u32, _: &FileLockOptions) -> io::Result<bool> {
        let mut s = self.0.lock().unwrap();
        if s.probe_all_exited {
            return Ok(true);
        }
        if s.other_user || pid != EXITED_PID {
            return Ok(false);
        }
        match &mut s.probe_responses {
            Some(0) => Ok(false),
            Some(n) => {
                *n -= 1;
                Ok(true)
            }
            None => Ok(true),
        }
    }
}
fn options(ms: u64) -> FileLockOptions {
    FileLockOptions {
        wait_ms: Some(ms),
        ..Default::default()
    }
}
fn write_options() -> WriteFileAtomicOptions {
    WriteFileAtomicOptions {
        mode: 0o600,
        dir_mode: None,
    }
}
fn setup() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("document");
    (d, p)
}
fn record(pid: u32) -> String {
    format!("{pid}\n")
}
async fn held(target: &Path, text: &str) {
    tokio::fs::write(sibling(target, ".lock"), text)
        .await
        .unwrap();
}
async fn assert_timeout(fake: &Fake, target: &Path, text: &str) {
    held(target, text).await;
    let called = std::cell::Cell::new(false);
    let err = with_file_lock_with(
        fake,
        target,
        || async {
            called.set(true);
            Ok(())
        },
        options(50),
    )
    .await
    .unwrap_err();
    let err = HarnessError::of(err.as_ref()).unwrap();
    assert!(
        err.to_string()
            .contains("timed out waiting for the writer lock")
    );
    assert_eq!(err.name(), "FileLockError");
    assert!(err.is_named(FileLockError::NAME));
    assert_eq!(err.code(), "FILE_LOCK_TIMEOUT");
    let detail = err.detail::<FileLockErrorDetails>().unwrap();
    assert_eq!(detail.lock_path, sibling(target, ".lock"));
    assert!(!called.get());
    assert_eq!(
        read_lock_record(&detail.lock_path).await.as_deref(),
        Some(text)
    );
}
fn is_lock_timeout(error: &BoxError) -> bool {
    HarnessError::of(error.as_ref()).is_some_and(|error| {
        error.is_named(FileLockError::NAME) && error.code() == "FILE_LOCK_TIMEOUT"
    })
}
#[tokio::test]
async fn preserves_non_utf8_binary_content() {
    let (_d, p) = setup();
    let bytes = [0, 255, 128, 0xcf, 0xfa, 0xed, 0xfe];
    write_file_atomic(
        &p,
        bytes,
        WriteFileAtomicOptions {
            mode: 0o700,
            dir_mode: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(tokio::fs::read(p).await.unwrap(), bytes);
}
#[tokio::test]
async fn creates_file_and_parents_with_stated_mode() {
    let (d, _) = setup();
    let p = d.path().join("nested/deep/doc.yaml");
    write_file_atomic(
        &p,
        "a: 1\n",
        WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: Some(0o700),
        },
    )
    .await
    .unwrap();
    assert_eq!(tokio::fs::read_to_string(&p).await.unwrap(), "a: 1\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            tokio::fs::metadata(&p).await.unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            tokio::fs::metadata(p.parent().unwrap())
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
#[tokio::test]
async fn replaces_existing_content_and_narrows_permissions() {
    let (_d, p) = setup();
    tokio::fs::write(&p, "old").await.unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644))
            .await
            .unwrap();
    }
    write_file_atomic(&p, "new", write_options()).await.unwrap();
    assert_eq!(tokio::fs::read_to_string(&p).await.unwrap(), "new");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            tokio::fs::metadata(&p).await.unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[tokio::test]
async fn replaces_symlink_itself_without_writing_referent() {
    let (d, p) = setup();
    let victim = d.path().join("victim");
    tokio::fs::write(&victim, "victim-content").await.unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&victim, &p).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&victim, &p).unwrap();
    write_file_atomic(&p, "replaced", write_options())
        .await
        .unwrap();
    assert!(!tokio::fs::symlink_metadata(&p).await.unwrap().is_symlink());
    assert_eq!(tokio::fs::read_to_string(&p).await.unwrap(), "replaced");
    assert_eq!(
        tokio::fs::read_to_string(&victim).await.unwrap(),
        "victim-content"
    );
}
#[tokio::test(start_paused = true)]
async fn retries_transient_windows_rename_and_commits() {
    let (d, p) = setup();
    tokio::fs::write(&p, "old").await.unwrap();
    let fake = Fake::default();
    {
        let mut s = fake.0.lock().unwrap();
        s.windows = true;
        s.rename_failures = [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::ResourceBusy,
            io::ErrorKind::PermissionDenied,
        ]
        .into();
    }
    write_file_atomic_with(&fake, &p, b"new", write_options())
        .await
        .unwrap();
    assert_eq!(fake.0.lock().unwrap().rename_attempts, 4);
    assert_eq!(tokio::fs::read_to_string(&p).await.unwrap(), "new");
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
}
#[tokio::test(start_paused = true)]
async fn leaves_no_temp_after_bounded_windows_retries_expire() {
    let (d, p) = setup();
    tokio::fs::write(&p, "old").await.unwrap();
    let fake = Fake::default();
    {
        let mut s = fake.0.lock().unwrap();
        s.windows = true;
        s.rename_failures = vec![io::ErrorKind::PermissionDenied; 9].into();
    }
    assert_eq!(
        write_file_atomic_with(&fake, &p, b"new", write_options())
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(fake.0.lock().unwrap().rename_attempts, 9);
    assert_eq!(tokio::fs::read_to_string(&p).await.unwrap(), "old");
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
}
#[tokio::test]
async fn does_not_retry_windows_failure_without_transient_code() {
    let (d, p) = setup();
    let fake = Fake::default();
    {
        let mut s = fake.0.lock().unwrap();
        s.windows = true;
        s.rename_failures.push_back(io::ErrorKind::Other);
    }
    assert!(
        write_file_atomic_with(&fake, &p, b"new", write_options())
            .await
            .unwrap_err()
            .to_string()
            .contains("without a code")
    );
    assert_eq!(fake.0.lock().unwrap().rename_attempts, 1);
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
}
#[tokio::test]
async fn does_not_retry_permission_failures_outside_windows() {
    let (_d, p) = setup();
    let fake = Fake::default();
    fake.0
        .lock()
        .unwrap()
        .rename_failures
        .push_back(io::ErrorKind::PermissionDenied);
    assert_eq!(
        write_file_atomic_with(&fake, &p, b"new", write_options())
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(fake.0.lock().unwrap().rename_attempts, 1);
}
#[tokio::test(start_paused = true)]
async fn cancels_waiter_without_releasing_holder() {
    let (_d, p) = setup();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let holder_path = p.clone();
    let holder = tokio::spawn(async move {
        with_file_lock(
            holder_path,
            || async {
                entered_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok(())
            },
            FileLockOptions::default(),
        )
        .await
        .unwrap();
    });
    entered_rx.await.unwrap();
    let controller = AbortController::new();
    let reason = SharedError::new(std::io::Error::other("caller cancelled"));
    let options = FileLockOptions {
        signal: Some(controller.signal()),
        wait_ms: Some(10000),
        ..Default::default()
    };
    let waiter = with_file_lock(
        &p,
        || async {
            panic!("cancelled operation ran");
            #[allow(unreachable_code)]
            Ok(())
        },
        options.clone(),
    );
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(1)).await;
        controller.abort_with(reason.clone());
    };
    let (result, ()) = tokio::join!(waiter, cancel);
    let error = result.unwrap_err();
    assert!(is_abort_error(error.as_ref()));
    assert_eq!(error.to_string(), "The operation was aborted");
    assert!(
        SharedError::of(error.source().unwrap())
            .unwrap()
            .ptr_eq(&reason)
    );
    assert_eq!(
        read_lock_record(&sibling(&p, ".lock")).await.unwrap(),
        record(std::process::id())
    );
    let error = with_file_lock(&p, || async { Ok(()) }, options)
        .await
        .unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&reason));
    release_tx.send(()).unwrap();
    holder.await.unwrap();
    assert_eq!(
        tokio::fs::metadata(sibling(&p, ".lock"))
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}
#[tokio::test(start_paused = true)]
async fn retries_eperm_only_when_lock_exists() {
    let (_d, p) = setup();
    held(&p, "holder\n").await;
    let fake = Fake::default();
    fake.0.lock().unwrap().permission_failures = 1;
    let release = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        remove(&sibling(&p, ".lock")).await.unwrap();
    };
    let (result, ()) = tokio::join!(
        with_file_lock_with(&fake, &p, || async { Ok(true) }, options(2000)),
        release
    );
    assert!(result.unwrap());
}
#[tokio::test(start_paused = true)]
async fn preserves_persistent_eperm_without_lock_on_both_platforms() {
    for windows in [false, true] {
        let (_d, p) = setup();
        let fake = Fake::default();
        {
            let mut s = fake.0.lock().unwrap();
            s.windows = windows;
            s.permission_failures = 2;
        }
        let result = with_file_lock_with(
            &fake,
            &p,
            || async {
                panic!("operation ran");
                #[allow(unreachable_code)]
                Ok(())
            },
            options(2000),
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .downcast_ref::<io::Error>()
                .is_some_and(permission)
        );
    }
}
#[tokio::test(start_paused = true)]
async fn acquires_windows_lock_released_before_probe() {
    let (d, p) = setup();
    held(&p, "holder\n").await;
    let fake = Fake::default();
    {
        let mut s = fake.0.lock().unwrap();
        s.windows = true;
        s.permission_failures = 1;
        s.release_before_probe = true;
    }
    assert_eq!(
        with_file_lock_with(&fake, &p, || async { Ok("acquired") }, options(2000))
            .await
            .unwrap(),
        "acquired"
    );
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
}
#[tokio::test]
async fn rejects_invalid_parent_before_running_operation() {
    let (_d, p) = setup();
    tokio::fs::write(&p, "occupied").await.unwrap();
    assert!(
        with_file_lock(
            p.join("document"),
            || async {
                panic!("operation ran");
                #[allow(unreachable_code)]
                Ok(())
            },
            options(0)
        )
        .await
        .unwrap_err()
        .is::<io::Error>()
    );
}
#[tokio::test]
async fn takes_over_exited_holder() {
    let (d, p) = setup();
    held(&p, &record(EXITED_PID)).await;
    let result = with_file_lock_with(
        &Fake::default(),
        &p,
        || async { Ok(read_lock_record(&sibling(&p, ".lock")).await.unwrap()) },
        options(0),
    )
    .await
    .unwrap();
    assert_eq!(result, record(std::process::id()));
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
}
#[cfg(unix)]
#[tokio::test]
async fn takes_over_lock_of_process_that_really_exited() {
    let (_d, p) = setup();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .arg("--list")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    child.wait().await.unwrap();
    held(&p, &record(pid)).await;
    assert_eq!(
        with_file_lock(&p, || async { Ok("acquired") }, options(0))
            .await
            .unwrap(),
        "acquired"
    );
}
#[tokio::test(start_paused = true)]
async fn admits_one_contender_at_a_time_on_same_exited_holder() {
    let (d, p) = setup();
    held(&p, &record(EXITED_PID)).await;
    let fake = Arc::new(Fake::default());
    let active = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let fake = fake.clone();
        let p = p.clone();
        let active = active.clone();
        handles.push(tokio::spawn(async move {
            with_file_lock_with(
                &*fake,
                &p,
                || async {
                    assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    assert_eq!(active.fetch_sub(1, Ordering::SeqCst), 1);
                    Ok(())
                },
                options(10000),
            )
            .await
            .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
}
#[tokio::test(start_paused = true)]
async fn waits_for_live_self_empty_incomplete_invalid_group_and_out_of_range_records() {
    for text in [
        record(1),
        record(std::process::id()),
        "".into(),
        "12".into(),
        "holder\n".into(),
        record(0),
        "2147483648\n".into(),
        "99999999999999999999\n".into(),
    ] {
        let (_d, p) = setup();
        assert_timeout(&Fake::default(), &p, &text).await;
    }
}
#[tokio::test(start_paused = true)]
async fn waits_for_holder_existing_under_another_user() {
    let (_d, p) = setup();
    let fake = Fake::default();
    fake.0.lock().unwrap().other_user = true;
    assert_timeout(&fake, &p, &record(EXITED_PID)).await;
}
#[tokio::test(start_paused = true)]
async fn keeps_own_lock_when_runtime_cannot_probe_itself() {
    let (_d, p) = setup();
    let fake = Fake::default();
    fake.0.lock().unwrap().probe_all_exited = true;
    assert_timeout(&fake, &p, &record(std::process::id())).await;
}
#[tokio::test(start_paused = true)]
async fn keeps_holder_that_reused_exited_pid_during_claim() {
    let (_d, p) = setup();
    let fake = Fake::default();
    {
        let mut s = fake.0.lock().unwrap();
        s.probe_responses = Some(1);
        s.after_claim = Some(record(EXITED_PID));
    }
    assert_timeout(&fake, &p, &record(EXITED_PID)).await;
}
#[tokio::test(start_paused = true)]
async fn waits_when_exited_lock_cannot_be_removed() {
    let (_d, p) = setup();
    let fake = Fake::default();
    fake.0.lock().unwrap().lock_removal_fails = true;
    assert_timeout(&fake, &p, &record(EXITED_PID)).await;
}
#[tokio::test(start_paused = true)]
async fn waits_for_lock_it_cannot_read() {
    let (_d, p) = setup();
    tokio::fs::create_dir(sibling(&p, ".lock")).await.unwrap();
    assert!(is_lock_timeout(
        &with_file_lock(&p, || async { Ok(()) }, options(50))
            .await
            .unwrap_err()
    ));
}
#[tokio::test(start_paused = true)]
async fn leaves_exited_holder_to_contender_that_claimed_record() {
    let (_d, p) = setup();
    let held_record = record(EXITED_PID);
    let hash = Sha256::digest(held_record.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    tokio::fs::write(
        sibling(&p, &format!(".lock.takeover-{}", &hash[..16])),
        "1\n",
    )
    .await
    .unwrap();
    assert_timeout(&Fake::default(), &p, &held_record).await;
}
#[tokio::test(start_paused = true)]
async fn keeps_lock_another_contender_acquired_during_claim() {
    let (d, p) = setup();
    held(&p, &record(EXITED_PID)).await;
    let fake = Fake::default();
    fake.0.lock().unwrap().after_claim = Some(record(std::process::id()));
    assert!(is_lock_timeout(
        &with_file_lock_with(&fake, &p, || async { Ok(()) }, options(50))
            .await
            .unwrap_err()
    ));
    assert_eq!(
        read_lock_record(&sibling(&p, ".lock")).await.unwrap(),
        record(std::process::id())
    );
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
}
#[tokio::test(start_paused = true)]
async fn retries_claim_refused_with_eperm() {
    let (_d, p) = setup();
    held(&p, &record(EXITED_PID)).await;
    let fake = Fake::default();
    fake.0.lock().unwrap().claim_failure = Some(io::ErrorKind::PermissionDenied);
    let release = async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        fake.0.lock().unwrap().claim_failure = None;
    };
    let (result, ()) = tokio::join!(
        with_file_lock_with(&fake, &p, || async { Ok("acquired") }, options(10000)),
        release
    );
    assert_eq!(result.unwrap(), "acquired");
}
#[tokio::test]
async fn surfaces_claim_failure_other_than_contention() {
    let (_d, p) = setup();
    held(&p, &record(EXITED_PID)).await;
    let fake = Fake::default();
    fake.0.lock().unwrap().claim_failure = Some(io::ErrorKind::Other);
    assert!(
        with_file_lock_with(
            &fake,
            &p,
            || async {
                panic!("operation ran");
                #[allow(unreachable_code)]
                Ok(())
            },
            options(2000)
        )
        .await
        .unwrap_err()
        .downcast_ref::<io::Error>()
        .is_some_and(|e| e.kind() == io::ErrorKind::Other)
    );
}
#[tokio::test]
async fn runs_operation_when_claim_cannot_be_removed_afterwards() {
    let (_d, p) = setup();
    held(&p, &record(EXITED_PID)).await;
    let fake = Fake::default();
    fake.0.lock().unwrap().claim_removal_fails = true;
    assert_eq!(
        with_file_lock_with(&fake, &p, || async { Ok("acquired") }, options(0))
            .await
            .unwrap(),
        "acquired"
    );
}
#[tokio::test(start_paused = true)]
async fn waits_for_caller_stated_limit_instead_of_default() {
    let (_d, p) = setup();
    held(&p, &record(std::process::id())).await;
    let start = Instant::now();
    assert!(is_lock_timeout(
        &with_file_lock(&p, || async { Ok("impatient") }, options(50))
            .await
            .unwrap_err()
    ));
    assert!(start.elapsed() < Duration::from_millis(1000));
    let release = async {
        remove(&sibling(&p, ".lock")).await.unwrap();
    };
    let (result, ()) = tokio::join!(
        with_file_lock(&p, || async { Ok("patient") }, options(10000)),
        release
    );
    assert_eq!(result.unwrap(), "patient");
}

#[tokio::test]
async fn preserves_operation_error_and_releases_lock_on_failure() {
    let (_d, p) = setup();
    let result: Result<(), BoxError> = with_file_lock(
        &p,
        || async { Err(HarnessError::new("operation failed", "OPERATION_FAILED").into()) },
        FileLockOptions::default(),
    )
    .await;
    let error = result.unwrap_err();
    let original = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(original.code(), "OPERATION_FAILED");
    assert_eq!(original.message(), "operation failed");
    assert_eq!(
        tokio::fs::metadata(sibling(&p, ".lock"))
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}

#[tokio::test]
async fn preaborted_acquisition_throws_the_reason_itself() {
    let (dir, p) = setup();
    let controller = AbortController::new();
    let reason = SharedError::new(HarnessError::new("cancelled", "CANCELLED"));
    controller.abort_with(reason.clone());
    let result = with_file_lock(
        &p,
        || async {
            panic!("cancelled operation ran");
            #[allow(unreachable_code)]
            Ok(())
        },
        FileLockOptions {
            signal: Some(controller.signal()),
            ..Default::default()
        },
    )
    .await;
    assert!(
        SharedError::of(result.unwrap_err().as_ref())
            .unwrap()
            .ptr_eq(&reason)
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
#[tokio::test]
async fn abort_after_acquisition_throws_reason_and_releases_the_lock() {
    let (dir, p) = setup();
    let controller = Arc::new(AbortController::new());
    let reason = SharedError::new(std::io::Error::other("cancelled"));
    let fake = Fake::default();
    fake.0.lock().unwrap().abort_after_lock = Some((controller.clone(), reason.clone()));
    let result = with_file_lock_with(
        &fake,
        &p,
        || async {
            panic!("cancelled operation ran");
            #[allow(unreachable_code)]
            Ok(())
        },
        FileLockOptions {
            signal: Some(controller.signal()),
            ..Default::default()
        },
    )
    .await;
    assert!(
        SharedError::of(result.unwrap_err().as_ref())
            .unwrap()
            .ptr_eq(&reason)
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
#[tokio::test(start_paused = true)]
async fn dropping_a_timed_out_operation_releases_lock_for_the_next_writer() {
    let (_dir, p) = setup();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let mut first = Box::pin(with_file_lock(
        &p,
        || async {
            entered_tx.send(()).unwrap();
            std::future::pending::<Result<(), BoxError>>().await
        },
        FileLockOptions::default(),
    ));
    tokio::select! {
        result=&mut first => panic!("holder finished: {result:?}"),
        result=entered_rx => result.unwrap(),
    }
    assert_eq!(
        read_lock_record(&sibling(&p, ".lock")).await.unwrap(),
        record(std::process::id())
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(1), first)
            .await
            .is_err()
    );
    assert_eq!(
        with_file_lock(&p, || async { Ok("next") }, options(0))
            .await
            .unwrap(),
        "next"
    );
    assert!(!sibling(&p, ".lock").exists());
}
#[test]
fn dropped_guard_does_not_remove_a_changed_holder_record() {
    let (_dir, p) = setup();
    let path = sibling(&p, ".lock");
    let guard = FileGuard::new(&path, record(std::process::id()).as_bytes(), FileKind::Lock);
    create_and_write(guard.shared.clone(), 0o600, false).unwrap();
    std::fs::write(&path, "successor\n").unwrap();
    drop(guard);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "successor\n");
}
#[cfg(unix)]
#[test]
fn dropped_guard_does_not_remove_successor_inode_with_same_pid() {
    let (_dir, p) = setup();
    let path = sibling(&p, ".lock");
    let guard = FileGuard::new(&path, record(std::process::id()).as_bytes(), FileKind::Lock);
    create_and_write(guard.shared.clone(), 0o600, false).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, record(std::process::id())).unwrap();
    drop(guard);
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        record(std::process::id())
    );
}
#[cfg(unix)]
#[test]
fn native_parent_process_is_not_proven_gone() {
    let Some(parent) = rustix::process::getppid() else {
        eprintln!("skipped parent PID probe: no visible parent (PID 1 or another PID namespace)");
        return;
    };
    assert!(!default_process_exited(parent.as_raw_nonzero().get() as u32).unwrap());
}
#[cfg(unix)]
#[test]
fn native_pid_one_is_not_proven_gone_for_non_root() {
    if rustix::process::getuid().is_root() {
        eprintln!("skipped PID 1 permission probe: test runs as root");
        return;
    }
    assert!(!default_process_exited(1).unwrap());
}
#[cfg(unix)]
#[test]
fn transient_rename_mapping_accepts_exactly_the_named_unix_errno_codes() {
    for errno in [
        rustix::io::Errno::ACCESS,
        rustix::io::Errno::BUSY,
        rustix::io::Errno::PERM,
    ] {
        assert!(transient(&io::Error::from_raw_os_error(
            errno.raw_os_error()
        )));
    }
    for errno in [
        rustix::io::Errno::IO,
        rustix::io::Errno::NOENT,
        rustix::io::Errno::AGAIN,
        rustix::io::Errno::EXIST,
        rustix::io::Errno::NOTEMPTY,
    ] {
        assert!(!transient(&io::Error::from_raw_os_error(
            errno.raw_os_error()
        )));
    }
    assert!(!transient(&io::Error::from(
        io::ErrorKind::PermissionDenied
    )));
    assert!(!transient(&io::Error::from(io::ErrorKind::ResourceBusy)));
}
#[cfg(windows)]
#[test]
fn transient_rename_mapping_uses_only_documented_windows_codes() {
    for code in [5, 32, 33, 170, 1314] {
        assert!(transient(&io::Error::from_raw_os_error(code)));
    }
    assert!(!transient(&io::Error::from_raw_os_error(2)));
    assert!(!transient(&io::Error::from(
        io::ErrorKind::PermissionDenied
    )));
}

fn pause_write(fake: &Fake, kind: FileKind) -> tokio::sync::oneshot::Receiver<PathBuf> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut state = fake.0.lock().unwrap();
    state.pause_write = Some(kind);
    state.paused = Some(tx);
    rx
}
fn pause_remove(fake: &Fake, kind: FileKind) -> tokio::sync::oneshot::Receiver<PathBuf> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut state = fake.0.lock().unwrap();
    state.pause_remove = Some(kind);
    state.paused = Some(tx);
    rx
}
async fn drop_at_pause<F: Future>(
    future: F,
    paused: tokio::sync::oneshot::Receiver<PathBuf>,
) -> PathBuf
where
    F::Output: std::fmt::Debug,
{
    let mut future = Box::pin(future);
    let path = tokio::select! {
        result = &mut future => panic!("operation completed before injected pause: {result:?}"),
        path = paused => path.unwrap(),
    };
    drop(future);
    path
}
fn takeover_claim(target: &Path, held: &str) -> PathBuf {
    let hash = Sha256::digest(held.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    sibling(target, &format!(".lock.takeover-{}", &hash[..16]))
}

async fn dropped_pending_lock_write(identity_failure: bool) {
    let (dir, target) = setup();
    let fake = Fake::default();
    fake.0.lock().unwrap().identity_failure = identity_failure;
    let paused = pause_write(&fake, FileKind::Lock);
    let path = drop_at_pause(
        with_file_lock_with(
            &fake,
            &target,
            || async {
                panic!("operation ran before write returned");
                #[allow(unreachable_code)]
                Ok(())
            },
            options(0),
        ),
        paused,
    )
    .await;
    assert_eq!(path, sibling(&target, ".lock"));
    assert!(!path.exists());
    assert_eq!(
        with_file_lock_with(&fake, &target, || async { Ok("next") }, options(0))
            .await
            .unwrap(),
        "next"
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
#[tokio::test]
async fn drop_during_lock_write_releases_lock_and_next_caller_acquires() {
    dropped_pending_lock_write(false).await;
}
#[tokio::test]
async fn failed_identity_read_falls_back_to_holder_record_on_drop() {
    dropped_pending_lock_write(true).await;
}

#[tokio::test]
async fn drop_during_claim_write_releases_claim_and_allows_takeover() {
    let (dir, target) = setup();
    let fake = Fake::default();
    let held_record = record(EXITED_PID);
    held(&target, &held_record).await;
    let paused = pause_write(&fake, FileKind::Claim);
    let path = drop_at_pause(
        with_file_lock_with(&fake, &target, || async { Ok(()) }, options(0)),
        paused,
    )
    .await;
    assert_eq!(path, takeover_claim(&target, &held_record));
    assert!(!path.exists());
    assert_eq!(
        read_lock_record(&sibling(&target, ".lock")).await.unwrap(),
        held_record
    );
    assert_eq!(
        with_file_lock_with(&fake, &target, || async { Ok("next") }, options(0))
            .await
            .unwrap(),
        "next"
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
#[tokio::test]
async fn drop_during_takeover_remove_releases_claim_and_allows_takeover() {
    let (dir, target) = setup();
    let fake = Fake::default();
    let held_record = record(EXITED_PID);
    held(&target, &held_record).await;
    let paused = pause_remove(&fake, FileKind::Lock);
    drop_at_pause(
        with_file_lock_with(&fake, &target, || async { Ok(()) }, options(0)),
        paused,
    )
    .await;
    assert!(!takeover_claim(&target, &held_record).exists());
    assert_eq!(
        read_lock_record(&sibling(&target, ".lock")).await.unwrap(),
        held_record
    );
    assert_eq!(
        with_file_lock_with(&fake, &target, || async { Ok("next") }, options(0))
            .await
            .unwrap(),
        "next"
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
#[tokio::test]
async fn drop_during_temp_write_removes_temp_and_preserves_target() {
    let (dir, target) = setup();
    let fake = Fake::default();
    std::fs::write(&target, "old").unwrap();
    let paused = pause_write(&fake, FileKind::Temp);
    let path = drop_at_pause(
        write_file_atomic_with(&fake, &target, b"new", write_options()),
        paused,
    )
    .await;
    assert!(!path.exists());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "old");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
#[tokio::test]
async fn drop_during_temp_cleanup_removes_temp() {
    let (dir, target) = setup();
    let fake = Fake::default();
    fake.0
        .lock()
        .unwrap()
        .rename_failures
        .push_back(io::ErrorKind::Other);
    let paused = pause_remove(&fake, FileKind::Temp);
    let path = drop_at_pause(
        write_file_atomic_with(&fake, &target, b"new", write_options()),
        paused,
    )
    .await;
    assert!(!path.exists());
    assert!(!target.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn a_blocking_creator_finishing_after_cancellation_removes_its_file() {
    let (_dir, target) = setup();
    let path = sibling(&target, ".lock");
    let guard = FileGuard::new(&path, record(std::process::id()).as_bytes(), FileKind::Lock);
    let owner = guard.shared.clone();
    let (release, resume) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        resume.recv().unwrap();
        create_and_write(owner, 0o600, false)
    });
    drop(guard);
    release.send(()).unwrap();
    assert_eq!(
        worker.join().unwrap().unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
    assert!(!path.exists());
}
#[test]
fn drop_after_exclusive_create_removes_an_empty_record_even_without_identity() {
    for fail_identity in [false, true] {
        let (_dir, target) = setup();
        let path = sibling(&target, ".lock");
        let guard = FileGuard::new(&path, record(std::process::id()).as_bytes(), FileKind::Lock);
        let file = Arc::new(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap(),
        );
        let metadata = if fail_identity {
            None
        } else {
            file.metadata().ok()
        };
        assert!(guard.shared.created(file.clone(), metadata));
        drop(guard);
        assert!(!path.exists());
        drop(file);
    }
}
#[tokio::test(start_paused = true)]
async fn abort_wins_when_wait_timer_and_signal_are_both_ready() {
    // Without biased selection, passing all 128 independent ties has probability 2^-128.
    for _ in 0..128 {
        let controller = AbortController::new();
        let reason = SharedError::new(std::io::Error::other("cancelled"));
        let options = FileLockOptions {
            signal: Some(controller.signal()),
            ..Default::default()
        };
        let mut waiting = Box::pin(wait(10, &options));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(
                waiting.as_mut().poll(cx).is_pending()
            ))
            .await
        );
        tokio::time::advance(Duration::from_millis(10)).await;
        controller.abort_with(reason.clone());
        let error = waiting.await.unwrap_err();
        assert!(is_abort_error(error.as_ref()));
        assert!(
            SharedError::of(error.source().unwrap())
                .unwrap()
                .ptr_eq(&reason)
        );
    }
}
