// Parser compatibility fixtures adapted from OpenAI Codex rust-v0.158.0, Apache-2.0.
// Copyright 2025 OpenAI. Modifications: Rust port of the TypeScript fixtures.
use hoh_fs_patch::{PatchOperation, parse_patch};
use serde_json::{Value, json};

fn patch(lines: &[&str]) -> String {
    [
        vec!["*** Begin Patch"],
        lines.to_vec(),
        vec!["*** End Patch"],
    ]
    .concat()
    .join("\n")
}
fn parsed(input: &str) -> Value {
    serde_json::to_value(parse_patch(input).unwrap()).unwrap()
}

#[test]
fn preserves_file_order_repeated_paths_and_move_endpoints() {
    let input = patch(&[
        "*** Add File: dir/a.txt",
        "+hello",
        "+",
        "*** Update File: dir/a.txt",
        "*** Move to: dir/b.txt",
        "@@ hello",
        "-hello",
        "+hi",
        "+there",
        " same",
        "-same",
        "+same",
        "@@",
        " last",
        "+tail",
        "*** End of File",
        "*** Delete File: /tmp/obsolete",
        "*** Add File: dir/a.txt",
    ]);
    let operations = parse_patch(&input).unwrap();
    assert_eq!(
        serde_json::to_value(&operations).unwrap(),
        json!([
        {"kind":"add","path":"dir/a.txt","lineNumber":2,"content":"hello\n\n"},
        {"kind":"update","path":"dir/a.txt","lineNumber":5,"moveTo":"dir/b.txt","hunks":[
            {"lineNumber":7,"context":"hello","oldLines":["hello","same","same"],"newLines":["hi","there","same","same"],"isEndOfFile":false},
            {"lineNumber":14,"oldLines":["last"],"newLines":["last","tail"],"isEndOfFile":true}]},
        {"kind":"delete","path":"/tmp/obsolete","lineNumber":18},
        {"kind":"add","path":"dir/a.txt","lineNumber":19,"content":""}])
    );
    assert_eq!(
        serde_json::from_str::<Vec<PatchOperation>>(&serde_json::to_string(&operations).unwrap())
            .unwrap(),
        operations
    );
}

#[test]
fn accepts_empty_patch_and_empty_creation() {
    assert_eq!(parsed(&patch(&[])), json!([]));
    assert_eq!(
        parsed(&patch(&["*** Add File: empty"])),
        json!([{"kind":"add","path":"empty","lineNumber":2,"content":""}])
    );
}

