//! End-to-end tool-registry tests against the real local backend. The policy deployment verifies
//! observed-state and guarded mutation; the bare deployment proves unconditional tools have no
//! policy-service dependency. Assertions read files back byte-for-byte rather than trusting tool
//! messages.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hoh_fs::{FsEditIntent, FsObservation, FsObserved, FsVersion, FsVersionGuard};
use hoh_timeout::AbortController;
use hoh_tools::Agent;
use serde_json::json;
use support::patch::*;
use support::{error_info, named};

fn code(result: &hoh_tools::ToolExecutionResult) -> Option<String> {
    error_info(result).map(|(_, code)| code)
}

fn not_observed(path: &str) -> String {
    format!("Error: cannot modify \"{path}\": file has not been read — read the file, then retry")
}

async fn guarded() -> T {
    setup(PatchOptions {
        without_apply_patch: true,
        ..PatchOptions::default()
    })
    .await
}

async fn bare() -> T {
    setup(PatchOptions {
        without_policy: true,
        without_apply_patch: true,
    })
    .await
}

mod guarded_deployment {
    use super::*;

    #[tokio::test]
    async fn without_read_creates_and_edits_a_new_file_while_protecting_unobserved_existing_files()
    {
        let t = guarded().await;
        t.tool_fs.dispose().await;
        let tool_fs = t
            .ctx
            .plugin::<hoh_tool_fs::ToolFs>(json!({ "read": false }))
            .unwrap();
        tool_fs.settled().await.unwrap();
        std::fs::write(t.path("existing.txt"), "original").unwrap();
        assert!(
            !t.call(
                "write",
                json!({ "file_path": "new.txt", "content": "created" })
            )
            .await
            .is_error()
        );
        assert!(
            !t.call(
                "edit",
                json!({ "file_path": "new.txt", "old_string": "created", "new_string": "edited" })
            )
            .await
            .is_error()
        );
        assert_eq!(t.bytes("new.txt").unwrap(), "edited");
        let overwrite = t
            .call(
                "write",
                json!({ "file_path": "existing.txt", "content": "overwrite" }),
            )
            .await;
        assert!(overwrite.is_error());
        assert_eq!(code(&overwrite).as_deref(), Some("FS_NOT_OBSERVED"));
        let edit = t
            .call("edit", json!({ "file_path": "existing.txt", "old_string": "original", "new_string": "overwrite" }))
            .await;
        assert_eq!(code(&edit).as_deref(), Some("FS_NOT_OBSERVED"));
        assert_eq!(t.bytes("existing.txt").unwrap(), "original");
        assert!(t.env.tools.get("read", None, None).is_none());
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn creates_a_file_with_exactly_the_requested_bytes() {
        let t = guarded().await;
        let result = t
            .call(
                "write",
                json!({ "file_path": "new.txt", "content": "line one\nline two\n" }),
            )
            .await;
        assert!(!result.is_error());
        assert_eq!(t.bytes("new.txt").unwrap(), "line one\nline two\n");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_overwriting_an_existing_file_without_reading_it_first() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "original").unwrap();
        let result = t
            .call(
                "write",
                json!({ "file_path": "a.txt", "content": "clobber" }),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(code(&result).as_deref(), Some("FS_NOT_OBSERVED"));
        assert_eq!(result.text(), not_observed(&t.path_text("a.txt")));
        assert_eq!(t.bytes("a.txt").unwrap(), "original");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn allows_overwriting_after_a_read() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "original").unwrap();
        assert!(
            !t.call("read", json!({ "file_path": "a.txt" }))
                .await
                .is_error()
        );
        let result = t
            .call(
                "write",
                json!({ "file_path": "a.txt", "content": "replaced" }),
            )
            .await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(t.bytes("a.txt").unwrap(), "replaced");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_a_full_overwrite_when_the_file_changed_since_the_read() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "original").unwrap();
        t.call("read", json!({ "file_path": "a.txt" })).await;
        std::fs::write(t.path("a.txt"), "changed-externally").unwrap();
        let result = t
            .call(
                "write",
                json!({ "file_path": "a.txt", "content": "replaced" }),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(code(&result).as_deref(), Some("FS_STALE_VERSION"));
        // The model-facing text names the remedy, not just the condition.
        assert!(result.text().contains("file changed since it was read"));
        assert!(result.text().contains("re-read the file, then retry"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn the_stale_remedy_is_actionable_re_reading_unblocks_the_retried_write() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "original").unwrap();
        t.call("read", json!({ "file_path": "a.txt" })).await;
        std::fs::write(t.path("a.txt"), "changed-externally").unwrap();
        let stale = t
            .call(
                "write",
                json!({ "file_path": "a.txt", "content": "replaced" }),
            )
            .await;
        assert!(stale.is_error());
        assert_eq!(code(&stale).as_deref(), Some("FS_STALE_VERSION"));
        assert!(
            !t.call("read", json!({ "file_path": "a.txt" }))
                .await
                .is_error()
        );
        let retried = t
            .call(
                "write",
                json!({ "file_path": "a.txt", "content": "replaced" }),
            )
            .await;
        assert!(!retried.is_error());
        assert_eq!(t.bytes("a.txt").unwrap(), "replaced");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn read_returns_line_numbered_content() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "alpha\nbeta").unwrap();
        let result = t.call("read", json!({ "file_path": "a.txt" })).await;
        let text = result.text();
        assert!(text.contains("1: alpha"));
        assert!(text.contains("2: beta"));
        assert!(text.contains("(End of file - total 2 lines)"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn read_reports_a_binary_file_as_an_error() {
        let t = guarded().await;
        std::fs::write(t.path("bin"), [0x00, 0x01, 0x02]).unwrap();
        let result = t.call("read", json!({ "file_path": "bin" })).await;
        assert!(result.is_error());
        assert_eq!(code(&result).as_deref(), Some("FS_NOT_TEXT"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn read_paginates_a_multi_line_file_with_offset_and_limit() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "one\ntwo\nthree\nfour").unwrap();
        let result = t
            .call(
                "read",
                json!({ "file_path": "a.txt", "offset": 2, "limit": 2 }),
            )
            .await;
        let text = result.text();
        assert!(text.contains("2: two"));
        assert!(text.contains("3: three"));
        assert!(text.contains("(Showing lines 2-3 of 4. Use offset=4 to continue.)"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn edit_applies_a_unique_literal_replacement_after_a_read() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "hello world").unwrap();
        t.call("read", json!({ "file_path": "a.txt" })).await;
        let result = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "world", "new_string": "there" }),
            )
            .await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(t.bytes("a.txt").unwrap(), "hello there");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn edit_rejects_an_edit_before_any_read_leaving_the_file_untouched() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "hello world").unwrap();
        let result = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "world", "new_string": "there" }),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(code(&result).as_deref(), Some("FS_NOT_OBSERVED"));
        assert_eq!(result.text(), not_observed(&t.path_text("a.txt")));
        assert_eq!(t.bytes("a.txt").unwrap(), "hello world");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_windowed_read_authorizes_an_edit_when_the_file_is_unchanged() {
        let t = guarded().await;
        // A file with more lines than the read window; read only the first line.
        let lines: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
        std::fs::write(t.path("a.txt"), lines.join("\n")).unwrap();
        let read = t
            .call(
                "read",
                json!({ "file_path": "a.txt", "offset": 1, "limit": 1 }),
            )
            .await;
        assert!(!read.is_error());
        assert!(read.text().contains("(Showing lines 1-1 of 20"));
        // Editing a line OUTSIDE the window is authorized because the file is unchanged.
        let result = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "line 12", "new_string": "LINE 12" }),
            )
            .await;
        assert!(!result.is_error(), "{}", result.text());
        let expected: Vec<String> = lines
            .iter()
            .map(|line| {
                if line == "line 12" {
                    "LINE 12".to_owned()
                } else {
                    line.clone()
                }
            })
            .collect();
        assert_eq!(t.bytes("a.txt").unwrap(), expected.join("\n"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_an_edit_when_the_file_changed_since_the_windowed_read() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "hello world").unwrap();
        t.call(
            "read",
            json!({ "file_path": "a.txt", "offset": 1, "limit": 1 }),
        )
        .await;
        std::fs::write(t.path("a.txt"), "goodbye").unwrap();
        let result = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "world", "new_string": "there" }),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(code(&result).as_deref(), Some("FS_STALE_VERSION"));
        assert!(result.text().contains("file changed since it was read"));
        assert!(result.text().contains("re-read the file, then retry"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn the_stale_remedy_is_actionable_re_reading_unblocks_the_retried_edit() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "hello world").unwrap();
        t.call("read", json!({ "file_path": "a.txt" })).await;
        std::fs::write(t.path("a.txt"), "hello brave world").unwrap();
        let args = json!({ "file_path": "a.txt", "old_string": "world", "new_string": "there" });
        let stale = t.call("edit", args.clone()).await;
        assert!(stale.is_error());
        assert_eq!(code(&stale).as_deref(), Some("FS_STALE_VERSION"));
        assert!(
            !t.call("read", json!({ "file_path": "a.txt" }))
                .await
                .is_error()
        );
        let retried = t.call("edit", args).await;
        assert!(!retried.is_error());
        assert_eq!(t.bytes("a.txt").unwrap(), "hello brave there");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_an_ambiguous_match_without_replace_all() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "a a a").unwrap();
        t.call("read", json!({ "file_path": "a.txt" })).await;
        let result = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "a", "new_string": "b" }),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(code(&result).as_deref(), Some("FS_AMBIGUOUS_EDIT"));
        assert_eq!(t.bytes("a.txt").unwrap(), "a a a");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn replaces_all_matches_with_replace_all() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "a a a").unwrap();
        t.call("read", json!({ "file_path": "a.txt" })).await;
        let result = t
            .call("edit", json!({ "file_path": "a.txt", "old_string": "a", "new_string": "b", "replace_all": true }))
            .await;
        assert!(!result.is_error());
        assert_eq!(t.bytes("a.txt").unwrap(), "b b b");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn supports_a_full_write_edit_cycle_without_an_intervening_read() {
        let t = guarded().await;
        t.call(
            "write",
            json!({ "file_path": "a.txt", "content": "one two" }),
        )
        .await;
        let result = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "two", "new_string": "three" }),
            )
            .await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(t.bytes("a.txt").unwrap(), "one three");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_direct_provider_read_records_no_observed_state_so_a_later_edit_rejects() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "hello world").unwrap();
        // Reach AROUND the tool — an explicit escape hatch for non-tool consumers.
        let target = t.target("a.txt").await;
        hoh_fs::FileSystem::read_text(&*t.local, &target, None)
            .await
            .unwrap();
        let result = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "world", "new_string": "there" }),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(code(&result).as_deref(), Some("FS_NOT_OBSERVED"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_failed_reread_records_absence_so_write_can_safely_recreate_the_file() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "original").unwrap();
        t.call("read", json!({ "file_path": "a.txt" })).await;
        std::fs::remove_file(t.path("a.txt")).unwrap(); // out-of-band deletion

        // The original positive observation still protects the first mutation.
        let edit = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "original", "new_string": "x" }),
            )
            .await;
        assert!(edit.is_error());
        assert_eq!(code(&edit).as_deref(), Some("FS_STALE_VERSION"));
        let write = t
            .call(
                "write",
                json!({ "file_path": "a.txt", "content": "premature" }),
            )
            .await;
        assert!(write.is_error());
        assert_eq!(code(&write).as_deref(), Some("FS_STALE_VERSION"));

        // A read-not-found is an authoritative negative observation for this owner. It still
        // fails as a read, but changes the next write guard.
        let reread = t.call("read", json!({ "file_path": "a.txt" })).await;
        assert!(reread.is_error());
        assert_eq!(code(&reread).as_deref(), Some("FS_NOT_FOUND"));

        // Absence never authorizes edit: there is no content/version to edit.
        let retried = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "original", "new_string": "x" }),
            )
            .await;
        assert!(retried.is_error());
        assert_eq!(code(&retried).as_deref(), Some("FS_NOT_FOUND"));

        // The retried write uses createIfAbsent; the provider remains responsible for rejecting a
        // concurrent creator at publication time.
        let recovered = t
            .call("write", json!({ "file_path": "a.txt", "content": "fresh" }))
            .await;
        assert!(!recovered.is_error(), "{}", recovered.text());
        assert_eq!(t.bytes("a.txt").unwrap(), "fresh");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn read_stats_once_and_write_and_edit_never_stat_in_the_tool() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "hello world").unwrap();
        // The working directory is resolved once up front; only file stats are the tool's.
        t.call("read", json!({ "file_path": "a.txt" })).await;
        assert_eq!(t.file_stats(), 1);
        let edited = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "world", "new_string": "there" }),
            )
            .await;
        assert!(!edited.is_error());
        assert_eq!(t.file_stats(), 1);
        let written = t
            .call("write", json!({ "file_path": "a.txt", "content": "fresh" }))
            .await;
        assert!(!written.is_error());
        assert_eq!(t.file_stats(), 1);
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_missing_read_still_stats_once_and_its_recovery_write_stats_zero_times() {
        let t = guarded().await;
        let missing = t.call("read", json!({ "file_path": "missing.txt" })).await;
        assert!(missing.is_error());
        assert_eq!(code(&missing).as_deref(), Some("FS_NOT_FOUND"));
        assert_eq!(t.file_stats(), 1);
        let created = t
            .call(
                "write",
                json!({ "file_path": "missing.txt", "content": "fresh" }),
            )
            .await;
        assert!(!created.is_error());
        assert_eq!(t.file_stats(), 1);
        t.kernel.shutdown().await;
    }
}

