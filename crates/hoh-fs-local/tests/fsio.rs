mod common;
use common::*;
use hoh_error::{BoxError, SharedError};
use hoh_fs::*;
use hoh_fs_local::fsio::*;
use hoh_timeout::AbortController;
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::fs;
async fn rt(e: &Env, path: &str) -> LocalTarget {
    resolve_local_target(e.dir.path().to_str().unwrap(), path)
        .await
        .unwrap()
}
fn p(e: &Env, name: &str) -> LocalTarget {
    target(e.path(name))
}
async fn clean(e: &Env) {
    let mut r = fs::read_dir(e.dir.path()).await.unwrap();
    while let Some(entry) = r.next_entry().await.unwrap() {
        assert!(!entry.file_name().to_string_lossy().contains(".tmp"));
    }
}
async fn write(
    e: &Env,
    name: &str,
    body: &str,
    hooks: &FsIoInternals,
    options: AtomicWriteOptions,
) -> Result<(), BoxError> {
    write_file_atomic(&e.path(name), body, None, None, hooks, options).await
}
mod resolution {
    use super::*;
    #[tokio::test]
    async fn resolves_a_relative_path_from_cwd_and_realpaths_it() {
        let e = Env::new();
        let t = e.file("a.txt", "hi").await;
        let r = rt(&e, "a.txt").await;
        assert_eq!(r.display_path, e.path("a.txt"));
        assert_eq!(r.target_key, t.target_key);
    }
    #[tokio::test]
    async fn uses_the_realpathed_parent_and_basename_when_the_file_does_not_exist_stable_across_create()
     {
        let e = Env::new();
        let t = rt(&e, "missing.txt").await;
        assert_eq!(t.target_key.as_str(), e.path("missing.txt"));
        e.file("missing.txt", "x").await;
        assert_eq!(rt(&e, "missing.txt").await.target_key, t.target_key);
    }
    #[tokio::test]
    async fn two_paths_to_the_same_file_via_a_symlink_share_one_target_key() {
        let e = Env::new();
        e.file("real.txt", "hi").await;
        fs::symlink(e.path("real.txt"), e.path("link.txt"))
            .await
            .unwrap();
        let r = rt(&e, "link.txt").await;
        assert_eq!(r.target_key, rt(&e, "real.txt").await.target_key);
        assert_eq!(r.display_path, e.path("link.txt"));
    }
    #[tokio::test]
    async fn realpaths_the_nearest_existing_ancestor_when_intermediate_dirs_are_missing() {
        let e = Env::new();
        assert_eq!(
            rt(&e, "no-such-dir/child.txt").await.target_key.as_str(),
            e.path("no-such-dir/child.txt")
        );
    }
    #[tokio::test]
    async fn keeps_the_key_stable_across_create_when_an_ancestor_is_a_symlink() {
        let e = Env::new();
        fs::create_dir(e.path("real-root")).await.unwrap();
        fs::symlink(e.path("real-root"), e.path("link-root"))
            .await
            .unwrap();
        let t = rt(&e, "link-root/sub/file.txt").await;
        fs::create_dir(e.path("real-root/sub")).await.unwrap();
        fs::write(e.path("real-root/sub/file.txt"), "hi")
            .await
            .unwrap();
        assert_eq!(
            t.target_key,
            rt(&e, "link-root/sub/file.txt").await.target_key
        );
    }
    #[tokio::test]
    async fn rejects_a_blank_path() {
        let e = Env::new();
        error(
            resolve_local_target(e.dir.path().to_str().unwrap(), "   ").await,
            "FS_NOT_FOUND",
        );
    }
    #[tokio::test]
    async fn anchors_a_relative_cwd_before_retaining_parent_traversal_in_a_display_path() {
        let e = Env::new();
        fs::create_dir(e.path("nested")).await.unwrap();
        let cwd = hoh_util_workspace_path::node_path::relative(
            "/",
            std::env::current_dir().unwrap().to_str().unwrap(),
            e.dir.path().to_str().unwrap(),
        );
        let t = resolve_local_target(&cwd, "nested/../created.txt")
            .await
            .unwrap();
        assert!(t.display_path.starts_with('/'));
        assert_eq!(t.target_key.as_str(), e.path("created.txt"));
        assert_eq!(
            resolve_local_target(&cwd, &t.display_path)
                .await
                .unwrap()
                .target_key,
            t.target_key
        );
    }
    #[tokio::test]
    async fn rejects_parent_traversal_through_a_missing_directory_hierarchy() {
        let e = Env::new();
        error(
            resolve_local_target(
                e.dir.path().to_str().unwrap(),
                "missing/deeper/../created.txt",
            )
            .await,
            "FS_NOT_FOUND",
        );
    }
    #[tokio::test]
    async fn resolves_parent_traversal_after_a_symlink_in_the_provider_filesystem() {
        let e = Env::new();
        fs::create_dir_all(e.path("physical/nested")).await.unwrap();
        fs::create_dir(e.path("lexical")).await.unwrap();
        fs::symlink(e.path("physical/nested"), e.path("lexical/link"))
            .await
            .unwrap();
        let t = resolve_local_target(&format!("{}/..", e.path("lexical/link")), "created.txt")
            .await
            .unwrap();
        assert_eq!(t.target_key.as_str(), e.path("physical/created.txt"));
        e.file("physical/created.txt", "physical").await;
        assert_eq!(
            rt(&e, "lexical/link/../created.txt").await.target_key,
            t.target_key
        );
        error(
            resolve_local_target(e.dir.path().to_str().unwrap(), "missing/../created.txt").await,
            "FS_NOT_FOUND",
        );
    }
    #[tokio::test]
    async fn rejects_a_path_whose_ancestor_is_a_file_with_a_structured_fs_error_enotdir() {
        let e = Env::new();
        e.file("afile", "i am a file").await;
        error(
            resolve_local_target(e.dir.path().to_str().unwrap(), "afile/child.txt").await,
            "FS_NOT_FOUND",
        );
    }
}
mod metadata {
    use super::*;
    #[tokio::test]
    async fn returns_null_for_a_missing_path_and_metadata_for_a_file() {
        let e = Env::new();
        assert!(probe(&e.path("nope")).await.unwrap().is_none());
        e.file("a.txt", "hi").await;
        let i = probe(&e.path("a.txt")).await.unwrap().unwrap();
        assert_eq!(i.r#type, FsType::File);
        assert_eq!(i.size, 2);
        assert!(!i.version.as_str().is_empty());
    }
    #[tokio::test]
    async fn reports_a_directory_and_a_non_regular_type() {
        let e = Env::new();
        fs::create_dir(e.path("sub")).await.unwrap();
        assert_eq!(
            probe(&e.path("sub")).await.unwrap().unwrap().r#type,
            FsType::Directory
        );
    }
    #[tokio::test]
    async fn reports_a_socket_special_file_as_type_other() {
        let e = Env::new();
        let socket = match std::os::unix::net::UnixDatagram::bind(e.path("sock")) {
            Ok(s) => s,
            Err(e) if matches!(e.raw_os_error(), Some(1 | 13 | 95)) => return,
            Err(e) => panic!("{e}"),
        };
        assert_eq!(
            probe(&e.path("sock")).await.unwrap().unwrap().r#type,
            FsType::Other
        );
        drop(socket);
    }
    #[tokio::test]
    async fn returns_null_when_an_ancestor_path_segment_is_a_file_enotdir_not_a_raw_throw() {
        let e = Env::new();
        e.file("afile", "i am a file").await;
        assert!(probe(&e.path("afile/child.txt")).await.unwrap().is_none());
    }
    #[tokio::test]
    async fn reports_symlinks_without_following_them() {
        let e = Env::new();
        e.file("real.txt", "hi").await;
        fs::symlink(e.path("real.txt"), e.path("link.txt"))
            .await
            .unwrap();
        assert_eq!(
            probe_no_follow(&e.path("real.txt"))
                .await
                .unwrap()
                .unwrap()
                .r#type,
            FsPathType::File
        );
        let i = probe_no_follow(&e.path("link.txt")).await.unwrap().unwrap();
        assert_eq!(i.r#type, FsPathType::Symlink);
        assert!(i.size > 0);
        assert!(!i.version.as_str().is_empty());
    }
    #[tokio::test]
    async fn returns_null_for_a_missing_path_or_a_file_valued_ancestor_path_segment() {
        let e = Env::new();
        assert!(probe_no_follow(&e.path("missing")).await.unwrap().is_none());
        e.file("afile", "x").await;
        assert!(
            probe_no_follow(&e.path("afile/child.txt"))
                .await
                .unwrap()
                .is_none()
        );
    }
}
mod listing {
    use super::*;
    #[tokio::test]
    async fn preserves_physical_parent_traversal_in_displayed_child_paths() {
        let e = Env::new();
        fs::create_dir_all(e.path("physical/nested")).await.unwrap();
        fs::create_dir(e.path("lexical")).await.unwrap();
        fs::symlink(e.path("physical/nested"), e.path("lexical/link"))
            .await
            .unwrap();
        e.file("physical/same.txt", "physical file").await;
        e.file("lexical/same.txt", "lexical file").await;
        let parent = rt(&e, "lexical/link/..").await;
        let entries = list_directory(&parent, None).await.unwrap();
        let c = entries.iter().find(|c| c.name == "same.txt").unwrap();
        assert_eq!(
            fs::read_to_string(&c.target.display_path).await.unwrap(),
            "physical file"
        );
        assert_eq!(
            rt(&e, &c.target.display_path).await.target_key,
            c.target.target_key
        );
    }
    #[tokio::test]
    async fn lists_direct_children_in_stable_order_without_reading_content() {
        let e = Env::new();
        fs::create_dir_all(e.path("skills/dir-skill"))
            .await
            .unwrap();
        e.file("skills/zeta.md", "zeta").await;
        e.file("skills/alpha.md", "alpha").await;
        fs::symlink(
            e.path("skills/missing-target"),
            e.path("skills/broken-link"),
        )
        .await
        .unwrap();
        let entries = list_directory(&p(&e, "skills"), None).await.unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|c| (c.name.as_str(), c.r#type))
                .collect::<Vec<_>>(),
            vec![
                ("alpha.md", FsType::File),
                ("broken-link", FsType::Other),
                ("dir-skill", FsType::Directory),
                ("zeta.md", FsType::File)
            ]
        );
        assert_eq!(entries[0].size, Some(5));
        assert!(entries[0].version.is_some());
        assert!(entries[1].version.is_none());
        assert!(entries[2].size.is_none());
    }
    #[tokio::test]
    async fn derives_child_target_keys_from_the_listed_parent_identity() {
        let e = Env::new();
        fs::create_dir(e.path("real-one")).await.unwrap();
        fs::create_dir(e.path("real-two")).await.unwrap();
        e.file("real-one/same.txt", "one").await;
        e.file("real-two/same.txt", "different two").await;
        fs::symlink(e.path("real-one"), e.path("link"))
            .await
            .unwrap();
        let t = rt(&e, "link").await;
        fs::remove_file(e.path("link")).await.unwrap();
        fs::symlink(e.path("real-two"), e.path("link"))
            .await
            .unwrap();
        let entries = list_directory(&t, None).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "same.txt");
        assert_eq!(entries[0].target.display_path, e.path("link/same.txt"));
        assert_eq!(
            entries[0].target.target_key.as_str(),
            e.path("real-one/same.txt")
        );
        assert_eq!(entries[0].size, Some(3));
    }
    #[tokio::test]
    async fn rejects_missing_non_directory_and_aborted_listing_requests() {
        let e = Env::new();
        error(
            list_directory(&p(&e, "missing"), None).await,
            "FS_NOT_FOUND",
        );
        e.file("a.txt", "hi").await;
        error(
            list_directory(&p(&e, "a.txt"), None).await,
            "FS_NOT_DIRECTORY",
        );
        error(
            list_directory(&p(&e, ""), Some(&aborted())).await,
            "FS_ABORTED",
        );
    }
    #[tokio::test]
    async fn translates_directory_permission_failures_into_fs_permission_denied() {
        let e = Env::new();
        fs::create_dir(e.path("restricted")).await.unwrap();
        fs::set_permissions(e.path("restricted"), std::fs::Permissions::from_mode(0o0))
            .await
            .unwrap();
        let r = list_directory(&p(&e, "restricted"), None).await;
        fs::set_permissions(e.path("restricted"), std::fs::Permissions::from_mode(0o700))
            .await
            .unwrap();
        if r.is_err() {
            error(r, "FS_PERMISSION_DENIED");
        }
    }
    #[tokio::test]
    async fn translates_preflight_metadata_io_failures_into_fs_io_error() {
        let e = Env::new();
        fs::symlink(e.path("loop"), e.path("loop")).await.unwrap();
        error(list_directory(&p(&e, "loop"), None).await, "FS_IO_ERROR");
    }
    #[tokio::test]
    async fn translates_child_resolution_failures_into_structured_listing_errors() {
        let e = Env::new();
        fs::create_dir(e.path("listed")).await.unwrap();
        fs::symlink(e.path("listed/loop"), e.path("listed/loop"))
            .await
            .unwrap();
        error(list_directory(&p(&e, "listed"), None).await, "FS_IO_ERROR");
    }
    #[tokio::test]
    async fn translates_child_permission_failures_into_fs_permission_denied() {
        let e = Env::new();
        fs::create_dir(e.path("listed")).await.unwrap();
        fs::create_dir_all(e.path("protected/secret"))
            .await
            .unwrap();
        fs::symlink(e.path("protected/secret"), e.path("listed/secret-link"))
            .await
            .unwrap();
        fs::set_permissions(e.path("protected"), std::fs::Permissions::from_mode(0o0))
            .await
            .unwrap();
        let r = list_directory(&p(&e, "listed"), None).await;
        fs::set_permissions(e.path("protected"), std::fs::Permissions::from_mode(0o700))
            .await
            .unwrap();
        if r.is_err() {
            error(r, "FS_PERMISSION_DENIED");
        }
    }
}
mod whole_text {
    use super::*;
    #[tokio::test]
    async fn reads_a_small_file() {
        let e = Env::new();
        let t = e.file("a.txt", "one\ntwo\nthree").await;
        assert_eq!(read_whole_text(&t, None).await.unwrap(), "one\ntwo\nthree");
    }
    #[tokio::test]
    async fn rejects_a_missing_file_and_a_directory() {
        let e = Env::new();
        error(read_whole_text(&p(&e, "nope"), None).await, "FS_NOT_FOUND");
        error(
            read_whole_text(&p(&e, ""), None).await,
            "FS_NOT_REGULAR_FILE",
        );
    }
    #[tokio::test]
    async fn rejects_binary_and_invalid_utf8() {
        let e = Env::new();
        for (name, bytes) in [("bin", [0x68, 0, 0x69]), ("bad", [0x68, 0xff, 0x69])] {
            let t = e.file(name, bytes).await;
            error(read_whole_text(&t, None).await, "FS_NOT_TEXT");
        }
    }
    #[tokio::test]
    async fn honors_a_pre_aborted_signal() {
        let e = Env::new();
        let t = e.file("a.txt", "one").await;
        error(read_whole_text(&t, Some(&aborted())).await, "FS_ABORTED");
    }
    #[tokio::test]
    async fn passes_a_live_non_aborted_signal_through() {
        let e = Env::new();
        let t = e.file("a.txt", "one\ntwo").await;
        assert_eq!(
            read_whole_text(&t, Some(&AbortController::new().signal()))
                .await
                .unwrap(),
            "one\ntwo"
        );
    }
    #[test]
    fn translates_a_mid_read_abort_error_into_fs_aborted() {
        isolated_io_test(async {
            let e = Env::new();
            let t = e.file("a.txt", "one\ntwo").await;
            let c = Arc::new(AbortController::new());
            let s = c.signal();
            let hold = hold_io().await;
            let f = read_whole_text(&t, Some(&s));
            tokio::pin!(f);
            assert!(poll_once(f.as_mut()).await.is_pending());
            c.abort();
            drop(hold);
            error(f.await, "FS_ABORTED");
        });
    }
}
mod diff {
    use super::*;
    #[tokio::test]
    async fn returns_normalized_text_only_when_the_opened_file_is_strictly_below_the_limit() {
        let e = Env::new();
        e.file("basis.txt", "a\r\nb").await;
        assert_eq!(
            read_text_for_diff(&e.path("basis.txt"), 5, None)
                .await
                .unwrap()
                .as_deref(),
            Some("a\nb")
        );
        assert!(
            read_text_for_diff(&e.path("basis.txt"), 4, None)
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn bounds_the_actual_opened_file_rather_than_trusting_an_earlier_path_size() {
        let e = Env::new();
        e.file("replaced.txt", "tiny").await;
        assert!(fs::metadata(e.path("replaced.txt")).await.unwrap().len() < 8);
        e.file("replaced.txt", "123456789").await;
        assert!(
            read_text_for_diff(&e.path("replaced.txt"), 8, None)
                .await
                .unwrap()
                .is_none()
        );
    }
    async fn resize(body: &'static str) {
        let e = Env::new();
        e.file("size.txt", "abcdef").await;
        let h = FsIoInternals {
            after_diff_stat: Some(Arc::new(move |path| {
                Box::pin(async move {
                    fs::write(path, body).await?;
                    Ok(())
                })
            })),
            ..Default::default()
        };
        assert!(
            read_text_for_diff_with_internals(&e.path("size.txt"), 32, None, &h)
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn returns_null_when_the_opened_file_shrinks_after_descriptor_stat() {
        resize("abc").await;
    }
    #[tokio::test]
    async fn returns_null_when_the_opened_file_grows_after_descriptor_stat() {
        resize("abcdef-grown").await;
    }
    #[tokio::test]
    async fn returns_null_when_the_file_vanishes_before_the_basis_open_deletion_race() {
        let e = Env::new();
        assert!(
            read_text_for_diff(&e.path("deleted-after-preflight.txt"), 32, None)
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn returns_null_when_the_opened_descriptor_is_no_longer_a_regular_file() {
        let e = Env::new();
        e.file("swapped.txt", "abcdef").await;
        let h = FsIoInternals {
            diff_metadata: Some(Arc::new(|_| {
                Box::pin(async {
                    Ok(PathInfo {
                        version: "v".into(),
                        mode: 0o600,
                        r#type: FsType::Other,
                        size: 6,
                    })
                })
            })),
            ..Default::default()
        };
        assert!(
            read_text_for_diff_with_internals(&e.path("swapped.txt"), 32, None, &h)
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn propagates_a_non_errno_fault_instead_of_masking_it_as_a_null_basis() {
        let e = Env::new();
        e.file("faulted.txt", "abcdef").await;
        let sentinel = SharedError::new(std::io::Error::other("forged programming fault"));
        let s = sentinel.clone();
        let h = FsIoInternals {
            before_operation: Some(Arc::new(move |_, _| Err(s.clone().into()))),
            ..Default::default()
        };
        let err = read_text_for_diff_with_internals(&e.path("faulted.txt"), 32, None, &h)
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<SharedError>().unwrap().ptr_eq(&sentinel));
    }
    #[tokio::test]
    async fn returns_null_for_binary_and_invalid_utf8_without_blocking_the_caller_write() {
        let e = Env::new();
        for (name, bytes) in [("bin", [0x68, 0, 0x69]), ("bad", [0x68, 0xff, 0x69])] {
            e.file(name, bytes).await;
            assert!(
                read_text_for_diff(&e.path(name), 8, None)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }
    #[tokio::test]
    async fn honors_a_pre_aborted_signal() {
        let e = Env::new();
        e.file("basis.txt", "text").await;
        error(
            read_text_for_diff(&e.path("basis.txt"), 8, Some(&aborted())).await,
            "FS_ABORTED",
        );
    }
    #[tokio::test]
    async fn observes_cancellation_immediately_after_stage() {
        for stage in ["open", "stat"] {
            let e = Env::new();
            e.file("basis.txt", "text").await;
            let c = Arc::new(AbortController::new());
            let s = c.signal();
            let allocations = Arc::new(AtomicUsize::new(0));
            let stat_calls = Arc::new(AtomicUsize::new(0));
            let a = allocations.clone();
            let n = stat_calls.clone();
            let mut h = FsIoInternals {
                inspect_diff_allocate: Some(Arc::new(move |_| {
                    a.fetch_add(1, Ordering::SeqCst);
                })),
                diff_metadata: Some(Arc::new(move |_| {
                    n.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async {
                        Ok(PathInfo {
                            version: "v".into(),
                            mode: 0o600,
                            r#type: FsType::File,
                            size: 4,
                        })
                    })
                })),
                ..Default::default()
            };
            let checkpoint: PathHook = Arc::new(move |_| {
                c.abort();
                Box::pin(async { Ok(()) })
            });
            if stage == "open" {
                h.after_diff_open = Some(checkpoint);
            } else {
                h.after_diff_stat = Some(checkpoint);
            }
            error(
                read_text_for_diff_with_internals(&e.path("basis.txt"), 8, Some(&s), &h).await,
                "FS_ABORTED",
            );
            assert_eq!(
                stat_calls.load(Ordering::SeqCst),
                usize::from(stage == "stat")
            );
            assert_eq!(allocations.load(Ordering::SeqCst), 0);
        }
    }
    #[tokio::test]
    async fn bounds_descriptor_reads_and_observes_cancellation_before_the_next_chunk() {
        let e = Env::new();
        e.file("large-basis.txt", "x".repeat(200 * 1024)).await;
        let c = Arc::new(AbortController::new());
        let s = c.signal();
        let lengths = Arc::new(Mutex::new(vec![]));
        let l = lengths.clone();
        let h = FsIoInternals {
            inspect_diff_read: Some(Arc::new(move |n| l.lock().unwrap().push(n))),
            after_diff_read: Some(Arc::new(move |_| {
                c.abort();
                Box::pin(async { Ok(()) })
            })),
            ..Default::default()
        };
        error(
            read_text_for_diff_with_internals(
                &e.path("large-basis.txt"),
                200 * 1024 + 1,
                Some(&s),
                &h,
            )
            .await,
            "FS_ABORTED",
        );
        assert_eq!(*lengths.lock().unwrap(), vec![64 * 1024]);
    }
}
mod streaming {
    use super::*;
    #[tokio::test]
    async fn streams_the_whole_file_as_decoded_text() {
        let e = Env::new();
        let t = e.file("a.txt", "one\ntwo\nthree").await;
        assert_eq!(
            collect(stream_whole_text(t, None)).await.unwrap(),
            "one\ntwo\nthree"
        );
    }
    #[tokio::test]
    async fn streams_a_large_multi_chunk_file_correctly() {
        let e = Env::new();
        let content = (0..50)
            .map(|i| format!("line {i}: {}", "x".repeat(3000)))
            .collect::<Vec<_>>()
            .join("\n");
        let t = e.file("big.txt", &content).await;
        assert_eq!(collect(stream_whole_text(t, None)).await.unwrap(), content);
    }
    #[tokio::test]
    async fn rejects_a_missing_file_directory_binary_and_invalid_utf8() {
        let e = Env::new();
        error(
            collect(stream_whole_text(p(&e, "nope"), None)).await,
            "FS_NOT_FOUND",
        );
        error(
            collect(stream_whole_text(p(&e, ""), None)).await,
            "FS_NOT_REGULAR_FILE",
        );
        for (name, bytes) in [("bin", [0x68, 0, 0x69]), ("bad", [0x68, 0xff, 0x69])] {
            let t = e.file(name, bytes).await;
            error(collect(stream_whole_text(t, None)).await, "FS_NOT_TEXT");
        }
    }
    #[tokio::test]
    async fn honors_a_pre_aborted_signal() {
        let e = Env::new();
        let t = e.file("a.txt", "one").await;
        error(
            collect(stream_whole_text(t, Some(aborted()))).await,
            "FS_ABORTED",
        );
    }
    #[tokio::test]
    async fn passes_a_live_non_aborted_signal_through_the_stream() {
        let e = Env::new();
        let t = e.file("a.txt", "one\ntwo").await;
        assert_eq!(
            collect(stream_whole_text(t, Some(AbortController::new().signal())))
                .await
                .unwrap(),
            "one\ntwo"
        );
    }
    #[tokio::test]
    async fn translates_a_mid_stream_abort_into_fs_aborted() {
        use futures_util::StreamExt;
        let e = Env::new();
        let t = e.file("big.txt", "x".repeat(256 * 1024)).await;
        let c = Arc::new(AbortController::new());
        let mut stream = stream_whole_text(t, Some(c.signal()));
        assert!(!stream.next().await.unwrap().unwrap().is_empty());
        c.abort();
        error(stream.next().await.unwrap(), "FS_ABORTED");
    }
}
mod atomic {
    use super::*;
    #[tokio::test]
    async fn writes_through_a_private_staging_dir_and_owner_only_temp_file() {
        let e = Env::new();
        e.file("a.txt", "old").await;
        fs::set_permissions(e.path("a.txt"), std::fs::Permissions::from_mode(0o640))
            .await
            .unwrap();
        let inspected = Arc::new(AtomicBool::new(false));
        let i = inspected.clone();
        let h = FsIoInternals {
            inspect_temp: Some(Arc::new(move |s| {
                let i = i.clone();
                Box::pin(async move {
                    let d = fs::metadata(s.staging_dir).await?;
                    let t = fs::metadata(s.temp_path).await?;
                    assert!(d.is_dir() && t.is_file());
                    assert_eq!(d.permissions().mode() & 0o777, 0o700);
                    assert_eq!(t.permissions().mode() & 0o777, 0o600);
                    i.store(true, Ordering::SeqCst);
                    Ok(())
                })
            })),
            ..Default::default()
        };
        write_file_atomic(
            &e.path("a.txt"),
            "hello",
            Some(0o640),
            None,
            &h,
            Default::default(),
        )
        .await
        .unwrap();
        assert!(inspected.load(Ordering::SeqCst));
        assert_eq!(e.bytes("a.txt").await, "hello");
        assert_eq!(
            fs::metadata(e.path("a.txt"))
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        clean(&e).await;
    }
    #[tokio::test]
    async fn maps_a_non_collision_guarded_create_publication_failure_and_cleans_staging() {
        let e = Env::new();
        let h = FsIoInternals {
            link_file: Some(Arc::new(|_, _| {
                Box::pin(async { Err(errno("EACCES", "link denied")) })
            })),
            ..Default::default()
        };
        let err = error(
            write(
                &e,
                "a.txt",
                "ours",
                &h,
                AtomicWriteOptions {
                    create_if_absent: true,
                    ..Default::default()
                },
            )
            .await,
            "FS_IO_ERROR",
        );
        assert_eq!(
            hoh_error::HarnessError::of(err.as_ref())
                .unwrap()
                .cause()
                .unwrap()
                .to_string(),
            "link denied"
        );
        assert!(!fs::try_exists(e.path("a.txt")).await.unwrap());
        clean(&e).await;
    }
    #[tokio::test]
    async fn maps_a_guarded_create_target_inspection_failure_and_cleans_staging() {
        let e = Env::new();
        let h = FsIoInternals {
            link_file: Some(Arc::new(|_, _| {
                Box::pin(async { Err(errno("EIO", "link failed")) })
            })),
            inspect_publication_target: Some(Arc::new(|_| {
                Box::pin(async { Err(errno("EACCES", "inspection denied")) })
            })),
            ..Default::default()
        };
        let err = error(
            write(
                &e,
                "a.txt",
                "ours",
                &h,
                AtomicWriteOptions {
                    create_if_absent: true,
                    ..Default::default()
                },
            )
            .await,
            "FS_IO_ERROR",
        );
        assert_eq!(
            hoh_error::HarnessError::of(err.as_ref())
                .unwrap()
                .cause()
                .unwrap()
                .to_string(),
            "inspection denied"
        );
        assert!(!fs::try_exists(e.path("a.txt")).await.unwrap());
        clean(&e).await;
    }
    #[tokio::test]
    async fn rejects_a_guarded_create_collision_that_vanishes_before_inspection() {
        let e = Env::new();
        let h = FsIoInternals {
            link_file: Some(Arc::new(|_, _| {
                Box::pin(async { Err(errno("EEXIST", "target existed")) })
            })),
            ..Default::default()
        };
        let err = error(
            write(
                &e,
                "a.txt",
                "ours",
                &h,
                AtomicWriteOptions {
                    create_if_absent: true,
                    ..Default::default()
                },
            )
            .await,
            "FS_NOT_OBSERVED",
        );
        assert_eq!(
            err.to_string(),
            format!(
                "cannot overwrite existing \"{}\" without reading it first",
                e.path("a.txt")
            )
        );
        assert!(!fs::try_exists(e.path("a.txt")).await.unwrap());
        clean(&e).await;
    }
    #[tokio::test]
    async fn uses_the_display_path_and_target_type_when_guarded_publication_finds_a_competitor() {
        for dir in [false, true] {
            let e = Env::new();
            let path = e.path("a.txt");
            let display = e.path("linked-workspace/a.txt");
            let h = FsIoInternals {
                inspect_temp: Some(Arc::new(move |_| {
                    let path = path.clone();
                    Box::pin(async move {
                        if dir {
                            fs::create_dir(path).await?;
                        } else {
                            fs::write(path, "competitor").await?;
                        }
                        Ok(())
                    })
                })),
                ..Default::default()
            };
            let err = error(
                write(
                    &e,
                    "a.txt",
                    "ours",
                    &h,
                    AtomicWriteOptions {
                        display_path: Some(display.clone()),
                        create_if_absent: true,
                        ..Default::default()
                    },
                )
                .await,
                if dir {
                    "FS_NOT_REGULAR_FILE"
                } else {
                    "FS_NOT_OBSERVED"
                },
            );
            assert_eq!(
                err.to_string(),
                if dir {
                    format!("cannot write \"{display}\": not a regular file")
                } else {
                    format!("cannot overwrite existing \"{display}\" without reading it first")
                }
            );
            if dir {
                assert!(fs::metadata(e.path("a.txt")).await.unwrap().is_dir());
            } else {
                assert_eq!(e.bytes("a.txt").await, "competitor");
            }
        }
    }
    #[tokio::test]
    async fn does_not_turn_post_commit_staging_cleanup_failure_into_a_failed_guarded_write() {
        let e = Env::new();
        let h = FsIoInternals {
            remove_staging_dir: Some(Arc::new(|_| {
                Box::pin(async { Err(std::io::Error::other("staging cleanup failed").into()) })
            })),
            ..Default::default()
        };
        write(
            &e,
            "a.txt",
            "ours",
            &h,
            AtomicWriteOptions {
                create_if_absent: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(e.bytes("a.txt").await, "ours");
    }
    #[tokio::test]
    async fn creates_new_files_owner_only_by_default() {
        let e = Env::new();
        write(
            &e,
            "a.txt",
            "hello",
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            fs::metadata(e.path("a.txt"))
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[tokio::test]
    async fn opens_staging_paths_exclusively_a_pre_existing_path_is_never_clobbered() {
        let e = Env::new();
        fs::create_dir(e.path(".fixed-temp.tmpdir")).await.unwrap();
        e.file(".fixed-temp.tmpdir/PRECIOUS", "keep").await;
        let h = FsIoInternals {
            temp_dir_name: Some(Arc::new(|_| Ok(".fixed-temp.tmpdir".into()))),
            ..Default::default()
        };
        let err = error(
            write(&e, "a.txt", "hello", &h, Default::default()).await,
            "FS_IO_ERROR",
        );
        assert_eq!(
            hoh_error::HarnessError::of(err.as_ref())
                .unwrap()
                .cause()
                .unwrap()
                .downcast_ref::<ErrnoError>()
                .unwrap()
                .code
                .as_deref(),
            Some("EEXIST")
        );
        assert_eq!(e.bytes(".fixed-temp.tmpdir/PRECIOUS").await, "keep");
        assert!(!fs::try_exists(e.path("a.txt")).await.unwrap());
    }
    #[tokio::test]
    async fn creates_parent_directories_as_needed() {
        let e = Env::new();
        write(
            &e,
            "nested/deep/a.txt",
            "hi",
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.bytes("nested/deep/a.txt").await, "hi");
    }
    #[tokio::test]
    async fn passes_a_live_non_aborted_signal_through_the_write() {
        let e = Env::new();
        write_file_atomic(
            &e.path("a.txt"),
            "hi",
            None,
            Some(&AbortController::new().signal()),
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.bytes("a.txt").await, "hi");
    }
    #[tokio::test]
    async fn aborts_before_writing_when_the_signal_is_already_aborted() {
        let e = Env::new();
        error(
            write_file_atomic(
                &e.path("a.txt"),
                "hi",
                None,
                Some(&aborted()),
                &Default::default(),
                Default::default(),
            )
            .await,
            "FS_ABORTED",
        );
        assert!(!fs::try_exists(e.path("a.txt")).await.unwrap());
    }
    #[tokio::test]
    async fn cleans_up_the_temp_file_when_the_final_rename_fails() {
        let e = Env::new();
        fs::create_dir(e.path("occupied")).await.unwrap();
        assert!(
            write(
                &e,
                "occupied",
                "hi",
                &Default::default(),
                Default::default()
            )
            .await
            .is_err()
        );
        clean(&e).await;
    }
}
mod literal {
    use super::*;
    #[test]
    fn replaces_a_unique_match() {
        assert_eq!(
            apply_literal_edit("a b c", "b", "X", false, "f").unwrap(),
            LiteralEdit {
                content: "a X c".into(),
                replacements: 1
            }
        );
    }
    #[test]
    fn rejects_zero_matches() {
        assert_eq!(
            apply_literal_edit("a b c", "z", "X", false, "f")
                .unwrap_err()
                .code(),
            "FS_EDIT_NOT_FOUND"
        );
    }
    #[test]
    fn rejects_an_empty_old_string_without_scanning_forever() {
        assert_eq!(
            apply_literal_edit("a b c", "", "X", false, "f")
                .unwrap_err()
                .code(),
            "FS_EDIT_NOT_FOUND"
        );
    }
    #[test]
    fn rejects_multiple_matches_without_replace_all() {
        assert_eq!(
            apply_literal_edit("a a a", "a", "X", false, "f")
                .unwrap_err()
                .code(),
            "FS_AMBIGUOUS_EDIT"
        );
    }
    #[test]
    fn replaces_all_matches_with_replace_all() {
        assert_eq!(
            apply_literal_edit("a a a", "a", "X", true, "f").unwrap(),
            LiteralEdit {
                content: "X X X".into(),
                replacements: 3
            }
        );
    }
    #[test]
    fn matches_across_normalized_line_endings() {
        assert_eq!(
            apply_literal_edit("one\ntwo", "one\ntwo", "x", false, "f")
                .unwrap()
                .replacements,
            1
        );
    }
}
mod editing {
    use super::*;
    #[tokio::test]
    async fn round_trips_crlf_matches_on_lf_writes_back_crlf() {
        let e = Env::new();
        e.file("crlf.txt", "one\r\ntwo\r\n").await;
        let original = read_for_edit(&e.path("crlf.txt"), &e.path("crlf.txt"), None)
            .await
            .unwrap();
        assert_eq!(original.line_endings, LineEndings::CRLF);
        let edited = apply_literal_edit(&original.content, "two", "TWO", false, "f").unwrap();
        assert_eq!(
            restore_line_endings(&edited.content, original.line_endings),
            "one\r\nTWO\r\n"
        );
    }
    #[tokio::test]
    async fn rejects_a_binary_file_and_invalid_utf8() {
        let e = Env::new();
        for (name, bytes) in [("bin", [0x68, 0, 0x69]), ("bad", [0x68, 0xff, 0x69])] {
            e.file(name, bytes).await;
            error(
                read_for_edit(&e.path(name), &e.path(name), None).await,
                "FS_NOT_TEXT",
            );
        }
    }
    #[tokio::test]
    async fn classifies_a_missing_file_and_a_directory_read_for_editing() {
        let e = Env::new();
        let a = error(
            read_for_edit(&e.path("missing"), "~/missing", None).await,
            "FS_NOT_FOUND",
        );
        assert_eq!(a.to_string(), "cannot edit \"~/missing\": not found");
        let b = error(
            read_for_edit(e.dir.path().to_str().unwrap(), "~/dir", None).await,
            "FS_NOT_REGULAR_FILE",
        );
        assert_eq!(b.to_string(), "cannot edit \"~/dir\": not a regular file");
    }
    #[tokio::test]
    async fn passes_a_live_non_aborted_signal_through_the_read() {
        let e = Env::new();
        e.file("a.txt", "one\ntwo").await;
        assert_eq!(
            read_for_edit(
                &e.path("a.txt"),
                &e.path("a.txt"),
                Some(&AbortController::new().signal())
            )
            .await
            .unwrap()
            .content,
            "one\ntwo"
        );
    }
    #[test]
    fn translates_a_mid_read_abort_error_into_fs_aborted() {
        isolated_io_test(async {
            let e = Env::new();
            e.file("a.txt", "one\ntwo").await;
            let c = Arc::new(AbortController::new());
            let s = c.signal();
            let path = e.path("a.txt");
            let hold = hold_io().await;
            let f = read_for_edit(&path, &path, Some(&s));
            tokio::pin!(f);
            assert!(poll_once(f.as_mut()).await.is_pending());
            c.abort();
            drop(hold);
            error(f.await, "FS_ABORTED");
        });
    }
}
mod failures {
    use super::*;
    #[tokio::test]
    async fn native_removal_keeps_node_lstat_and_directory_error_texts() {
        let e = Env::new();
        e.file("blocker", "file").await;
        for (path, code, message) in [
            (e.path("missing"), "ENOENT", "no such file or directory"),
            (e.path("blocker/child"), "ENOTDIR", "not a directory"),
        ] {
            let err = error(
                remove_file(&path, "shown", &Default::default()).await,
                "FS_IO_ERROR",
            );
            assert_eq!(
                err.to_string(),
                format!("cannot delete \"shown\": {code}: {message}, lstat '{path}'")
            );
            let cause = hoh_error::HarnessError::of(err.as_ref())
                .unwrap()
                .cause()
                .unwrap()
                .downcast_ref::<ErrnoError>()
                .unwrap();
            assert_eq!(cause.code.as_deref(), Some(code));
        }
        fs::create_dir(e.path("dir")).await.unwrap();
        let err = error(
            remove_file(&e.path("dir"), "shown", &Default::default()).await,
            "FS_IO_ERROR",
        );
        assert_eq!(
            err.to_string(),
            format!(
                "cannot delete \"shown\": Path is a directory: rm returned EISDIR (is a directory) {}",
                e.path("dir")
            )
        );
        assert!(fs::metadata(e.path("dir")).await.unwrap().is_dir());
    }
    #[tokio::test]
    async fn a_fallible_staging_name_hook_returns_the_callers_error_unchanged() {
        for directory in [false, true] {
            let e = Env::new();
            e.file("a", "good").await;
            let sentinel = SharedError::new(std::io::Error::other("name factory failed"));
            let s = sentinel.clone();
            let hook: NameHook = Arc::new(move |_| Err(s.clone().into()));
            let mut h = FsIoInternals::default();
            if directory {
                h.temp_dir_name = Some(hook);
            } else {
                h.temp_name = Some(hook);
            }
            let err = write(&e, "a", "new", &h, Default::default())
                .await
                .unwrap_err();
            assert!(err.downcast_ref::<SharedError>().unwrap().ptr_eq(&sentinel));
            assert_eq!(e.bytes("a").await, "good");
            clean(&e).await;
        }
    }
    #[tokio::test]
    async fn shared_errno_keeps_its_identity_as_the_classified_failure_cause() {
        let e = Env::new();
        let sentinel = SharedError::new(ErrnoError::new(Some("EACCES".into()), "permission fault"));
        let s = sentinel.clone();
        let h = FsIoInternals {
            inspect_temp: Some(Arc::new(move |_| {
                let s = s.clone();
                Box::pin(async move { Err(s.into()) })
            })),
            ..Default::default()
        };
        let err = error(
            write(&e, "a", "new", &h, Default::default()).await,
            "FS_PERMISSION_DENIED",
        );
        assert!(
            hoh_error::HarnessError::of(err.as_ref())
                .unwrap()
                .cause()
                .unwrap()
                .downcast_ref::<SharedError>()
                .unwrap()
                .ptr_eq(&sentinel)
        );
        e.file("a", "old").await;
        let s = sentinel.clone();
        let h = FsIoInternals {
            remove_file: Some(Arc::new(move |_| {
                let s = s.clone();
                Box::pin(async move { Err(s.into()) })
            })),
            ..Default::default()
        };
        let err = error(
            remove_file(&e.path("a"), &e.path("a"), &h).await,
            "FS_PERMISSION_DENIED",
        );
        assert!(
            hoh_error::HarnessError::of(err.as_ref())
                .unwrap()
                .cause()
                .unwrap()
                .downcast_ref::<SharedError>()
                .unwrap()
                .ptr_eq(&sentinel)
        );
        assert_eq!(e.bytes("a").await, "old");
        let h = FsIoInternals {
            before_operation: Some(Arc::new(move |_, _| Err(sentinel.clone().into()))),
            ..Default::default()
        };
        assert!(
            read_text_for_diff_with_internals(&e.path("a"), 32, None, &h)
                .await
                .unwrap()
                .is_none()
        );
        let sentinel = SharedError::new(hoh_error::HarnessError::new(
            "coded hook error",
            "HOOK_CODE",
        ));
        let s = sentinel.clone();
        let h = FsIoInternals {
            inspect_temp: Some(Arc::new(move |_| {
                let s = s.clone();
                Box::pin(async move { Err(s.into()) })
            })),
            ..Default::default()
        };
        let err = error(
            write(&e, "a", "new", &h, Default::default()).await,
            "FS_IO_ERROR",
        );
        assert!(
            hoh_error::HarnessError::of(err.as_ref())
                .unwrap()
                .cause()
                .unwrap()
                .downcast_ref::<SharedError>()
                .unwrap()
                .ptr_eq(&sentinel)
        );
        let h = FsIoInternals {
            before_operation: Some(Arc::new(move |_, _| Err(sentinel.clone().into()))),
            ..Default::default()
        };
        assert!(
            read_text_for_diff_with_internals(&e.path("a"), 32, None, &h)
                .await
                .unwrap()
                .is_none()
        );
        let h = FsIoInternals {
            inspect_temp: Some(Arc::new(|_| {
                Box::pin(async { Err(hoh_timeout::AbortError::default().into()) })
            })),
            ..Default::default()
        };
        let err = error(
            write(&e, "a", "new", &h, Default::default()).await,
            "FS_ABORTED",
        );
        assert_eq!(err.to_string(), "write aborted");
        assert_eq!(e.bytes("a").await, "old");
    }
    #[tokio::test]
    async fn maps_denied_parent_and_staging_creation_to_typed_permission_failures() {
        let e = Env::new();
        let d = e.path("locked");
        fs::create_dir(&d).await.unwrap();
        fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555))
            .await
            .unwrap();
        let result = async {
            for path in [e.path("locked/file"), e.path("locked/missing/file")] {
                let r = write_file_atomic(
                    &path,
                    "content",
                    None,
                    None,
                    &Default::default(),
                    Default::default(),
                )
                .await;
                if r.is_ok() {
                    fs::remove_file(&path).await.unwrap();
                    return;
                }
                let err = error(r, "FS_PERMISSION_DENIED");
                assert_eq!(
                    err.to_string(),
                    format!("cannot write \"{path}\": permission denied")
                );
                let err = error(
                    write_file_atomic(
                        &path,
                        "content",
                        None,
                        None,
                        &Default::default(),
                        AtomicWriteOptions {
                            display_path: Some("~/shown".into()),
                            ..Default::default()
                        },
                    )
                    .await,
                    "FS_PERMISSION_DENIED",
                );
                assert_eq!(
                    err.to_string(),
                    "cannot write \"~/shown\": permission denied"
                );
            }
        };
        result.await;
        fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755))
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn maps_a_parent_path_blocked_by_a_regular_file_to_fs_io_error_on_every_platform() {
        let e = Env::new();
        e.file("blocker", "file").await;
        let err = error(
            write(
                &e,
                "blocker/child",
                "content",
                &Default::default(),
                Default::default(),
            )
            .await,
            "FS_IO_ERROR",
        );
        assert!(
            err.to_string()
                .starts_with(&format!("cannot write \"{}\": ", e.path("blocker/child")))
        );
        assert!(
            hoh_error::HarnessError::of(err.as_ref())
                .unwrap()
                .cause()
                .is_some()
        );
        assert_eq!(e.bytes("blocker").await, "file");
    }
    #[tokio::test]
    async fn preserves_unexpected_failures_during_staging() {
        #[derive(Debug)]
        struct NumericCode {
            code: i32,
        }
        impl std::fmt::Display for NumericCode {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("invalid errno")
            }
        }
        impl std::error::Error for NumericCode {}
        #[derive(Debug)]
        struct ThrownString(&'static str);
        impl std::fmt::Display for ThrownString {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }
        impl std::error::Error for ThrownString {}
        let numeric = NumericCode { code: 1 };
        assert_eq!(numeric.code, 1);
        let values: Vec<BoxError> = vec![
            std::io::Error::other("provider defect").into(),
            Box::new(numeric),
            Box::new(ThrownString("provider defect")),
            std::io::Error::from_raw_os_error(5).into(),
        ];
        for value in values {
            let e = Env::new();
            let sentinel = SharedError::new(value);
            let s = sentinel.clone();
            let h = FsIoInternals {
                inspect_temp: Some(Arc::new(move |_| {
                    let s = s.clone();
                    Box::pin(async move { Err(s.into()) })
                })),
                ..Default::default()
            };
            let err = write(&e, "a", "new", &h, Default::default())
                .await
                .unwrap_err();
            assert!(err.downcast_ref::<SharedError>().unwrap().ptr_eq(&sentinel));
            assert!(
                fs::read_dir(e.dir.path())
                    .await
                    .unwrap()
                    .next_entry()
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }
    #[tokio::test]
    async fn classifies_metadata_resolution_and_read_faults_on_a_symlink_loop() {
        let e = Env::new();
        fs::symlink(e.path("loop"), e.path("loop")).await.unwrap();
        error(probe(&e.path("loop")).await, "FS_IO_ERROR");
        error(
            resolve_local_target(e.dir.path().to_str().unwrap(), "loop").await,
            "FS_IO_ERROR",
        );
        error(read_whole_text(&p(&e, "loop"), None).await, "FS_IO_ERROR");
    }
    #[tokio::test]
    async fn classifies_a_regular_file_replaced_by_a_directory_after_the_raw_read_stat() {
        let e = Env::new();
        let t = e.file("swap.bin", "bytes").await;
        let h = FsIoInternals {
            inspect_read_bytes_after_stat: Some(Arc::new(|t| {
                Box::pin(async move {
                    fs::remove_file(t.target_key.as_str()).await?;
                    fs::create_dir(t.target_key.as_str()).await?;
                    Ok(())
                })
            })),
            ..Default::default()
        };
        let err = error(
            read_whole_bytes(&t, None, 1024, &h).await,
            "FS_NOT_REGULAR_FILE",
        );
        assert_eq!(
            hoh_error::HarnessError::of(err.as_ref())
                .unwrap()
                .cause()
                .unwrap()
                .downcast_ref::<ErrnoError>()
                .unwrap()
                .code
                .as_deref(),
            Some("EISDIR")
        );
    }
    #[tokio::test]
    async fn maps_unreadable_file_content_to_fs_permission_denied() {
        let e = Env::new();
        let t = e.file("locked.txt", "secret").await;
        fs::set_permissions(e.path("locked.txt"), std::fs::Permissions::from_mode(0o0))
            .await
            .unwrap();
        let r = read_whole_text(&t, None).await;
        if r.is_err() {
            let err = error(r, "FS_PERMISSION_DENIED");
            assert_eq!(
                err.to_string(),
                format!("cannot read \"{}\": permission denied", t.display_path)
            );
            error(
                collect(stream_whole_text(t.clone(), None)).await,
                "FS_PERMISSION_DENIED",
            );
            error(
                read_byte_window(
                    &t,
                    FsByteRange {
                        offset: 0,
                        length: 4,
                    },
                    None,
                )
                .await,
                "FS_PERMISSION_DENIED",
            );
            let err = error(
                read_for_edit(t.target_key.as_str(), &t.display_path, None).await,
                "FS_PERMISSION_DENIED",
            );
            assert_eq!(
                err.to_string(),
                format!("cannot edit \"{}\": permission denied", t.display_path)
            );
        }
        fs::set_permissions(e.path("locked.txt"), std::fs::Permissions::from_mode(0o600))
            .await
            .unwrap();
    }
}
