//! A process-local size limit never affects the other tests or shared builds.
mod common;
use common::*;
use hoh_error::HarnessError;
use hoh_fs::*;
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
#[test]
fn a_failed_temporary_write_preserves_the_existing_file() {
    let output = std::process::Command::new("sh")
        .args(["-c", "trap '' XFSZ; exec \"$@\"", "hoh-fs-local-fsize"])
        .arg(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "limited_child", "--test-threads=1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[tokio::test]
#[ignore = "executed by isolated wrapper with SIGXFSZ ignored"]
async fn limited_child() {
    let e = Env::new();
    let t = e.file("file.txt", "known good").await;
    let old = getrlimit(Resource::Fsize);
    setrlimit(
        Resource::Fsize,
        Rlimit {
            current: Some(64),
            maximum: old.maximum,
        },
    )
    .unwrap();
    let result =
        e.fs.write_text(&t, &"x".repeat(1024), None, None, None)
            .await;
    setrlimit(Resource::Fsize, old).unwrap();
    let error = error(result, "FS_IO_ERROR");
    let cause = HarnessError::of(error.as_ref())
        .unwrap()
        .cause()
        .unwrap()
        .downcast_ref::<ErrnoError>()
        .unwrap();
    assert_eq!(cause.code.as_deref(), Some("EFBIG"));
    assert_eq!(cause.message, "EFBIG: file too large, write");
    assert_eq!(e.bytes("file.txt").await, "known good");
    let mut entries = tokio::fs::read_dir(e.dir.path()).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        assert!(!entry.file_name().to_string_lossy().contains(".tmp"));
    }
}
