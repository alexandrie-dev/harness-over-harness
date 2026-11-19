use std::fs;
use std::path::{Path, PathBuf};

use hoh_error::{BoxError, HarnessError};
use hoh_fs::{
    FileSystem, Fs, FsEditRequest, FsErrorCode, FsMoveOptions, FsResolveOptions, FsTarget,
    FsWriteOperation,
};
use hoh_fs_local::Config;
use hoh_fs_patch::PatchHunk;
use hoh_fs_sandbox::{FsSandboxPlugin, SandboxedFileSystem};
use hoh_kernel::{Context, Kernel};
use hoh_sandbox::{SandboxExecutionPolicy, SandboxMode};
use hoh_sandbox_policy::{
    SandboxPolicy, SandboxPolicyRequest, SandboxPolicyService, set_sandbox_mode,
};
use hoh_session::Session;
use hoh_session_projection::SessionProjectionRegistry;
use serde_json::json;
use tempfile::{Builder, TempDir};

/// The real shared policy service, mounted as the composition does: projections first, then the
/// resolver with the deployment default `mode` over `workspace`.
async fn mount_policy(ctx: &Context, mode: SandboxMode, workspace: &Path) {
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SandboxPolicyService>(json!({
        "mode": mode.as_str(),
        "workspaceRoot": workspace,
    }))
    .unwrap()
    .settled()
    .await
    .unwrap();
}

struct Fixture {
    _base: TempDir,
    workspace: PathBuf,
    outside: PathBuf,
    kernel: Kernel,
    fs: SandboxedFileSystem,
}

impl Fixture {
    async fn new(mode: SandboxMode) -> Self {
        let base = Builder::new()
            .prefix("hoh-fssbx-")
            .tempdir_in(std::env::current_dir().expect("current directory"))
            .expect("temporary directory");
        let base_path = base.path().canonicalize().expect("canonical base");
        let workspace = base_path.join("ws");
        let outside = base_path.join("out");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&outside).unwrap();
        let kernel = Kernel::start();
        let ctx = kernel.root();
        mount_policy(&ctx, mode, &workspace).await;
        let policy = ctx.get::<SandboxPolicy>().expect("sandbox policy service");
        let fs = SandboxedFileSystem::new(
            Config {
                cwd: Some(workspace.to_string_lossy().into_owned()),
                ..Config::default()
            },
            (*policy).clone(),
        )
        .expect("sandbox filesystem");
        Self {
            _base: base,
            workspace,
            outside,
            kernel,
            fs,
        }
    }

    async fn target(&self, path: &Path) -> FsTarget {
        self.fs
            .resolve(&path.to_string_lossy(), FsResolveOptions::default())
            .await
            .expect("resolve target")
    }

    fn policy(&self, mode: SandboxMode) -> SandboxExecutionPolicy {
        SandboxExecutionPolicy {
            mode,
            workspace_root: self.workspace.to_string_lossy().into_owned(),
            additional_writable_roots: None,
            session_id: None,
        }
    }
}

fn assert_error_code(error: &BoxError, code: &str) {
    let error = HarnessError::of(error.as_ref()).expect("structured error");
    assert_eq!(error.code(), code, "{}", error.message());
}

fn assert_denial(error: &BoxError, path: &Path, mode: SandboxMode) {
    let error = HarnessError::of(error.as_ref()).expect("structured FsError");
    assert_eq!(error.name(), "FsError");
    assert_eq!(error.code(), "FS_SANDBOX_DENIED");
    assert_eq!(
        error.message(),
        format!(
            "cannot write \"{}\": file access denied under {} mode",
            path.display(),
            mode
        )
    );
}

#[tokio::test]
async fn reports_the_deployment_default_mode_what_the_tool_layer_advertises_against() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    assert_eq!(fixture.fs.sandbox_mode(), Some(SandboxMode::WorkspaceWrite));
}

#[tokio::test]
async fn read_only_denies_write_leaving_no_file_on_disk() {
    let fixture = Fixture::new(SandboxMode::ReadOnly).await;
    let path = fixture.workspace.join("denied.txt");
    let target = fixture.target(&path).await;
    let error = fixture
        .fs
        .write_text(&target, "x", None, None, None)
        .await
        .expect_err("write must be denied");
    assert_denial(&error, &path, SandboxMode::ReadOnly);
    assert!(!path.exists());
}

