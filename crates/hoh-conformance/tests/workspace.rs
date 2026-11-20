//! `workspace.spec.ts` (4 cases) against `workspace::capture`.
use hoh_conformance::workspace::capture;
use serde_json::json;
use std::fs;

fn root() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("dsh-workspace-snapshot-")
        .tempdir_in(std::env::var("TMPDIR").unwrap_or_else(|_| "/dev/shm".into()))
        .unwrap()
}

#[test]
fn captures_readable_text_binary_bytes_links_and_empty_directories_in_path_order() {
    let dir = root();
    let p = dir.path();
    fs::write(p.join("a.txt"), "hello\n").unwrap();
    fs::write(p.join("b.bin"), [0xff, 0x01]).unwrap();
    fs::create_dir(p.join("empty")).unwrap();
    std::os::unix::fs::symlink("a.txt", p.join("link")).unwrap();
    assert_eq!(
        capture(p, &[], false).unwrap(),
        vec![
            json!({"path":"a.txt","kind":"text","content":"hello\n"}),
            json!({"path":"b.bin","kind":"binary","base64":"/wE="}),
            json!({"path":"empty","kind":"empty-directory"}),
            json!({"path":"link","kind":"symlink","target":"a.txt"}),
        ]
    );
}

#[test]
fn keeps_generic_marker_files_but_omits_declared_runtime_roots_and_the_expected_empty_marker() {
    let dir = root();
    let p = dir.path();
    fs::create_dir(p.join(".dsh")).unwrap();
    fs::write(p.join(".dsh/runtime.json"), "{}").unwrap();
    fs::write(p.join(".empty"), "").unwrap();
    fs::write(p.join("visible.txt"), "visible").unwrap();
    assert_eq!(
        capture(p, &[".dsh"], false).unwrap(),
        vec![
            json!({"path":".empty","kind":"text","content":""}),
            json!({"path":"visible.txt","kind":"text","content":"visible"}),
        ]
    );
    assert_eq!(
        capture(p, &[], true).unwrap(),
        vec![
            json!({"path":".dsh/runtime.json","kind":"text","content":"{}"}),
            json!({"path":"visible.txt","kind":"text","content":"visible"}),
        ]
    );
}

#[test]
fn uses_nested_expected_markers_for_empty_directories_without_hiding_actual_workspace_files() {
    let dir = root();
    let p = dir.path();
    fs::create_dir(p.join("locked")).unwrap();
    fs::write(p.join("locked/.empty"), "").unwrap();
    assert_eq!(
        capture(p, &[], true).unwrap(),
        vec![json!({"path":"locked","kind":"empty-directory"})]
    );
    assert_eq!(
        capture(p, &[], false).unwrap(),
        vec![json!({"path":"locked/.empty","kind":"text","content":""})]
    );
}

#[test]
fn treats_nul_bearing_utf8_as_binary_workspace_state() {
    let dir = root();
    fs::write(dir.path().join("nul.bin"), [0x61, 0x00, 0x62]).unwrap();
    assert_eq!(
        capture(dir.path(), &[], false).unwrap(),
        vec![json!({"path":"nul.bin","kind":"binary","base64":"YQBi"})]
    );
}

/// Review 222017 №6: a backslash is an ordinary file-name character on POSIX.
#[cfg(unix)]
#[test]
fn a_backslash_in_a_file_name_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a\\b"), "x").unwrap();
    let entries = hoh_conformance::workspace::capture(dir.path(), &[], false).unwrap();
    assert_eq!(entries[0]["path"], "a\\b");
}
