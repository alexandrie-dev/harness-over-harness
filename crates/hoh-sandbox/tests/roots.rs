use hoh_sandbox::*;
fn policy(root: &str, extra: Option<Vec<String>>, mode: SandboxMode) -> SandboxExecutionPolicy {
    SandboxExecutionPolicy {
        mode,
        workspace_root: root.into(),
        additional_writable_roots: extra,
        session_id: None,
    }
}
#[tokio::test]
async fn resolves_symlinks_an_existing_path_realpaths() {
    let d = tempfile::tempdir().unwrap();
    let link = d.path().join("alias");
    tokio::fs::create_dir(d.path().join("physical"))
        .await
        .unwrap();
    tokio::fs::symlink(d.path().join("physical"), &link)
        .await
        .unwrap();
    assert_eq!(
        canonical_path(link.to_str().unwrap()).await,
        d.path().join("physical").to_str().unwrap()
    );
}
#[tokio::test]
async fn returns_the_spelling_as_is_when_the_path_cannot_be_resolved() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("missing/anywhere");
    assert_eq!(
        canonical_path(p.to_str().unwrap()).await,
        p.to_str().unwrap()
    );
}
#[tokio::test]
async fn workspace_write_grants_only_the_workspace_when_no_extra_root_is_configured() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().to_str().unwrap();
    assert_eq!(
        workspace_writable_roots(&policy(p, None, SandboxMode::WorkspaceWrite))
            .await
            .unwrap(),
        [canonical_path(p).await]
    );
}
#[tokio::test]
async fn workspace_write_deduplicates_physical_extra_directories_and_rejects_symlink_aliases() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().to_str().unwrap();
    let extra = d.path().join("extra");
    tokio::fs::create_dir(&extra).await.unwrap();
    let extra = extra.to_str().unwrap();
    tokio::fs::symlink(extra, d.path().join("alias"))
        .await
        .unwrap();
    assert_eq!(
        workspace_writable_roots(&policy(
            p,
            Some(vec![extra.into(), extra.into()]),
            SandboxMode::WorkspaceWrite
        ))
        .await
        .unwrap(),
        [p, extra]
    );
    let e = workspace_writable_roots(&policy(
        p,
        Some(vec![d.path().join("alias").to_str().unwrap().into()]),
        SandboxMode::WorkspaceWrite,
    ))
    .await
    .unwrap_err();
    assert!(e.message().contains("use the physical directory path"));
}
#[tokio::test]
async fn rejects_invalid_extra_roots_and_checks_them_again_on_each_call() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_str().unwrap();
    let file = d.path().join("file.txt");
    tokio::fs::write(&file, "file").await.unwrap();
    let extra = d.path().join("extra");
    for path in [file.clone(), file.join("child"), extra.clone()] {
        let e = workspace_writable_roots(&policy(
            root,
            Some(vec![path.to_str().unwrap().into()]),
            SandboxMode::WorkspaceWrite,
        ))
        .await
        .unwrap_err();
        assert!(
            e.message()
                .contains("Invalid additional writable directory")
        );
    }
    let p = policy(
        root,
        Some(vec![extra.to_str().unwrap().into()]),
        SandboxMode::WorkspaceWrite,
    );
    tokio::fs::create_dir(&extra).await.unwrap();
    assert_eq!(workspace_writable_roots(&p).await.unwrap().len(), 2);
    tokio::fs::remove_dir(&extra).await.unwrap();
    assert!(
        workspace_writable_roots(&p)
            .await
            .unwrap_err()
            .message()
            .contains("cannot read the directory")
    );
}
#[tokio::test]
async fn read_only_grants_nothing_even_when_extra_roots_are_carried_on_the_policy() {
    let d = tempfile::tempdir().unwrap();
    let p = policy(
        d.path().to_str().unwrap(),
        Some(vec![d.path().join("extra").to_str().unwrap().into()]),
        SandboxMode::ReadOnly,
    );
    assert!(workspace_writable_roots(&p).await.unwrap().is_empty());
    assert!(writable_roots(&p).await.unwrap().is_empty());
}
#[tokio::test]
async fn read_only_grants_nothing() {
    let d = tempfile::tempdir().unwrap();
    assert!(
        writable_roots(&policy(
            d.path().to_str().unwrap(),
            None,
            SandboxMode::ReadOnly
        ))
        .await
        .unwrap()
        .is_empty()
    );
}
#[tokio::test]
async fn workspace_write_grants_the_workspace_plus_platform_temp_areas_canonical_and_deduplicated()
{
    let d = tempfile::tempdir().unwrap();
    let roots = writable_roots(&policy(
        d.path().to_str().unwrap(),
        None,
        SandboxMode::WorkspaceWrite,
    ))
    .await
    .unwrap();
    for p in [
        d.path().to_string_lossy().into_owned(),
        "/tmp".into(),
        std::env::temp_dir().to_string_lossy().into_owned(),
    ] {
        assert!(roots.contains(&canonical_path(&p).await));
    }
    let unique: std::collections::HashSet<_> = roots.iter().collect();
    assert_eq!(roots.len(), unique.len());
}
#[tokio::test]
async fn workspace_write_includes_an_explicit_extra_directory_beside_the_temp_areas() {
    let d = tempfile::tempdir().unwrap();
    let extra = d.path().join("extra");
    tokio::fs::create_dir(&extra).await.unwrap();
    assert!(
        writable_roots(&policy(
            d.path().to_str().unwrap(),
            Some(vec![extra.to_str().unwrap().into()]),
            SandboxMode::WorkspaceWrite
        ))
        .await
        .unwrap()
        .contains(&extra.to_str().unwrap().to_owned())
    );
}