#[tokio::test]
async fn read_only_denies_edit_of_an_existing_file_the_content_is_unchanged() {
    let fixture = Fixture::new(SandboxMode::ReadOnly).await;
    let path = fixture.workspace.join("file.txt");
    fs::write(&path, "original").unwrap();
    let target = fixture.target(&path).await;
    let error = fixture
        .fs
        .edit_text(
            &target,
            FsEditRequest {
                old_string: "original".into(),
                new_string: "changed".into(),
                replace_all: false,
            },
            None,
            None,
            None,
        )
        .await
        .expect_err("edit must be denied");
    assert_denial(&error, &path, SandboxMode::ReadOnly);
    assert_eq!(fs::read_to_string(path).unwrap(), "original");
}

#[tokio::test]
async fn read_only_allows_reads_every_mode_permits_reading() {
    let fixture = Fixture::new(SandboxMode::ReadOnly).await;
    let path = fixture.workspace.join("readable.txt");
    fs::write(&path, "hello").unwrap();
    let target = fixture.target(&path).await;
    assert_eq!(fixture.fs.read_text(&target, None).await.unwrap(), "hello");
}

/// `fs-sandbox.spec.ts:96-128`, through the real `hoh-sandbox-policy`: a grant registered for one
/// session by a plugin applies to that session only, a session `read-only` override beats it, and
/// unloading the plugin revokes it.
#[tokio::test]
async fn workspace_write_applies_session_grants_to_writes_and_edits_until_their_plugin_unloads() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let ctx = fixture.kernel.root();
    let policy = ctx.get::<SandboxPolicy>().expect("sandbox policy service");
    let first = Session::create("memory-first".into(), None, None, None, None).unwrap();
    let second = Session::create("memory-second".into(), None, None, None, None).unwrap();
    let path = fixture.outside.join("memory.txt");
    let target = fixture.target(&path).await;
    let resolve = |session: &Session| {
        policy
            .resolve(SandboxPolicyRequest {
                session: Some(session),
                mode: None,
            })
            .unwrap()
    };
    let write = |session: &Session, content: &'static str| {
        let policy = resolve(session);
        let (fs, target) = (&fixture.fs, &target);
        async move {
            fs.write_text(target, content, None, None, Some(policy))
                .await
        }
    };
    let edit = |session: &Session| {
        let policy = resolve(session);
        let (fs, target) = (&fixture.fs, &target);
        async move {
            fs.edit_text(
                target,
                FsEditRequest {
                    old_string: "memory".into(),
                    new_string: "edited".into(),
                    replace_all: false,
                },
                None,
                None,
                Some(policy),
            )
            .await
        }
    };

    assert_error_code(
        &write(&first, "denied")
            .await
            .expect_err("outside must begin denied"),
        "FS_SANDBOX_DENIED",
    );
    // The grant belongs to a plugin: it lives exactly as long as that plugin is loaded.
    let grant_policy = (*policy).clone();
    let grant_ctx_session = first.clone();
    let outside = fixture.outside.to_string_lossy().into_owned();
    let plugin = ctx
        .inject(move |scope, (): ()| {
            let (policy, session, outside) = (
                grant_policy.clone(),
                grant_ctx_session.clone(),
                outside.clone(),
            );
            async move {
                policy.register_writable_root(&scope, &session, outside)?;
                Ok(())
            }
        })
        .unwrap();
    plugin.settled().await.unwrap();

    write(&first, "memory").await.unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "memory");
    assert_error_code(
        &write(&second, "other session")
            .await
            .expect_err("another session has no grant"),
        "FS_SANDBOX_DENIED",
    );
    assert_error_code(
        &edit(&second)
            .await
            .expect_err("another session cannot edit"),
        "FS_SANDBOX_DENIED",
    );
    edit(&first).await.unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "edited");

    set_sandbox_mode(&first, SandboxMode::ReadOnly).unwrap();
    assert_error_code(
        &write(&first, "read-only")
            .await
            .expect_err("override must deny"),
        "FS_SANDBOX_DENIED",
    );
    assert_error_code(
        &edit(&first).await.expect_err("override must deny an edit"),
        "FS_SANDBOX_DENIED",
    );
    set_sandbox_mode(&first, SandboxMode::WorkspaceWrite).unwrap();
    write(&first, "updated").await.unwrap();

    plugin.dispose().await;
    assert_error_code(
        &write(&first, "revoked")
            .await
            .expect_err("revoked root must deny"),
        "FS_SANDBOX_DENIED",
    );
    assert_error_code(
        &edit(&first)
            .await
            .expect_err("revoked root must deny an edit"),
        "FS_SANDBOX_DENIED",
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "updated");
}

