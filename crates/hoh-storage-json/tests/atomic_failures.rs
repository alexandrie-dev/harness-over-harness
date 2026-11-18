//! Process-local RLIMIT_FSIZE regressions. No limit or signal is changed in
//! the parent test process, nor in another package's tests/builds.
#![cfg(unix)]

use hoh_storage::{KvFacet, KvLayout, KvUnitDescriptor, StorageBackend};
use hoh_storage_json::{JsonStorageBackend, atomic::NodeFsError};
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
use serde_json::json;
use std::process::Command;
use tokio::fs;

fn run_child(case: &str) {
    // POSIX preserves ignored signals across exec. A shell trap avoids unsafe
    // sigaction/pre_exec and only affects this dedicated child process.
    let output = Command::new("sh")
        .args(["-c", "trap '' XFSZ; exec \"$@\"", "hoh-storage-json-fsize"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "file_size_limit_child",
            "--test-threads=1",
        ])
        .env("HOH_STORAGE_JSON_FSIZE_CASE", case)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{case}: child failed\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn per_record_put_preserves_old_bytes_after_a_short_write() {
    run_child("per-record-put");
}
#[test]
fn single_put_preserves_old_bytes_after_a_short_write() {
    run_child("single-put");
}
#[test]
fn per_record_global_preserves_old_bytes_after_a_short_write() {
    run_child("per-record-global");
}
#[test]
fn single_global_preserves_old_bytes_after_a_short_write() {
    run_child("single-global");
}

#[tokio::test]
#[ignore = "invoked by four isolated child-process wrappers with SIGXFSZ ignored"]
async fn file_size_limit_child() {
    let case = std::env::var("HOH_STORAGE_JSON_FSIZE_CASE").unwrap();
    let per_record = case.starts_with("per-record-");
    let global = case.ends_with("-global");
    let root = tempfile::tempdir().unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = backend
        .open(KvUnitDescriptor {
            name: "limited".into(),
            version: 1,
            tables: vec!["t".into()],
            has_global: true,
            layout: per_record.then_some(KvLayout::PerRecord),
            compatible_versions: None,
        })
        .await
        .unwrap();
    unit.put_record("t", "k", json!({"old":"record"}))
        .await
        .unwrap();
    unit.set_global(json!({"old":"global"})).await.unwrap();
    let path = root.path().join(if !per_record {
        "limited.json"
    } else if global {
        "limited/global.json"
    } else {
        "limited/t/k.json"
    });
    let before = fs::read(&path).await.unwrap();
    let snapshot = unit.load_all().await.unwrap();
    let old_limit = getrlimit(Resource::Fsize);
    setrlimit(
        Resource::Fsize,
        Rlimit {
            current: Some(64),
            maximum: old_limit.maximum,
        },
    )
    .unwrap();
    let payload = json!({"blob":"x".repeat(1024)});
    let result = if global {
        unit.set_global(payload).await
    } else {
        unit.put_record("t", "k", payload).await
    };
    setrlimit(Resource::Fsize, old_limit).unwrap();
    let after = fs::read(&path).await.unwrap();
    let error = result.expect_err("a short temporary write must reject before rename");
    let error = error
        .downcast_ref::<NodeFsError>()
        .expect("native write errno is preserved");
    assert_eq!(error.code, Some("EFBIG"));
    assert_eq!(error.to_string(), "EFBIG: file too large, write");
    assert_eq!(
        after, before,
        "the committed target must remain byte-identical"
    );
    assert_eq!(
        unit.load_all().await.unwrap(),
        snapshot,
        "rejected writes leave memory unchanged"
    );
    let mut entries = fs::read_dir(path.parent().unwrap()).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        assert!(
            !entry.file_name().to_string_lossy().ends_with(".tmp"),
            "failed temporary file was removed"
        );
    }
    backend.close().await.unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let reopened = backend
        .open(KvUnitDescriptor {
            name: "limited".into(),
            version: 1,
            tables: vec!["t".into()],
            has_global: true,
            layout: per_record.then_some(KvLayout::PerRecord),
            compatible_versions: None,
        })
        .await
        .unwrap();
    assert_eq!(reopened.load_all().await.unwrap(), snapshot);
    backend.close().await.unwrap();
}
