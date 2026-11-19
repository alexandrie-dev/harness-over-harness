//! Sequential commits retain known effects and distinguish unchanged from uncertain failures.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hoh_error::HarnessError;
use hoh_fs::{FsError, FsErrorCode, FsObserved, FsVersionGuard};
use hoh_sandbox::SandboxMode;
use hoh_tool_fs::patch::PatchOperationKind::{Add, Delete, Move, Update};
use hoh_tool_fs::patch::{PatchExecutionReport, PatchOperationOutcome};
use support::patch::*;

fn not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

async fn guard(t: &T, name: &str) -> Option<FsVersionGuard> {
    Some(FsVersionGuard {
        version: t.version(name).await,
    })
}

fn set_hooks(t: &T, set: impl FnOnce(&mut Hooks)) {
    set(&mut support::lock(&t.spy.hooks));
}

fn failed(
    operation: hoh_tool_fs::patch::PatchOperationRecord,
    code: &str,
    message: &str,
    changes: Vec<hoh_tool_fs::patch::PatchFileChange>,
) -> PatchOperationOutcome {
    PatchOperationOutcome::Failed {
        operation,
        failure: failure(code, message),
        changes,
    }
}

fn unknown(
    operation: hoh_tool_fs::patch::PatchOperationRecord,
    code: &str,
    message: &str,
) -> PatchOperationOutcome {
    PatchOperationOutcome::Unknown {
        operation,
        failure: failure(code, message),
    }
}

