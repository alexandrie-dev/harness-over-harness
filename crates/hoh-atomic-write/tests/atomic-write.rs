use super::*;
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
            return Err(io::Error::new(
                kind,
                "injected rename failure without a code",
            ));
        }
        tokio::fs::rename(from, to).await
    }
    async fn remove(&self, path: &Path) -> io::Result<()> {
        let fail = {
            let s = self.0.lock().unwrap();
            claim(path) && s.claim_removal_fails || lock(path) && s.lock_removal_fails
        };
        if fail {
            return Err(io::ErrorKind::ResourceBusy.into());
        }
        remove(path).await
    }
    async fn write_new(&self, path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
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
        write_new(path, data, mode).await?;
        if claim(path) {
            let replacement = self.0.lock().unwrap().after_claim.take();
            if let Some(record) = replacement {
                let name = path.to_string_lossy();
                let (lock_name, _) = name.split_once(".takeover-").unwrap();
                tokio::fs::write(lock_name, record).await?;
            }
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
    let AtomicWriteError::Lock(err) = err else {
        panic!("expected FileLockError")
    };
    assert!(
        err.to_string()
            .contains("timed out waiting for the writer lock")
    );
    assert_eq!(err.name, "FileLockError");
    assert_eq!(err.code, "FILE_LOCK_TIMEOUT");
    assert_eq!(err.lock_path, sibling(target, ".lock"));
    assert!(!called.get());
    assert_eq!(
        read_lock_record(&err.lock_path).await.as_deref(),
        Some(text)
    );
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
    let (tx, rx) = watch::channel(false);
    let options = FileLockOptions {
        signal: Some(rx),
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
        tx.send(true).unwrap();
    };
    let (result, ()) = tokio::join!(waiter, cancel);
    assert!(matches!(result, Err(AtomicWriteError::Aborted)));
    assert_eq!(
        read_lock_record(&sibling(&p, ".lock")).await.unwrap(),
        record(std::process::id())
    );
    assert!(matches!(
        with_file_lock(&p, || async { Ok(()) }, options).await,
        Err(AtomicWriteError::Aborted)
    ));
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
        assert!(matches!(result,Err(AtomicWriteError::Io(e)) if permission(&e)));
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
    assert!(matches!(
        with_file_lock(
            p.join("document"),
            || async {
                panic!("operation ran");
                #[allow(unreachable_code)]
                Ok(())
            },
            options(0)
        )
        .await,
        Err(AtomicWriteError::Io(_))
    ));
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
    assert!(matches!(
        with_file_lock(&p, || async { Ok(()) }, options(50)).await,
        Err(AtomicWriteError::Lock(_))
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
    assert!(matches!(
        with_file_lock_with(&fake, &p, || async { Ok(()) }, options(50)).await,
        Err(AtomicWriteError::Lock(_))
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
        matches!(with_file_lock_with(&fake,&p,||async {panic!("operation ran");#[allow(unreachable_code)] Ok(())},options(2000)).await,Err(AtomicWriteError::Io(e)) if e.kind()==io::ErrorKind::Other)
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
    assert!(matches!(
        with_file_lock(&p, || async { Ok("impatient") }, options(50)).await,
        Err(AtomicWriteError::Lock(_))
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
    let result: Result<(), AtomicWriteError> = with_file_lock(
        &p,
        || async {
            Err(AtomicWriteError::Operation(Box::new(HarnessError::new(
                "operation failed",
                "OPERATION_FAILED",
            ))))
        },
        FileLockOptions::default(),
    )
    .await;
    let AtomicWriteError::Operation(error) = result.unwrap_err() else {
        panic!("operation error was replaced")
    };
    let original = error.downcast_ref::<HarnessError>().unwrap();
    assert_eq!(original.code, "OPERATION_FAILED");
    assert_eq!(original.message, "operation failed");
    assert_eq!(
        tokio::fs::metadata(sibling(&p, ".lock"))
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}
