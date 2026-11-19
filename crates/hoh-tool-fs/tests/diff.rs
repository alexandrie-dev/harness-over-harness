//! Unit tests for the result-time contextual-diff computation (`src/diff.rs`): the pure
//! before/after → `FileDiff` hunk builder and the defensive `meta` narrowing. These pin the exact
//! hunk reconstruction (context lines, multi-hunk replaceAll, pure insertion/deletion, no-op) that
//! UIs render.

use hoh_tool_fs::diff::{DIFF_CONTEXT, compute_hunk_diffs, diffs_from_meta};
use hoh_tools::FileDiff;
use serde_json::{Value, json};

fn lines(n: usize) -> String {
    let body: Vec<String> = (1..=n).map(|i| format!("line{i}")).collect();
    format!("{}\n", body.join("\n"))
}

fn diff(path: &str, old: Option<&str>, new: &str) -> FileDiff {
    FileDiff {
        path: path.into(),
        old_text: old.map(str::to_owned),
        new_text: new.into(),
    }
}

#[test]
fn a_single_line_change_yields_one_hunk_with_context_lines_on_both_sides() {
    let before = lines(8);
    let after = before.replace("line4", "CHANGED");
    let diffs = compute_hunk_diffs("f.txt", &before, &after);
    assert_eq!(
        diffs,
        [diff(
            "f.txt",
            Some("line1\nline2\nline3\nline4\nline5\nline6\nline7"),
            "line1\nline2\nline3\nCHANGED\nline5\nline6\nline7",
        )]
    );
}

#[test]
fn a_scattered_replace_all_yields_one_file_diff_per_hunk() {
    let before = lines(20);
    let after = before.replace("line3", "A").replace("line16", "B");
    let diffs = compute_hunk_diffs("f.txt", &before, &after);
    assert_eq!(diffs.len(), 2);
    assert_eq!(diffs[0].path, "f.txt");
    assert!(diffs[0].old_text.as_deref().unwrap().contains("line3"));
    assert!(diffs[0].new_text.contains('A'));
    assert!(diffs[1].old_text.as_deref().unwrap().contains("line16"));
    assert!(diffs[1].new_text.contains('B'));
    // The two hunks are distinct sites, not one merged block.
    assert!(!diffs[0].new_text.contains('B'));
    assert!(!diffs[1].new_text.contains('A'));
}

#[test]
fn identical_before_and_after_yields_no_hunks() {
    assert_eq!(compute_hunk_diffs("f.txt", "same\n", "same\n"), []);
}

#[test]
fn a_pure_insertion_into_empty_content_reports_old_text_none() {
    assert_eq!(
        compute_hunk_diffs("f.txt", "", "brand new\n"),
        [diff("f.txt", None, "brand new")]
    );
}

#[test]
fn a_pure_deletion_of_the_whole_file_reports_new_text_empty() {
    assert_eq!(
        compute_hunk_diffs("f.txt", "gone\n", ""),
        [diff("f.txt", Some("gone"), "")]
    );
}

#[test]
fn drops_the_no_newline_marker_from_a_no_trailing_newline_change() {
    let diffs = compute_hunk_diffs("f.txt", "x", "y");
    // The marker line (starting with "\") must never leak into a diff block.
    assert_eq!(diffs, [diff("f.txt", Some("x"), "y")]);
    assert!(!diffs[0].old_text.as_deref().unwrap().contains('\\'));
    assert!(!diffs[0].new_text.contains('\\'));
}

#[test]
fn uses_diff_context_surrounding_lines() {
    assert_eq!(DIFF_CONTEXT, 3);
    let before = lines(20);
    let after = before.replace("line10", "CHANGED");
    let diffs = compute_hunk_diffs("f.txt", &before, &after);
    let first = &diffs[0];
    // 3 context above (7,8,9) + the change + 3 below (11,12,13) = 7 lines a side.
    assert_eq!(first.old_text.as_deref().unwrap().split('\n').count(), 7);
    assert_eq!(first.new_text.split('\n').count(), 7);
    assert_eq!(
        first.old_text.as_deref().unwrap().split('\n').next(),
        Some("line7")
    );
}

fn good() -> Value {
    json!({ "diffs": [{ "path": "f.txt", "oldText": "a", "newText": "b" }] })
}

#[test]
fn diffs_from_meta_narrows_a_well_formed_payload() {
    assert_eq!(
        diffs_from_meta(Some(&good())),
        Some(vec![diff("f.txt", Some("a"), "b")])
    );
}

#[test]
fn diffs_from_meta_accepts_a_diff_whose_old_text_is_null() {
    let meta = json!({ "diffs": [{ "path": "f.txt", "oldText": null, "newText": "x" }] });
    assert_eq!(
        diffs_from_meta(Some(&meta)),
        Some(vec![diff("f.txt", None, "x")])
    );
}

#[test]
fn diffs_from_meta_rejects_missing_or_non_object_meta() {
    assert_eq!(diffs_from_meta(None), None);
    assert_eq!(diffs_from_meta(Some(&Value::Null)), None);
    assert_eq!(diffs_from_meta(Some(&json!("nope"))), None);
    assert_eq!(diffs_from_meta(Some(&json!([]))), None);
}

#[test]
fn diffs_from_meta_rejects_a_missing_empty_or_non_array_diffs_field() {
    assert_eq!(diffs_from_meta(Some(&json!({}))), None);
    assert_eq!(diffs_from_meta(Some(&json!({ "diffs": [] }))), None);
    assert_eq!(diffs_from_meta(Some(&json!({ "diffs": "x" }))), None);
}

#[test]
fn diffs_from_meta_rejects_a_diffs_array_containing_a_malformed_entry() {
    for diffs in [
        json!([{ "path": "f.txt", "oldText": "a" }]),
        json!([{ "path": 1, "oldText": "a", "newText": "b" }]),
        json!([{ "path": "f", "oldText": 5, "newText": "b" }]),
        json!([{ "path": "f", "oldText": "a", "newText": 7 }]),
        json!([null]),
        json!(["x"]),
        json!([[]]),
    ] {
        assert_eq!(diffs_from_meta(Some(&json!({ "diffs": diffs }))), None);
    }
}