#[tokio::test]
async fn retains_the_first_commit_when_removal_fails_and_observes_only_published_changes() {
    for kind in [Delete, Move] {
        let t = setup(PatchOptions::default()).await;
        t.file("source", "old\n", true).await;
        let source_guard = t.edit_intent("source").await.unwrap();
        // The removal of the source is denied (the source `removeFile` of the provider internals).
        support::lock(&t.local.internals).remove_file = Some(Arc::new(|_path| {
            Box::pin(async { Err(HarnessError::new("removal denied", "EACCES").into()) })
        }));
        let a = t.record(Add, "a", 0, 2, None);
        let b = t.record(
            kind,
            "source",
            1,
            4,
            (kind == Move).then_some("destination"),
        );
        let c = t.record(Add, "c", 2, if kind == Move { 9 } else { 5 }, None);
        let destination = t.path_text("destination");
        let middle = if kind == Move {
            mv("source", "destination")
        } else {
            remove("source")
        };
        let report = t
            .run(&patch(&[&add("a"), &middle, &add("c")]))
            .await
            .unwrap();
        assert_eq!(
            report,
            commit(
                vec![
                    completed(a.clone(), vec![change(&a.path, None, Some("new\n"))]),
                    failed(
                        b.clone(),
                        "FS_PERMISSION_DENIED",
                        &format!("cannot delete \"{}\": removal denied", b.path),
                        if kind == Move {
                            vec![change(&destination, None, Some("new\n"))]
                        } else {
                            vec![]
                        },
                    ),
                    skipped(c),
                ],
                &if kind == Move {
                    vec![a.path.as_str(), destination.as_str()]
                } else {
                    vec![a.path.as_str()]
                },
                false,
            ),
            "{kind:?}"
        );
        assert_eq!(t.bytes("a").unwrap(), "new\n");
        assert_eq!(t.bytes("source").unwrap(), "old\n");
        assert!(not_found(&t.bytes("c").unwrap_err()));
        assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
        assert_eq!(t.edit_intent("source").await.unwrap(), source_guard);
        let expected: Vec<String> = if kind == Move {
            vec![a.path.clone(), destination.clone()]
        } else {
            vec![a.path.clone()]
        };
        assert_eq!(t.observed_paths(), expected);
        if kind == Move {
            assert_eq!(t.bytes("destination").unwrap(), "new\n");
            assert_eq!(
                t.edit_intent("destination").await.unwrap(),
                guard(&t, "destination").await
            );
        }
        t.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn stops_between_operations_after_cancellation_without_undoing_the_first_write() {
    let t = setup(PatchOptions::default()).await;
    let controller = t.controller.clone();
    t.ctx
        .on::<FsObserved>(move |_| {
            controller.abort();
        })
        .unwrap();
    let a = t.record(Add, "a", 0, 2, None);
    assert_eq!(
        t.run(&patch(&[&add("a"), &add("b"), &add("c")]))
            .await
            .unwrap(),
        commit(
            vec![
                completed(a.clone(), vec![change(&a.path, None, Some("new\n"))]),
                skipped(t.record(Add, "b", 1, 4, None)),
                skipped(t.record(Add, "c", 2, 6, None)),
            ],
            &[&a.path],
            true,
        )
    );
    assert_eq!(t.bytes("a").unwrap(), "new\n");
    assert!(not_found(&t.bytes("b").unwrap_err()));
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    let error = t.edit_intent("b").await.unwrap_err();
    assert_eq!(
        HarnessError::of(error.as_ref()).unwrap().code(),
        "FS_NOT_OBSERVED"
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn reports_cancellation_during_staged_writing_as_unknown_without_a_stat_comparison() {
    let t = setup(PatchOptions::default()).await;
    let writes = Arc::new(AtomicUsize::new(0));
    let controller = t.controller.clone();
    support::lock(&t.local.internals).inspect_temp = Some(Arc::new(move |_staged| {
        if writes.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
            controller.abort();
        }
        Box::pin(async { Ok(()) })
    }));
    let a = t.record(Add, "a", 0, 2, None);
    // The staged file never reaches its target, so a stat comparison would have called this failed.
    assert_eq!(
        t.run(&patch(&[&add("a"), &add("b"), &add("c")]))
            .await
            .unwrap(),
        commit(
            vec![
                completed(a.clone(), vec![change(&a.path, None, Some("new\n"))]),
                unknown(
                    t.record(Add, "b", 1, 4, None),
                    "FS_ABORTED",
                    "write aborted — re-read the affected files before retrying",
                ),
                skipped(t.record(Add, "c", 2, 6, None)),
            ],
            &[&a.path],
            true,
        )
    );
    assert_eq!(t.bytes("a").unwrap(), "new\n");
    assert!(not_found(&t.bytes("b").unwrap_err()));
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    let error = t.edit_intent("b").await.unwrap_err();
    assert_eq!(
        HarnessError::of(error.as_ref()).unwrap().code(),
        "FS_NOT_OBSERVED"
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn does_not_mark_a_fully_completed_patch_cancelled_when_its_final_observation_aborts() {
    let t = setup(PatchOptions::default()).await;
    let controller = t.controller.clone();
    t.ctx
        .on::<FsObserved>(move |_| {
            controller.abort();
        })
        .unwrap();
    let a = t.record(Add, "a", 0, 2, None);
    assert_eq!(
        t.run(&patch(&[&add("a")])).await.unwrap(),
        commit(
            vec![completed(
                a.clone(),
                vec![change(&a.path, None, Some("new\n"))]
            )],
            &[&a.path],
            false
        )
    );
    assert_eq!(t.bytes("a").unwrap(), "new\n");
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn reports_unknown_after_the_provider_changes_disk_and_loses_its_reply_without_recording_an_observation()
 {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    let observed = t.edit_intent("a").await.unwrap();
    set_hooks(&t, |hooks| {
        hooks.patch_text_lost_reply = Some(Arc::new(|()| {
            Some(FsError::new("reply lost", FsErrorCode::IoError).into())
        }));
    });
    assert_eq!(
        t.run(&patch(&[&update("a"), &add("b")])).await.unwrap(),
        commit(
            vec![
                unknown(
                    t.record(Update, "a", 0, 2, None),
                    "FS_IO_ERROR",
                    "reply lost — re-read the affected files before retrying",
                ),
                skipped(t.record(Add, "b", 1, 6, None)),
            ],
            &[],
            false,
        )
    );
    assert_eq!(t.bytes("a").unwrap(), "new\n");
    assert!(not_found(&t.bytes("b").unwrap_err()));
    assert!(t.is_unobserved_empty());
    assert_eq!(t.edit_intent("a").await.unwrap(), observed);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn verifies_an_io_failure_after_provider_entry() {
    for scenario in ["unchanged", "stat fails", "stat defect"] {
        let t = setup(PatchOptions::default()).await;
        t.file("a", "old\n", true).await;
        let observed = t.edit_intent("a").await.unwrap();
        let spy = t.spy.clone();
        set_hooks(&t, |hooks| {
            hooks.patch_text = Some(Arc::new(move |()| {
                if scenario != "unchanged" {
                    support::lock(&spy.hooks).stat = Some(Arc::new(move |()| {
                        Some(if scenario == "stat fails" {
                            FsError::new("stat failed", FsErrorCode::IoError).into()
                        } else {
                            "stat defect".into()
                        })
                    }));
                }
                Some(FsError::new("write failed", FsErrorCode::IoError).into())
            }));
        });
        let request = t.run(&patch(&[&update("a"), &add("b")])).await;
        if scenario == "stat defect" {
            assert_eq!(request.unwrap_err().to_string(), "stat defect");
        } else {
            let first = if scenario == "unchanged" {
                failed(
                    t.record(Update, "a", 0, 2, None),
                    "FS_IO_ERROR",
                    "write failed",
                    vec![],
                )
            } else {
                unknown(
                    t.record(Update, "a", 0, 2, None),
                    "FS_IO_ERROR",
                    "write failed — re-read the affected files before retrying",
                )
            };
            assert_eq!(
                request.unwrap(),
                commit(
                    vec![first, skipped(t.record(Add, "b", 1, 6, None))],
                    &[],
                    false
                ),
                "{scenario}"
            );
        }
        set_hooks(&t, |hooks| hooks.stat = None);
        assert_eq!(t.bytes("a").unwrap(), "old\n");
        assert!(t.is_unobserved_empty());
        assert_eq!(t.edit_intent("a").await.unwrap(), observed);
        t.kernel.shutdown().await;
    }
}

#[tokio::test]
async fn checks_both_move_targets_when_its_destination_was_published_but_the_reply_is_lost() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    let observed = t.edit_intent("a").await.unwrap();
    set_hooks(&t, |hooks| {
        hooks.move_file = Some(Arc::new(|_source, destination| {
            std::fs::write(destination.target_key.as_ref() as &str, "published\n").unwrap();
            Some(Err(
                FsError::new("move reply lost", FsErrorCode::IoError).into()
            ))
        }));
    });
    assert_eq!(
        t.run(&patch(&[&mv("a", "b"), &add("c")])).await.unwrap(),
        commit(
            vec![
                unknown(
                    t.record(Move, "a", 0, 2, Some("b")),
                    "FS_IO_ERROR",
                    "move reply lost — re-read the affected files before retrying",
                ),
                skipped(t.record(Add, "c", 1, 7, None)),
            ],
            &[],
            false,
        )
    );
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert_eq!(t.bytes("b").unwrap(), "published\n");
    assert!(t.is_unobserved_empty());
    assert_eq!(t.edit_intent("a").await.unwrap(), observed);
    let error = t.edit_intent("b").await.unwrap_err();
    assert_eq!(
        HarnessError::of(error.as_ref()).unwrap().code(),
        "FS_NOT_OBSERVED"
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn retains_preceding_changes_when_an_external_edit_invalidates_the_next_commit_guard() {
    let t = setup(PatchOptions::default()).await;
    t.file("b", "old\n", true).await;
    let observed = t.edit_intent("b").await.unwrap();
    let external = t.path("b");
    set_hooks(&t, |hooks| {
        hooks.after_write_text = Some(Arc::new(move || {
            std::fs::write(&external, "external edit\n").unwrap()
        }));
    });
    let a = t.record(Add, "a", 0, 2, None);
    let b = t.record(Update, "b", 1, 4, None);
    let report = t
        .run(&patch(&[&add("a"), &update("b"), &add("c")]))
        .await
        .unwrap();
    let hoh_tool_fs::patch::PatchExecutionReport::Commit {
        outcomes,
        affected_paths,
        cancelled,
    } = report
    else {
        panic!("expected a commit report");
    };
    assert_eq!(outcomes.len(), 3);
    assert_eq!(
        outcomes[0],
        completed(a.clone(), vec![change(&a.path, None, Some("new\n"))])
    );
    let PatchOperationOutcome::Failed {
        operation,
        failure,
        changes,
    } = &outcomes[1]
    else {
        panic!("expected the second operation to fail: {:?}", outcomes[1]);
    };
    assert_eq!(operation, &b);
    assert_eq!(failure.code, "FS_STALE_VERSION");
    assert!(
        failure.message.contains("re-read the file, then retry"),
        "{}",
        failure.message
    );
    assert!(changes.is_empty());
    assert_eq!(outcomes[2], skipped(t.record(Add, "c", 2, 8, None)));
    assert_eq!(affected_paths, std::slice::from_ref(&a.path));
    assert!(!cancelled);
    assert_eq!(t.bytes("a").unwrap(), "new\n");
    assert_eq!(t.bytes("b").unwrap(), "external edit\n");
    assert!(not_found(&t.bytes("c").unwrap_err()));
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    assert_eq!(t.edit_intent("b").await.unwrap(), observed);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn propagates_programmer_defects_from_mutation_and_preserves_observed_state() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    set_hooks(&t, |hooks| {
        hooks.patch_text = Some(Arc::new(|()| Some("mutation defect".into())));
    });
    let error = t.run(&patch(&[&update("a")])).await.unwrap_err();
    assert_eq!(error.to_string(), "mutation defect");
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert!(t.is_unobserved_empty());
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn reports_the_locked_move_source_after_an_external_edit_following_preflight() {
    let t = setup(PatchOptions {
        without_policy: true,
        ..PatchOptions::default()
    })
    .await;
    t.file("a", "old\n", true).await;
    let external = t.path("a");
    set_hooks(&t, |hooks| {
        hooks.before_move_file = Some(Arc::new(move || {
            std::fs::write(&external, "old\nexternal\n").unwrap()
        }));
    });
    let report = t.run(&patch(&[&mv("a", "b")])).await.unwrap();
    let expected_changes = vec![
        change(&t.path_text("b"), None, Some("new\nexternal\n")),
        change(&t.path_text("a"), Some("old\nexternal\n"), None),
    ];
    let hoh_tool_fs::patch::PatchExecutionReport::Commit { outcomes, .. } = report else {
        panic!("expected a commit report");
    };
    assert_eq!(
        outcomes,
        [completed(
            t.record(Move, "a", 0, 2, Some("b")),
            expected_changes
        )]
    );
    // The source text was read once during preflight; the commit result came from the locked move.
    assert_eq!(t.spy.read_text_calls.load(Ordering::SeqCst), 1);
    assert_eq!(t.bytes("b").unwrap(), "new\nexternal\n");
    t.kernel.shutdown().await;
}

fn sandboxed(mode: SandboxMode) -> PatchOptions {
    PatchOptions {
        mode: Some(mode),
        ..PatchOptions::default()
    }
}

/// The failure code and message of a failed outcome, with no changes retained.
fn denial(
    outcome: &PatchOperationOutcome,
) -> (&hoh_tool_fs::patch::PatchOperationRecord, &str, &str) {
    match outcome {
        PatchOperationOutcome::Failed {
            operation,
            failure,
            changes,
        } => {
            assert!(changes.is_empty(), "{changes:?}");
            (operation, &failure.code, &failure.message)
        }
        other => panic!("expected a failed outcome, got {other:?}"),
    }
}

/// `tests/patch/commit.spec.ts:193`: the real sandboxing provider under read-only policy.
#[tokio::test]
async fn stops_at_the_first_provider_sandbox_denial_under_read_only_policy() {
    let t = setup(sandboxed(SandboxMode::ReadOnly)).await;
    t.file("a", "old\n", true).await;
    let b = t.record(Add, "b", 0, 2, None);
    let report = t
        .run_with(
            &patch(&[&add("b"), &update("a"), &remove("a")]),
            Some(t.policy(SandboxMode::ReadOnly)),
        )
        .await
        .unwrap();
    let PatchExecutionReport::Commit {
        outcomes,
        affected_paths,
        cancelled,
    } = report
    else {
        panic!("expected a commit report, got {report:?}");
    };
    assert_eq!(outcomes.len(), 3, "{outcomes:?}");
    let (operation, code, message) = denial(&outcomes[0]);
    assert_eq!(operation, &b);
    assert_eq!(code, "FS_SANDBOX_DENIED");
    assert!(
        message.contains("[sandbox: file access denied under read-only mode]"),
        "{message}"
    );
    assert_eq!(outcomes[1], skipped(t.record(Update, "a", 1, 4, None)));
    assert_eq!(outcomes[2], skipped(t.record(Delete, "a", 2, 8, None)));
    assert!(affected_paths.is_empty());
    assert!(!cancelled);
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert!(not_found(&t.bytes("b").unwrap_err()));
    assert!(t.is_unobserved_empty());
    t.kernel.shutdown().await;
}

/// `tests/patch/commit.spec.ts:207`: an earlier write stays when a later move leaves the roots.
#[tokio::test]
async fn retains_earlier_writes_when_the_provider_denies_a_move_destination_outside_writable_roots()
{
    let t = setup(sandboxed(SandboxMode::WorkspaceWrite)).await;
    t.file("a", "old\n", true).await;
    let outside = std::env::current_dir()
        .unwrap()
        .join("patch-commit-outside-root")
        .to_string_lossy()
        .into_owned();
    let b = t.record(Add, "b", 0, 2, None);
    let report = t
        .run_with(
            &patch(&[&add("b"), &mv("a", &outside)]),
            Some(t.policy(SandboxMode::WorkspaceWrite)),
        )
        .await
        .unwrap();
    let PatchExecutionReport::Commit {
        outcomes,
        affected_paths,
        cancelled,
    } = report
    else {
        panic!("expected a commit report, got {report:?}");
    };
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert_eq!(
        outcomes[0],
        completed(b.clone(), vec![change(&b.path, None, Some("new\n"))])
    );
    let (operation, code, message) = denial(&outcomes[1]);
    assert_eq!(operation, &t.record(Move, "a", 1, 4, Some(&outside)));
    assert_eq!(code, "FS_SANDBOX_DENIED");
    assert!(
        message.contains("[sandbox: file access denied under workspace-write mode]"),
        "{message}"
    );
    assert_eq!(affected_paths, std::slice::from_ref(&b.path));
    assert!(!cancelled);
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert_eq!(t.bytes("b").unwrap(), "new\n");
    assert!(!std::path::Path::new(&outside).exists());
    assert_eq!(t.observed_paths(), [b.path]);
    t.kernel.shutdown().await;
}
