//! The registered patch capability shares native values and preserves known partial effects.

mod support;

use std::sync::Arc;

use hoh_fs::{FsError, FsErrorCode, FsMoveOutcome, FsMoveSourceFailure};
use hoh_scope::{CreateScopeOptions, create_scope};
use hoh_system_prompt::{AssembleContext, SystemPromptKey};
use hoh_tools::ToolRestriction;
use serde_json::json;
use support::patch::*;
use support::{error_info, named};

const GUIDANCE: &str = concat!(
    "Prefer apply_patch over separate edit calls for changes across files or separated regions. ",
    "Give every hunk enough surrounding context lines to match one place in the current file. ",
    "The whole patch is checked first (syntax, hunk matching, missing files) and a rejected patch changes nothing; once applying starts, operations run in order, stop at the first failure, and are not rolled back, so re-read affected files before retrying when needed.",
);

fn set_hooks(t: &T, set: impl FnOnce(&mut Hooks)) {
    set(&mut support::lock(&t.spy.hooks));
}

fn not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

fn content_text(result: &hoh_tools::ToolExecutionResult) -> String {
    result.text()
}

#[tokio::test]
async fn omits_patch_registration_and_guidance_when_disabled() {
    let t = setup(PatchOptions {
        without_apply_patch: true,
        ..PatchOptions::default()
    })
    .await;
    assert!(t.env.tools.get("apply_patch", None, None).is_none());
    let prompt = t.ctx.get::<SystemPromptKey>().unwrap();
    let assembly = prompt.assemble(AssembleContext::default()).await.unwrap();
    assert!(
        assembly
            .sections
            .iter()
            .all(|section| section.name != "tool:apply_patch")
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn registers_patch_guidance_when_enabled_and_removes_tool_and_section_on_disposal() {
    let t = setup(PatchOptions::default()).await;
    assert_eq!(
        t.env
            .tools
            .get("apply_patch", None, None)
            .unwrap()
            .description,
        concat!(
            "Create, update, delete, or move UTF-8 text files with one patch; ",
            "prefer it over multiple edit calls when a change spans several files or separated regions."
        )
    );
    let agent = support::TestAgent::new();
    let scope = create_scope(
        &t.ctx,
        agent.key.clone(),
        CreateScopeOptions { parent: None },
    )
    .unwrap();
    let prompt = t.ctx.get::<SystemPromptKey>().unwrap();
    let section = || {
        let prompt = prompt.clone();
        let key = agent.key.clone();
        async move {
            prompt
                .assemble(AssembleContext::new(Some(key)))
                .await
                .unwrap()
                .sections
                .into_iter()
                .find(|section| section.name == "tool:apply_patch")
                .map(|section| section.text)
        }
    };
    assert_eq!(section().await.as_deref(), Some(GUIDANCE));
    let release = t
        .env
        .tools
        .restrict(
            &scope.ctx,
            ToolRestriction {
                allow: None,
                deny: Some(vec!["apply_patch".into()]),
            },
        )
        .unwrap();
    assert_eq!(section().await.as_deref(), Some(""));
    release.dispose().await;
    assert_eq!(section().await.as_deref(), Some(GUIDANCE));
    t.tool_fs.dispose().await;
    assert!(t.env.tools.get("apply_patch", None, None).is_none());
    assert_eq!(section().await, None);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn returns_exact_native_values_and_applied_metadata_for_all_four_operation_kinds() {
    let t = setup(PatchOptions::default()).await;
    t.file("edit", "old\n", true).await;
    t.file("delete", "gone\n", true).await;
    t.file("source", "old\n", true).await;
    let result = t
        .call_patch(&patch(&[
            &update("edit"),
            &add("created"),
            &remove("delete"),
            &mv("source", "destination"),
        ]))
        .await;
    assert!(!result.is_error(), "{}", result.text());
    let path = |name: &str| t.path_text(name);
    let affected_paths: Vec<String> = ["edit", "created", "delete", "destination", "source"]
        .iter()
        .map(|name| path(name))
        .collect();
    assert_eq!(
        result.value(),
        Some(&json!({
            "phase": "commit",
            "cancelled": false,
            "operations": [
                { "index": 0, "kind": "update", "path": path("edit"), "patchLine": 2, "status": "completed",
                  "changes": [{ "path": path("edit"), "before": "old\n", "after": "new\n" }] },
                { "index": 1, "kind": "add", "path": path("created"), "patchLine": 6, "status": "completed",
                  "changes": [{ "path": path("created"), "before": null, "after": "new\n" }] },
                { "index": 2, "kind": "delete", "path": path("delete"), "patchLine": 8, "status": "completed",
                  "changes": [{ "path": path("delete"), "before": "gone\n", "after": null }] },
                { "index": 3, "kind": "move", "path": path("source"), "destination": path("destination"),
                  "patchLine": 9, "status": "completed",
                  "changes": [
                      { "path": path("destination"), "before": null, "after": "new\n" },
                      { "path": path("source"), "before": "old\n", "after": null },
                  ] },
            ],
            "affectedPaths": affected_paths,
        }))
    );
    assert_eq!(
        result.text(),
        format!(
            "Success. Updated the following files:\nA {}\nM {}\nM {}\nD {}",
            path("created"),
            path("edit"),
            path("destination"),
            path("delete")
        )
    );
    // Creations store no diff, as for write; removals store the provider's bounded basis.
    assert_eq!(
        result.meta(),
        Some(&json!({
            "stopped": false,
            "diffs": [
                { "path": path("edit"), "oldText": "old", "newText": "new" },
                { "path": path("delete"), "oldText": "gone\n", "newText": "" },
                { "path": path("source"), "oldText": "old\n", "newText": "" },
            ],
        }))
    );
    assert_eq!(t.bytes("edit").unwrap(), "new\n");
    assert_eq!(t.bytes("destination").unwrap(), "new\n");
    assert!(not_found(&t.bytes("source").unwrap_err()));
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn returns_the_empty_patch_confirmation() {
    let t = setup(PatchOptions::default()).await;
    let result = t.call_patch(&patch(&[])).await;
    assert!(!result.is_error());
    assert_eq!(
        result.value(),
        Some(
            &json!({ "phase": "commit", "cancelled": false, "operations": [], "affectedPaths": [] })
        )
    );
    assert_eq!(result.text(), "Success. No operations in patch.");
    assert_eq!(
        result.meta(),
        Some(&json!({ "diffs": [], "stopped": false }))
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn throws_typed_syntax_and_aggregated_unread_file_refusals_without_diffs() {
    let t = setup(PatchOptions::default()).await;
    let syntax = t.call_patch("invalid").await;
    assert!(syntax.is_error());
    assert_eq!(error_info(&syntax), named("PatchToolError", "PATCH_SYNTAX"));
    assert_eq!(
        content_text(&syntax),
        "Error: Invalid patch at line 1: Expected '*** Begin Patch'"
    );
    t.file("a", "old\n", false).await;
    t.file("b", "old\n", false).await;
    let result = t.call_patch(&patch(&[&update("a"), &update("b")])).await;
    assert!(result.is_error());
    assert_eq!(error_info(&result), named("FsError", "FS_NOT_OBSERVED"));
    let (a, b) = (t.path_text("a"), t.path_text("b"));
    assert_eq!(
        content_text(&result),
        format!(
            "Error: Patch rejected before any change (2 problems):\n\
             - operation 1 (update {a}, patch line 2): cannot modify \"{a}\": file has not been read — read the file, then retry\n\
             - operation 2 (update {b}, patch line 6): cannot modify \"{b}\": file has not been read — read the file, then retry"
        )
    );
    assert!(result.meta().is_none());
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn preserves_the_symbolic_link_refusal_code() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    std::os::unix::fs::symlink("a", t.path("link")).unwrap();
    let result = t.call_patch(&patch(&[&remove("link")])).await;
    assert_eq!(
        error_info(&result),
        named("PatchToolError", "PATCH_SYMLINK_SOURCE")
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn keeps_a_partial_move_as_a_value_with_its_destination_diff_and_exact_stop_text() {
    let t = setup(PatchOptions::default()).await;
    t.file("a", "old\n", true).await;
    // Preserve the actual destination write while deterministically retaining the source.
    set_hooks(&t, |hooks| {
        hooks.move_file = Some(Arc::new(|source, destination| {
            let read = std::fs::read_to_string(source.target_key.as_ref() as &str).unwrap();
            std::fs::write(destination.target_key.as_ref() as &str, "new\n").unwrap();
            Some(Ok(FsMoveOutcome::Retained {
                source_before: Some(read),
                destination: hoh_fs::FsWriteOutcome {
                    operation: hoh_fs::FsWriteOperation::Create,
                    version: "moved".into(),
                    before: None,
                    after: "new\n".into(),
                },
                source_failure: FsMoveSourceFailure {
                    code: FsErrorCode::PermissionDenied,
                    message: "source removal denied".into(),
                },
            }))
        }));
    });
    let result = t
        .call_patch(&patch(&[&add("first"), &mv("a", "b"), &add("last")]))
        .await;
    assert!(!result.is_error(), "{}", result.text());
    let (first, a, b, last) = (
        t.path_text("first"),
        t.path_text("a"),
        t.path_text("b"),
        t.path_text("last"),
    );
    assert_eq!(
        result.text(),
        format!(
            "Patch stopped after 1 of 3 operations.\nCompleted:\nA {first}\nFailed:\n\
             - operation 2 (move {a} -> {b}, patch line 4): source removal denied\n\
             \x20 destination {b} was written; source {a} remains\nNot executed:\n- operation 3 (add {last})"
        )
    );
    assert_eq!(
        result.meta(),
        Some(&json!({ "stopped": true, "diffs": [] }))
    );
    assert_eq!(t.bytes("a").unwrap(), "old\n");
    assert_eq!(t.bytes("b").unwrap(), "new\n");
    assert!(not_found(&t.bytes("last").unwrap_err()));
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn reports_uncertain_writes_with_no_invented_diff_and_the_reread_remedy() {
    let t = setup(PatchOptions::default()).await;
    set_hooks(&t, |hooks| {
        hooks.write_text_lost_reply = Some(Arc::new(|()| {
            Some(FsError::new("connection lost", FsErrorCode::IoError).into())
        }));
    });
    let result = t.call_patch(&patch(&[&add("a"), &add("b")])).await;
    assert!(!result.is_error(), "{}", result.text());
    let (a, b) = (t.path_text("a"), t.path_text("b"));
    assert_eq!(
        result.text(),
        format!(
            "Patch stopped after 0 of 2 operations.\nOutcome unknown:\n\
             - operation 1 (add {a}, patch line 2): connection lost — re-read the affected files before retrying\n\
             Not executed:\n- operation 2 (add {b})"
        )
    );
    assert_eq!(
        result.meta(),
        Some(&json!({ "diffs": [], "stopped": true }))
    );
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn advertises_no_escalation_for_a_non_confining_backend() {
    let bare = setup(PatchOptions::default()).await;
    let definition = bare.env.tools.get("apply_patch", None, None).unwrap();
    assert!(!definition.parameters.contains_key("sandbox_permissions"));
    bare.kernel.shutdown().await;
}

#[tokio::test]
async fn retains_completed_operations_after_a_real_write_permission_failure() {
    use std::os::unix::fs::PermissionsExt;
    let t = setup(PatchOptions::default()).await;
    let locked = t.path("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    // A privileged environment is not denied by the mode; the source test skips for root.
    if std::fs::write(locked.join("probe"), "x").is_ok() {
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        t.kernel.shutdown().await;
        return;
    }
    let result = t
        .call_patch(&patch(&[
            &add("first"),
            &add("locked/denied"),
            &add("last"),
        ]))
        .await;
    assert!(!result.is_error(), "{}", result.text());
    let first = t.path_text("first");
    let denied = t.path_text("locked/denied");
    assert_eq!(
        result.text(),
        format!(
            "Patch stopped after 1 of 3 operations.\nCompleted:\nA {first}\nFailed:\n\
             - operation 2 (add {denied}, patch line 4): cannot write \"{denied}\": permission denied\nNot executed:\n\
             - operation 3 (add {})",
            t.path_text("last")
        )
    );
    assert_eq!(
        result.meta(),
        Some(&json!({ "stopped": true, "diffs": [] }))
    );
    assert_eq!(t.bytes("first").unwrap(), "new\n");
    assert!(not_found(&t.bytes("last").unwrap_err()));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    t.kernel.shutdown().await;
}
