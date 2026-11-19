mod common;
use common::*;
use hoh_fs::*;
use hoh_fs_local::{Config, FsLocalPlugin, MAX_DIFF_BASIS_BYTES};
use hoh_kernel::Kernel;
use hoh_timeout::AbortController;
use std::{os::unix::fs::MetadataExt, sync::Arc};
use tokio::fs;
mod registration {
    use super::*;
    #[tokio::test]
    async fn registers_local_file_system_as_ctx_fs_with_a_default_cwd() {
        let k = Kernel::start();
        let c = k.root();
        let f = c.plugin::<FsLocalPlugin>(serde_json::json!({})).unwrap();
        f.settled().await.unwrap();
        assert!(c.get::<Fs>().is_some());
        let expected = std::env::current_dir()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let local = hoh_fs_local::LocalFileSystem::new(
            serde_json::from_value(serde_json::json!({})).unwrap(),
        )
        .unwrap();
        assert_eq!(local.config.cwd, expected);
        assert_eq!(local.config.diff_basis_max_bytes, 10 * 1024 * 1024);
        let schema = schemars::schema_for!(Config);
        assert_eq!(
            schema.as_value()["properties"]["cwd"]["default"],
            serde_json::json!(expected)
        );
        k.shutdown().await;
    }
    #[test]
    fn maps_only_absolute_host_paths_into_its_process_path_namespace() {
        let e = Env::new();
        assert_eq!(
            e.fs.process_path_from_host_path(&e.path("image.png")),
            Some(e.path("image.png"))
        );
        assert!(e.fs.process_path_from_host_path("image.png").is_none());
    }
    #[test]
    fn rejects_non_positive_fractional_unsafe_or_unallocatable_diff_basis_limits() {
        assert!(serde_json::from_value::<Config>(serde_json::json!({"cwd":null})).is_err());
        let default: Config = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(
            default.cwd.as_deref(),
            Some(std::env::current_dir().unwrap().to_str().unwrap())
        );
        assert!(
            hoh_fs_local::LocalFileSystem::new(Config {
                diff_basis_max_bytes: MAX_DIFF_BASIS_BYTES as f64,
                ..Default::default()
            })
            .is_ok()
        );
        for n in [
            0.0,
            -1.0,
            1.5,
            MAX_DIFF_BASIS_BYTES as f64 + 1.0,
            9_007_199_254_740_992.0,
        ] {
            let err = hoh_fs_local::LocalFileSystem::new(Config {
                diff_basis_max_bytes: n,
                ..Default::default()
            })
            .err()
            .unwrap();
            assert_eq!(
                err.to_string(),
                format!(
                    "fs-local: diffBasisMaxBytes must be a positive safe integer no greater than {MAX_DIFF_BASIS_BYTES}"
                )
            );
        }
    }
}
mod resolve {
    use super::*;
    #[tokio::test]
    async fn resolves_a_relative_path_against_opts_cwd_not_config_cwd() {
        let e = Env::new();
        let other = Env::new();
        other.file("x.txt", "in other").await;
        let t =
            e.fs.resolve(
                "x.txt",
                FsResolveOptions {
                    cwd: Some(other.path("")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(e.fs.read_text(&t, None).await.unwrap(), "in other");
        error(
            e.fs.read_text(&e.target("x.txt").await, None).await,
            "FS_NOT_FOUND",
        );
    }
    #[tokio::test]
    async fn ignores_opts_cwd_for_an_absolute_path() {
        let e = Env::new();
        e.file("abs.txt", "absolute").await;
        let t =
            e.fs.resolve(
                &e.path("abs.txt"),
                FsResolveOptions {
                    cwd: Some(e.path("nonexistent-base")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(e.fs.read_text(&t, None).await.unwrap(), "absolute");
    }
    #[tokio::test]
    async fn honors_a_pre_aborted_signal() {
        let e = Env::new();
        error(
            e.fs.resolve(
                "a.txt",
                FsResolveOptions {
                    signal: Some(aborted()),
                    ..Default::default()
                },
            )
            .await,
            "FS_ABORTED",
        );
    }
    #[test]
    fn honors_a_signal_aborted_while_resolution_is_in_flight() {
        isolated_io_test(async {
            let e = Env::new();
            let c = Arc::new(AbortController::new());
            let hold = hold_io().await;
            let options = FsResolveOptions {
                signal: Some(c.signal()),
                ..Default::default()
            };
            let f = e.fs.resolve("a.txt", options);
            tokio::pin!(f);
            assert!(poll_once(f.as_mut()).await.is_pending());
            c.abort();
            drop(hold);
            error(f.await, "FS_ABORTED");
        });
    }
    #[tokio::test]
    async fn projects_process_paths_file_urls_and_canonical_containment() {
        let e = Env::new();
        fs::create_dir(e.path("nested")).await.unwrap();
        let child = e.file("nested/file.txt", "text").await;
        let root = e.target(".").await;
        let outside = e.target("..").await;
        assert_eq!(e.fs.process_path(&child), e.path("nested/file.txt"));
        assert_eq!(
            e.fs.file_url(&child),
            url::Url::from_file_path(e.path("nested/file.txt"))
                .unwrap()
                .to_string()
        );
        assert!(e.fs.contains(&root, &root));
        assert!(e.fs.contains(&root, &child));
        assert!(!e.fs.contains(&root, &outside));
    }
}
mod home_prefix {
    use super::*;
    fn home(e: &Env, h: &Env) {
        e.hooks().home_directory = Some(h.path(""));
    }
    #[tokio::test]
    async fn resolves_and_lstats_a_leading_tilde_against_the_home_directory() {
        let e = Env::new();
        let h = Env::new();
        home(&e, &h);
        h.file("note.txt", "home note").await;
        assert_eq!(e.target("~").await.display_path, h.path(""));
        let t = e.target("~/note.txt").await;
        assert_eq!(t.display_path, h.path("note.txt"));
        assert_eq!(e.fs.read_text(&t, None).await.unwrap(), "home note");
        assert_eq!(
            e.fs.lstat("~/note.txt", None, None)
                .await
                .unwrap()
                .unwrap()
                .r#type,
            FsPathType::File
        );
    }
    #[tokio::test]
    async fn keeps_tilde_user_and_later_tilde_segments_relative_to_cwd() {
        let e = Env::new();
        assert_eq!(
            e.target("~user/x.txt").await.display_path,
            e.path("~user/x.txt")
        );
        assert_eq!(
            e.target("a/~/b.txt").await.display_path,
            e.path("a/~/b.txt")
        );
    }
    #[tokio::test]
    async fn keeps_posix_tilde_backslash_literal_and_physical_parent_traversal_after_tilde_slash() {
        let e = Env::new();
        let h = Env::new();
        home(&e, &h);
        assert_eq!(
            e.target("~\\note.txt").await.display_path,
            e.path("~\\note.txt")
        );
        fs::create_dir_all(h.path("real/sub")).await.unwrap();
        h.file("real/a.txt", "beside link target").await;
        fs::symlink(h.path("real/sub"), h.path("link"))
            .await
            .unwrap();
        assert_eq!(
            e.fs.read_text(&e.target("~/link/../a.txt").await, None)
                .await
                .unwrap(),
            "beside link target"
        );
    }
    #[tokio::test]
    async fn lists_a_child_literally_named_tilde_under_its_parent() {
        let e = Env::new();
        fs::create_dir(e.path("~")).await.unwrap();
        let entries = e.fs.list_dir(&e.target(".").await, None).await.unwrap();
        assert_eq!(entries[0].target.display_path, e.path("~"));
        assert_eq!(entries[0].target.target_key.as_str(), e.path("~"));
    }
}
mod stat {
    use super::*;
    #[tokio::test]
    async fn returns_file_metadata_directory_type_and_undefined_for_absent() {
        let e = Env::new();
        let t = e.file("a.txt", "hello").await;
        let i = e.fs.stat(&t, None).await.unwrap().unwrap();
        assert_eq!(i.r#type, FsType::File);
        assert_eq!(i.size, Some(5));
        assert!(!i.version.as_str().is_empty());
        assert_eq!(
            e.fs.stat(&e.target(".").await, None)
                .await
                .unwrap()
                .unwrap()
                .r#type,
            FsType::Directory
        );
        assert!(
            e.fs.stat(&e.target("missing.txt").await, None)
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn changes_version_after_a_same_size_rewrite_even_when_mtime_is_restored() {
        let e = Env::new();
        let t = e.file("same-size.txt", "first").await;
        let before = fs::metadata(e.path("same-size.txt")).await.unwrap();
        let v = e.version(&t).await;
        e.fs.write_text(&t, "other", None, None, None)
            .await
            .unwrap();
        let f = std::fs::File::open(e.path("same-size.txt")).unwrap();
        let times = std::fs::FileTimes::new()
            .set_accessed(before.accessed().unwrap())
            .set_modified(before.modified().unwrap());
        f.set_times(times).unwrap();
        let after = fs::metadata(e.path("same-size.txt")).await.unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(after.mtime_nsec(), before.mtime_nsec());
        assert_ne!(e.version(&t).await, v);
    }
    #[tokio::test]
    async fn honors_a_pre_aborted_signal() {
        let e = Env::new();
        error(
            e.fs.stat(&e.target("a.txt").await, Some(aborted())).await,
            "FS_ABORTED",
        );
    }
}
mod lstat {
    use super::*;
    #[tokio::test]
    async fn reports_path_metadata_without_following_the_final_symlink_component() {
        let e = Env::new();
        e.file("real.txt", "hello").await;
        fs::symlink(e.path("real.txt"), e.path("link.txt"))
            .await
            .unwrap();
        assert_eq!(
            e.fs.lstat("real.txt", None, None)
                .await
                .unwrap()
                .unwrap()
                .r#type,
            FsPathType::File
        );
        assert_eq!(
            e.fs.lstat("link.txt", None, None)
                .await
                .unwrap()
                .unwrap()
                .r#type,
            FsPathType::Symlink
        );
        assert!(
            e.fs.lstat("missing.txt", None, None)
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn resolves_relative_paths_against_opts_cwd_and_honors_a_pre_aborted_signal() {
        let e = Env::new();
        let other = Env::new();
        other.file("x.txt", "in other").await;
        assert_eq!(
            e.fs.lstat("x.txt", Some(&other.path("")), None)
                .await
                .unwrap()
                .unwrap()
                .r#type,
            FsPathType::File
        );
        error(
            e.fs.lstat("x.txt", Some(&other.path("")), Some(aborted()))
                .await,
            "FS_ABORTED",
        );
        error(e.fs.lstat("   ", None, None).await, "FS_NOT_FOUND");
    }
}
#[tokio::test]
async fn rejects_stat_and_lstat_when_their_signals_abort_while_the_metadata_probes_are_in_flight() {
    let e = Env::new();
    let t = e.file("slow.txt", "hello").await;
    let c = Arc::new(AbortController::new());
    let ac = c.clone();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let s = seen.clone();
    e.hooks().after_metadata = Some(Arc::new(move |op, _| {
        s.lock().unwrap().push(op);
        ac.abort();
        Box::pin(async { Ok(()) })
    }));
    error(e.fs.stat(&t, Some(c.signal())).await, "FS_ABORTED");
    let c = Arc::new(AbortController::new());
    let ac = c.clone();
    let s = seen.clone();
    e.hooks().after_metadata = Some(Arc::new(move |op, _| {
        s.lock().unwrap().push(op);
        ac.abort();
        Box::pin(async { Ok(()) })
    }));
    error(
        e.fs.lstat("slow.txt", None, Some(c.signal())).await,
        "FS_ABORTED",
    );
    assert_eq!(*seen.lock().unwrap(), vec!["stat", "lstat"]);
}
mod text {
    use super::*;
    #[tokio::test]
    async fn reads_whole_file_text() {
        let e = Env::new();
        let t = e.file("a.txt", "one\ntwo\nthree").await;
        assert_eq!(e.fs.read_text(&t, None).await.unwrap(), "one\ntwo\nthree");
    }
    #[tokio::test]
    async fn streams_the_same_text() {
        let e = Env::new();
        let t = e.file("a.txt", "one\ntwo\nthree").await;
        assert_eq!(
            collect(e.fs.stream_text(&t, None).await.unwrap())
                .await
                .unwrap(),
            "one\ntwo\nthree"
        );
    }
    #[tokio::test]
    async fn rejects_a_missing_file_a_directory_binary_and_invalid_utf8() {
        let e = Env::new();
        error(
            e.fs.read_text(&e.target("nope").await, None).await,
            "FS_NOT_FOUND",
        );
        error(
            e.fs.read_text(&e.target(".").await, None).await,
            "FS_NOT_REGULAR_FILE",
        );
        for (name, bytes) in [("bin", [0x68, 0, 0x69]), ("bad", [0x68, 0xff, 0x69])] {
            let t = e.file(name, bytes).await;
            error(e.fs.read_text(&t, None).await, "FS_NOT_TEXT");
        }
    }
}
mod bytes {
    use super::*;
    #[tokio::test]
    async fn reads_raw_bytes_without_decoding_or_nul_rejection() {
        let e = Env::new();
        let raw = [0x68, 0, 0x69, 0xff];
        let t = e.file("a.bin", raw).await;
        assert_eq!(e.fs.read_bytes(&t, None, 4).await.unwrap(), raw);
    }
    #[tokio::test]
    async fn accepts_a_file_exactly_at_max_bytes_and_rejects_one_past_it() {
        let e = Env::new();
        let t = e.file("a.bin", [1; 4]).await;
        assert_eq!(e.fs.read_bytes(&t, None, 4).await.unwrap().len(), 4);
        error(e.fs.read_bytes(&t, None, 3).await, "FS_TOO_LARGE");
    }
    #[tokio::test]
    async fn bounds_content_io_when_a_file_grows_after_stat_preflight() {
        let e = Env::new();
        let t = e.file("a.bin", [1; 4]).await;
        e.hooks().inspect_read_bytes_after_stat = Some(Arc::new(|t| {
            Box::pin(async move {
                fs::write(t.target_key.as_str(), vec![2; 1024 * 1024]).await?;
                Ok(())
            })
        }));
        error(e.fs.read_bytes(&t, None, 4).await, "FS_TOO_LARGE");
    }
    #[tokio::test]
    async fn rejects_a_missing_file_and_a_directory() {
        let e = Env::new();
        error(
            e.fs.read_bytes(&e.target("nope").await, None, 1024).await,
            "FS_NOT_FOUND",
        );
        error(
            e.fs.read_bytes(&e.target(".").await, None, 1024).await,
            "FS_NOT_REGULAR_FILE",
        );
    }
    #[tokio::test]
    async fn reads_under_a_live_signal_and_rejects_an_already_aborted_one_with_fs_aborted() {
        let e = Env::new();
        let t = e.file("a.bin", "data").await;
        assert_eq!(
            e.fs.read_bytes(&t, Some(AbortController::new().signal()), 1024)
                .await
                .unwrap()
                .len(),
            4
        );
        error(
            e.fs.read_bytes(&t, Some(aborted()), 1024).await,
            "FS_ABORTED",
        );
    }
}
mod range {
    use super::*;
    async fn ramp() -> (Env, FsTarget) {
        let e = Env::new();
        let t = e.file("ramp.bin", (0..=255).collect::<Vec<u8>>()).await;
        (e, t)
    }
    #[tokio::test]
    async fn reads_a_window_from_the_middle_of_a_file_larger_than_the_window() {
        let (e, t) = ramp().await;
        assert_eq!(
            e.fs.read_byte_range(
                &t,
                FsByteRange {
                    offset: 100,
                    length: 4
                },
                Some(AbortController::new().signal())
            )
            .await
            .unwrap(),
            vec![100, 101, 102, 103]
        );
    }
    #[tokio::test]
    async fn shortens_a_window_the_file_ends_inside_and_empties_one_at_or_past_the_end() {
        let (e, t) = ramp().await;
        for (offset, expected) in [(253, vec![253, 254, 255]), (256, vec![]), (1000, vec![])] {
            assert_eq!(
                e.fs.read_byte_range(&t, FsByteRange { offset, length: 10 }, None)
                    .await
                    .unwrap(),
                expected
            );
        }
    }
    #[tokio::test]
    async fn returns_an_empty_window_for_length_zero_without_touching_content() {
        let (e, t) = ramp().await;
        assert!(
            e.fs.read_byte_range(
                &t,
                FsByteRange {
                    offset: 0,
                    length: 0
                },
                None
            )
            .await
            .unwrap()
            .is_empty()
        );
    }
    #[tokio::test]
    async fn carries_nul_and_invalid_utf8_untouched() {
        let e = Env::new();
        let raw = [0, 0xff, 0xfe, 0x41];
        let t = e.file("a.bin", raw).await;
        assert_eq!(
            e.fs.read_byte_range(
                &t,
                FsByteRange {
                    offset: 0,
                    length: 4
                },
                None
            )
            .await
            .unwrap(),
            raw
        );
    }
    #[tokio::test]
    async fn rejects_a_missing_file_a_directory_and_an_already_aborted_signal() {
        let e = Env::new();
        let r = FsByteRange {
            offset: 0,
            length: 1,
        };
        error(
            e.fs.read_byte_range(&e.target("nope").await, r, None).await,
            "FS_NOT_FOUND",
        );
        error(
            e.fs.read_byte_range(&e.target(".").await, r, None).await,
            "FS_NOT_REGULAR_FILE",
        );
        let t = e.file("a.bin", "data").await;
        error(
            e.fs.read_byte_range(&t, r, Some(aborted())).await,
            "FS_ABORTED",
        );
    }
}
mod list {
    use super::*;
    #[tokio::test]
    async fn lists_files_and_directories_in_stable_name_order_with_resolved_child_targets() {
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
        let entries =
            e.fs.list_dir(&e.target("skills").await, None)
                .await
                .unwrap();
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
        for entry in &entries {
            assert_eq!(
                entry.target.display_path,
                e.path(&format!("skills/{}", entry.name))
            );
            if entry.version.is_some() {
                assert_eq!(
                    entry.target.target_key.as_str(),
                    fs::canonicalize(&entry.target.display_path)
                        .await
                        .unwrap()
                        .to_str()
                        .unwrap()
                );
            }
        }
        assert_eq!(entries[0].size, Some(5));
        assert!(entries[0].version.is_some());
        assert!(entries[1].version.is_none());
        assert!(entries[2].size.is_none());
    }
    #[tokio::test]
    async fn reports_a_missing_directory_as_fs_not_found() {
        let e = Env::new();
        error(
            e.fs.list_dir(&e.target("missing").await, None).await,
            "FS_NOT_FOUND",
        );
    }
    #[tokio::test]
    async fn reports_a_file_target_as_fs_not_directory() {
        let e = Env::new();
        let t = e.file("a.txt", "text").await;
        error(e.fs.list_dir(&t, None).await, "FS_NOT_DIRECTORY");
    }
    #[tokio::test]
    async fn honors_a_pre_aborted_signal() {
        let e = Env::new();
        fs::create_dir(e.path("skills")).await.unwrap();
        error(
            e.fs.list_dir(&e.target("skills").await, Some(aborted()))
                .await,
            "FS_ABORTED",
        );
    }
}
mod writing {
    use super::*;
    #[tokio::test]
    async fn create_if_absent_creates_a_new_file() {
        let e = Env::new();
        let t = e.target("new.txt").await;
        let r =
            e.fs.write_text(&t, "fresh", Some(FsWriteIntent::CreateIfAbsent), None, None)
                .await
                .unwrap();
        assert_eq!(r.operation, FsWriteOperation::Create);
        assert_eq!(e.bytes("new.txt").await, "fresh");
    }
    #[tokio::test]
    async fn create_if_absent_rejects_an_existing_file_as_fs_not_observed() {
        let e = Env::new();
        let t = e.file("a.txt", "old").await;
        error(
            e.fs.write_text(&t, "new", Some(FsWriteIntent::CreateIfAbsent), None, None)
                .await,
            "FS_NOT_OBSERVED",
        );
        assert_eq!(e.bytes("a.txt").await, "old");
    }
    #[tokio::test]
    async fn create_if_absent_preserves_a_competitor_created_after_the_initial_probe() {
        let e = Env::new();
        let t = e.target("a.txt").await;
        let path = e.path("a.txt");
        e.hooks().inspect_temp = Some(Arc::new(move |_| {
            let path = path.clone();
            Box::pin(async move {
                fs::write(path, "competitor").await?;
                Ok(())
            })
        }));
        error(
            e.fs.write_text(&t, "ours", Some(FsWriteIntent::CreateIfAbsent), None, None)
                .await,
            "FS_NOT_OBSERVED",
        );
        assert_eq!(e.bytes("a.txt").await, "competitor");
    }
    #[tokio::test]
    async fn reports_a_create_if_absent_race_with_the_unresolved_display_path() {
        let e = Env::new();
        fs::create_dir(e.path("real-workspace")).await.unwrap();
        fs::symlink(e.path("real-workspace"), e.path("linked-workspace"))
            .await
            .unwrap();
        let t = e.target("linked-workspace/a.txt").await;
        let path = e.path("real-workspace/a.txt");
        e.hooks().inspect_temp = Some(Arc::new(move |_| {
            let path = path.clone();
            Box::pin(async move {
                fs::write(path, "competitor").await?;
                Ok(())
            })
        }));
        let err = error(
            e.fs.write_text(&t, "ours", Some(FsWriteIntent::CreateIfAbsent), None, None)
                .await,
            "FS_NOT_OBSERVED",
        );
        assert_eq!(
            err.to_string(),
            format!(
                "cannot overwrite existing \"{}\" without reading it first",
                e.path("linked-workspace/a.txt")
            )
        );
        assert_eq!(e.bytes("real-workspace/a.txt").await, "competitor");
    }
    #[tokio::test]
    async fn create_if_absent_rejects_a_competing_directory_as_not_a_regular_file() {
        let e = Env::new();
        let t = e.target("a.txt").await;
        let path = e.path("a.txt");
        e.hooks().inspect_temp = Some(Arc::new(move |_| {
            let path = path.clone();
            Box::pin(async move {
                fs::create_dir(path).await?;
                Ok(())
            })
        }));
        error(
            e.fs.write_text(&t, "ours", Some(FsWriteIntent::CreateIfAbsent), None, None)
                .await,
            "FS_NOT_REGULAR_FILE",
        );
        assert!(fs::metadata(e.path("a.txt")).await.unwrap().is_dir());
    }
    #[tokio::test]
    async fn create_if_absent_rejects_and_preserves_a_dangling_symbolic_link() {
        let e = Env::new();
        fs::symlink(e.path("missing-target"), e.path("dangling"))
            .await
            .unwrap();
        let t = e.target("dangling").await;
        error(
            e.fs.write_text(&t, "ours", Some(FsWriteIntent::CreateIfAbsent), None, None)
                .await,
            "FS_NOT_REGULAR_FILE",
        );
        assert!(
            fs::symlink_metadata(e.path("dangling"))
                .await
                .unwrap()
                .is_symlink()
        );
        assert_eq!(
            fs::read(e.path("dangling")).await.unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }
    #[tokio::test]
    async fn replace_if_version_replaces_when_the_version_matches() {
        let e = Env::new();
        let t = e.file("a.txt", "old").await;
        let r =
            e.fs.write_text(&t, "new", replace(e.version(&t).await), None, None)
                .await
                .unwrap();
        assert_eq!(r.operation, FsWriteOperation::Update);
        assert_eq!(e.bytes("a.txt").await, "new");
    }
    #[tokio::test]
    async fn replace_if_version_rejects_a_stale_version() {
        let e = Env::new();
        let t = e.file("a.txt", "v1").await;
        let stale = e.version(&t).await;
        e.file("a.txt", "changed-externally").await;
        error(
            e.fs.write_text(&t, "v2", replace(stale), None, None).await,
            "FS_STALE_VERSION",
        );
    }
    #[tokio::test]
    async fn replace_if_version_rejects_a_deleted_target_as_stale_without_recreating_it() {
        let e = Env::new();
        let t = e.file("a.txt", "v1").await;
        let v = e.version(&t).await;
        fs::remove_file(e.path("a.txt")).await.unwrap();
        error(
            e.fs.write_text(&t, "v2", replace(v), None, None).await,
            "FS_STALE_VERSION",
        );
        assert!(!fs::try_exists(e.path("a.txt")).await.unwrap());
    }
    #[tokio::test]
    async fn rejects_writing_onto_a_directory() {
        let e = Env::new();
        error(
            e.fs.write_text(
                &e.target(".").await,
                "x",
                Some(FsWriteIntent::CreateIfAbsent),
                None,
                None,
            )
            .await,
            "FS_NOT_REGULAR_FILE",
        );
    }
    #[tokio::test]
    async fn unconditionally_creates_a_new_file_with_no_expectation_bare_provider() {
        let e = Env::new();
        let r =
            e.fs.write_text(&e.target("new.txt").await, "fresh", None, None, None)
                .await
                .unwrap();
        assert_eq!(r.operation, FsWriteOperation::Create);
        assert_eq!(e.bytes("new.txt").await, "fresh");
    }
    #[tokio::test]
    async fn unconditionally_overwrites_an_existing_file_with_no_expectation_bare_provider() {
        let e = Env::new();
        let t = e.file("a.txt", "old").await;
        let r =
            e.fs.write_text(&t, "clobbered", None, None, None)
                .await
                .unwrap();
        assert_eq!(r.operation, FsWriteOperation::Update);
        assert_eq!(e.bytes("a.txt").await, "clobbered");
    }
    #[tokio::test]
    async fn rejects_writing_onto_a_directory_even_with_no_expectation() {
        let e = Env::new();
        error(
            e.fs.write_text(&e.target(".").await, "x", None, None, None)
                .await,
            "FS_NOT_REGULAR_FILE",
        );
    }
    #[tokio::test]
    async fn a_create_reports_before_null_and_after_the_written_content_no_prior_file() {
        let e = Env::new();
        let r =
            e.fs.write_text(&e.target("new.txt").await, "fresh", None, None, None)
                .await
                .unwrap();
        assert!(r.before.is_none());
        assert_eq!(r.after, "fresh");
    }
    #[tokio::test]
    async fn an_overwrite_reports_before_the_old_content_and_after_the_new_content() {
        let e = Env::new();
        let t = e.file("a.txt", "old body").await;
        let r =
            e.fs.write_text(&t, "new body", None, None, None)
                .await
                .unwrap();
        assert_eq!(r.before.as_deref(), Some("old body"));
        assert_eq!(r.after, "new body");
    }
    #[tokio::test]
    async fn an_overwrite_returns_lf_normalized_before_and_after_a_crlf_rewrite_is_not_every_line_changed()
     {
        let e = Env::new();
        let t = e.file("a.txt", "a\r\nb\r\nc\r\n").await;
        let r =
            e.fs.write_text(&t, "a\r\nB\r\nc\r\n", None, None, None)
                .await
                .unwrap();
        assert_eq!(r.before.as_deref(), Some("a\nb\nc\n"));
        assert_eq!(r.after, "a\nB\nc\n");
    }
    #[tokio::test]
    async fn an_overwrite_of_a_binary_prior_file_reports_before_null_undiffable_still_succeeds() {
        let e = Env::new();
        let t = e.file("a.bin", [0, 1, 2]).await;
        let r =
            e.fs.write_text(&t, "now text", None, None, None)
                .await
                .unwrap();
        assert_eq!(r.operation, FsWriteOperation::Update);
        assert!(r.before.is_none());
        assert_eq!(r.after, "now text");
    }
    #[tokio::test]
    async fn an_overwrite_of_an_invalid_utf8_non_nul_prior_file_reports_before_null_still_succeeds()
    {
        let e = Env::new();
        let t = e.file("a.bin", [0x68, 0xff, 0x69]).await;
        let r =
            e.fs.write_text(&t, "now valid", None, None, None)
                .await
                .unwrap();
        assert_eq!(r.operation, FsWriteOperation::Update);
        assert!(r.before.is_none());
        assert_eq!(r.after, "now valid");
    }
    #[tokio::test]
    async fn an_overwrite_of_a_prior_file_at_the_whole_file_bound_reports_before_null_undiffable_still_succeeds()
     {
        let e = Env::limit(8);
        let t = e.file("big.txt", "12345678").await;
        let r = e.fs.write_text(&t, "tiny", None, None, None).await.unwrap();
        assert_eq!(r.operation, FsWriteOperation::Update);
        assert!(r.before.is_none());
        assert_eq!(r.after, "tiny");
    }
    #[tokio::test]
    async fn an_overwrite_whose_new_content_is_at_the_whole_file_bound_reports_before_null_no_huge_contextual_diff()
     {
        let e = Env::limit(8);
        let t = e.file("grow.txt", "tiny").await;
        let r =
            e.fs.write_text(&t, "12345678", None, None, None)
                .await
                .unwrap();
        assert!(r.before.is_none());
        assert_eq!(r.after, "12345678");
    }
    #[tokio::test]
    async fn gates_the_new_content_by_utf8_byte_length_not_character_count() {
        let e = Env::limit(8);
        let t = e.file("cjk.txt", "tiny").await;
        let r =
            e.fs.write_text(&t, "你好吗", None, None, None)
                .await
                .unwrap();
        assert!(r.before.is_none());
        assert_eq!(r.after, "你好吗");
    }
    #[tokio::test]
    async fn an_overwrite_with_both_sides_below_the_whole_file_bound_keeps_its_contextual_before_basis()
     {
        let e = Env::limit(8);
        let t = e.file("small.txt", "1234567").await;
        let r = e.fs.write_text(&t, "new", None, None, None).await.unwrap();
        assert_eq!(r.before.as_deref(), Some("1234567"));
        assert_eq!(r.after, "new");
    }
    #[tokio::test]
    async fn releases_per_target_mutation_locks_after_success_and_failure() {
        let e = Env::new();
        let t = e.target("a.txt").await;
        e.fs.write_text(
            &t,
            "created",
            Some(FsWriteIntent::CreateIfAbsent),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(e.fs.mutation_lock_count(), 0);
        error(
            e.fs.write_text(&t, "again", Some(FsWriteIntent::CreateIfAbsent), None, None)
                .await,
            "FS_NOT_OBSERVED",
        );
        assert_eq!(e.fs.mutation_lock_count(), 0);
    }
    #[tokio::test]
    async fn replace_if_version_returns_the_post_write_version_matches_a_fresh_stat() {
        let e = Env::new();
        let t = e.file("a.txt", "v1").await;
        let v = e.version(&t).await;
        let r =
            e.fs.write_text(
                &t,
                "a much longer replacement body",
                replace(v.clone()),
                None,
                None,
            )
            .await
            .unwrap();
        assert_ne!(r.version, v);
        assert_eq!(r.version, e.version(&t).await);
    }
    #[tokio::test]
    async fn honors_a_pre_aborted_signal_without_creating_the_file() {
        let e = Env::new();
        let t = e.target("aborted.txt").await;
        error(
            e.fs.write_text(&t, "x", None, Some(aborted()), None).await,
            "FS_ABORTED",
        );
        assert!(!fs::try_exists(e.path("aborted.txt")).await.unwrap());
        assert_eq!(e.fs.mutation_lock_count(), 0);
    }
    #[tokio::test]
    async fn two_concurrent_guarded_writes_one_updates_the_other_is_rejected_as_stale() {
        let e = Env::new();
        let t = e.file("a.txt", "base").await;
        let v = e.version(&t).await;
        let (a, b) = tokio::join!(
            e.fs.write_text(&t, "one", replace(v.clone()), None, None),
            e.fs.write_text(&t, "two", replace(v), None, None)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        if a.is_err() {
            error(a, "FS_STALE_VERSION");
        } else {
            error(b, "FS_STALE_VERSION");
        }
        assert_eq!(e.fs.mutation_lock_count(), 0);
    }
}
mod editing {
    use super::*;
    #[tokio::test]
    async fn applies_a_literal_edit_at_the_matching_version() {
        let e = Env::new();
        let t = e.file("a.txt", "hello world").await;
        let r =
            e.fs.edit_text(
                &t,
                edit("world", "there"),
                guard(e.version(&t).await),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(r.after, "hello there");
        assert_eq!(e.bytes("a.txt").await, "hello there");
    }
    #[tokio::test]
    async fn reports_before_after_content_the_applied_hunk_basis_lf_normalized() {
        let e = Env::new();
        let t = e.file("a.txt", "a\r\nOLD\r\nb\r\n").await;
        let r =
            e.fs.edit_text(&t, edit("OLD", "NEW"), None, None, None)
                .await
                .unwrap();
        assert_eq!(r.before, "a\nOLD\nb\n");
        assert_eq!(r.after, "a\nNEW\nb\n");
        assert_eq!(e.bytes("a.txt").await, "a\r\nNEW\r\nb\r\n");
    }
    #[tokio::test]
    async fn checks_the_stale_version_before_literal_matching() {
        let e = Env::new();
        let t = e.file("a.txt", "hello world").await;
        let v = e.version(&t).await;
        e.file("a.txt", "goodbye").await;
        error(
            e.fs.edit_text(&t, edit("world", "there"), guard(v), None, None)
                .await,
            "FS_STALE_VERSION",
        );
    }
    #[tokio::test]
    async fn unconditionally_edits_the_current_content_with_no_expectation_bare_provider() {
        let e = Env::new();
        let t = e.file("a.txt", "hello world").await;
        let r =
            e.fs.edit_text(&t, edit("world", "there"), None, None, None)
                .await
                .unwrap();
        assert_eq!(r.after, "hello there");
        assert_eq!(e.bytes("a.txt").await, "hello there");
    }
    #[tokio::test]
    async fn reports_a_missing_target_as_fs_stale_version_even_with_no_expectation_bare_provider() {
        let e = Env::new();
        error(
            e.fs.edit_text(
                &e.target("missing.txt").await,
                edit("a", "b"),
                None,
                None,
                None,
            )
            .await,
            "FS_STALE_VERSION",
        );
    }
    #[tokio::test]
    async fn still_reports_literal_match_codes_with_no_expectation_fs_edit_not_found_unrelated_to_freshness()
     {
        let e = Env::new();
        let t = e.file("a.txt", "hello world").await;
        error(
            e.fs.edit_text(&t, edit("absent", "x"), None, None, None)
                .await,
            "FS_EDIT_NOT_FOUND",
        );
    }
    #[tokio::test]
    async fn rejects_a_deleted_target_as_stale_before_matching() {
        let e = Env::new();
        let t = e.file("a.txt", "hello").await;
        let v = e.version(&t).await;
        fs::remove_file(e.path("a.txt")).await.unwrap();
        error(
            e.fs.edit_text(&t, edit("hello", "bye"), guard(v), None, None)
                .await,
            "FS_STALE_VERSION",
        );
    }
    #[tokio::test]
    async fn rejects_a_non_regular_target() {
        let e = Env::new();
        error(
            e.fs.edit_text(
                &e.target(".").await,
                edit("a", "b"),
                guard("v".into()),
                None,
                None,
            )
            .await,
            "FS_NOT_REGULAR_FILE",
        );
    }
    #[tokio::test]
    async fn rejects_zero_matches_and_ambiguous_matches_at_the_right_version() {
        let e = Env::new();
        let t = e.file("a.txt", "a a a").await;
        let v = e.version(&t).await;
        error(
            e.fs.edit_text(&t, edit("z", "X"), guard(v.clone()), None, None)
                .await,
            "FS_EDIT_NOT_FOUND",
        );
        error(
            e.fs.edit_text(&t, edit("a", "X"), guard(v), None, None)
                .await,
            "FS_AMBIGUOUS_EDIT",
        );
    }
    #[tokio::test]
    async fn replaces_all_matches_with_replace_all() {
        let e = Env::new();
        let t = e.file("a.txt", "a a a").await;
        let r =
            e.fs.edit_text(
                &t,
                FsEditRequest {
                    replace_all: true,
                    ..edit("a", "b")
                },
                guard(e.version(&t).await),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(r.after, "b b b");
        assert_eq!(e.bytes("a.txt").await, "b b b");
    }
    #[tokio::test]
    async fn rejects_invalid_utf8_without_rewriting_the_file() {
        let e = Env::new();
        let raw = [0x68, 0xff, 0x69];
        let t = e.file("bad.txt", raw).await;
        error(
            e.fs.edit_text(&t, edit("h", "H"), guard(e.version(&t).await), None, None)
                .await,
            "FS_NOT_TEXT",
        );
        assert_eq!(fs::read(e.path("bad.txt")).await.unwrap(), raw);
    }
    #[tokio::test]
    async fn two_concurrent_edits_one_wins_the_other_is_rejected_as_stale() {
        let e = Env::new();
        let t = e.file("a.txt", "base").await;
        let v = e.version(&t).await;
        let (a, b) = tokio::join!(
            e.fs.edit_text(&t, edit("base", "one"), guard(v.clone()), None, None),
            e.fs.edit_text(&t, edit("base", "two"), guard(v), None, None)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        if a.is_err() {
            error(a, "FS_STALE_VERSION");
        } else {
            error(b, "FS_STALE_VERSION");
        }
        assert_eq!(e.fs.mutation_lock_count(), 0);
    }
    #[tokio::test]
    async fn honors_a_pre_aborted_signal_without_rewriting_the_file() {
        let e = Env::new();
        let t = e.file("a.txt", "keep").await;
        error(
            e.fs.edit_text(&t, edit("keep", "x"), None, Some(aborted()), None)
                .await,
            "FS_ABORTED",
        );
        assert_eq!(e.bytes("a.txt").await, "keep");
        assert_eq!(e.fs.mutation_lock_count(), 0);
    }
    #[tokio::test]
    async fn a_successful_edit_refreshes_the_version_so_an_immediate_follow_up_edit_proceeds() {
        let e = Env::new();
        let t = e.file("a.txt", "one two").await;
        let first =
            e.fs.edit_text(
                &t,
                edit("one", "ONE"),
                guard(e.version(&t).await),
                None,
                None,
            )
            .await
            .unwrap();
        let second =
            e.fs.edit_text(&t, edit("two", "TWO"), guard(first.version), None, None)
                .await
                .unwrap();
        assert_eq!(second.after, "ONE TWO");
        assert_eq!(e.bytes("a.txt").await, "ONE TWO");
    }
    #[tokio::test]
    async fn concurrent_write_vs_edit_at_the_same_version_one_wins_the_other_is_stale() {
        let e = Env::new();
        let t = e.file("a.txt", "base").await;
        let v = e.version(&t).await;
        let (a, b) = tokio::join!(
            e.fs.write_text(&t, "written", replace(v.clone()), None, None),
            e.fs.edit_text(&t, edit("base", "edited"), guard(v), None, None)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        if a.is_err() {
            error(a, "FS_STALE_VERSION");
        } else {
            error(b, "FS_STALE_VERSION");
        }
        assert_eq!(e.fs.mutation_lock_count(), 0);
    }
}
mod symlink_identity {
    use super::*;
    #[tokio::test]
    async fn two_paths_to_the_same_file_via_a_symlink_share_one_version_and_write_the_real_target()
    {
        let e = Env::new();
        let real = e.file("real.txt", "hello").await;
        fs::symlink(e.path("real.txt"), e.path("link.txt"))
            .await
            .unwrap();
        let link = e.target("link.txt").await;
        assert_eq!(link.target_key, real.target_key);
        e.fs.edit_text(
            &link,
            edit("hello", "bye"),
            guard(e.version(&real).await),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(e.bytes("real.txt").await, "bye");
        assert!(
            fs::symlink_metadata(e.path("link.txt"))
                .await
                .unwrap()
                .is_symlink()
        );
    }
    #[tokio::test]
    async fn a_stale_change_is_detected_across_both_paths() {
        let e = Env::new();
        let real = e.file("real.txt", "hello").await;
        fs::symlink(e.path("real.txt"), e.path("link.txt"))
            .await
            .unwrap();
        let v = e.version(&real).await;
        e.file("real.txt", "changed").await;
        error(
            e.fs.edit_text(
                &e.target("link.txt").await,
                edit("hello", "bye"),
                guard(v),
                None,
                None,
            )
            .await,
            "FS_STALE_VERSION",
        );
    }
}
#[tokio::test]
async fn disposing_the_fiber_withdraws_ctx_fs() {
    let e = Env::new();
    let k = Kernel::start();
    let c = k.root();
    let f = c
        .plugin::<FsLocalPlugin>(serde_json::json!({"cwd":e.path("")}))
        .unwrap();
    f.settled().await.unwrap();
    assert!(c.get::<Fs>().is_some());
    f.dispose().await;
    assert!(c.get::<Fs>().is_none());
    k.shutdown().await;
}
