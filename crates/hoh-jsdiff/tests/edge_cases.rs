use std::fmt::Write as _;

use hoh_jsdiff::{PatchOptions, create_patch, diff_lines, structured_patch};

fn patch(old: &str, new: &str, context: usize) -> String {
    create_patch(
        "file",
        old,
        new,
        None,
        None,
        &PatchOptions {
            context,
            ..PatchOptions::default()
        },
    )
    .unwrap()
}

#[test]
fn both_texts_empty() {
    let options = PatchOptions::default();
    assert_eq!(diff_lines("", "", &options).unwrap(), Vec::new());
    assert!(
        structured_patch("", "", "", "", None, None, &options)
            .unwrap()
            .hunks
            .is_empty()
    );
}

#[test]
fn identical_texts_have_no_hunks() {
    let patch = structured_patch(
        "",
        "",
        "same\ntext\n",
        "same\ntext\n",
        None,
        None,
        &PatchOptions::default(),
    )
    .unwrap();
    assert!(patch.hunks.is_empty());
}

#[test]
fn one_side_empty() {
    assert_eq!(
        patch("", "first\nsecond\n", 4),
        "Index: file\n===================================================================\n--- file\n+++ file\n@@ -0,0 +1,2 @@\n+first\n+second\n"
    );
    assert_eq!(
        patch("first\nsecond\n", "", 4),
        "Index: file\n===================================================================\n--- file\n+++ file\n@@ -1,2 +0,0 @@\n-first\n-second\n"
    );
}

#[test]
fn only_final_newline_differs_in_both_directions() {
    assert_eq!(
        patch("last", "last\n", 4),
        "Index: file\n===================================================================\n--- file\n+++ file\n@@ -1,1 +1,1 @@\n-last\n\\ No newline at end of file\n+last\n"
    );
    assert_eq!(
        patch("last\n", "last", 4),
        "Index: file\n===================================================================\n--- file\n+++ file\n@@ -1,1 +1,1 @@\n-last\n+last\n\\ No newline at end of file\n"
    );
}

#[test]
fn crlf_texts_preserve_carriage_returns() {
    assert_eq!(
        patch("a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\n", 4),
        "Index: file\n===================================================================\n--- file\n+++ file\n@@ -1,3 +1,3 @@\n a\r\n-b\r\n+B\r\n c\r\n"
    );
}

#[test]
fn changes_at_first_and_last_lines() {
    let at_first = structured_patch(
        "",
        "",
        "old\nmiddle\nlast\n",
        "new\nmiddle\nlast\n",
        None,
        None,
        &PatchOptions::default(),
    )
    .unwrap();
    assert_eq!(at_first.hunks[0].lines[0], "-old");
    assert_eq!(at_first.hunks[0].lines[1], "+new");

    let at_last = structured_patch(
        "",
        "",
        "first\nmiddle\nold\n",
        "first\nmiddle\nnew\n",
        None,
        None,
        &PatchOptions::default(),
    )
    .unwrap();
    let lines = &at_last.hunks[0].lines;
    assert_eq!(lines[lines.len() - 2], "-old");
    assert_eq!(lines[lines.len() - 1], "+new");
}

fn changed_lines(second_change: usize) -> (String, String) {
    let mut old = String::new();
    let mut new = String::new();
    for line in 1..=30 {
        writeln!(old, "line{line}").unwrap();
        if line == 5 || line == second_change {
            writeln!(new, "changed{line}").unwrap();
        } else {
            writeln!(new, "line{line}").unwrap();
        }
    }
    (old, new)
}

#[test]
fn changes_closer_than_twice_context_share_one_hunk() {
    let (old, new) = changed_lines(12);
    let patch = structured_patch("", "", &old, &new, None, None, &PatchOptions::default()).unwrap();
    assert_eq!(patch.hunks.len(), 1);
}

#[test]
fn changes_farther_apart_than_twice_context_have_two_hunks() {
    let (old, new) = changed_lines(20);
    let patch = structured_patch("", "", &old, &new, None, None, &PatchOptions::default()).unwrap();
    assert_eq!(patch.hunks.len(), 2);
}

#[test]
fn context_zero_omits_unchanged_lines() {
    assert_eq!(
        patch("one\ntwo\nthree\n", "one\nTWO\nthree\n", 0),
        "Index: file\n===================================================================\n--- file\n+++ file\n@@ -2,1 +2,1 @@\n-two\n+TWO\n"
    );
}

#[test]
fn max_edit_length_hit_returns_none() {
    let options = PatchOptions {
        max_edit_length: Some(0),
        ..PatchOptions::default()
    };
    assert_eq!(diff_lines("a\n", "b\n", &options), None);
    assert_eq!(
        structured_patch("", "", "a\n", "b\n", None, None, &options),
        None
    );
}

#[test]
fn four_thousand_lines_with_scattered_changes_finish_quickly() {
    let mut old = String::new();
    let mut new = String::new();
    for line in 0..4_000 {
        writeln!(old, "line {line}").unwrap();
        if line % 157 == 0 {
            writeln!(new, "changed {line}").unwrap();
        } else {
            writeln!(new, "line {line}").unwrap();
        }
    }

    let changes = diff_lines(&old, &new, &PatchOptions::default()).unwrap();
    let reconstructed_old: String = changes
        .iter()
        .filter(|change| !change.added)
        .map(|change| change.value.as_str())
        .collect();
    let reconstructed_new: String = changes
        .iter()
        .filter(|change| !change.removed)
        .map(|change| change.value.as_str())
        .collect();
    assert_eq!(reconstructed_old, old);
    assert_eq!(reconstructed_new, new);
    assert_eq!(changes.iter().filter(|change| change.removed).count(), 26);
}

#[test]
fn non_ascii_text() {
    let changes = diff_lines(
        "Привет\nмир 🦀\n",
        "Привет\nRust 🦀\n",
        &PatchOptions::default(),
    )
    .unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0].value, "Привет\n");
    assert_eq!(changes[1].value, "мир 🦀\n");
    assert_eq!(changes[2].value, "Rust 🦀\n");
    assert_eq!(
        patch("Привет\nмир 🦀\n", "Привет\nRust 🦀\n", 4),
        "Index: file\n===================================================================\n--- file\n+++ file\n@@ -1,2 +1,2 @@\n Привет\n-мир 🦀\n+Rust 🦀\n"
    );
}
