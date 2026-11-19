use hoh_subprocess::SubprocessOutputReader;
use hoh_subprocess_local::output::*;
use std::sync::{Arc, Mutex};
use std::{
    fs::File,
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
#[derive(Default)]
struct FaultIo {
    close: AtomicBool,
    write: AtomicBool,
    open: AtomicBool,
    unlink: AtomicBool,
    unlinked: Mutex<Vec<std::path::PathBuf>>,
    diagnostics: Mutex<Vec<String>>,
    close_calls: AtomicUsize,
}
impl SpillIo for FaultIo {
    fn reporter_failure(&self, line: &str) {
        self.diagnostics.lock().unwrap().push(line.into());
    }
    fn open(&self, path: &Path) -> Result<File, hoh_error::BoxError> {
        if self.open.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::from_raw_os_error(17).into());
        }
        NativeSpillIo.open(path)
    }
    fn write(&self, file: &mut File, bytes: &[u8]) -> Result<(), hoh_error::BoxError> {
        if self.write.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::from_raw_os_error(28).into());
        }
        NativeSpillIo.write(file, bytes)
    }
    fn close(&self, file: File) -> Result<(), SpillCloseFailure> {
        self.close_calls.fetch_add(1, Ordering::SeqCst);
        if self.close.swap(false, Ordering::SeqCst) {
            return Err(SpillCloseFailure::Retained {
                file,
                error: std::io::Error::from_raw_os_error(5).into(),
            });
        }
        NativeSpillIo.close(file)
    }
    fn unlink(&self, path: &Path) -> Result<(), hoh_error::BoxError> {
        self.unlinked.lock().unwrap().push(path.into());
        if self.unlink.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::from_raw_os_error(5).into());
        }
        NativeSpillIo.unlink(path)
    }
}
#[test]
fn contains_a_reporter_that_throws_and_keeps_collecting() {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(FaultIo::default());
    let collector = OutputCollector::new_with_io(
        4,
        "loud-reporter",
        Some(SpillOptions {
            dir: dir.path().join("absent"),
            max_bytes: 100,
            on_failure: Arc::new(|_, _| {
                Err(Box::new(hoh_subprocess_local::LocalError::Error(
                    "logger down".into(),
                )))
            }),
        }),
        io.clone(),
    );
    collector.push(b"aaaa");
    collector.push(b"bbbb");
    collector.push(b"cc");
    assert_eq!(
        *io.diagnostics.lock().unwrap(),
        ["dsh-subprocess-local: spill failure reporter threw: Error: logger down\n"]
    );
    let out = collector.finalize();
    assert_eq!(out.text, "bbcc");
    assert!(out.truncated);
    assert!(out.spill_path.is_none());
}
fn spill(dir: &tempfile::TempDir, max_bytes: u64) -> SpillOptions {
    SpillOptions {
        dir: dir.path().into(),
        max_bytes,
        on_failure: Arc::new(|error, _| panic!("unexpected spill failure: {error}")),
    }
}
#[test]
fn keeps_the_tail_of_a_single_oversized_chunk() {
    let dir = tempfile::tempdir().unwrap();
    let collector = OutputCollector::new(10, "test", Some(spill(&dir, 100)));
    collector.push(b"0123456789abcdef");
    let out = collector.finalize();
    assert_eq!(out.text, "6789abcdef");
    assert!(out.truncated);
    assert_eq!(
        std::fs::read_to_string(out.spill_path.unwrap()).unwrap(),
        "0123456789abcdef"
    );
}
#[test]
fn retains_a_byte_exact_tail_across_uneven_chunk_boundaries() {
    let collector = OutputCollector::new(10, "exact-tail", None);
    for bytes in [b"aaaa".as_slice(), b"bbbbbb", b"cc"] {
        collector.push(bytes);
    }
    let out = collector.finalize();
    assert_eq!(out.text, "aabbbbbbcc");
    assert_eq!(out.text.len(), 10);
    assert!(out.truncated);
}
#[test]
fn read_from_returns_increments_and_flags_lossy_reads() {
    let dir = tempfile::tempdir().unwrap();
    let collector = OutputCollector::new(10, "test", Some(spill(&dir, 100)));
    collector.push(b"aaaaa");
    let first = collector.read_from(0);
    assert_eq!(
        (first.text, first.lossy, first.next_offset),
        ("aaaaa".into(), false, 5)
    );
    collector.push(b"bbbbb");
    let second = collector.read_from(first.next_offset);
    assert_eq!(second.text, "bbbbb");
    assert!(!second.lossy);
    collector.push(&[b'c'; 20]);
    let third = collector.read_from(second.next_offset);
    assert!(third.lossy);
    assert_eq!(third.text, "cccccccccc");
    assert!(third.spill_path.is_some());
}
#[test]
fn discards_a_spill_that_exceeds_its_configured_cap() {
    let dir = tempfile::tempdir().unwrap();
    let collector = OutputCollector::new(4, "bounded", Some(spill(&dir, 8)));
    collector.push(b"aaaa");
    collector.push(b"bbbb");
    let path = collector.read_from(0).spill_path.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "aaaabbbb");
    collector.push(b"c");
    collector.push(b"dddd");
    let out = collector.finalize();
    assert_eq!(out.text, "dddd");
    assert!(out.truncated);
    assert!(out.spill_path.is_none());
    assert!(!std::path::Path::new(&path).exists());
}
#[test]
fn does_not_create_a_spill_when_the_first_overflowing_chunk_exceeds_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let collector = OutputCollector::new(4, "no-spill", Some(spill(&dir, 4)));
    collector.push(b"abcdefgh");
    let out = collector.finalize();
    assert_eq!(out.text, "efgh");
    assert!(out.truncated);
    assert!(out.spill_path.is_none());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