mod bare_provider {
    use super::*;

    #[tokio::test]
    async fn read_works_it_never_needed_policy() {
        let t = bare().await;
        std::fs::write(t.path("a.txt"), "alpha\nbeta").unwrap();
        let result = t.call("read", json!({ "file_path": "a.txt" })).await;
        assert!(!result.is_error());
        assert!(result.text().contains("1: alpha"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn reports_a_host_metadata_fault_as_a_typed_filesystem_failure() {
        let t = bare().await;
        std::os::unix::fs::symlink(t.path("loop"), t.path("loop")).unwrap();
        let result = t.call("read", json!({ "file_path": "loop" })).await;
        assert!(result.is_error());
        assert_eq!(error_info(&result), named("FsError", "FS_IO_ERROR"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn reports_unreadable_content_as_fs_permission_denied() {
        use std::os::unix::fs::PermissionsExt;
        let t = bare().await;
        let file = t.path("locked.txt");
        std::fs::write(&file, "secret").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        // A privileged environment is not denied by the mode; the source test skips for root.
        let readable = std::fs::read(&file).is_ok();
        if !readable {
            let result = t.call("read", json!({ "file_path": "locked.txt" })).await;
            assert!(result.is_error());
            assert_eq!(
                error_info(&result),
                named("FsError", "FS_PERMISSION_DENIED")
            );
            assert!(result.text().contains("permission denied"));
        }
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn write_unconditionally_creates_a_new_file() {
        let t = bare().await;
        let result = t
            .call(
                "write",
                json!({ "file_path": "new.txt", "content": "fresh" }),
            )
            .await;
        assert!(!result.is_error());
        assert_eq!(t.bytes("new.txt").unwrap(), "fresh");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn write_unconditionally_overwrites_an_existing_unread_file() {
        let t = bare().await;
        std::fs::write(t.path("a.txt"), "original").unwrap();
        let result = t
            .call(
                "write",
                json!({ "file_path": "a.txt", "content": "clobbered" }),
            )
            .await;
        assert!(!result.is_error());
        assert_eq!(t.bytes("a.txt").unwrap(), "clobbered");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn edit_unconditionally_edits_an_unread_existing_file() {
        let t = bare().await;
        std::fs::write(t.path("a.txt"), "hello world").unwrap();
        let result = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "world", "new_string": "there" }),
            )
            .await;
        assert!(!result.is_error());
        assert_eq!(t.bytes("a.txt").unwrap(), "hello there");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn edit_of_a_missing_target_reports_fs_stale_version_even_on_the_unguarded_path() {
        let t = bare().await;
        let result = t
            .call(
                "edit",
                json!({ "file_path": "missing.txt", "old_string": "a", "new_string": "b" }),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(code(&result).as_deref(), Some("FS_STALE_VERSION"));
        // Even without policy, the stale text carries the re-read remedy.
        assert!(result.text().contains("file changed since it was read"));
        assert!(result.text().contains("re-read the file, then retry"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn edit_still_enforces_literal_match_codes_unrelated_to_freshness() {
        let t = bare().await;
        std::fs::write(t.path("a.txt"), "hello world").unwrap();
        let result = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "absent", "new_string": "x" }),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(code(&result).as_deref(), Some("FS_EDIT_NOT_FOUND"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn neither_write_nor_edit_stats_in_the_tool_on_the_bare_path() {
        let t = bare().await;
        std::fs::write(t.path("a.txt"), "hello world").unwrap();
        assert!(
            !t.call("write", json!({ "file_path": "a.txt", "content": "x y" }))
                .await
                .is_error()
        );
        assert!(
            !t.call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "y", "new_string": "z" })
            )
            .await
            .is_error()
        );
        assert_eq!(t.file_stats(), 0);
        t.kernel.shutdown().await;
    }
}

mod per_session_cwd {
    use super::*;

    async fn call_in(
        t: &T,
        agent: &Arc<support::TestAgent>,
        name: &str,
        args: serde_json::Value,
    ) -> hoh_tools::ToolExecutionResult {
        let agent: Arc<dyn Agent> = agent.clone();
        support::call(&t.ctx, name, args, Some(agent)).await
    }

    #[tokio::test]
    async fn writes_a_relative_path_into_the_session_cwd_not_the_config_cwd() {
        let t = guarded().await;
        let session_dir = tempfile::tempdir().unwrap();
        let session_cwd = std::fs::canonicalize(session_dir.path()).unwrap();
        let agent =
            support::TestAgent::in_dir(&t.ctx, "session-cwd", &session_cwd.to_string_lossy());
        let result = call_in(
            &t,
            &agent,
            "write",
            json!({ "file_path": "note.txt", "content": "hi" }),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        // Verify the WORLD: the file is in the session dir, and NOT in config.cwd.
        assert_eq!(
            std::fs::read_to_string(session_cwd.join("note.txt")).unwrap(),
            "hi"
        );
        assert!(!t.path("note.txt").exists());
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn read_and_edit_both_resolve_against_the_session_cwd_end_to_end() {
        let t = guarded().await;
        let session_dir = tempfile::tempdir().unwrap();
        let session_cwd = std::fs::canonicalize(session_dir.path()).unwrap();
        // ONE session object across both calls — observed-state keys by owner identity.
        let agent =
            support::TestAgent::in_dir(&t.ctx, "session-cwd-2", &session_cwd.to_string_lossy());
        std::fs::write(session_cwd.join("code.txt"), "alpha").unwrap();
        assert!(
            !call_in(&t, &agent, "read", json!({ "file_path": "code.txt" }))
                .await
                .is_error()
        );
        let edited = call_in(
            &t,
            &agent,
            "edit",
            json!({ "file_path": "code.txt", "old_string": "alpha", "new_string": "beta" }),
        )
        .await;
        assert!(!edited.is_error(), "{}", edited.text());
        assert_eq!(
            std::fs::read_to_string(session_cwd.join("code.txt")).unwrap(),
            "beta"
        );
        t.kernel.shutdown().await;
    }
}

mod signal_concurrency_and_observed_contract {
    use super::*;

    #[tokio::test]
    async fn a_pre_aborted_registry_call_skips_read_write_edit_with_aborted_before_dispatch() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "hello").unwrap();
        let aborted = || {
            let controller = AbortController::new();
            controller.abort();
            controller.signal()
        };
        let call_aborted = |name: &'static str, args: serde_json::Value| {
            let signal = aborted();
            let agent = t.agent.clone();
            let ctx = t.ctx.clone();
            async move { support::call_with_signal(&ctx, name, args, Some(agent), signal).await }
        };
        let read = call_aborted("read", json!({ "file_path": "a.txt" })).await;
        assert!(read.is_error());
        assert_eq!(
            error_info(&read),
            named("AbortError", hoh_tools::TOOL_ABORTED_BEFORE_DISPATCH)
        );
        let write = call_aborted("write", json!({ "file_path": "new.txt", "content": "x" })).await;
        assert!(write.is_error());
        assert_eq!(
            error_info(&write),
            named("AbortError", hoh_tools::TOOL_ABORTED_BEFORE_DISPATCH)
        );
        assert!(!t.path("new.txt").exists());
        // Read first (un-aborted, SAME session owner) so the edit clears the observation gate;
        // then the registry skips the aborted edit before its body.
        assert!(
            !t.call("read", json!({ "file_path": "a.txt" }))
                .await
                .is_error()
        );
        let edit = call_aborted(
            "edit",
            json!({ "file_path": "a.txt", "old_string": "hello", "new_string": "bye" }),
        )
        .await;
        assert!(edit.is_error());
        assert_eq!(
            error_info(&edit),
            named("AbortError", hoh_tools::TOOL_ABORTED_BEFORE_DISPATCH)
        );
        assert_eq!(t.bytes("a.txt").unwrap(), "hello"); // unchanged
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn two_concurrent_edits_of_the_same_file_same_session_one_wins_one_stale() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "base value here").unwrap();
        assert!(
            !t.call("read", json!({ "file_path": "a.txt" }))
                .await
                .is_error()
        );
        // Both guards must be captured before a successful edit refreshes the session's observed
        // version: the listener waits at a barrier of two.
        let captured: Arc<tokio::sync::Barrier> = Arc::new(tokio::sync::Barrier::new(2));
        let versions: Arc<std::sync::Mutex<Vec<Option<FsVersion>>>> = Arc::default();
        let seen = versions.clone();
        let barrier = captured.clone();
        let observed_calls = Arc::new(AtomicUsize::new(0));
        let counted = observed_calls.clone();
        let disposer = t
            .ctx
            .listen()
            .prepend()
            .on_waterfall::<FsEditIntent, Option<FsVersionGuard>, _>(move |_event, next| {
                let seen = seen.clone();
                let barrier = barrier.clone();
                let counted = counted.clone();
                async move {
                    let intent = next.run().await?;
                    support::lock(&seen).push(intent.as_ref().map(|guard| guard.version.clone()));
                    counted.fetch_add(1, Ordering::SeqCst);
                    barrier.wait().await;
                    Ok(intent)
                }
            })
            .unwrap();
        let first = t.call(
            "edit",
            json!({ "file_path": "a.txt", "old_string": "base", "new_string": "ONE", "replace_all": false }),
        );
        let second = t.call(
            "edit",
            json!({ "file_path": "a.txt", "old_string": "value", "new_string": "TWO", "replace_all": false }),
        );
        let results = tokio::join!(first, second);
        let results = [results.0, results.1];
        let versions = support::lock(&versions).clone();
        assert_eq!(versions.len(), 2);
        assert!(versions[0].is_some());
        assert_eq!(versions[1], versions[0]);
        let errors: Vec<_> = results.iter().filter(|result| result.is_error()).collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(code(errors[0]).as_deref(), Some("FS_STALE_VERSION"));
        // The world is consistent: exactly one edit landed.
        let on_disk = t.bytes("a.txt").unwrap();
        assert!(
            on_disk == "ONE value here" || on_disk == "base TWO here",
            "{on_disk}"
        );
        disposer.dispose().await;
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn a_stale_observed_version_from_an_older_read_fails_closed_at_edit_cas() {
        let t = guarded().await;
        std::fs::write(t.path("a.txt"), "older content\n").unwrap();
        let target = t.target("a.txt").await;
        let first = hoh_fs::FileSystem::stat(&*t.local, &target, None)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !t.call("read", json!({ "file_path": "a.txt" }))
                .await
                .is_error()
        );
        std::fs::write(t.path("a.txt"), "newer current content\n").unwrap();
        let second = hoh_fs::FileSystem::stat(&*t.local, &target, None)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(second.version, first.version);
        assert!(
            !t.call("read", json!({ "file_path": "a.txt" }))
                .await
                .is_error()
        );
        // Reproduce an older concurrent read winning the observation race.
        t.ctx.emit(&FsObserved {
            target,
            observation: FsObservation::Present {
                version: first.version,
            },
            actor: Some(t.exec.clone() as hoh_fs::FsActor),
        });
        let edit = t
            .call(
                "edit",
                json!({ "file_path": "a.txt", "old_string": "newer", "new_string": "edited" }),
            )
            .await;
        assert!(edit.is_error());
        assert_eq!(code(&edit).as_deref(), Some("FS_STALE_VERSION"));
        assert_eq!(t.bytes("a.txt").unwrap(), "newer current content\n");
        t.kernel.shutdown().await;
    }
}

mod current_working_directory {
    use super::*;

    #[tokio::test]
    async fn routes_subsequent_files_to_the_new_directory_while_keeping_the_original_root() {
        let t = guarded().await;
        let next = t.path("next");
        std::fs::create_dir(&next).unwrap();
        std::fs::write(t.path("note.txt"), "original").unwrap();
        std::fs::write(next.join("note.txt"), "current").unwrap();
        let agent = support::TestAgent::in_dir(
            &t.ctx,
            "current-directory-files",
            &t.workspace.to_string_lossy(),
        );
        let dyn_agent: Arc<dyn Agent> = agent.clone();
        support::ensure_live(&t.ctx, &dyn_agent);
        let owner = hoh_working_directory::live_agent(&t.ctx, &agent.session)
            .expect("the agent is registered");
        t.env
            .working_directory
            .set(&owner, &next.to_string_lossy(), None)
            .await
            .unwrap();
        let current = t.env.working_directory.get(&agent.session);
        assert_eq!(current, next.to_string_lossy());
        let read = support::call(
            &t.ctx,
            "read",
            json!({ "file_path": "note.txt" }),
            Some(dyn_agent.clone()),
        )
        .await;
        assert!(!read.is_error(), "{}", read.text());
        assert!(read.text().contains("current"));
        assert_eq!(read.meta().unwrap()["path"], format!("{current}/note.txt"));
        let write = support::call(
            &t.ctx,
            "write",
            json!({ "file_path": "created.txt", "content": "new" }),
            Some(dyn_agent.clone()),
        )
        .await;
        assert!(!write.is_error(), "{}", write.text());
        assert_eq!(
            write.meta(),
            Some(
                &json!({ "operation": "create", "path": format!("{current}/created.txt"), "diffs": [] })
            )
        );
        let edit = support::call(
            &t.ctx,
            "edit",
            json!({ "file_path": "created.txt", "old_string": "new", "new_string": "edited" }),
            Some(dyn_agent),
        )
        .await;
        assert!(!edit.is_error(), "{}", edit.text());
        assert_eq!(
            edit.meta().unwrap()["path"],
            format!("{current}/created.txt")
        );
        assert_eq!(
            edit.meta().unwrap()["diffs"][0]["path"],
            format!("{current}/created.txt")
        );
        assert_eq!(
            std::fs::read_to_string(next.join("created.txt")).unwrap(),
            "edited"
        );
        assert!(!t.path("created.txt").exists());
        t.kernel.shutdown().await;
    }
}