#[tokio::test]
async fn workspace_write_rejects_invalid_grants_before_workspace_writes() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let file = fixture.outside.join("file.txt");
    fs::write(&file, "file").unwrap();
    let path = fixture.workspace.join("blocked.txt");
    let target = fixture.target(&path).await;
    for invalid in [
        fixture.outside.join("missing"),
        file.clone(),
        file.join("child"),
    ] {
        let mut policy = fixture.policy(SandboxMode::WorkspaceWrite);
        policy.additional_writable_roots = Some(vec![
            fixture.outside.to_string_lossy().into_owned(),
            invalid.to_string_lossy().into_owned(),
        ]);
        let error = fixture
            .fs
            .write_text(&target, "blocked", None, None, Some(policy))
            .await
            .expect_err("invalid grant must fail closed");
        assert_error_code(&error, "SANDBOX_INVALID_ROOT");
        assert!(!path.exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_write_rejects_an_extra_directory_replaced_by_a_symlink_to_an_ungranted_sibling()
{
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let extra = fixture.workspace.join("extra");
    fs::create_dir(&extra).unwrap();
    let mut policy = fixture.policy(SandboxMode::WorkspaceWrite);
    policy.additional_writable_roots = Some(vec![extra.to_string_lossy().into_owned()]);
    let before = fixture.target(&extra.join("before.txt")).await;
    fixture
        .fs
        .write_text(&before, "allowed", None, None, Some(policy.clone()))
        .await
        .unwrap();
    fs::remove_dir_all(&extra).unwrap();
    symlink(&fixture.outside, &extra).unwrap();
    let escaped = fixture.target(&fixture.outside.join("escape.txt")).await;
    let error = fixture
        .fs
        .write_text(&escaped, "denied", None, None, Some(policy))
        .await
        .expect_err("replaced grant must fail closed");
    assert_error_code(&error, "SANDBOX_INVALID_ROOT");
    assert!(!fixture.outside.join("escape.txt").exists());
}

#[tokio::test]
async fn workspace_write_a_write_under_the_workspace_lands() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let path = fixture.workspace.join("nested/ok.txt");
    let target = fixture.target(&path).await;
    let outcome = fixture
        .fs
        .write_text(&target, "inside", None, None, None)
        .await
        .unwrap();
    assert_eq!(outcome.operation, FsWriteOperation::Create);
    assert_eq!(fs::read_to_string(path).unwrap(), "inside");
}

#[tokio::test]
async fn workspace_write_a_write_to_the_platform_temp_area_lands() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let temp = Builder::new().prefix("hoh-fssbx-tmp-").tempdir().unwrap();
    let path = temp.path().join("temp.txt");
    let target = fixture.target(&path).await;
    fixture
        .fs
        .write_text(&target, "temp", None, None, None)
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "temp");
}

#[tokio::test]
async fn workspace_write_an_absolute_path_outside_the_workspace_is_denied_no_file_created() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let path = fixture.outside.join("escape.txt");
    let target = fixture.target(&path).await;
    let error = fixture
        .fs
        .write_text(&target, "x", None, None, None)
        .await
        .expect_err("absolute escape must deny");
    assert_denial(&error, &path, SandboxMode::WorkspaceWrite);
    assert!(!path.exists());
}

