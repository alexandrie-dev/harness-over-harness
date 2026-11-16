// Fixtures adapted from OpenAI Codex rust-v0.158.0 (Apache-2.0), via DSH.
// Copyright 2025 OpenAI. Modifications: Rust port of the TypeScript fixtures.
use hoh_fs_patch::{ApplyHunksResult, PatchHunk, PatchOperation, apply_hunks, parse_patch};
use serde_json::json;

fn update(body: &[&str]) -> Vec<PatchHunk> {
    let input = format!(
        "*** Begin Patch\n*** Update File: a\n{}\n*** End Patch",
        body.join("\n")
    );
    match parse_patch(&input).unwrap().remove(0) {
        PatchOperation::Update { hunks, .. } => hunks,
        _ => panic!("Expected one parsed update"),
    }
}

fn success(before: &str, body: &[&str], after: &str) {
    let hunks = update(body);
    let original = serde_json::to_string(&hunks).unwrap();
    let result = apply_hunks(before, &hunks);
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({"ok":true,"after":after})
    );
    assert_eq!(
        serde_json::from_str::<ApplyHunksResult>(&serde_json::to_string(&result).unwrap()).unwrap(),
        result
    );
    assert_eq!(serde_json::to_string(&hunks).unwrap(), original);
}

macro_rules! successes {
    ($($name:ident: $before:expr, [$($line:expr),*], $after:expr;)*) => {$(
        #[test] fn $name() { success($before, &[$($line),*], $after); }
    )*};
}
successes! {
    multiple_hunks_search_original_text: "foo\nbar\nbaz\nqux\n", ["@@", " foo", "-bar", "+BAR", "@@", " baz", "-qux", "+QUX"], "foo\nBAR\nbaz\nQUX\n";
    interleaved_replacements_and_eof_addition: "a\nb\nc\nd\ne\nf\n", ["@@", " a", "-b", "+B", "@@", " c", " d", "-e", "+E", "@@", " f", "+g", "*** End of File"], "a\nB\nc\nd\nE\nf\ng\n";
    append_precedes_removal_in_patch_order: "line1\nline2\nline3\n", ["@@", "+after-context", "+second-line", "@@", " line1", "-line2", "-line3", "+line2-replacement"], "line1\nline2-replacement\nafter-context\nsecond-line\n";
    ascii_patch_matches_unicode_dashes: "import asyncio  # local import – avoids top‑level dep\n", ["-import asyncio  # local import - avoids top-level dep", "+import asyncio  # HELLO"], "import asyncio  # HELLO\n";
    first_line_replacement: "foo\nbar\nbaz\n", ["-foo", "+FOO", " bar"], "FOO\nbar\nbaz\n";
    last_line_replacement: "foo\nbar\nbaz\n", [" foo", " bar", "-baz", "+BAZ"], "foo\nbar\nBAZ\n";
    pure_insertion_with_eof: "foo\nbar\nbaz\n", ["+quux", "*** End of File"], "foo\nbar\nbaz\nquux\n";
    exact_sequence: "foo\nbar\nbaz\n", ["-bar", "-baz", "+done"], "foo\ndone\n";
    trailing_whitespace_ignored_both_sides: "foo   \nbar\t\t\n", ["-foo\t", "-bar ", "+done"], "done\n";
    leading_and_trailing_whitespace_ignored: "    foo   \n   bar\t\n", ["- foo", "-bar", "+done"], "done\n";
    exact_wins_over_earlier_trimmed_match: "x \nx\n", ["-x", "+X"], "x \nX\n";
    trailing_only_wins_over_fully_trimmed: " x\nx \n", ["-x", "+X"], " x\nX\n";
    trimmed_wins_over_punctuation: "x–y\n x-y \n", ["-x-y", "+X"], "x–y\nX\n";
    first_remaining_repeated_sequence: "x\nx\nx\n", ["-x", "+first", "@@", "-x", "+second"], "first\nsecond\nx\n";
    anchors_advance_beyond_match: "fn a\nx\nfn b\nx\n", ["@@ fn b", "-x", "+X"], "fn a\nx\nfn b\nX\n";
    repeated_anchors_use_cursor: "fn\nx\nfn\nx\n", ["@@ fn", "-x", "+first", "@@ fn", "-x", "+second"], "fn\nfirst\nfn\nsecond\n";
    anchors_use_punctuation_tolerance: "fn “a”\nx\n", ["@@ fn \"a\"", "-x", "+X"], "fn “a”\nX\n";
    empty_file_insertion: "", ["+hello"], "hello\n";
    insertion_after_unterminated_line: "hello", ["+world"], "hello\nworld\n";
    insertion_before_final_empty_line: "hello\n\n", ["+world"], "hello\nworld\n";
    insertion_into_one_newline_file: "\n", ["+hello"], "hello\n";
    anchored_pure_insertion_appends: "fn\nx\ny\n", ["@@ fn", "+last"], "fn\nx\ny\nlast\n";
    same_position_insertions_keep_order: "x\n", ["+first", "@@", "+second"], "x\nfirst\nsecond\n";
    pure_deletion_retains_surroundings: "a\nb\nc\n", ["-b"], "a\nc\n";
    deletion_to_empty: "a\n", ["-a"], "";
    replacement_adds_final_newline: "a", ["-a", "+b"], "b\n";
    eof_prefers_tail: "x\nx \n", ["-x", "+X", "*** End of File"], "x\nX\n";
    eof_can_search_before_cursor: "x\n", ["@@ x", "-x", "+X", "*** End of File"], "X\n";
    overlapping_eof_hunks_keep_order: "x\n", ["-x", "+first", "@@", "-x", "+second", "*** End of File"], "first\n";
    trailing_empty_context_retries_without_sentinels: "a\n", ["-a", "+b", ""], "b\n";
    trailing_empty_old_line_keeps_insertion: "a\n", ["-a", "-", "+b"], "b\n";
    empty_retry_matches_cursor: "", ["-", "+b"], "b\n";
    empty_retry_after_anchor_matches_cursor: "a\n", ["@@ a", "-", "+b"], "a\nb\n";
    tolerated_context_replaced_from_patch: "  context\r\nold\n", [" context", "-old", "+new"], "context\nnew\n";
    crlf_retains_cr_on_untouched_lines: "a\r\nb\r\n", ["-a", "+A"], "A\nb\r\n";
    append_does_not_normalize_crlf: "a\r\n", ["+b"], "a\r\nb\n";
    lone_cr_is_ordinary: "a\rb\n", ["-a\rb", "+c"], "c\n";
    non_ascii_survives: "你好🙂\n", [" 你好🙂", "+é"], "你好🙂\né\n";
    long_whitespace_survives_searches: &format!("{}x–y\n", " ".repeat(32_000)), ["-x-y", "+done"], "done\n";
}

