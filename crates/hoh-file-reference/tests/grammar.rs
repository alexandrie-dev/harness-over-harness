use hoh_file_reference::{
    ActiveAtToken, FileReferenceCandidate, FileReferenceKind, active_at_token, format_file_mention,
};
use serde_json::json;

fn candidate(path: &str, kind: FileReferenceKind) -> FileReferenceCandidate {
    FileReferenceCandidate {
        path: path.to_owned(),
        kind,
    }
}

#[test]
fn recognizes_plain_quoted_and_non_token_input() {
    assert_eq!(
        active_at_token("@src/tu", 7),
        Some(ActiveAtToken {
            prefix: "@src/tu".into(),
            query: "src/tu".into(),
            quoted: false,
        })
    );
    assert_eq!(
        active_at_token("read @\"docs/design n", 20),
        Some(ActiveAtToken {
            prefix: "@\"docs/design n".into(),
            query: "docs/design n".into(),
            quoted: true,
        })
    );
    assert_eq!(active_at_token("mail a@b.test", 13), None);
    assert_eq!(active_at_token("done @src/x\" next", 17), None);
}

#[test]
fn uses_javascript_whitespace_and_utf16_cursor_columns() {
    assert_eq!(
        active_at_token("😀\u{feff}@x", 5),
        Some(ActiveAtToken {
            prefix: "@x".into(),
            query: "x".into(),
            quoted: false,
        })
    );
    assert_eq!(active_at_token("left\u{0085}@x", 7), None);
    assert_eq!(active_at_token("@x", usize::MAX).unwrap().query, "x");
}

#[test]
fn a_closed_quote_falls_back_to_the_plain_token_grammar() {
    assert_eq!(
        active_at_token("@\"name\"", 7),
        Some(ActiveAtToken {
            prefix: "@\"name\"".into(),
            query: "\"name\"".into(),
            quoted: false,
        })
    );
}

#[test]
fn formats_files_directories_and_preserved_quotes() {
    assert_eq!(
        format_file_mention(&candidate("src/index.ts", FileReferenceKind::File), false),
        Some("@src/index.ts".into())
    );
    assert_eq!(
        format_file_mention(&candidate("src", FileReferenceKind::Directory), false),
        Some("@src/".into())
    );
    assert_eq!(
        format_file_mention(
            &candidate("docs/design notes.md", FileReferenceKind::File),
            false
        ),
        Some("@\"docs/design notes.md\"".into())
    );
    assert_eq!(
        format_file_mention(
            &candidate("docs/design notes", FileReferenceKind::Directory),
            false
        ),
        Some("@\"docs/design notes/".into())
    );
    assert_eq!(
        format_file_mention(&candidate("README.md", FileReferenceKind::File), true),
        Some("@\"README.md\"".into())
    );
}

#[test]
fn rejects_unrepresentable_paths_and_quotes_javascript_whitespace() {
    for path in [
        "bad\nname",
        "bad \"name\".md",
        "bad\"name.md",
        "bad\u{0085}name",
    ] {
        assert_eq!(
            format_file_mention(&candidate(path, FileReferenceKind::File), false),
            None
        );
    }
    assert_eq!(
        format_file_mention(&candidate("a\u{feff}b", FileReferenceKind::File), false),
        Some("@\"a\u{feff}b\"".into())
    );
}

#[test]
fn serializes_candidate_with_source_field_order_and_literals() {
    let value = serde_json::to_value(candidate("src", FileReferenceKind::Directory)).unwrap();
    assert_eq!(value, json!({ "path": "src", "kind": "directory" }));
    assert_eq!(
        value.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["path", "kind"]
    );
}