#[tokio::test]
async fn workspace_write_a_dot_dot_traversal_out_of_the_workspace_is_denied() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let requested = fixture.workspace.join("../sibling-escape.txt");
    let target = fixture.target(&requested).await;
    let error = fixture
        .fs
        .write_text(&target, "x", None, None, None)
        .await
        .expect_err("dot-dot escape must deny");
    assert_denial(&error, &requested, SandboxMode::WorkspaceWrite);
    assert!(!requested.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_write_a_symlinked_directory_inside_the_workspace_pointing_out_is_denied() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    symlink(&fixture.outside, fixture.workspace.join("link")).unwrap();
    let requested = fixture.workspace.join("link/f.txt");
    let target = fixture.target(&requested).await;
    let error = fixture
        .fs
        .write_text(&target, "x", None, None, None)
        .await
        .expect_err("symlink escape must deny");
    assert_denial(&error, &requested, SandboxMode::WorkspaceWrite);
    assert!(!fixture.outside.join("f.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_write_a_new_file_under_a_symlinked_out_directory_is_denied() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    symlink(&fixture.outside, fixture.workspace.join("link")).unwrap();
    let requested = fixture.workspace.join("link/newdir/deep.txt");
    let target = fixture.target(&requested).await;
    let error = fixture
        .fs
        .write_text(&target, "x", None, None, None)
        .await
        .expect_err("deep symlink escape must deny");
    assert_denial(&error, &requested, SandboxMode::WorkspaceWrite);
    assert!(!fixture.outside.join("newdir").exists());
}

#[tokio::test]
async fn workspace_write_an_edit_outside_the_workspace_is_denied_the_original_is_untouched() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let path = fixture.outside.join("file.txt");
    fs::write(&path, "original").unwrap();
    let target = fixture.target(&path).await;
    let error = fixture
        .fs
        .edit_text(
            &target,
            FsEditRequest {
                old_string: "original".into(),
                new_string: "x".into(),
                replace_all: false,
            },
            None,
            None,
            None,
        )
        .await
        .expect_err("outside edit must deny");
    assert_denial(&error, &path, SandboxMode::WorkspaceWrite);
    assert_eq!(fs::read_to_string(path).unwrap(), "original");
}

#[tokio::test]
async fn workspace_write_an_edit_inside_the_workspace_lands() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let path = fixture.workspace.join("edit.txt");
    fs::write(&path, "original").unwrap();
    let target = fixture.target(&path).await;
    let outcome = fixture
        .fs
        .edit_text(
            &target,
            FsEditRequest {
                old_string: "original".into(),
                new_string: "changed".into(),
                replace_all: false,
            },
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(outcome.after, "changed");
    assert_eq!(fs::read_to_string(path).unwrap(), "changed");
}

#[tokio::test]
async fn workspace_write_mutates_the_freshly_checked_identity_not_a_stale_outside_target_key() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let inside = fixture.workspace.join("landed.txt");
    let outside = fixture.outside.join("escaped.txt");
    let stale = FsTarget {
        display_path: inside.to_string_lossy().into_owned(),
        target_key: outside.to_string_lossy().into_owned().into(),
    };
    fixture
        .fs
        .write_text(&stale, "inside", None, None, None)
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(inside).unwrap(), "inside");
    assert!(!outside.exists());
}

#[tokio::test]
async fn workspace_write_the_workspace_root_itself_passes_the_fence_failing_only_on_file_type() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let target = fixture.target(&fixture.workspace).await;
    let error = fixture
        .fs
        .write_text(&target, "x", None, None, None)
        .await
        .expect_err("directory write must fail");
    assert_error_code(&error, "FS_NOT_REGULAR_FILE");
}

#[tokio::test]
async fn workspace_write_with_the_filesystem_root_as_workspace_grants_writes_anywhere_on_that_volume()
 {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let path = fixture.outside.join("anywhere.txt");
    let target = fixture.target(&path).await;
    let mut policy = fixture.policy(SandboxMode::WorkspaceWrite);
    policy.workspace_root = "/".into();
    fixture
        .fs
        .write_text(&target, "anywhere", None, None, Some(policy))
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "anywhere");
}

