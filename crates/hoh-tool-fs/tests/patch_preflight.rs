//! Whole-patch admission rejects all writes while preserving prior observation state.

mod support;

use std::sync::Arc;

use hoh_error::HarnessError;
use hoh_fs::{FsError, FsErrorCode, FsPathInfo, FsPathType, FsVersionGuard};
use hoh_tool_fs::patch::PatchExecutionReport;
use hoh_tool_fs::patch::PatchOperationKind::{Add, Delete, Move, Update};
use support::patch::*;

fn not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

fn code(error: &hoh_error::BoxError) -> String {
    HarnessError::of(error.as_ref()).unwrap().code().to_owned()
}

async fn guard(t: &T, name: &str) -> Option<FsVersionGuard> {
    Some(FsVersionGuard {
        version: t.version(name).await,
    })
}

fn set_hooks(t: &T, set: impl FnOnce(&mut Hooks)) {
    set(&mut support::lock(&t.spy.hooks));
}

#[tokio::test]
async fn reports_syntax_errors_before_resolving_paths() {
    let t = setup(PatchOptions::default()).await;
    let report = t.run("invalid").await.unwrap();
    assert_eq!(
        report,
        PatchExecutionReport::Syntax {
            failure: failure(
                "PATCH_SYNTAX",
                "Invalid patch at line 1: Expected '*** Begin Patch'"
            )
        }
    );
    assert_eq!(
        t.spy
            .resolve_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(t.is_unobserved_empty());
    assert!(not_found(&t.bytes("a").unwrap_err()));
    assert_eq!(
        code(&t.edit_intent("a").await.unwrap_err()),
        "FS_NOT_OBSERVED"
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn collects_independent_unread_and_confirmed_absent_failures_and_skips_valid_writes() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", false).await;
    t.absent("b").await;
    let a = t.record(Update, "a", 0, 2, None);
    let b = t.record(Update, "b", 1, 6, None);
    let report = t
        .run(&patch(&[&update("a"), &update("b"), &add("c")]))
        .await
        .unwrap();
    assert_eq!(
        report,
        preflight(vec![
            refusal(
                a.clone(),
                "FS_NOT_OBSERVED",
                &format!(
                    "cannot modify \"{}\": file has not been read — read the file, then retry",
                    a.path
                ),
            ),
            refusal(
                b.clone(),
                "FS_NOT_FOUND",
                &format!("cannot edit \"{}\": not found", b.path)
            ),
        ])
    );
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert!(not_found(&t.bytes("c").unwrap_err()));
    assert!(t.is_unobserved_empty());
    assert_eq!(
        code(&t.edit_intent("a").await.unwrap_err()),
        "FS_NOT_OBSERVED"
    );
    assert_eq!(code(&t.edit_intent("b").await.unwrap_err()), "FS_NOT_FOUND");
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn reports_the_hunk_line_and_stage_preview_without_simulating_a_failed_update() {
    let long = "🙂".repeat(81);
    let preview = format!("{}…", "🙂".repeat(80));
    for (pattern, marker, stage, preview) in [
        ("missing".to_owned(), "@@", "oldLines", "missing".to_owned()),
        (long, "@@", "oldLines", preview),
        (
            "old".to_owned(),
            "@@ absent anchor",
            "context",
            "absent anchor".to_owned(),
        ),
    ] {
        let t = setup(PatchOptions::default()).await;
        t.file("a", "old\n", true).await;
        let a = t.record(Update, "a", 0, 2, None);
        let report = t
            .run(&patch(&[
                &format!("*** Update File: a\n{marker}\n-{pattern}\n+new"),
                &update("a"),
            ]))
            .await
            .unwrap();
        let mut expected = refusal(
            a.clone(),
            "FS_PATCH_MISMATCH",
            &format!(
                "cannot patch \"{}\": hunk at patch line 3 ({stage}) does not match — {preview}",
                a.path
            ),
        );
        expected.failure.hunk_line = Some(3);
        assert_eq!(report, preflight(vec![expected]));
        assert_eq!(t.bytes("a").unwrap(), "old\n");
        assert!(t.is_unobserved_empty());
        assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
        t.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn rejects_a_source_the_provider_reports_as_a_symbolic_link() {
    for kind in [Delete, Move] {
        let t = setup(PatchOptions::default()).await;
        t.file("a", "old\n", true).await;
        set_hooks(&t, |hooks| {
            hooks.lstat = Some(FsPathInfo {
                r#type: FsPathType::Symlink,
                version: "link".into(),
                size: None,
            });
        });
        let a = t.record(kind, "a", 0, 2, (kind == Move).then_some("b"));
        let text = if kind == Delete {
            remove("a")
        } else {
            mv("a", "b")
        };
        assert_eq!(
            t.run(&patch(&[&text])).await.unwrap(),
            preflight(vec![refusal(
                a.clone(),
                "PATCH_SYMLINK_SOURCE",
                &format!(
                    "cannot {} \"{}\": path is a symbolic link — operate on its target path instead",
                    kind.as_str(),
                    a.path
                ),
            )])
        );
        assert_eq!(t.bytes("a").unwrap(), "old\n");
        assert!(t.is_unobserved_empty());
        t.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn rejects_a_symbolic_link_source_without_changing_its_target() {
    for kind in [Delete, Move] {
        let t = setup(PatchOptions::default()).await;
        t.file("a", "old\n", true).await;
        std::os::unix::fs::symlink("a", t.path("link")).unwrap();
        let a = t.record(kind, "link", 0, 2, (kind == Move).then_some("b"));
        let text = if kind == Delete {
            remove("link")
        } else {
            mv("link", "b")
        };
        assert_eq!(
            t.run(&patch(&[&text])).await.unwrap(),
            preflight(vec![refusal(
                a.clone(),
                "PATCH_SYMLINK_SOURCE",
                &format!(
                    "cannot {} \"{}\": path is a symbolic link — operate on its target path instead",
                    kind.as_str(),
                    a.path
                ),
            )])
        );
        assert_eq!(t.bytes("link").unwrap(), "old\n");
        assert!(t.is_unobserved_empty());
        assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
        t.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn rejects_a_move_to_an_alias_of_its_source() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    let a = t.record(Move, "a", 0, 2, Some("a"));
    assert_eq!(
        t.run(&patch(&[&mv("a", "./a")])).await.unwrap(),
        preflight(vec![refusal(
            a.clone(),
            "FS_SAME_TARGET",
            &format!("cannot move \"{}\" to itself", a.path),
        )])
    );
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert!(t.is_unobserved_empty());
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn attributes_an_unread_destination_to_its_resolved_path() {
    for kind in [Add, Move] {
        let t = setup(PatchOptions::default()).await;
        t.file("a", "old\n", true).await;
        t.file("b", "existing\n", false).await;
        let operation = if kind == Add {
            t.record(Add, "b", 0, 2, None)
        } else {
            t.record(Move, "a", 0, 2, Some("b"))
        };
        let b = t.record(Add, "b", 0, 2, None);
        let text = if kind == Add { add("b") } else { mv("a", "b") };
        assert_eq!(
            t.run(&patch(&[&text])).await.unwrap(),
            preflight(vec![refusal(
                operation,
                "FS_NOT_OBSERVED",
                &format!(
                    "cannot modify \"{}\": file has not been read — read the file, then retry",
                    b.path
                ),
            )])
        );
        assert_eq!(t.bytes("a").unwrap(), "old\n");
        assert_eq!(t.bytes("b").unwrap(), "existing\n");
        assert!(t.is_unobserved_empty());
        assert_eq!(
            code(&t.edit_intent("b").await.unwrap_err()),
            "FS_NOT_OBSERVED"
        );
        t.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn rejects_non_regular_targets_with_bare_guards() {
    for kind in [Update, Delete, Add, Move] {
        let t = setup(PatchOptions {
            without_policy: true,
            ..PatchOptions::default()
        })
        .await;
        std::fs::create_dir(t.path("directory")).unwrap();
        t.file("a", "old\n", true).await;
        let operation = if kind == Move {
            t.record(kind, "a", 0, 2, Some("directory"))
        } else {
            t.record(kind, "directory", 0, 2, None)
        };
        let text = match kind {
            Add => add("directory"),
            Delete => remove("directory"),
            Move => mv("a", "directory"),
            Update => update("directory"),
        };
        assert_eq!(
            t.run(&patch(&[&text])).await.unwrap(),
            preflight(vec![refusal(
                operation,
                "FS_NOT_REGULAR_FILE",
                &format!(
                    "cannot modify \"{}\": not a regular file",
                    t.path_text("directory")
                ),
            )]),
            "{kind:?}"
        );
        assert_eq!(t.bytes("a").unwrap(), "old\n");
        let target = t.target("directory").await;
        let info = hoh_fs::FileSystem::stat(&*t.local, &target, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(info.r#type, hoh_fs::FsType::Directory);
        assert!(t.is_unobserved_empty());
        assert_eq!(t.edit_intent("a").await.unwrap(), None);
        t.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn rejects_a_stale_observed_version() {
    for kind in [Update, Add] {
        let t = setup(PatchOptions::default()).await;
        t.file("a", "old\n", true).await;
        let observed = t.edit_intent("a").await.unwrap();
        std::fs::write(t.path("a"), "external edit\n").unwrap();
        let a = t.record(kind, "a", 0, 2, None);
        let text = if kind == Add { add("a") } else { update("a") };
        assert_eq!(
            t.run(&patch(&[&text])).await.unwrap(),
            preflight(vec![refusal(
                a.clone(),
                "FS_STALE_VERSION",
                &format!(
                    "cannot {} \"{}\": file changed since it was read — re-read the file, then retry",
                    if kind == Add { "write" } else { "edit" },
                    a.path
                ),
            )]),
            "{kind:?}"
        );
        assert_eq!(t.bytes("a").unwrap(), "external edit\n");
        assert!(t.is_unobserved_empty());
        assert_eq!(t.edit_intent("a").await.unwrap(), observed);
        t.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn rejects_a_nul_byte_beyond_the_read_sampling_window_before_any_write() {
    let t = setup(PatchOptions::default()).await;
    let binary = format!("{}\0tail\n", "text\n".repeat(2048));
    t.file("a", &binary, true).await;
    assert_eq!(
        t.run(&patch(&[
            &add("created"),
            "*** Update File: a\n@@\n-text\n+new"
        ]))
        .await
        .unwrap(),
        preflight(vec![refusal(
            t.record(Update, "a", 1, 4, None),
            "FS_NOT_TEXT",
            &format!("cannot patch \"{}\": binary file", t.path_text("a")),
        )])
    );
    assert!(not_found(&t.bytes("created").unwrap_err()));
    assert_eq!(t.bytes("a").unwrap(), binary);
    assert!(t.is_unobserved_empty());
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn preserves_a_preflight_read_failure() {
    for code in [FsErrorCode::NotText, FsErrorCode::TooLarge] {
        let t = setup(PatchOptions::default()).await;
        t.file("a", "old\n", true).await;
        set_hooks(&t, |hooks| {
            hooks.read_text = Some(Arc::new(move |()| {
                Some(FsError::new("cannot read fixture", code).into())
            }));
        });
        assert_eq!(
            t.run(&patch(&[&update("a")])).await.unwrap(),
            preflight(vec![refusal(
                t.record(Update, "a", 0, 2, None),
                code.as_str(),
                "cannot read fixture"
            )])
        );
        assert_eq!(t.bytes("a").unwrap(), "old\n");
        assert!(t.is_unobserved_empty());
        assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
        t.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn simulates_the_missing_source_after_delete_or_move_without_performing_it() {
    for kind in [Delete, Move] {
        let t = setup(PatchOptions::default()).await;
        t.file("a", "old\n", true).await;
        let a = t.record(Update, "a", 1, if kind == Move { 7 } else { 3 }, None);
        let first = if kind == Delete {
            remove("a")
        } else {
            mv("a", "b")
        };
        assert_eq!(
            t.run(&patch(&[&first, &update("a")])).await.unwrap(),
            preflight(vec![refusal(
                a.clone(),
                "FS_NOT_FOUND",
                &format!("cannot edit \"{}\": not found", a.path)
            )]),
            "{kind:?}"
        );
        assert_eq!(t.bytes("a").unwrap(), "old\n");
        assert!(t.is_unobserved_empty());
        assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
        t.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn propagates_programming_defects_from_preflight_without_writes() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    set_hooks(&t, |hooks| {
        hooks.read_text = Some(Arc::new(|()| Some("provider defect".into())));
    });
    let error = t.run(&patch(&[&update("a")])).await.unwrap_err();
    assert_eq!(error.to_string(), "provider defect");
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert!(t.is_unobserved_empty());
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn rethrows_a_resolution_defect_instead_of_labelling_it_a_patch_syntax_failure() {
    let t = setup(PatchOptions::default()).await;
    set_hooks(&t, |hooks| {
        hooks.resolve = Some(Arc::new(|_| Some("resolver defect".into())));
    });
    let error = t.run(&patch(&[&add("a")])).await.unwrap_err();
    assert_eq!(error.to_string(), "resolver defect");
    set_hooks(&t, |hooks| hooks.resolve = None);
    assert!(not_found(&t.bytes("a").unwrap_err()));
    assert!(t.is_unobserved_empty());
    assert_eq!(
        code(&t.edit_intent("a").await.unwrap_err()),
        "FS_NOT_OBSERVED"
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_repeated_deletion_against_simulated_absence() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    let a = t.record(Delete, "a", 0, 2, None);
    assert_eq!(
        t.run(&patch(&[&remove("a"), &remove("a")])).await.unwrap(),
        preflight(vec![refusal(
            t.record(Delete, "a", 1, 3, None),
            "FS_NOT_FOUND",
            &format!("cannot delete \"{}\": not found", a.path),
        )])
    );
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert!(t.is_unobserved_empty());
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn preserves_source_simulation_after_a_move_destination_fails_admission() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    t.file("b", "unread\n", false).await;
    let a = t.record(Move, "a", 0, 2, Some("b"));
    assert_eq!(
        t.run(&patch(&[&mv("a", "b"), &update("a")])).await.unwrap(),
        preflight(vec![refusal(
            a.clone(),
            "FS_NOT_OBSERVED",
            &format!(
                "cannot modify \"{}\": file has not been read — read the file, then retry",
                a.destination.clone().unwrap()
            ),
        )])
    );
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert_eq!(t.bytes("b").unwrap(), "unread\n");
    assert!(t.is_unobserved_empty());
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn names_the_missing_move_source_operation() {
    let t = setup(PatchOptions {
        without_policy: true,
        ..PatchOptions::default()
    })
    .await;
    assert_eq!(
        t.run(&patch(&[&mv("missing", "destination")]))
            .await
            .unwrap(),
        preflight(vec![refusal(
            t.record(Move, "missing", 0, 2, Some("destination")),
            "FS_NOT_FOUND",
            &format!("cannot move \"{}\": not found", t.path_text("missing")),
        )])
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn applies_unread_externally_changed_and_overwritten_targets_on_context_match_alone_without_a_policy()
 {
    let t = setup(PatchOptions {
        without_policy: true,
        ..PatchOptions::default()
    })
    .await;
    t.file("a", "old\n", false).await;
    t.file("b", "old\n", true).await;
    std::fs::write(t.path("b"), "old\nexternal\n").unwrap();
    t.file("c", "existing\n", false).await;
    let report = t
        .run(&patch(&[
            &update("a"),
            &update("b"),
            &add("c"),
            &remove("a"),
        ]))
        .await
        .unwrap();
    assert!(
        matches!(report, PatchExecutionReport::Commit { .. }),
        "{report:?}"
    );
    assert!(not_found(&t.bytes("a").unwrap_err()));
    assert_eq!(t.bytes("b").unwrap(), "new\nexternal\n");
    assert_eq!(t.bytes("c").unwrap(), "new\n");
    // Commits still publish observations for a policy mounted later.
    assert_eq!(
        t.observation_kinds(),
        [
            (t.path_text("a"), "present"),
            (t.path_text("b"), "present"),
            (t.path_text("c"), "present"),
            (t.path_text("a"), "absent"),
        ]
    );
    assert_eq!(t.edit_intent("b").await.unwrap(), None);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn still_rejects_a_mismatched_hunk_without_a_policy() {
    let t = setup(PatchOptions {
        without_policy: true,
        ..PatchOptions::default()
    })
    .await;
    t.file("a", "other\n", false).await;
    let a = t.record(Update, "a", 0, 2, None);
    let mut expected = refusal(
        a.clone(),
        "FS_PATCH_MISMATCH",
        &format!(
            "cannot patch \"{}\": hunk at patch line 3 (oldLines) does not match — old",
            a.path
        ),
    );
    expected.failure.hunk_line = Some(3);
    assert_eq!(
        t.run(&patch(&[&update("a")])).await.unwrap(),
        preflight(vec![expected])
    );
    assert_eq!(t.bytes("a").unwrap(), "other\n");
    t.kernel.shutdown().await;
}