#[test]
fn keeps_collecting_when_the_spill_directory_has_been_removed_enoent_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let failures = Arc::new(Mutex::new(vec![]));
    let recorded = failures.clone();
    let options = SpillOptions {
        dir: dir.path().join("absent"),
        max_bytes: 100,
        on_failure: Arc::new(move |error, label| {
            recorded
                .lock()
                .unwrap()
                .push((error.to_string(), label.to_owned()));
            Ok(())
        }),
    };
    let collector = OutputCollector::new(4, "enoent", Some(options));
    collector.push(b"aaaa");
    collector.push(b"bbbb");
    collector.push(b"cc");
    let failures = failures.lock().unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].1, "enoent");
    let out = collector.finalize();
    assert_eq!(out.text, "bbcc");
    assert!(out.truncated);
    assert!(out.spill_path.is_none());
}
#[test]
fn keeps_collecting_when_the_spill_directory_is_a_file_enotdir_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, "").unwrap();
    let calls = Arc::new(Mutex::new(0));
    let recorded = calls.clone();
    let collector = OutputCollector::new(
        4,
        "enotdir",
        Some(SpillOptions {
            dir: file,
            max_bytes: 100,
            on_failure: Arc::new(move |_, _| {
                *recorded.lock().unwrap() += 1;
                Ok(())
            }),
        }),
    );
    collector.push(b"aaaa");
    collector.push(b"bbbb");
    assert_eq!(*calls.lock().unwrap(), 1);
    let out = collector.finalize();
    assert_eq!(out.text, "bbbb");
    assert!(out.truncated);
    assert!(out.spill_path.is_none());
}
#[test]
fn does_not_truncate_output_exactly_at_the_cap() {
    let collector = OutputCollector::new(4, "exact", None);
    collector.push(b"abcd");
    let out = collector.finalize();
    assert_eq!(out.text, "abcd");
    assert!(!out.truncated);
    assert!(out.spill_path.is_none());
}
#[test]
fn offsets_and_utf8_are_bytes_not_characters() {
    let collector = OutputCollector::new(4, "utf8", None);
    collector.push("é€".as_bytes());
    let read = collector.read_from(0);
    assert_eq!(read.text, "�€");
    assert!(read.lossy);
    assert_eq!(read.next_offset, 5);
    assert_eq!(collector.read_from(3).text, "��");
    assert_eq!(collector.read_from(99).next_offset, 5);
}