#[tokio::test]
async fn danger_full_access_writes_anywhere_unfenced() {
    let fixture = Fixture::new(SandboxMode::DangerFullAccess).await;
    let path = fixture.outside.join("free.txt");
    let target = fixture.target(&path).await;
    fixture
        .fs
        .write_text(&target, "free", None, None, None)
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "free");
}

#[tokio::test]
async fn a_workspace_write_stamp_on_a_read_only_default_lets_a_contained_write_land_for_that_call_only()
 {
    let fixture = Fixture::new(SandboxMode::ReadOnly).await;
    let path = fixture.workspace.join("escalated.txt");
    let target = fixture.target(&path).await;
    fixture
        .fs
        .write_text(
            &target,
            "granted",
            None,
            None,
            Some(fixture.policy(SandboxMode::WorkspaceWrite)),
        )
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "granted");
    let plain = fixture.workspace.join("plain.txt");
    let target = fixture.target(&plain).await;
    assert_error_code(
        &fixture
            .fs
            .write_text(&target, "x", None, None, None)
            .await
            .expect_err("default must remain read-only"),
        "FS_SANDBOX_DENIED",
    );
}

#[tokio::test]
async fn a_danger_full_access_stamp_bypasses_the_fence_for_that_call() {
    let fixture = Fixture::new(SandboxMode::ReadOnly).await;
    let path = fixture.outside.join("granted-full.txt");
    let target = fixture.target(&path).await;
    fixture
        .fs
        .write_text(
            &target,
            "full",
            None,
            None,
            Some(fixture.policy(SandboxMode::DangerFullAccess)),
        )
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "full");
}

#[tokio::test]
async fn registers_as_ctx_fs_and_unregisters_cleanly_from_a_child_fiber() {
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount_policy(&ctx, SandboxMode::WorkspaceWrite, &fixture.workspace).await;
    let config = json!({ "cwd": fixture.workspace });
    let mut fiber = ctx.plugin::<FsSandboxPlugin>(config.clone()).unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(
        ctx.get::<Fs>().unwrap().sandbox_mode(),
        Some(SandboxMode::WorkspaceWrite)
    );
    fiber.dispose().await;
    assert!(ctx.get::<Fs>().is_none());
    fiber = ctx.plugin::<FsSandboxPlugin>(config).unwrap();
    fiber.settled().await.unwrap();
    assert!(ctx.get::<Fs>().is_some());
    kernel.shutdown().await;
}

#[tokio::test]
async fn the_denial_is_a_structured_fs_error_distinct_from_a_host_permission_error() {
    let fixture = Fixture::new(SandboxMode::ReadOnly).await;
    let path = fixture.workspace.join("x.txt");
    let target = fixture.target(&path).await;
    let error = fixture
        .fs
        .write_text(&target, "x", None, None, None)
        .await
        .expect_err("must deny");
    let error = HarnessError::of(error.as_ref()).expect("HarnessError");
    assert_eq!(error.name(), "FsError");
    assert_eq!(error.code(), FsErrorCode::SandboxDenied.as_str());
}

