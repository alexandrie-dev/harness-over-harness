//! Exact Host projections and stopped output without invented file effects.

mod support;

use hoh_tool_fs::apply_patch_output::{patch_presentation_meta, render_patch};
use hoh_tool_fs::apply_patch_presentation::{present_patch_call, present_patch_result};
use hoh_tool_fs::diff::stopped_from_meta;
use hoh_tools::{ToolCallView, ToolResult};
use serde_json::{Value, json};
use support::patch::*;

fn call_view(patch_text: &str) -> Value {
    serde_json::to_value(present_patch_call(patch_text).map(ToolCallView::Diff)).unwrap()
}

fn result_view(result: &ToolResult) -> Value {
    serde_json::to_value(present_patch_result(result)).unwrap()
}

#[tokio::test]
async fn previews_every_operation_using_raw_paths_counting_distinct_files_including_move_endpoints()
{
    let text = patch(&[
        &add("a"),
        &update("b"),
        &remove("c"),
        &mv("d", "e"),
        "*** Update File: empty\n@@\n+insert",
        &update_with("b", "new", "last"),
    ]);
    let locations: Vec<_> = ["a", "b", "c", "d", "e", "empty"]
        .iter()
        .map(|path| json!({ "path": path }))
        .collect();
    let expected = json!({
        "card": "diff",
        "title": "Apply patch (6 files)",
        "diffs": [
            { "path": "a", "oldText": null, "newText": "new\n" },
            { "path": "b", "oldText": "old", "newText": "new" },
            { "path": "d", "oldText": "old", "newText": "new" },
            { "path": "empty", "oldText": null, "newText": "insert" },
            { "path": "b", "oldText": "new", "newText": "last" },
        ],
        "locations": locations,
    });
    assert_eq!(call_view(&text), expected);
    let t = setup(PatchOptions::default()).await;
    let definition = t.env.tools.get("apply_patch", None, None).unwrap();
    let present = definition.present_call.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(present(&json!({ "input": text }))).unwrap(),
        expected
    );
    assert_eq!(
        call_view(&patch(&[&remove("only")])),
        json!({ "card": "diff", "title": "Apply patch (1 file)", "diffs": [], "locations": [{ "path": "only" }] })
    );
    assert_eq!(call_view("invalid"), Value::Null);
    t.kernel.shutdown().await;
}

#[tokio::test]
async fn falls_back_on_failed_or_missing_metadata_and_titles_known_stopped_diffs() {
    let diffs = json!([{ "path": "a", "oldText": "old", "newText": "new" }]);
    let result = |is_error: bool, meta: Option<Value>| ToolResult {
        content: vec![],
        is_error,
        meta,
    };
    assert_eq!(
        result_view(&result(true, Some(json!({ "diffs": diffs })))),
        Value::Null
    );
    assert_eq!(result_view(&result(false, None)), Value::Null);
    assert_eq!(
        result_view(&result(false, Some(json!({ "diffs": diffs })))),
        json!({ "card": "diff", "title": "Apply patch", "diffs": diffs })
    );
    let stopped = result(false, Some(json!({ "diffs": diffs, "stopped": true })));
    let expected = json!({ "card": "diff", "title": "Apply patch (stopped)", "diffs": diffs });
    assert_eq!(result_view(&stopped), expected);
    let t = setup(PatchOptions::default()).await;
    let definition = t.env.tools.get("apply_patch", None, None).unwrap();
    let present = definition.present_result.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(present(&json!({ "input": patch(&[]) }), &stopped)).unwrap(),
        expected
    );
    t.kernel.shutdown().await;
}

#[test]
fn rejects_a_malformed_stopped_flag() {
    for meta in [
        None,
        Some(Value::Null),
        Some(json!([])),
        Some(json!("stopped")),
        Some(json!({})),
        Some(json!({ "stopped": "true" })),
        Some(json!({ "stopped": false })),
    ] {
        assert!(!stopped_from_meta(meta.as_ref()), "{meta:?}");
    }
}

#[test]
fn stores_no_diff_for_creations_or_for_removals_without_a_bounded_basis() {
    let value = json!({
        "phase": "commit", "cancelled": false, "affectedPaths": ["a", "b"],
        "operations": [
            { "index": 0, "kind": "delete", "path": "a", "patchLine": 2, "status": "completed",
              "changes": [{ "path": "a", "before": null, "after": null }] },
            { "index": 1, "kind": "add", "path": "d", "patchLine": 3, "status": "completed",
              "changes": [{ "path": "d", "before": null, "after": "created\n" }] },
            { "index": 2, "kind": "delete", "path": "e", "patchLine": 5, "status": "completed",
              "changes": [{ "path": "e", "before": "gone\n", "after": null }] },
            { "index": 3, "kind": "update", "path": "b", "patchLine": 6, "status": "failed",
              "code": "FS_PERMISSION_DENIED", "message": "denied" },
            { "index": 4, "kind": "add", "path": "c", "patchLine": 7, "status": "skipped" },
        ],
    });
    assert_eq!(
        patch_presentation_meta(&value),
        json!({ "stopped": true, "diffs": [{ "path": "e", "oldText": "gone\n", "newText": "" }] })
    );
    assert_eq!(
        render_patch(&value),
        "Patch stopped after 3 of 5 operations.\nCompleted:\nA d\nD a\nD e\nFailed:\n\
         - operation 4 (update b, patch line 6): denied\nNot executed:\n- operation 5 (add c)"
    );
}

#[test]
fn reports_cancellation_and_a_move_failure_with_no_written_destination() {
    let value = json!({
        "phase": "commit", "cancelled": true, "affectedPaths": [],
        "operations": [
            { "index": 0, "kind": "move", "path": "a", "destination": "b", "patchLine": 2,
              "status": "failed", "message": "cancelled", "changes": [] },
            { "index": 1, "kind": "delete", "path": "c", "patchLine": 7, "status": "skipped" },
        ],
    });
    assert_eq!(
        render_patch(&value),
        "Patch stopped after 0 of 2 operations.\nFailed:\n\
         - operation 1 (move a -> b, patch line 2): cancelled\nNot executed:\n- operation 2 (delete c)"
    );
}