#[test]
fn normalizes_interior_punctuation_on_either_side() {
    for (characters, ascii) in [
        ("‐‑‒–—―−", '-'),
        ("‘’‚‛", '\''),
        ("“”„‟", '"'),
        (
            "\u{a0}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{202f}\u{205f}\u{3000}",
            ' ',
        ),
    ] {
        for character in characters.chars() {
            success(
                &format!("a{character}b\n"),
                &[&format!("-a{ascii}b"), "+done"],
                "done\n",
            );
            success(
                &format!("a{ascii}b\n"),
                &[&format!("-a{character}b"), "+done"],
                "done\n",
            );
        }
    }
}

#[test]
fn trims_rust_white_space() {
    for space in ['\u{85}', '\u{a0}', '\u{2003}', '\u{3000}'] {
        success(&format!("{space}x{space}\n"), &["-x", "+done"], "done\n");
    }
}

fn failure(before: &str, body: &[&str], line: u64, stage: &str, start: u64, pattern: &[&str]) {
    let hunks = update(body);
    let original = serde_json::to_string(&hunks).unwrap();
    let result = apply_hunks(before, &hunks);
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({"ok":false,"failure":{"hunkLineNumber":line,"stage":stage,"searchStart":start,"pattern":pattern}})
    );
    assert_eq!(
        serde_json::from_str::<ApplyHunksResult>(&serde_json::to_string(&result).unwrap()).unwrap(),
        result
    );
    assert_eq!(serde_json::to_string(&hunks).unwrap(), original);
}
macro_rules! failures {
    ($($name:ident: $before:expr, [$($line:expr),*], $ln:expr, $stage:expr, $start:expr, [$($pattern:expr),*];)*) => {$(
        #[test] fn $name() { failure($before, &[$($line),*], $ln, $stage, $start, &[$($pattern),*]); }
    )*};
}
failures! {
    pattern_longer_than_file: "one\n", ["-too","-many","-lines"], 3,"oldLines",0,["too","many","lines"];
    anchor_not_found: "a\n", ["@@ missing","-a","+b"],3,"context",0,["missing"];
    old_content_not_found_after_anchor: "fn\na\n", ["@@ fn","-missing","+b"],3,"oldLines",1,["missing"];
    later_context_only_before_cursor: "fn\na\n", ["-a","+b","@@ fn","+c"],5,"context",2,["fn"];
    later_old_lines_only_before_cursor: "a\nb\n", ["-b","+B","@@","-a","+A"],5,"oldLines",2,["a"];
    inserted_text_not_search_candidate: "a\n", ["-a","+b","@@","-b","+c"],5,"oldLines",1,["b"];
    eof_does_not_fall_back: "x\ny\n", ["-x","+X","*** End of File"],3,"oldLines",0,["x"];
    failed_retry_reports_original_pattern: "a\n", ["-missing","-","+b","+"],3,"oldLines",0,["missing",""];
    bom_is_not_whitespace: "\u{feff}a\n", ["-a","+b"],3,"oldLines",0,["a"];
    combining_marks_not_normalized: "é\n", ["-é","+b"],3,"oldLines",0,["é"];
    en_quad_outside_mapping: "a\u{2000}b\n", ["-a b","+b"],3,"oldLines",0,["a b"];
    lone_cr_does_not_split: "a\rb", ["-a","-b","+c"],3,"oldLines",0,["a","b"];
}

