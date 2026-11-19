//! Resolution failures preserve operation order and use raw spelling only for unresolved paths.

mod support;

use std::sync::Arc;

use hoh_fs::{FsError, FsErrorCode, FsVersionGuard};
use hoh_tool_fs::patch::PatchOperationKind::{Add, Move};
use support::patch::*;

#[tokio::test]
async fn collects_path_resolution_failures_while_retaining_resolved_paths_and_checking_remaining_operations()
 {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    lock_hooks(&t, |hooks| {
        hooks.resolve = Some(Arc::new(|path: String| {
            (path == "./denied").then(|| {
                FsError::new("cannot resolve denied path", FsErrorCode::PermissionDenied).into()
            })
        }));
    });
    let report = t
        .run(&patch(&[&update("a"), &add("./denied"), &add("c")]))
        .await
        .unwrap();
    let denied = hoh_tool_fs::patch::PatchOperationRecord {
        kind: Add,
        path: "./denied".into(),
        index: 1,
        patch_line: 6,
        destination: None,
    };
    assert_eq!(
        report,
        preflight(vec![refusal(
            denied,
            "FS_PERMISSION_DENIED",
            "cannot resolve denied path"
        )])
    );
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert_eq!(
        t.bytes("c").unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(t.is_unobserved_empty());
    assert_eq!(
        t.edit_intent("a").await.unwrap(),
        Some(FsVersionGuard {
            version: t.version("a").await
        })
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn retains_a_resolved_move_source_when_only_its_destination_cannot_be_resolved() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    lock_hooks(&t, |hooks| {
        hooks.resolve = Some(Arc::new(|path: String| {
            (path == "./denied")
                .then(|| FsError::new("destination unread", FsErrorCode::NotObserved).into())
        }));
    });
    let report = t
        .run(&patch(&[&mv("a", "./denied"), &update("a")]))
        .await
        .unwrap();
    let mut operation = t.record(Move, "a", 0, 2, None);
    operation.destination = Some("./denied".into());
    assert_eq!(
        report,
        preflight(vec![refusal(
            operation,
            "FS_NOT_OBSERVED",
            "cannot modify \"./denied\": file has not been read — read the file, then retry",
        )])
    );
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert!(t.is_unobserved_empty());
    assert_eq!(
        t.edit_intent("a").await.unwrap(),
        Some(FsVersionGuard {
            version: t.version("a").await
        })
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn returns_aborted_resolution_refusals_for_a_signal_aborted_before_entry() {
    let t = setup(PatchOptions::default()).await;
    t.controller.abort();
    // The source fixture's working directory ignores the signal; the real service refuses an
    // aborted one before any path is resolved, so the call runs without an agent.
    let report = t
        .run_without_agent(&patch(&[&add("a"), &mv("b", "c")]))
        .await
        .unwrap();
    let first = hoh_tool_fs::patch::PatchOperationRecord {
        kind: Add,
        path: "a".into(),
        index: 0,
        patch_line: 2,
        destination: None,
    };
    let second = hoh_tool_fs::patch::PatchOperationRecord {
        kind: Move,
        path: "b".into(),
        index: 1,
        patch_line: 4,
        destination: Some("c".into()),
    };
    assert_eq!(
        report,
        preflight(vec![
            refusal(first, "FS_ABORTED", "resolve aborted"),
            refusal(second, "FS_ABORTED", "resolve aborted"),
        ])
    );
    assert_eq!(
        t.bytes("a").unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(t.is_unobserved_empty());
    let error = t.edit_intent("a").await.unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        "FS_NOT_OBSERVED"
    );
    t.kernel.shutdown().await;
}

fn lock_hooks(t: &T, set: impl FnOnce(&mut Hooks)) {
    set(&mut support::lock(&t.spy.hooks));
}
