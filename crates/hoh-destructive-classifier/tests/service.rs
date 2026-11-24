//! The service's own entry points: commands, deletions and overwrites against a real folder.

use std::sync::Arc;

use hoh_destructive_classifier::{DestructiveClassifierService, RealFs, Risk};

fn folder() -> tempfile::TempDir {
    std::fs::create_dir_all("/dev/shm/hoh").expect("scratch root");
    let dir = tempfile::Builder::new()
        .prefix("o1-classifier-")
        .tempdir_in("/dev/shm/hoh")
        .expect("temp dir");
    std::fs::write(dir.path().join("old.txt"), "keep").expect("file");
    dir
}

fn service() -> DestructiveClassifierService {
    DestructiveClassifierService::new(Arc::new(RealFs))
}

#[tokio::test]
async fn a_command_is_judged_like_the_window_guard() {
    let dir = folder();
    let cwd = Some(dir.path().to_path_buf());
    assert!(service().command("ls -la", cwd.clone()).await.is_none());
    let asked = service()
        .command("rm old.txt", cwd.clone())
        .await
        .expect("a removal is asked");
    assert!(asked.details.contains("rm old.txt"), "{}", asked.details);
    // The whole folder path, not its last two names (QA P2: «Documents/HoH»).
    let folder = format!("(folder {})", dir.path().display());
    assert!(asked.headline.contains(&folder), "{}", asked.headline);
    let risks = service().command_risks("rm old.txt", cwd).await;
    assert!(
        matches!(risks.as_slice(), [Risk::Delete { .. }]),
        "{risks:?}"
    );
}

#[test]
fn a_deletion_is_always_asked_and_lists_the_files() {
    let dir = folder();
    assert!(service().deletion(&[], Some(dir.path())).is_none());
    let paths = ["old.txt".to_owned(), "missing.txt".to_owned()];
    let asked = service().deletion(&paths, Some(dir.path())).expect("asked");
    assert_eq!(asked.details, "Files:\nold.txt\nmissing.txt");
    assert!(!asked.headline.is_empty());
}

#[test]
fn an_overwrite_is_asked_only_for_existing_files() {
    let dir = folder();
    let new = ["new.txt".to_owned()];
    assert!(service().overwrite(&new, Some(dir.path())).is_none());
    let both = ["new.txt".to_owned(), "old.txt".to_owned()];
    let asked = service().overwrite(&both, Some(dir.path())).expect("asked");
    assert_eq!(asked.details, "Files:\nold.txt");
    let absolute = [dir.path().join("old.txt").to_string_lossy().into_owned()];
    assert!(service().overwrite(&absolute, None).is_some());
}