#[test]
fn contains_close_failures_and_drops_the_spill_path() {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(FaultIo::default());
    let collector =
        OutputCollector::new_with_io(4, "closefail", Some(spill(&dir, 100)), io.clone());
    collector.push(b"aaaa");
    collector.push(b"bbbb");
    assert!(collector.read_from(0).spill_path.is_some());
    io.close.store(true, Ordering::SeqCst);
    let out = collector.finalize();
    assert!(!io.close.load(Ordering::SeqCst));
    assert_eq!(out.text, "bbbb");
    assert!(out.truncated);
    assert!(out.spill_path.is_none());
}
#[test]
fn contains_cleanup_failures_while_disabling_an_oversize_spill() {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(FaultIo::default());
    let collector =
        OutputCollector::new_with_io(4, "cleanup-fail", Some(spill(&dir, 8)), io.clone());
    collector.push(b"aaaa");
    collector.push(b"bbbb");
    let path = collector.read_from(0).spill_path.unwrap();
    io.close.store(true, Ordering::SeqCst);
    io.unlink.store(true, Ordering::SeqCst);
    collector.push(b"c");
    assert!(!io.close.load(Ordering::SeqCst));
    assert!(!io.unlink.load(Ordering::SeqCst));
    assert!(collector.finalize().spill_path.is_none());
    assert_eq!(
        io.close_calls.load(Ordering::SeqCst),
        2,
        "finalize retries the failed discard-close"
    );
    std::fs::remove_file(path).unwrap();
}
#[test]
fn withdraws_a_spill_whose_append_fails_after_the_file_exists_enospc_on_write() {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(FaultIo::default());
    let failures = Arc::new(Mutex::new(vec![]));
    let recorded = failures.clone();
    let options = SpillOptions {
        dir: dir.path().into(),
        max_bytes: 100,
        on_failure: Arc::new(move |error, label| {
            recorded.lock().unwrap().push((error, label.to_owned()));
            Ok(())
        }),
    };
    let collector = OutputCollector::new_with_io(4, "enospc", Some(options), io.clone());
    collector.push(b"aaaa");
    collector.push(b"bbbb");
    let path = collector.read_from(0).spill_path.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "aaaabbbb");
    io.write.store(true, Ordering::SeqCst);
    collector.push(b"cccc");
    assert!(!io.write.load(Ordering::SeqCst));
    assert_eq!(failures.lock().unwrap().len(), 1);
    assert_eq!(
        failures.lock().unwrap()[0]
            .0
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(28)
    );
    assert!(!Path::new(&path).exists());
    collector.push(b"dd");
    assert_eq!(failures.lock().unwrap().len(), 1);
    let out = collector.finalize();
    assert_eq!(out.text, "ccdd");
    assert!(out.truncated);
    assert!(out.spill_path.is_none());
}
#[test]
fn does_not_unlink_a_path_it_never_created_when_the_exclusive_open_fails_eexist() {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(FaultIo::default());
    let failures = Arc::new(Mutex::new(vec![]));
    let recorded = failures.clone();
    let collector = OutputCollector::new_with_io(
        4,
        "eexist",
        Some(SpillOptions {
            dir: dir.path().into(),
            max_bytes: 100,
            on_failure: Arc::new(move |error, _| {
                recorded.lock().unwrap().push(error);
                Ok(())
            }),
        }),
        io.clone(),
    );
    collector.push(b"aaaa");
    io.open.store(true, Ordering::SeqCst);
    collector.push(b"bbbb");
    assert!(!io.open.load(Ordering::SeqCst));
    assert_eq!(
        failures.lock().unwrap()[0]
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(17)
    );
    assert!(io.unlinked.lock().unwrap().is_empty());
    let out = collector.finalize();
    assert_eq!(out.text, "bbbb");
    assert!(out.truncated);
    assert!(out.spill_path.is_none());
}

#[tokio::test]
async fn names_the_removed_directory_only_for_enoent_and_appends_the_error_for_every_code() {
    use hoh_subprocess_local::runner_protocol::*;
    let kernel = hoh_kernel::Kernel::start();
    let ctx = kernel.root();
    let report = log_spill_failure(ctx.logger(), "test owner".into());
    for (message, code, label) in [("gone", "ENOENT", "stdout"), ("full", "ENOSPC", "stderr")] {
        report(
            deserialize_runner_error(SerializedRunnerError {
                name: "Error".into(),
                message: message.into(),
                code: Some(code.into()),
                syscall: None,
                path: None,
            }),
            label,
        )
        .unwrap();
    }
    let records = ctx.logger().buffer();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].message,
        "test owner could not write the complete stdout stream to its spill file; the result keeps only the in-memory tail and reports no full-output path. The spill directory no longer exists; a temporary-file cleaner removing it while empty is the usual cause. Error: gone"
    );
    assert_eq!(
        records[1].message,
        "test owner could not write the complete stderr stream to its spill file; the result keeps only the in-memory tail and reports no full-output path. Error: full"
    );
    kernel.shutdown().await;
}
