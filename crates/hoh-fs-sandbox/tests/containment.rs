use std::fs;

use hoh_error::HarnessError;
use hoh_fs_sandbox::containment::is_path_under;
use tempfile::{Builder, TempDir};

fn tempdir() -> TempDir {
    Builder::new()
        .prefix("hoh-fssbx-containment-")
        .tempdir_in(std::env::current_dir().expect("current directory"))
        .expect("temporary directory")
}

#[tokio::test]
async fn accepts_equal_paths_descendants_and_a_filesystem_root_boundary() {
    let base = tempdir();
    let base = base.path().canonicalize().expect("canonical base");
    assert!(
        is_path_under(base.to_str().unwrap(), base.to_str().unwrap(), None)
            .await
            .unwrap()
    );
    assert!(
        is_path_under(
            base.join("child").to_str().unwrap(),
            base.to_str().unwrap(),
            None,
        )
        .await
        .unwrap()
    );
    assert!(
        is_path_under(base.to_str().unwrap(), "/", None)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn uses_case_insensitive_lexical_comparison_for_windows_style_containment() {
    let base = tempdir();
    let base = base.path().canonicalize().expect("canonical base");
    let text = base.to_str().unwrap();
    assert!(
        is_path_under(
            &format!("{}/child", text.to_uppercase()),
            &text.to_lowercase(),
            Some(false),
        )
        .await
        .unwrap()
    );
    assert!(
        is_path_under(
            base.join("case-sensitive-child").to_str().unwrap(),
            text,
            Some(true),
        )
        .await
        .unwrap()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn recognizes_an_alias_equivalent_root_by_filesystem_identity_for_a_missing_target() {
    use std::os::unix::fs::symlink;

    let base = tempdir();
    let real_root = base.path().join("real");
    let alias_root = base.path().join("alias");
    fs::create_dir(&real_root).unwrap();
    symlink(&real_root, &alias_root).unwrap();
    assert!(
        is_path_under(
            real_root.join("missing/file.txt").to_str().unwrap(),
            alias_root.to_str().unwrap(),
            None,
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn denies_unrelated_and_missing_roots() {
    let base = tempdir();
    let allowed = base.path().join("allowed");
    let outside = base.path().join("outside");
    fs::create_dir(&allowed).unwrap();
    fs::create_dir(&outside).unwrap();
    let target = outside.join("file.txt");
    assert!(
        !is_path_under(target.to_str().unwrap(), allowed.to_str().unwrap(), None)
            .await
            .unwrap()
    );
    assert!(
        !is_path_under(
            target.to_str().unwrap(),
            base.path().join("missing-root").to_str().unwrap(),
            None,
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn treats_a_regular_file_path_segment_as_a_missing_target_not_containment() {
    let base = tempdir();
    let allowed = base.path().join("allowed");
    let blocker = base.path().join("blocker");
    fs::create_dir(&allowed).unwrap();
    fs::write(&blocker, "not a directory").unwrap();
    assert!(
        !is_path_under(
            blocker.join("child.txt").to_str().unwrap(),
            allowed.to_str().unwrap(),
            None,
        )
        .await
        .unwrap()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn classifies_an_ancestor_inspection_fault_as_a_filesystem_error() {
    use std::os::unix::fs::symlink;

    let base = tempdir();
    let root = base.path().join("root");
    let loop_path = base.path().join("loop");
    fs::create_dir(&root).unwrap();
    symlink(&loop_path, &loop_path).unwrap();
    let error = is_path_under(loop_path.to_str().unwrap(), root.to_str().unwrap(), None)
        .await
        .expect_err("symlink loop must fail");
    let error = HarnessError::of(error.as_ref()).expect("structured FsError");
    assert_eq!(error.name(), "FsError");
    assert_eq!(error.code(), "FS_IO_ERROR");
}