#[test]
fn empty_hunk_list_applies_normalize_to_lf_reconstruction() {
    let operations = parse_patch("*** Begin Patch\n*** End Patch").unwrap();
    let hunks: Vec<_> = operations
        .into_iter()
        .flat_map(|operation| match operation {
            PatchOperation::Update { hunks, .. } => hunks,
            _ => vec![],
        })
        .collect();
    for (before, after) in [("", ""), ("a", "a\n"), ("a\n", "a\n"), ("a\n\n", "a\n")] {
        assert_eq!(
            apply_hunks(before, &hunks),
            ApplyHunksResult::Success {
                after: after.into()
            }
        );
    }
}

#[test]
fn failure_patterns_independent_of_hunk_arrays() {
    let hunks = update(&["-missing", "+replacement"]);
    let mut result = apply_hunks("other\n", &hunks);
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({"ok":false,"failure":{"hunkLineNumber":3,"stage":"oldLines","searchStart":0,"pattern":["missing"]}})
    );
    let ApplyHunksResult::Failure { failure } = &mut result else {
        panic!("Expected failure")
    };
    failure.pattern.push("caller annotation".into());
    assert_eq!(hunks[0].old_lines, ["missing"]);
}

#[test]
fn serialized_application_records_preserve_source_key_order() {
    assert_eq!(
        serde_json::to_string(&apply_hunks("a\n", &update(&["-a", "+b"]))).unwrap(),
        r#"{"ok":true,"after":"b\n"}"#,
    );
    assert_eq!(
        serde_json::to_string(&apply_hunks("a\n", &update(&["@@ missing", "+b"]))).unwrap(),
        r#"{"ok":false,"failure":{"hunkLineNumber":3,"stage":"context","searchStart":0,"pattern":["missing"]}}"#,
    );
    assert_eq!(
        serde_json::to_string(&apply_hunks("a\n", &update(&["-missing", "+b"]))).unwrap(),
        r#"{"ok":false,"failure":{"hunkLineNumber":3,"stage":"oldLines","searchStart":0,"pattern":["missing"]}}"#,
    );
}
