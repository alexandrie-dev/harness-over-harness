//! Successful simulations and commits agree on bytes and observation guards.

mod support;

use hoh_fs::FsVersionGuard;
use hoh_tool_fs::patch::PatchOperationKind::{Add, Delete, Move, Update};
use support::patch::*;

fn not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

async fn guard(t: &T, name: &str) -> Option<FsVersionGuard> {
    Some(FsVersionGuard {
        version: t.version(name).await,
    })
}

#[tokio::test]
async fn applies_multiple_files_and_hunks_while_preserving_storage_line_endings() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\r\ntail\r\n", true).await;
    let text = patch(&[
        "*** Update File: a\n@@\n-old\n+new\n@@\n-tail\n+end",
        &add("b"),
    ]);
    let a = t.record(Update, "a", 0, 2, None);
    let b = t.record(Add, "b", 1, 9, None);
    assert_eq!(
        t.run(&text).await.unwrap(),
        commit(
            vec![
                completed(
                    a.clone(),
                    vec![change(&a.path, Some("old\ntail\n"), Some("new\nend\n"))]
                ),
                completed(b.clone(), vec![change(&b.path, None, Some("new\n"))]),
            ],
            &[&a.path, &b.path],
            false,
        )
    );
    assert_eq!(t.bytes("a").unwrap(), "new\r\nend\r\n");
    assert_eq!(t.bytes("b").unwrap(), "new\n");
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    assert_eq!(t.edit_intent("b").await.unwrap(), guard(&t, "b").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn retains_aliased_updates_and_reacquires_the_version_produced_by_the_preceding_update() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    let a = t.record(Update, "./a", 0, 2, None);
    let second = t.record(Update, "a", 1, 6, None);
    let report = t
        .run(&patch(&[&update("./a"), &update_with("a", "new", "last")]))
        .await
        .unwrap();
    assert_eq!(
        report,
        commit(
            vec![
                completed(
                    a.clone(),
                    vec![change(&a.path, Some("old\n"), Some("new\n"))]
                ),
                completed(second, vec![change(&a.path, Some("new\n"), Some("last\n"))]),
            ],
            &[&a.path],
            false,
        )
    );
    // The second patch is guarded by the version the first one published (the first observation).
    let guards = support::lock(&t.spy.patch_text_guards).clone();
    assert_eq!(guards.len(), 2);
    let first_observed = match support::lock(&t.observations)
        .first()
        .map(|(_, observation)| observation.clone())
    {
        Some(hoh_fs::FsObservation::Present { version }) => Some(FsVersionGuard { version }),
        _ => None,
    };
    assert_eq!(guards[1], first_observed);
    assert_eq!(t.bytes("a").unwrap(), "last\n");
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn simulates_deletion_then_recreation_at_the_same_target() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    let a = t.record(Delete, "a", 0, 2, None);
    let second = t.record(Add, "a", 1, 3, None);
    assert_eq!(
        t.run(&patch(&[&remove("a"), &add("a")])).await.unwrap(),
        commit(
            vec![
                completed(a.clone(), vec![change(&a.path, Some("old\n"), None)]),
                completed(second, vec![change(&a.path, None, Some("new\n"))]),
            ],
            &[&a.path],
            false,
        )
    );
    assert_eq!(t.bytes("a").unwrap(), "new\n");
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn simulates_creation_then_update_before_the_file_exists_on_disk() {
    let t = setup(PatchOptions::default()).await;
    let a = t.record(Add, "a", 0, 2, None);
    let second = t.record(Update, "a", 1, 4, None);
    assert_eq!(
        t.run(&patch(&[&add("a"), &update_with("a", "new", "last")]))
            .await
            .unwrap(),
        commit(
            vec![
                completed(a.clone(), vec![change(&a.path, None, Some("new\n"))]),
                completed(second, vec![change(&a.path, Some("new\n"), Some("last\n"))]),
            ],
            &[&a.path],
            false,
        )
    );
    assert_eq!(t.bytes("a").unwrap(), "last\n");
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn moves_and_patches_in_one_provider_call_then_updates_the_simulated_destination() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\r\n", true).await;
    t.file("b", "previous\n", true).await;
    let a = t.record(Move, "a", 0, 2, Some("b"));
    let b = t.record(Update, "b", 1, 7, None);
    let b_path = t.path_text("b");
    let a_path = t.path_text("a");
    assert_eq!(
        t.run(&patch(&[&mv("a", "b"), &update_with("b", "new", "last")]))
            .await
            .unwrap(),
        commit(
            vec![
                completed(
                    a,
                    vec![
                        change(&b_path, Some("previous\n"), Some("new\n")),
                        change(&a_path, Some("old\n"), None),
                    ],
                ),
                completed(b, vec![change(&b_path, Some("new\n"), Some("last\n"))]),
            ],
            &[&b_path, &a_path],
            false,
        )
    );
    assert_eq!(*support::lock(&t.spy.move_hunks), [1]);
    assert_eq!(
        t.bytes("a").unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert_eq!(t.bytes("b").unwrap(), "last\r\n");
    let error = t.edit_intent("a").await.unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        "FS_NOT_FOUND"
    );
    assert_eq!(t.edit_intent("b").await.unwrap(), guard(&t, "b").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn supports_unconditional_updates_and_moves_without_observation_policy() {
    let t = setup(PatchOptions {
        without_policy: true,
        ..PatchOptions::default()
    })
    .await;
    t.file("a", "old\n", false).await;
    let a = t.record(Update, "a", 0, 2, None);
    let b = t.record(Move, "a", 1, 6, Some("b"));
    let destination = t.path_text("b");
    assert_eq!(
        t.run(&patch(&[&update("a"), &move_with("a", "b", "new", "last")]))
            .await
            .unwrap(),
        commit(
            vec![
                completed(
                    a.clone(),
                    vec![change(&a.path, Some("old\n"), Some("new\n"))]
                ),
                completed(
                    b,
                    vec![
                        change(&destination, None, Some("last\n")),
                        change(&a.path, Some("new\n"), None)
                    ],
                ),
            ],
            &[&a.path, &destination],
            false,
        )
    );
    assert_eq!(t.bytes("b").unwrap(), "last\n");
    assert!(not_found(&t.bytes("a").unwrap_err()));
    assert_eq!(t.edit_intent("b").await.unwrap(), None);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn allows_an_add_to_overwrite_a_target_already_created_in_the_patch() {
    let t = setup(PatchOptions::default()).await;
    let a = t.record(Add, "a", 0, 2, None);
    assert_eq!(
        t.run(&patch(&[&add("a"), &add_with("a", "last")]))
            .await
            .unwrap(),
        commit(
            vec![
                completed(a.clone(), vec![change(&a.path, None, Some("new\n"))]),
                completed(
                    t.record(Add, "a", 1, 4, None),
                    vec![change(&a.path, Some("new\n"), Some("last\n"))],
                ),
            ],
            &[&a.path],
            false,
        )
    );
    assert_eq!(t.bytes("a").unwrap(), "last\n");
    assert_eq!(t.edit_intent("a").await.unwrap(), guard(&t, "a").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn updates_through_a_symbolic_link_and_shares_its_observation_with_the_target() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    std::os::unix::fs::symlink("a", t.path("link")).unwrap();
    let link = t.record(Update, "link", 0, 2, None);
    let target = t.path_text("a");
    assert_eq!(
        t.run(&patch(&[&update("link")])).await.unwrap(),
        commit(
            vec![completed(
                link,
                vec![change(&target, Some("old\n"), Some("new\n"))]
            )],
            &[&target],
            false,
        )
    );
    assert_eq!(t.bytes("link").unwrap(), "new\n");
    assert_eq!(t.bytes("a").unwrap(), "new\n");
    assert_eq!(t.edit_intent("link").await.unwrap(), guard(&t, "a").await);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn deletes_a_file_created_earlier_in_the_patch_and_records_confirmed_absence() {
    let t = setup(PatchOptions::default()).await;
    let a = t.record(Add, "a", 0, 2, None);
    assert_eq!(
        t.run(&patch(&[&add("a"), &remove("a")])).await.unwrap(),
        commit(
            vec![
                completed(a.clone(), vec![change(&a.path, None, Some("new\n"))]),
                completed(
                    t.record(Delete, "a", 1, 4, None),
                    vec![change(&a.path, Some("new\n"), None)],
                ),
            ],
            &[&a.path],
            false,
        )
    );
    assert!(not_found(&t.bytes("a").unwrap_err()));
    let error = t.edit_intent("a").await.unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        "FS_NOT_FOUND"
    );
    t.kernel.shutdown().await;
}

/// `tests/patch/success.spec.ts:100` (`it.each`): guarded changes commit under the permissive modes.
#[tokio::test]
async fn commits_guarded_changes_under_each_permissive_policy() {
    for mode in [
        hoh_sandbox::SandboxMode::WorkspaceWrite,
        hoh_sandbox::SandboxMode::DangerFullAccess,
    ] {
        let t = setup(PatchOptions {
            mode: Some(mode),
            ..PatchOptions::default()
        })
        .await;
        let a = t.record(Add, "a", 0, 2, None);
        assert_eq!(
            t.run_with(&patch(&[&add("a")]), Some(t.policy(mode)))
                .await
                .unwrap(),
            commit(
                vec![completed(
                    a.clone(),
                    vec![change(&a.path, None, Some("new\n"))]
                )],
                &[&a.path],
                false,
            ),
            "{mode:?}"
        );
        assert_eq!(t.bytes("a").unwrap(), "new\n");
        assert_eq!(
            t.edit_intent("a").await.unwrap(),
            guard(&t, "a").await,
            "{mode:?}"
        );
        t.kernel.shutdown().await;
    }
}