#[test]
fn unwraps_literal_heredocs_without_shell_evaluation() {
    for wrapper in ["<<EOF", "<<'EOF'", "<<\"EOF\""] {
        assert_eq!(
            parsed(&format!(
                "\n{wrapper}\n{}\nEOF\n",
                patch(&["*** Add File: $(touch sentinel)", "+`literal` $HOME"])
            )),
            json!([{"kind":"add","path":"$(touch sentinel)","lineNumber":4,"content":"`literal` $HOME\n"}])
        );
    }
}
#[test]
fn upstream_heredoc_terminator_suffix_rule() {
    assert_eq!(
        parsed(&format!("<<EOF\n{}\nanythingEOF", patch(&[]))),
        json!([])
    );
}
#[test]
fn normalizes_patch_line_endings() {
    for newline in ["\n", "\r\n", "\r\r\n"] {
        assert_eq!(
            parsed(&patch(&["*** Add File: a", "+a", "+b"]).replace('\n', newline)),
            json!([{"kind":"add","path":"a","lineNumber":2,"content":"a\nb\n"}])
        );
    }
}
#[test]
fn retains_content_whitespace_and_bare_empty_context() {
    assert_eq!(
        parsed(&patch(&[
            "*** Update File: a",
            "-a\t ",
            "+b  ",
            "",
            " \t",
            "@@ anchor  ",
            "+c"
        ])),
        json!([{"kind":"update","path":"a","lineNumber":2,"hunks":[
        {"lineNumber":3,"oldLines":["a\t ","","\t"],"newLines":["b  ","","\t"],"isEndOfFile":false},
        {"lineNumber":7,"context":"anchor","oldLines":[],"newLines":["c"],"isEndOfFile":false}]}])
    );
}
#[test]
fn accepts_implicit_first_hunk() {
    for line in ["", " ", "+new", "-old"] {
        let value = parsed(&patch(&["*** Update File: a", line]));
        assert_eq!(value[0]["kind"], "update");
        assert_eq!(value[0]["hunks"].as_array().unwrap().len(), 1);
        assert_eq!(value[0]["hunks"][0]["lineNumber"], 3);
    }
}
#[test]
fn keeps_indented_markers_as_context() {
    let context = [
        "*** Add File: b",
        "*** Delete File: c",
        "*** Update File: d",
        "*** End Patch",
        "*** Move to: e",
        "*** End of File",
        "@@",
        "*** Environment ID: e",
    ];
    let lines: Vec<_> = context.iter().map(|s| format!(" {s}")).collect();
    let mut body = vec!["*** Update File: a"];
    body.extend(lines.iter().map(String::as_str));
    let value = parsed(&patch(&body));
    assert_eq!(value[0]["kind"], "update");
    assert_eq!(value[0]["path"], "a");
    assert_eq!(value[0]["hunks"].as_array().unwrap().len(), 1);
    assert_eq!(value[0]["hunks"][0]["oldLines"], json!(context));
    assert_eq!(value[0]["hunks"][0]["newLines"], json!(context));
}
#[test]
fn trims_header_whitespace_preserves_path_and_unicode() {
    assert_eq!(
        parsed(
            "\u{85} \n *** Begin Patch \t\n *** Add File: ../a/./文件 \t\n+é é \t\n *** End Patch\u{85}"
        ),
        json!([{"kind":"add","path":"../a/./文件","lineNumber":3,"content":"é é \t\n"}])
    );
}
#[test]
fn trims_unicode_marker_spaces_retains_bom_and_content() {
    for space in ['\u{85}', '\u{a0}', '\u{2003}', '\u{3000}'] {
        let content = format!("{space}🙂\u{feff}{space}");
        let input = format!(
            "{space}*** Begin Patch{space}\n{space}*** Update File: a{space}\n*** Move to: b{space}\n@@ anchor\u{feff}{space}\n+{content}\n*** End Patch{space}"
        );
        assert_eq!(
            parsed(&input),
            json!([{"kind":"update","path":"a","lineNumber":2,"moveTo":"b","hunks":[{"lineNumber":4,"context":"anchor\u{feff}","oldLines":[],"newLines":[content],"isEndOfFile":false}]}])
        );
    }
}
#[test]
fn preserves_long_interior_whitespace() {
    let content = format!("{}x\t ", " ".repeat(32_000));
    assert_eq!(
        parsed(&patch(&[
            "*** Add File: a",
            &format!("+{content}"),
            "*** Update File: b",
            &format!("-{content}"),
            &format!("+{content}y")
        ])),
        json!([
        {"kind":"add","path":"a","lineNumber":2,"content":format!("{content}\n")},
        {"kind":"update","path":"b","lineNumber":4,"hunks":[{"lineNumber":5,"oldLines":[content],"newLines":[format!("{content}y")],"isEndOfFile":false}]}])
    );
}
#[test]
fn blank_lines_after_eof_then_new_hunk_or_file() {
    let value = parsed(&patch(&[
        "*** Update File: a",
        "@@",
        "-a",
        "+b",
        "*** End of File",
        "",
        " \t",
        "@@ next",
        "+c",
        "*** End of File",
        "*** Delete File: b",
    ]));
    assert_eq!(value.as_array().unwrap().len(), 2);
    assert_eq!(value[0]["kind"], "update");
    assert_eq!(value[0]["hunks"].as_array().unwrap().len(), 2);
    assert_eq!(value[0]["hunks"][0]["isEndOfFile"], true);
    assert_eq!(value[0]["hunks"][1]["context"], "next");
    assert_eq!(value[0]["hunks"][1]["isEndOfFile"], true);
    assert_eq!(
        value[1],
        json!({"kind":"delete","path":"b","lineNumber":12})
    );
}
#[test]
fn eof_before_first_hunk_has_no_effect() {
    let value = parsed(&patch(&["*** Update File: a", "*** End of File", "+a"]));
    assert_eq!(value[0]["kind"], "update");
    assert_eq!(value[0]["hunks"].as_array().unwrap().len(), 1);
    assert_eq!(value[0]["hunks"][0]["newLines"], json!(["a"]));
    assert_eq!(value[0]["hunks"][0]["isEndOfFile"], false);
}
#[test]
fn accepts_trimmed_and_redundant_final_marker() {
    assert_eq!(
        parse_patch("*** Begin Patch\n*** Update File: a\n+x\n \t*** End Patch \t\n")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(parsed(&patch(&["*** End Patch", " \t"])), json!([]));
}

fn rejected(input: &str, line: u64, reason: &str) {
    let caught = parse_patch(input).unwrap_err();
    assert_eq!(caught.line_number, line, "{input}");
    assert!(caught.reason.contains(reason), "{caught}");
    assert!(caught.to_string().contains(&format!("line {line}:")));
    assert_eq!(
        caught.to_string(),
        format!("Invalid patch at line {line}: {}", caught.reason)
    );
}
#[test]
fn rejects_malformed_or_unsupported_input_at_original_line() {
    for (input, line, reason) in [
        ("", 1, "Begin Patch"),
        (" \t\u{85}\n\u{3000}", 2, "Begin Patch"),
        ("not a patch", 1, "Begin Patch"),
        ("*** Begin Patch", 1, "End Patch"),
        ("*** Begin Patch\n*** Add File: a\n+x", 3, "End Patch"),
        ("\u{feff}*** Begin Patch\n*** End Patch", 1, "Begin Patch"),
        ("<<EOF\nbad\n*** End Patch\nEOF", 2, "Begin Patch"),
        ("<<EOF\n*** Begin Patch\nbad\nEOF", 3, "End Patch"),
        (
            "<<EOF\n*** Begin Patch\n*** End Patch\nSTOP",
            1,
            "Begin Patch",
        ),
        ("<<EOF\n*** End Patch\nEOF", 1, "Begin Patch"),
        (
            "apply_patch <<EOF\n*** Begin Patch\n*** End Patch\nEOF",
            1,
            "Begin Patch",
        ),
        (
            "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-a\n+b",
            1,
            "Begin Patch",
        ),
    ] {
        rejected(input, line, reason);
    }
    for (body, line, reason) in [
        (vec![""], 2, "header"),
        (vec!["*** Add File: "], 2, "header"),
        (vec!["*** Unknown: a"], 2, "header"),
        (vec!["*** Add File: a", "unprefixed"], 3, "header"),
        (vec!["*** Delete File: a", "+content"], 3, "header"),
        (vec!["*** Move to: b"], 2, "header"),
        (vec!["*** Update File: a"], 2, "requires at least one hunk"),
        (
            vec!["*** Update File: a", "*** Move to: b"],
            2,
            "requires at least one hunk",
        ),
        (
            vec!["*** Update File: a", "*** Move to: b", "*** Move to: c"],
            4,
            "context",
        ),
        (
            vec!["*** Update File: a", "@@", "*** Move to: b"],
            4,
            "context",
        ),
        (vec!["*** Update File: a", "unprefixed"], 3, "context"),
        (vec!["*** Update File: a", "@@"], 4, "must contain"),
        (
            vec!["*** Update File: a", "@@", "@@ next"],
            4,
            "must contain",
        ),
        (
            vec!["*** Update File: a", "@@", "*** End of File"],
            4,
            "must contain",
        ),
        (
            vec!["*** Update File: a", "@@", "*** Add File: b"],
            4,
            "must contain",
        ),
        (
            vec!["*** Update File: a", "*** Delete File: b"],
            2,
            "requires at least one hunk",
        ),
        (
            vec!["*** Update File: a", "+a", "*** End of File", "+b"],
            5,
            "Expected '@@'",
        ),
        (vec!["*** End Patch", "+later"], 3, "after"),
        (vec!["*** Environment ID: env-1"], 2, "unsupported"),
        (vec!["*** Environment ID:"], 2, "unsupported"),
        (
            vec!["*** Add File: a", "*** Environment ID: env-1"],
            3,
            "header",
        ),
        (
            vec!["*** Update File: a", "+a", "*** Environment ID: env-1"],
            4,
            "context",
        ),
    ] {
        rejected(&patch(&body), line, reason);
    }
}
#[test]
fn counts_leading_and_heredoc_lines_in_diagnostics() {
    assert_eq!(
        parse_patch(&format!(
            "\n \n<<'EOF'\n{}\nEOF",
            patch(&["*** Update File: a", "@@"])
        ))
        .unwrap_err()
        .line_number,
        7
    );
}
