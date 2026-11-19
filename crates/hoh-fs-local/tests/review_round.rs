//! Regressions from the fs-local review round; expectations come from the source
//! (`fsio.ts`) and Node 22 (`fs`, `util.inspect`, ICU `localeCompare` for `en-US`).
mod common;
use common::*;
use hoh_error::HarnessError;
use hoh_fs::*;
use hoh_fs_local::fsio::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn close_error() -> hoh_error::BoxError {
    Box::new(ErrnoError::from_io(
        std::io::Error::from_raw_os_error(5),
        "close",
        None,
        None,
    ))
}
/// Fails the listed close attempts (1-based) with `EIO` and counts every attempt.
fn failing_closes(failing: &'static [usize]) -> (FsIoInternals, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let hooks = FsIoInternals {
        close_file: Some(Arc::new(move || {
            let attempt = seen.fetch_add(1, Ordering::SeqCst) + 1;
            if failing.contains(&attempt) {
                Err(close_error())
            } else {
                Ok(())
            }
        })),
        ..Default::default()
    };
    (hooks, calls)
}
async fn staging_residue(e: &Env) -> Vec<String> {
    let mut names = vec![];
    let mut entries = tokio::fs::read_dir(e.dir.path()).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.retain(|n| n.contains(".tmp"));
    names
}
fn message(error: &hoh_error::BoxError) -> String {
    HarnessError::of(error.as_ref()).unwrap().message().into()
}

