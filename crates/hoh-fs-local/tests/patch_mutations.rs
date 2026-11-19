mod common;
use common::*;
use hoh_error::{BoxError, SharedError};
use hoh_fs::*;
use hoh_fs_patch::PatchHunk;
use hoh_timeout::AbortController;
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
fn hunks(old: &str, new: &str, context: Option<&str>) -> Vec<PatchHunk> {
    vec![PatchHunk {
        line_number: 3,
        old_lines: vec![old.into()],
        new_lines: vec![new.into()],
        is_end_of_file: false,
        context: context.map(str::to_owned),
    }]
}
fn patch() -> Vec<PatchHunk> {
    hunks("old", "new", None)
}
#[tokio::test]
async fn keeps_storage_ending_and_returns_lf_bases() {
    for ending in ["\n", "\r\n"] {
        let e = Env::new();
        let t = e.file("a", format!("old{ending}tail{ending}")).await;
        let r =
            e.fs.patch_text(&t, &patch(), guard(e.version(&t).await), None, None)
                .await
                .unwrap();
        assert_eq!(
            r,
            FsEditOutcome {
                version: e.version(&t).await,
                before: "old\ntail\n".into(),
                after: "new\ntail\n".into()
            }
        );
        assert_eq!(e.bytes("a").await, format!("new{ending}tail{ending}"));
    }
}
#[tokio::test]
async fn reports_the_unmatched_hunk_stage_for_anchor_without_writing() {
    for context in [None, Some("missing")] {
        let e = Env::new();
        let t = e.file("a", "original\n").await;
        let err = error(
            e.fs.patch_text(&t, &hunks("old", "new", context), None, None, None)
                .await,
            "FS_PATCH_MISMATCH",
        );
        assert!(err.to_string().contains(&format!(
            "hunk at patch line 3 ({})",
            if context.is_some() {
                "context"
            } else {
                "oldLines"
            }
        )));
        assert_eq!(e.bytes("a").await, "original\n");
    }
}
#[tokio::test]
async fn cancels_staged_publication_and_leaves_the_original_file() {
    let e = Env::new();
    let t = e.file("a", "old\n").await;
    let c = Arc::new(AbortController::new());
    let controller = c.clone();
    e.hooks().inspect_temp = Some(Arc::new(move |_| {
        controller.abort();
        Box::pin(async { Ok(()) })
    }));
    error(
        e.fs.patch_text(&t, &patch(), None, Some(c.signal()), None)
            .await,
        "FS_ABORTED",
    );
    assert_eq!(e.bytes("a").await, "old\n");
}
async fn run(
    e: &Env,
    operation: &str,
    t: &FsTarget,
    d: &FsTarget,
    g: Option<FsVersionGuard>,
    s: Option<hoh_timeout::AbortSignal>,
) -> Result<(), BoxError> {
    match operation {
        "patch" => {
            e.fs.patch_text(t, &patch(), g, s, None).await?;
        }
        "delete" => {
            e.fs.delete_file(t, g, s, None).await?;
        }
        _ => {
            e.fs.move_file(
                t,
                d,
                FsMoveOptions {
                    expected_source: g,
                    ..Default::default()
                },
                s,
                None,
            )
            .await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn rejects_missing_directory_and_stale_sources() {
    for op in ["patch", "delete", "move"] {
        let e = Env::new();
        let d = e.file("destination", "untouched").await;
        let missing = e.target("missing").await;
        error(
            run(&e, op, &missing, &d, None, None).await,
            "FS_STALE_VERSION",
        );
        error(
            run(&e, op, &missing, &d, guard("old".into()), None).await,
            "FS_STALE_VERSION",
        );
        tokio::fs::create_dir(e.path("directory")).await.unwrap();
        error(
            run(&e, op, &e.target("directory").await, &d, None, None).await,
            "FS_NOT_REGULAR_FILE",
        );
        let t = e.file("a", "old\n").await;
        error(
            run(&e, op, &t, &d, guard("stale".into()), None).await,
            "FS_STALE_VERSION",
        );
        assert_eq!(e.bytes("a").await, "old\n");
        assert_eq!(e.bytes("destination").await, "untouched");
    }
}
#[tokio::test]
async fn removes_a_file_and_returns_basis_at_limit() {
    for (content, limit, before) in [
        ("old\r\n", 1024, Some("old\n")),
        ("abcd", 4, None),
        ("abcde", 4, None),
        ("\0binary", 1024, None),
    ] {
        let e = Env::limit(limit);
        let t = e.file("a", content).await;
        assert_eq!(
            e.fs.delete_file(&t, guard(e.version(&t).await), None, None)
                .await
                .unwrap()
                .before
                .as_deref(),
            before
        );
        assert!(!tokio::fs::try_exists(e.path("a")).await.unwrap());
    }
}
#[tokio::test]
async fn maps_removal_failure_and_keeps_the_file() {
    for code in ["EACCES", "EPERM", "EIO"] {
        let e = Env::new();
        let t = e.file("a", "old").await;
        e.hooks().remove_file = Some(Arc::new(move |_| {
            Box::pin(async move { Err(errno(code, "injected removal failure")) })
        }));
        error(
            e.fs.delete_file(&t, None, None, None).await,
            if code == "EIO" {
                "FS_IO_ERROR"
            } else {
                "FS_PERMISSION_DENIED"
            },
        );
        assert_eq!(e.bytes("a").await, "old");
    }
}
#[tokio::test]
async fn publishes_source_bytes_and_mode() {
    for exists in [false, true] {
        let e = Env::new();
        let s = e.file("a", "old\r\ntail\r\n").await;
        let d = e.target("nested/b").await;
        tokio::fs::create_dir(e.path("nested")).await.unwrap();
        if exists {
            tokio::fs::write(e.path("nested/b"), "destination\n")
                .await
                .unwrap();
        }
        tokio::fs::set_permissions(e.path("a"), std::fs::Permissions::from_mode(0o640))
            .await
            .unwrap();
        let r =
            e.fs.move_file(
                &s,
                &d,
                FsMoveOptions {
                    expected_source: guard(e.version(&s).await),
                    expected_destination: if exists {
                        replace(e.version(&d).await)
                    } else {
                        Some(FsWriteIntent::CreateIfAbsent)
                    },
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        let FsMoveOutcome::Deleted {
            source_before,
            destination,
        } = r
        else {
            panic!("source not deleted")
        };
        assert_eq!(source_before.as_deref(), Some("old\ntail\n"));
        assert_eq!(destination.version, e.version(&d).await);
        assert_eq!(
            destination.operation,
            if exists {
                FsWriteOperation::Update
            } else {
                FsWriteOperation::Create
            }
        );
        assert_eq!(
            destination.before.as_deref(),
            exists.then_some("destination\n")
        );
        assert_eq!(destination.after, "old\ntail\n");
        assert_eq!(e.bytes("nested/b").await, "old\r\ntail\r\n");
        assert!(!tokio::fs::try_exists(e.path("a")).await.unwrap());
        assert_eq!(
            tokio::fs::metadata(e.path("nested/b"))
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }
}
#[tokio::test]
async fn bounds_the_source_basis_by_diff_basis_max_bytes_while_still_publishing_the_whole_text() {
    let e = Env::limit(16);
    let text = format!("{}\n", "x".repeat(32));
    let s = e.file("large", &text).await;
    let d = e.target("moved").await;
    let r =
        e.fs.move_file(&s, &d, Default::default(), None, None)
            .await
            .unwrap();
    let FsMoveOutcome::Deleted {
        source_before,
        destination,
    } = r
    else {
        panic!()
    };
    assert!(source_before.is_none());
    assert!(destination.before.is_none());
    assert_eq!(destination.after, text);
    assert_eq!(e.bytes("moved").await, text);
}
#[tokio::test]
async fn creates_missing_parent_directories_and_applies_hunks_before_publication() {
    let e = Env::new();
    let s = e.file("a", "old\r\n").await;
    let d = e.target("new/sub/b").await;
    let r =
        e.fs.move_file(
            &s,
            &d,
            FsMoveOptions {
                hunks: Some(patch()),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    let FsMoveOutcome::Deleted { destination, .. } = r else {
        panic!()
    };
    assert_eq!(destination.after, "new\n");
    assert_eq!(destination.operation, FsWriteOperation::Create);
    assert!(destination.before.is_none());
    assert_eq!(e.bytes("new/sub/b").await, "new\r\n");
    assert!(!tokio::fs::try_exists(e.path("a")).await.unwrap());
}
#[tokio::test]
async fn declines_an_over_limit_destination_basis() {
    for large in [false, true] {
        let e = Env::limit(4);
        let s = e.file("a", if large { "large" } else { "a" }).await;
        let d = e.file("b", if large { "b" } else { "large" }).await;
        let FsMoveOutcome::Deleted { destination, .. } =
            e.fs.move_file(&s, &d, Default::default(), None, None)
                .await
                .unwrap()
        else {
            panic!()
        };
        assert!(destination.before.is_none());
    }
}
#[tokio::test]
async fn refuses_same_identities_destination_conflicts_directories_and_mismatched_hunks_without_changing_either_file()
 {
    let e = Env::new();
    let s = e.file("a", "old\n").await;
    let d = e.file("b", "destination").await;
    error(
        e.fs.move_file(&s, &e.target("./a").await, Default::default(), None, None)
            .await,
        "FS_SAME_TARGET",
    );
    for (t, expected, code) in [
        (&d, Some(FsWriteIntent::CreateIfAbsent), "FS_NOT_OBSERVED"),
        (&d, replace("stale".into()), "FS_STALE_VERSION"),
        (
            &e.target("absent").await,
            replace("stale".into()),
            "FS_STALE_VERSION",
        ),
    ] {
        error(
            e.fs.move_file(
                &s,
                t,
                FsMoveOptions {
                    expected_destination: expected,
                    ..Default::default()
                },
                None,
                None,
            )
            .await,
            code,
        );
    }
    tokio::fs::create_dir(e.path("directory")).await.unwrap();
    error(
        e.fs.move_file(
            &s,
            &e.target("directory").await,
            Default::default(),
            None,
            None,
        )
        .await,
        "FS_NOT_REGULAR_FILE",
    );
    error(
        e.fs.move_file(
            &s,
            &d,
            FsMoveOptions {
                hunks: Some(hunks("missing", "new", None)),
                ..Default::default()
            },
            None,
            None,
        )
        .await,
        "FS_PATCH_MISMATCH",
    );
    assert_eq!(e.bytes("a").await, "old\n");
    assert_eq!(e.bytes("b").await, "destination");
}
#[tokio::test]
async fn reports_source_failure_after_publishing_the_destination() {
    for code in ["EACCES", "EIO"] {
        let e = Env::new();
        let s = e.file("a", "old\n").await;
        let d = e.file("b", "previous").await;
        e.hooks().remove_file = Some(Arc::new(move |_| {
            Box::pin(async move { Err(errno(code, "injected")) })
        }));
        let r =
            e.fs.move_file(&s, &d, Default::default(), None, None)
                .await
                .unwrap();
        let FsMoveOutcome::Retained {
            source_before,
            destination,
            source_failure,
        } = r
        else {
            panic!()
        };
        assert_eq!(source_before.as_deref(), Some("old\n"));
        assert_eq!(
            destination,
            FsWriteOutcome {
                operation: FsWriteOperation::Update,
                version: e.version(&d).await,
                before: Some("previous".into()),
                after: "old\n".into()
            }
        );
        assert_eq!(
            source_failure.code,
            if code == "EACCES" {
                FsErrorCode::PermissionDenied
            } else {
                FsErrorCode::IoError
            }
        );
        assert_eq!(
            source_failure.message,
            format!("cannot delete \"{}\": injected", s.display_path)
        );
        assert_eq!(e.bytes("a").await, "old\n");
        assert_eq!(e.bytes("b").await, "old\n");
    }
}
#[tokio::test]
async fn keeps_both_files_and_releases_both_locks_after_destination_publication_fails() {
    let e = Env::new();
    let s = e.file("a", "old\n").await;
    let d = e.file("b", "previous").await;
    e.hooks().inspect_temp = Some(Arc::new(|_| {
        Box::pin(async { Err(std::io::Error::other("staging failure").into()) })
    }));
    assert!(
        e.fs.move_file(&s, &d, Default::default(), None, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("staging failure")
    );
    assert_eq!(e.bytes("a").await, "old\n");
    assert_eq!(e.bytes("b").await, "previous");
    e.hooks().inspect_temp = None;
    let (a, b) = tokio::join!(
        e.fs.write_text(&s, "left", None, None, None),
        e.fs.write_text(&d, "right", None, None, None)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(e.bytes("a").await, "left");
    assert_eq!(e.bytes("b").await, "right");
    assert_eq!(e.fs.mutation_lock_count(), 0);
}
#[tokio::test]
async fn ignores_cancellation_after_destination_staging_starts_and_reports_both_completed_effects()
{
    let e = Env::new();
    let s = e.file("a", "old").await;
    let d = e.target("b").await;
    let c = Arc::new(AbortController::new());
    let ac = c.clone();
    e.hooks().inspect_temp = Some(Arc::new(move |_| {
        ac.abort();
        Box::pin(async { Ok(()) })
    }));
    assert!(matches!(
        e.fs.move_file(&s, &d, Default::default(), Some(c.signal()), None)
            .await
            .unwrap(),
        FsMoveOutcome::Deleted { .. }
    ));
    assert_eq!(e.bytes("b").await, "old");
    assert!(!tokio::fs::try_exists(e.path("a")).await.unwrap());
}
#[tokio::test]
async fn orders_opposite_direction_moves_without_deadlock_and_retains_fifo_ownership() {
    let e = Env::new();
    let a = e.file("a", "left").await;
    let b = e.file("b", "right").await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let (n, r, c) = (entered.clone(), release.clone(), calls.clone());
    e.hooks().inspect_temp = Some(Arc::new(move |_| {
        let (n, r, c) = (n.clone(), r.clone(), c.clone());
        Box::pin(async move {
            if c.fetch_add(1, Ordering::SeqCst) == 0 {
                n.notify_one();
                r.notified().await;
            }
            Ok(())
        })
    }));
    let forward = e.fs.move_file(&a, &b, Default::default(), None, None);
    entered.notified().await;
    let backward = e.fs.move_file(&b, &a, Default::default(), None, None);
    release.notify_one();
    let (x, y) = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        tokio::join!(forward, backward)
    })
    .await
    .unwrap();
    assert!(matches!(x.unwrap(), FsMoveOutcome::Deleted { .. }));
    assert!(matches!(y.unwrap(), FsMoveOutcome::Deleted { .. }));
    assert_eq!(e.bytes("a").await, "left");
    assert!(!tokio::fs::try_exists(e.path("b")).await.unwrap());
}
#[tokio::test]
async fn moves_the_resolved_symlink_target_while_leaving_the_source_link() {
    let e = Env::new();
    e.file("a", "source").await;
    tokio::fs::symlink(e.path("a"), e.path("link"))
        .await
        .unwrap();
    assert_eq!(
        e.fs.lstat("link", None, None)
            .await
            .unwrap()
            .unwrap()
            .r#type,
        FsPathType::Symlink
    );
    e.fs.move_file(
        &e.target("link").await,
        &e.target("b").await,
        Default::default(),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(e.bytes("b").await, "source");
    assert_eq!(
        e.fs.lstat("link", None, None)
            .await
            .unwrap()
            .unwrap()
            .r#type,
        FsPathType::Symlink
    );
    assert!(!tokio::fs::try_exists(e.path("a")).await.unwrap());
}
#[tokio::test]
async fn rejects_pre_cancellation_without_changing_disk() {
    for op in ["patch", "delete", "move"] {
        let e = Env::new();
        let s = e.file("a", "old\n").await;
        let d = e.file("b", "destination").await;
        error(
            run(&e, op, &s, &d, None, Some(aborted())).await,
            "FS_ABORTED",
        );
        assert_eq!(e.bytes("a").await, "old\n");
        assert_eq!(e.bytes("b").await, "destination");
    }
}
#[tokio::test]
async fn observes_cancellation_at_the_last_pre_commit_checkpoint() {
    for op in ["delete", "move"] {
        let e = Env::new();
        let s = e.file("a", "old").await;
        let d = e.file("b", "destination").await;
        let c = Arc::new(AbortController::new());
        let ac = c.clone();
        e.hooks().inspect_before_mutation = Some(Arc::new(move |_| {
            ac.abort();
            Box::pin(async { Ok(()) })
        }));
        error(
            run(&e, op, &s, &d, None, Some(c.signal())).await,
            "FS_ABORTED",
        );
        assert_eq!(e.bytes("a").await, "old");
        assert_eq!(e.bytes("b").await, "destination");
        *e.hooks() = Default::default();
        assert!(matches!(
            e.fs.move_file(&s, &d, Default::default(), None, None)
                .await
                .unwrap(),
            FsMoveOutcome::Deleted { .. }
        ));
    }
}
#[tokio::test]
async fn rethrows_an_unexpected_source_removal_defect_after_destination_publication() {
    let e = Env::new();
    let s = e.file("a", "source").await;
    let d = e.target("b").await;
    let sentinel = SharedError::new(std::io::Error::other("unexpected removal defect"));
    let sent = sentinel.clone();
    e.hooks().remove_file_override = Some(Arc::new(move |_, _| {
        let sent = sent.clone();
        Box::pin(async move { Err(sent.into()) })
    }));
    let err =
        e.fs.move_file(&s, &d, Default::default(), None, None)
            .await
            .unwrap_err();
    assert!(err.downcast_ref::<SharedError>().unwrap().ptr_eq(&sentinel));
    assert_eq!(e.bytes("a").await, "source");
    assert_eq!(e.bytes("b").await, "source");
}
