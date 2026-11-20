use hoh_office_kit::*;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

fn store(
    root: &Path,
    retention: Duration,
    max_bytes: u64,
) -> (OutputStore, Arc<Mutex<Vec<String>>>) {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let observed = warnings.clone();
    let store = OutputStore::new(
        OutputStoreOptions {
            root: root.join("outputs"),
            retention,
            max_bytes,
        },
        move |message| observed.lock().unwrap().push(message),
    );
    (store, warnings)
}

async fn exists(path: &Path) -> bool {
    tokio::fs::symlink_metadata(path).await.is_ok()
}

async fn wait_until_missing(path: &Path) {
    for _ in 0..100 {
        if !exists(path).await {
            return;
        }
        tokio::task::yield_now().await;
    }
    assert!(!exists(path).await, "timed deletion did not settle");
}

#[tokio::test]
async fn saves_owner_only_files_under_hashed_owner_and_uuid_directories() {
    let root = tempfile::tempdir().unwrap();
    let (store, _) = store(root.path(), Duration::from_secs(60), 8);
    let path = store
        .save("session/a", "../report.pdf", &[1, 2])
        .await
        .unwrap();
    assert!(path.ends_with("report.pdf"));
    assert_eq!(
        path.ancestors().nth(3),
        Some(root.path().join("outputs").as_path())
    );
    assert_eq!(tokio::fs::read(&path).await.unwrap(), [1, 2]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            tokio::fs::metadata(&path)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        for level in 1..=3 {
            let mode = std::fs::metadata(path.ancestors().nth(level).unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "level {level}");
        }
        assert_eq!(
            tokio::fs::metadata(path.parent().unwrap())
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    store.dispose().await;
}

#[tokio::test]
async fn evicts_oldest_files_and_refuses_a_file_above_the_budget() {
    let root = tempfile::tempdir().unwrap();
    let (store, _) = store(root.path(), Duration::from_secs(60), 8);
    let first = store.save("a", "one.pdf", &[0; 4]).await.unwrap();
    let second = store.save("a", "two.pdf", &[0; 4]).await.unwrap();
    let third = store.save("b", "three.pdf", &[0; 4]).await.unwrap();
    assert!(!exists(&first).await);
    assert!(exists(&second).await);
    assert!(exists(&third).await);
    let error = store.save("a", "big.pdf", &[0; 9]).await.unwrap_err();
    assert_eq!(
        error.downcast_ref::<OfficeKitError>().unwrap().code,
        OfficeKitErrorCode::OutputTooLarge
    );
    assert!(
        store
            .save("a", "exact.pdf", &[0; 8])
            .await
            .unwrap()
            .ends_with("exact.pdf")
    );
}

#[tokio::test(start_paused = true)]
async fn deletes_a_file_when_retention_expires() {
    let root = tempfile::tempdir().unwrap();
    let (store, _) = store(root.path(), Duration::from_secs(1), 8);
    let path = store.save("a", "one.pdf", &[1]).await.unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    wait_until_missing(&path).await;
}

#[tokio::test]
async fn releases_one_owner_disposes_the_rest_and_refuses_late_saves() {
    let root = tempfile::tempdir().unwrap();
    let (store, _) = store(root.path(), Duration::from_secs(60), 8);
    let a = store.save("a", "a.pdf", &[1]).await.unwrap();
    let b = store.save("b", "b.pdf", &[1]).await.unwrap();
    store.release("a").await;
    store.release("a").await;
    assert!(!exists(&a).await);
    assert!(exists(&b).await);
    store.release("missing").await;
    store.dispose().await;
    assert!(!exists(&b).await);
    let error = store.save("a", "late.pdf", &[1]).await.unwrap_err();
    assert_eq!(
        error.downcast_ref::<OfficeKitError>().unwrap().code,
        OfficeKitErrorCode::Unavailable
    );
}

#[tokio::test]
async fn removes_the_private_directory_of_a_failed_write_and_continues() {
    let root = tempfile::tempdir().unwrap();
    let (store, _) = store(root.path(), Duration::from_secs(60), 8);
    assert!(store.save("a", "", &[1]).await.is_err());
    let outputs = root.path().join("outputs");
    let mut owners = tokio::fs::read_dir(&outputs).await.unwrap();
    let owner = owners.next_entry().await.unwrap().unwrap().path();
    let mut entries = tokio::fs::read_dir(owner).await.unwrap();
    assert!(entries.next_entry().await.unwrap().is_none());
    assert!(
        store
            .save("a", "ok.pdf", &[1])
            .await
            .unwrap()
            .ends_with("ok.pdf")
    );
}

fn generated_owner(root: &Path) -> std::path::PathBuf {
    root.join("outputs/session-000000000000000000000000")
}

fn generated_output(owner: &Path) -> std::path::PathBuf {
    owner.join("00000000-0000-4000-8000-000000000000")
}

fn expire(path: &Path) {
    let past = SystemTime::now() - Duration::from_secs(120);
    let file = std::fs::File::open(path).unwrap();
    file.set_times(
        std::fs::FileTimes::new()
            .set_accessed(past)
            .set_modified(past),
    )
    .unwrap();
}

#[tokio::test]
async fn sweeps_only_expired_generated_real_directories() {
    let root = tempfile::tempdir().unwrap();
    let owner = generated_owner(root.path());
    let expired = generated_output(&owner);
    let fresh = owner.join("00000000-0000-4000-8000-000000000001");
    tokio::fs::create_dir_all(&expired).await.unwrap();
    tokio::fs::write(expired.join("file.pdf"), b"old")
        .await
        .unwrap();
    tokio::fs::create_dir_all(&fresh).await.unwrap();
    expire(&expired);
    let unrelated = root.path().join("outputs/session-000/not-an-output");
    tokio::fs::create_dir_all(&unrelated).await.unwrap();
    let (store, _) = store(root.path(), Duration::from_secs(60), 8);
    store.save("a", "a.pdf", &[1]).await.unwrap();
    assert!(!exists(&expired).await);
    assert!(exists(&fresh).await);
    assert!(exists(&unrelated).await);
}

#[cfg(unix)]
#[tokio::test]
async fn startup_sweep_does_not_follow_owner_or_output_links() {
    let root = tempfile::tempdir().unwrap();
    let external_owner = root.path().join("external-owner");
    let external_output = root.path().join("external-output");
    tokio::fs::create_dir_all(&external_owner).await.unwrap();
    tokio::fs::create_dir_all(&external_output).await.unwrap();
    tokio::fs::write(external_owner.join("kept"), b"owner")
        .await
        .unwrap();
    tokio::fs::write(external_output.join("kept"), b"output")
        .await
        .unwrap();
    let owner_link = generated_owner(root.path());
    tokio::fs::create_dir_all(owner_link.parent().unwrap())
        .await
        .unwrap();
    std::os::unix::fs::symlink(&external_owner, &owner_link).unwrap();
    let real_owner = root.path().join("outputs/session-111111111111111111111111");
    tokio::fs::create_dir_all(&real_owner).await.unwrap();
    let output_link = generated_output(&real_owner);
    std::os::unix::fs::symlink(&external_output, &output_link).unwrap();
    let (store, _) = store(root.path(), Duration::from_secs(60), 8);
    store.save("a", "a.pdf", &[1]).await.unwrap();
    assert_eq!(
        tokio::fs::read(external_owner.join("kept")).await.unwrap(),
        b"owner"
    );
    assert_eq!(
        tokio::fs::read(external_output.join("kept")).await.unwrap(),
        b"output"
    );
}

#[tokio::test]
async fn output_names_are_sanitized_to_the_final_component() {
    let root = tempfile::tempdir().unwrap();
    let (store, _) = store(root.path(), Duration::from_secs(60), 8);
    let path = store
        .save("owner", "../../destination.csv", &[1])
        .await
        .unwrap();
    assert_eq!(path.file_name().unwrap(), "destination.csv");
    assert!(path.starts_with(root.path().join("outputs")));
}

#[tokio::test]
async fn owner_ids_do_not_appear_in_paths() {
    let root = tempfile::tempdir().unwrap();
    let (store, _) = store(root.path(), Duration::from_secs(60), 8);
    let path = store.save("secret/session", "a.pdf", &[1]).await.unwrap();
    assert!(!path.to_string_lossy().contains("secret"));
    assert!(!path.to_string_lossy().contains("session/"));
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn running_as_root() -> bool {
    std::fs::metadata("/proc/self").is_ok_and(|meta| {
        use std::os::unix::fs::MetadataExt;
        meta.uid() == 0
    })
}

#[cfg(unix)]
#[tokio::test]
async fn an_undeletable_earlier_output_fails_the_save_instead_of_looping() {
    if running_as_root() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let (store, _) = store(root.path(), Duration::from_secs(60), 8);
    let first = store.save("a", "one.pdf", &[0; 4]).await.unwrap();
    let owner = first.ancestors().nth(2).unwrap().to_owned();
    set_mode(&owner, 0o500);
    let outcome =
        tokio::time::timeout(Duration::from_secs(10), store.save("b", "two.pdf", &[0; 8])).await;
    set_mode(&owner, 0o700);
    let error = outcome
        .expect("save must not loop on an undeletable entry")
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<OfficeKitError>().unwrap().code,
        OfficeKitErrorCode::Unavailable
    );
    // Once deletion works again the same save evicts the old entry.
    store.save("b", "two.pdf", &[0; 8]).await.unwrap();
    store.dispose().await;
}

#[cfg(unix)]
#[tokio::test]
async fn an_expired_output_that_cannot_be_deleted_is_reported_by_the_timer() {
    if running_as_root() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let (store, warnings) = store(root.path(), Duration::from_millis(100), 8);
    let path = store.save("a", "one.pdf", &[1]).await.unwrap();
    let owner = path.ancestors().nth(2).unwrap().to_owned();
    set_mode(&owner, 0o500);
    for _ in 0..100 {
        if !warnings.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    set_mode(&owner, 0o700);
    assert!(
        warnings
            .lock()
            .unwrap()
            .iter()
            .any(|message| message.contains("could not delete output")),
        "the retention timer must report a failed deletion"
    );
    store.dispose().await;
}

#[cfg(unix)]
#[tokio::test]
async fn every_created_directory_level_is_owner_only_even_under_a_wide_umask() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let (store, _) = store(root.path(), Duration::from_secs(60), 8);
    let path = store.save("a", "one.pdf", &[1]).await.unwrap();
    for level in 1..=3 {
        let directory = path.ancestors().nth(level).unwrap();
        let mode = std::fs::metadata(directory).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o077, 0, "{} is {mode:o}", directory.display());
    }
    store.dispose().await;
}

#[tokio::test]
async fn expired_leftovers_are_swept_when_the_store_is_created() {
    let root = tempfile::tempdir().unwrap();
    let owner = generated_owner(root.path());
    let expired = generated_output(&owner);
    tokio::fs::create_dir_all(&expired).await.unwrap();
    expire(&expired);
    let (store, _) = store(root.path(), Duration::from_secs(60), 8);
    wait_until_missing_real(&expired).await;
    store.dispose().await;
}

async fn wait_until_missing_real(path: &Path) {
    for _ in 0..100 {
        if !exists(path).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("startup sweep did not remove {}", path.display());
}