mod close_before_publication {
    use super::*;
    #[tokio::test]
    async fn a_failed_close_rejects_and_preserves_the_destination() {
        let e = Env::new();
        e.file("a.txt", "old").await;
        let (hooks, calls) = failing_closes(&[1]);
        let result = write_file_atomic(
            &e.path("a.txt"),
            "new",
            None,
            None,
            &hooks,
            AtomicWriteOptions::default(),
        )
        .await;
        let failure = error(result, "FS_IO_ERROR");
        assert_eq!(
            message(&failure),
            format!(
                "cannot write \"{}\": EIO: i/o error, close",
                e.path("a.txt")
            )
        );
        let cause = HarnessError::of(failure.as_ref())
            .unwrap()
            .cause()
            .unwrap()
            .downcast_ref::<ErrnoError>()
            .unwrap();
        assert_eq!(cause.code.as_deref(), Some("EIO"));
        assert_eq!(e.bytes("a.txt").await, "old");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(staging_residue(&e).await.is_empty());
    }
    #[tokio::test]
    async fn a_failed_close_does_not_publish_a_new_exclusive_create() {
        let e = Env::new();
        let (hooks, _) = failing_closes(&[1]);
        let result = write_file_atomic(
            &e.path("new.txt"),
            "new",
            None,
            None,
            &hooks,
            AtomicWriteOptions {
                create_if_absent: true,
                ..Default::default()
            },
        )
        .await;
        error(result, "FS_IO_ERROR");
        assert!(!std::path::Path::new(&e.path("new.txt")).exists());
        assert!(staging_residue(&e).await.is_empty());
    }
    #[tokio::test]
    async fn a_second_close_failure_composes_the_message_like_the_source() {
        let e = Env::new();
        e.file("a.txt", "old").await;
        let (hooks, calls) = failing_closes(&[1, 2]);
        let result = write_file_atomic(
            &e.path("a.txt"),
            "new",
            None,
            None,
            &hooks,
            AtomicWriteOptions::default(),
        )
        .await;
        let failure = error(result, "FS_NOT_FOUND");
        let first = format!(
            "cannot write \"{}\": EIO: i/o error, close",
            e.path("a.txt")
        );
        assert_eq!(
            message(&failure),
            format!("write failed ({first}) and temp close failed (EIO: i/o error, close)")
        );
        assert_eq!(e.bytes("a.txt").await, "old");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(staging_residue(&e).await.is_empty());
    }
    #[tokio::test]
    async fn a_staging_failure_followed_by_a_close_failure_is_composed() {
        let e = Env::new();
        e.file("a.txt", "old").await;
        let (mut hooks, calls) = failing_closes(&[1]);
        hooks.inspect_temp = Some(Arc::new(|_| {
            Box::pin(async { Err(errno("ENOSPC", "ENOSPC: no space left on device, write")) })
        }));
        let result = write_file_atomic(
            &e.path("a.txt"),
            "new",
            None,
            None,
            &hooks,
            AtomicWriteOptions::default(),
        )
        .await;
        let failure = error(result, "FS_NOT_FOUND");
        assert_eq!(
            message(&failure),
            format!(
                "write failed (cannot write \"{}\": ENOSPC: no space left on device, write) and temp close failed (EIO: i/o error, close)",
                e.path("a.txt")
            )
        );
        assert_eq!(e.bytes("a.txt").await, "old");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn a_successful_write_still_closes_once_and_publishes() {
        let e = Env::new();
        e.file("a.txt", "old").await;
        let (hooks, calls) = failing_closes(&[]);
        write_file_atomic(
            &e.path("a.txt"),
            "new",
            None,
            None,
            &hooks,
            AtomicWriteOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.bytes("a.txt").await, "new");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

mod name_order {
    use super::*;
    async fn names(e: &Env, files: &[&str]) -> Vec<String> {
        for f in files {
            tokio::fs::write(e.path(f), "x").await.unwrap();
        }
        let dir = target(e.path(""));
        list_directory(&dir, None)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect()
    }
    // Expected orders follow UCA/CLDR root collation (ICU default for `en-US`:
    // tertiary strength, non-ignorable punctuation, lowercase before uppercase),
    // as in the CLDR root collation chart and UTS #10 §3 examples.
    #[tokio::test]
    async fn an_accented_latin_letter_sorts_with_its_base_letter() {
        let e = Env::new();
        assert_eq!(names(&e, &["z", "é", "e", "f"]).await, ["e", "é", "f", "z"]);
    }
    #[tokio::test]
    async fn accents_differ_only_after_the_base_letters() {
        let e = Env::new();
        assert_eq!(
            names(&e, &["Mélissa", "Melissa", "Melissb", "Éloi", "Elrond"]).await,
            ["Éloi", "Elrond", "Melissa", "Mélissa", "Melissb"]
        );
    }
    #[tokio::test]
    async fn cyrillic_follows_the_alphabet_and_yo_follows_ye() {
        let e = Env::new();
        assert_eq!(
            names(&e, &["ж", "ё", "е", "а", "я", "б"]).await,
            ["а", "б", "е", "ё", "ж", "я"]
        );
    }
    #[tokio::test]
    async fn digits_precede_letters_and_compare_as_text() {
        let e = Env::new();
        assert_eq!(
            names(&e, &["b", "10", "2", "a", "1"]).await,
            ["1", "10", "2", "a", "b"]
        );
    }
    #[tokio::test]
    async fn lowercase_precedes_uppercase_of_the_same_letter() {
        let e = Env::new();
        assert_eq!(names(&e, &["B", "a", "A", "b"]).await, ["a", "A", "b", "B"]);
    }
    #[tokio::test]
    async fn punctuation_is_not_ignored() {
        let e = Env::new();
        assert_eq!(
            names(&e, &["ab", "a-b", "a_b", "a.b"]).await,
            ["a_b", "a-b", "a.b", "ab"]
        );
    }
}

mod stream_bounds {
    use super::*;
    const TWO_53: u64 = 9_007_199_254_740_992;
    fn range_error(error: &hoh_error::BoxError) -> &ErrnoError {
        error.downcast_ref::<ErrnoError>().expect("bare RangeError")
    }
    #[tokio::test]
    async fn read_bytes_rejects_a_limit_beyond_the_safe_integers() {
        let e = Env::new();
        let t = e.file("empty.bin", "").await;
        let failure = read_whole_bytes(&t, None, TWO_53, &FsIoInternals::default())
            .await
            .unwrap_err();
        let range = range_error(&failure);
        assert_eq!(range.code.as_deref(), Some("ERR_OUT_OF_RANGE"));
        assert_eq!(
            range.message,
            "The value of \"end\" is out of range. It must be >= 0 && <= 9007199254740991. Received 9_007_199_254_740_992"
        );
    }
    #[tokio::test]
    async fn read_bytes_accepts_the_largest_safe_integer() {
        let e = Env::new();
        let t = e.file("empty.bin", "").await;
        let bytes = read_whole_bytes(&t, None, TWO_53 - 1, &FsIoInternals::default())
            .await
            .unwrap();
        assert!(bytes.is_empty());
    }
    #[tokio::test]
    async fn the_size_limit_is_checked_before_the_range() {
        let e = Env::new();
        let t = e.file("one.bin", "x").await;
        let failure = read_whole_bytes(&t, None, 0, &FsIoInternals::default())
            .await
            .unwrap_err();
        assert_code(failure.as_ref(), "FS_TOO_LARGE");
    }
    #[tokio::test]
    async fn a_byte_window_rejects_an_oversized_offset() {
        let e = Env::new();
        let t = e.file("a.bin", "abc").await;
        let failure = read_byte_window(
            &t,
            FsByteRange {
                offset: TWO_53,
                length: 1,
            },
            None,
        )
        .await
        .unwrap_err();
        let range = range_error(&failure);
        assert_eq!(range.code.as_deref(), Some("ERR_OUT_OF_RANGE"));
        assert_eq!(
            range.message,
            "The value of \"start\" is out of range. It must be >= 0 && <= 9007199254740991. Received 9_007_199_254_740_992"
        );
    }
    #[tokio::test]
    async fn a_byte_window_rejects_an_oversized_endpoint() {
        let e = Env::new();
        let t = e.file("a.bin", "abc").await;
        let failure = read_byte_window(
            &t,
            FsByteRange {
                offset: 10,
                length: TWO_53,
            },
            None,
        )
        .await
        .unwrap_err();
        let range = range_error(&failure);
        assert_eq!(range.code.as_deref(), Some("ERR_OUT_OF_RANGE"));
        assert_eq!(
            range.message,
            "The value of \"end\" is out of range. It must be >= 0 && <= 9007199254740991. Received 9_007_199_254_741_000"
        );
    }
    #[tokio::test]
    async fn a_zero_length_window_never_reaches_the_stream_constructor() {
        let e = Env::new();
        let t = e.file("a.bin", "abc").await;
        let bytes = read_byte_window(
            &t,
            FsByteRange {
                offset: TWO_53,
                length: 0,
            },
            None,
        )
        .await
        .unwrap();
        assert!(bytes.is_empty());
    }
}

mod null_bytes {
    use super::*;
    const MESSAGE: &str = "The argument 'path' must be a string, Uint8Array, or URL without null bytes. Received '/tmp/a\\x00b'";
    fn cause(error: &hoh_error::BoxError) -> &ErrnoError {
        HarnessError::of(error.as_ref())
            .unwrap()
            .cause()
            .unwrap()
            .downcast_ref::<ErrnoError>()
            .unwrap()
    }
    #[tokio::test]
    async fn probe_reports_node_invalid_arg_value_as_an_io_error() {
        let failure = error(probe("/tmp/a\0b").await, "FS_IO_ERROR");
        assert_eq!(
            message(&failure),
            format!("cannot stat \"/tmp/a\0b\": {MESSAGE}")
        );
        assert_eq!(
            cause(&failure).code.as_deref(),
            Some("ERR_INVALID_ARG_VALUE")
        );
        assert_eq!(cause(&failure).message, MESSAGE);
    }
    #[tokio::test]
    async fn no_follow_probe_and_target_resolution_use_the_same_taxonomy() {
        let failure = error(probe_no_follow("/tmp/a\0b").await, "FS_IO_ERROR");
        assert_eq!(cause(&failure).message, MESSAGE);
        let failure = error(
            resolve_local_target("/tmp", "/tmp/a\0b").await,
            "FS_IO_ERROR",
        );
        assert_eq!(
            message(&failure),
            format!("cannot resolve \"/tmp/a\0b\": {MESSAGE}")
        );
    }
    #[tokio::test]
    async fn the_received_value_is_quoted_like_util_inspect() {
        let failure = error(probe("/tmp/it's\0\n").await, "FS_IO_ERROR");
        assert_eq!(
            cause(&failure).message,
            "The argument 'path' must be a string, Uint8Array, or URL without null bytes. Received \"/tmp/it's\\x00\\n\""
        );
    }
    #[tokio::test]
    async fn a_long_received_value_is_cut_at_128_characters() {
        let path = format!("/tmp/{}\0", "x".repeat(200));
        let failure = error(probe(&path).await, "FS_IO_ERROR");
        let expected_tail = format!("Received '/tmp/{}...", "x".repeat(128 - "'/tmp/".len()));
        assert!(
            cause(&failure).message.ends_with(&expected_tail),
            "{}",
            cause(&failure).message
        );
    }
}

mod process_directory {
    use super::*;
    #[test]
    fn absolute_paths_do_not_consult_a_deleted_current_directory() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "process_directory::deleted_cwd_child",
                "--test-threads=1",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[test]
    #[ignore = "executed by an isolated wrapper that deletes its own working directory"]
    fn deleted_cwd_child() {
        let doomed = tempfile::tempdir().unwrap();
        std::env::set_current_dir(doomed.path()).unwrap();
        let path = doomed.path().to_path_buf();
        drop(doomed);
        assert!(!path.exists());
        assert_eq!(local_display_path("/tmp", "x").unwrap(), "/tmp/x");
        assert_eq!(
            local_display_path("/tmp", "/etc/passwd").unwrap(),
            "/etc/passwd"
        );
        assert_eq!(local_display_path("/tmp", "../etc").unwrap(), "/tmp/../etc");
        assert_eq!(local_display_path("/tmp", "a/../b").unwrap(), "/tmp/a/../b");
        // As in the source (`process.cwd()` for a relative base before anything else), a relative base still needs the process directory and reports Node's `uv_cwd` failure.
        let failure = local_display_path("rel", "x").unwrap_err();
        assert!(failure.to_string().ends_with(", uv_cwd"), "{failure}");
    }
}

mod read_phase {
    use super::*;
    #[tokio::test]
    async fn a_read_failure_after_a_successful_open_is_labelled_read() {
        // Offset 0 of /proc/self/mem is unmapped: open succeeds, read fails with EIO.
        let failure = error(
            read_for_edit("/proc/self/mem", "mem", None).await,
            "FS_IO_ERROR",
        );
        assert_eq!(
            message(&failure),
            "cannot edit \"mem\": EIO: i/o error, read"
        );
    }
    #[tokio::test]
    async fn an_open_failure_is_still_labelled_open_with_the_path() {
        let e = Env::new();
        let failure = error(
            read_for_edit(&e.path("missing.txt"), "missing.txt", None).await,
            "FS_NOT_FOUND",
        );
        assert_eq!(message(&failure), "cannot edit \"missing.txt\": not found");
    }
    #[tokio::test]
    async fn reading_a_directory_is_labelled_read() {
        let e = Env::new();
        let failure = error(
            read_for_edit(&e.path(""), "dir", None).await,
            "FS_NOT_REGULAR_FILE",
        );
        assert_eq!(message(&failure), "cannot edit \"dir\": not a regular file");
    }
}