#[tokio::test]
async fn fences_every_new_mutation_under_each_mode() {
    for mode in [
        SandboxMode::ReadOnly,
        SandboxMode::WorkspaceWrite,
        SandboxMode::DangerFullAccess,
    ] {
        let fixture = Fixture::new(mode).await;
        let source_path = fixture.workspace.join("source");
        let outside_path = fixture.outside.join("file");
        fs::write(&source_path, "source\n").unwrap();
        fs::write(&outside_path, "outside\n").unwrap();
        let source = fixture.target(&source_path).await;
        let outside = fixture.target(&outside_path).await;
        let patch = [PatchHunk {
            line_number: 3,
            old_lines: vec!["source".into()],
            new_lines: vec!["patched".into()],
            is_end_of_file: false,
            context: None,
        }];
        match mode {
            SandboxMode::ReadOnly => {
                assert_error_code(
                    &fixture
                        .fs
                        .patch_text(&source, &patch, None, None, None)
                        .await
                        .expect_err("patch must deny"),
                    "FS_SANDBOX_DENIED",
                );
                assert_error_code(
                    &fixture
                        .fs
                        .delete_file(&source, None, None, None)
                        .await
                        .expect_err("delete must deny"),
                    "FS_SANDBOX_DENIED",
                );
                assert_error_code(
                    &fixture
                        .fs
                        .move_file(&source, &outside, FsMoveOptions::default(), None, None)
                        .await
                        .expect_err("move must deny"),
                    "FS_SANDBOX_DENIED",
                );
                assert_eq!(fs::read_to_string(source_path).unwrap(), "source\n");
            }
            SandboxMode::WorkspaceWrite => {
                for (from, to) in [(&source, &outside), (&outside, &source)] {
                    assert_error_code(
                        &fixture
                            .fs
                            .move_file(from, to, FsMoveOptions::default(), None, None)
                            .await
                            .expect_err("cross-boundary move must deny"),
                        "FS_SANDBOX_DENIED",
                    );
                }
                assert_error_code(
                    &fixture
                        .fs
                        .patch_text(&outside, &[], None, None, None)
                        .await
                        .expect_err("outside patch must deny"),
                    "FS_SANDBOX_DENIED",
                );
                assert_error_code(
                    &fixture
                        .fs
                        .delete_file(&outside, None, None, None)
                        .await
                        .expect_err("outside delete must deny"),
                    "FS_SANDBOX_DENIED",
                );
                assert_eq!(fs::read_to_string(&source_path).unwrap(), "source\n");
                assert_eq!(fs::read_to_string(&outside_path).unwrap(), "outside\n");
                assert_eq!(
                    fixture
                        .fs
                        .patch_text(&source, &patch, None, None, None)
                        .await
                        .unwrap()
                        .after,
                    "patched\n"
                );
                let destination = fixture.target(&fixture.workspace.join("moved")).await;
                let moved = fixture
                    .fs
                    .move_file(&source, &destination, FsMoveOptions::default(), None, None)
                    .await
                    .unwrap();
                assert!(matches!(moved, hoh_fs::FsMoveOutcome::Deleted { .. }));
                assert_eq!(
                    fixture
                        .fs
                        .delete_file(&destination, None, None, None)
                        .await
                        .unwrap()
                        .before
                        .as_deref(),
                    Some("patched\n")
                );
            }
            SandboxMode::DangerFullAccess => {
                assert_eq!(
                    fixture
                        .fs
                        .patch_text(&source, &patch, None, None, None)
                        .await
                        .unwrap()
                        .after,
                    "patched\n"
                );
                fixture
                    .fs
                    .move_file(&source, &outside, FsMoveOptions::default(), None, None)
                    .await
                    .unwrap();
                assert_eq!(fs::read_to_string(&outside_path).unwrap(), "patched\n");
                assert_eq!(
                    fixture
                        .fs
                        .delete_file(&outside, None, None, None)
                        .await
                        .unwrap()
                        .before
                        .as_deref(),
                    Some("patched\n")
                );
            }
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn absolute_dot_dot_and_symlink_denials_leave_existing_files_untouched() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new(SandboxMode::WorkspaceWrite).await;
    let absolute = fixture.outside.join("absolute.txt");
    let dot_dot = fixture.workspace.parent().unwrap().join("dot-dot.txt");
    let symlink_target = fixture.outside.join("symlink.txt");
    for path in [&absolute, &dot_dot, &symlink_target] {
        fs::write(path, "original").unwrap();
    }
    symlink(&fixture.outside, fixture.workspace.join("link")).unwrap();
    let requests = [
        absolute.clone(),
        fixture.workspace.join("../dot-dot.txt"),
        fixture.workspace.join("link/symlink.txt"),
    ];
    for requested in requests {
        let target = fixture.target(&requested).await;
        let error = fixture
            .fs
            .write_text(&target, "changed", None, None, None)
            .await
            .expect_err("escape must deny");
        assert_error_code(&error, "FS_SANDBOX_DENIED");
    }
    for path in [absolute, dot_dot, symlink_target] {
        assert_eq!(fs::read_to_string(path).unwrap(), "original");
    }
}
