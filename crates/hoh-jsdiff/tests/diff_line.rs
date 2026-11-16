use hoh_jsdiff::{Change, PatchOptions, diff_lines};

fn changes_to_xml(changes: &[Change]) -> String {
    let mut output = String::new();
    for change in changes {
        if change.added {
            output.push_str("<ins>");
        } else if change.removed {
            output.push_str("<del>");
        }
        output.push_str(
            &change
                .value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;"),
        );
        if change.added {
            output.push_str("</ins>");
        } else if change.removed {
            output.push_str("</del>");
        }
    }
    output
}

fn diff_xml(old: &str, new: &str) -> String {
    changes_to_xml(&diff_lines(old, new, &PatchOptions::default()).unwrap())
}

#[test]
fn should_diff_lines() {
    assert_eq!(
        diff_xml("line\nold value\nline", "line\nnew value\nline"),
        "line\n<del>old value\n</del><ins>new value\n</ins>line"
    );
}

#[test]
fn should_treat_identical_lines_as_equal() {
    assert_eq!(
        diff_xml("line\nvalue\nline", "line\nvalue\nline"),
        "line\nvalue\nline"
    );
}

#[test]
fn should_handle_leading_and_trailing_whitespace() {
    assert_eq!(
        diff_xml("line\nvalue \nline", "line\nvalue\nline"),
        "line\n<del>value \n</del><ins>value\n</ins>line"
    );
}

#[test]
fn should_handle_windows_line_endings() {
    assert_eq!(
        diff_xml("line\r\nold value \r\nline", "line\r\nnew value\r\nline"),
        "line\r\n<del>old value \r\n</del><ins>new value\r\n</ins>line"
    );
}

#[test]
fn should_handle_empty_lines() {
    assert_eq!(
        diff_xml("line\n\nold value \n\nline", "line\n\nnew value\n\nline"),
        "line\n\n<del>old value \n</del><ins>new value\n</ins>\nline"
    );
}

#[test]
fn should_handle_empty_input() {
    assert_eq!(
        diff_xml("line\n\nold value \n\nline", ""),
        "<del>line\n\nold value \n\nline</del>"
    );
}

#[test]
fn should_prefer_to_do_deletions_before_insertions_like_unix_diff_does() {
    assert_eq!(
        diff_xml("a\nb\nc\nd\n", "a\nc\nb\nd\n"),
        "a\n<del>b\n</del>c\n<ins>b\n</ins>d\n"
    );
    assert_eq!(
        diff_xml("a\nc\nb\nd\n", "a\nb\nc\nd\n"),
        "a\n<del>c\n</del>b\n<ins>c\n</ins>d\n"
    );
}

#[test]
fn given_max_edit_length_terminates_early() {
    let options = PatchOptions {
        max_edit_length: Some(1),
        ..PatchOptions::default()
    };
    assert!(diff_lines("line\nold value\nline", "line\nnew value\nline", &options).is_none());
}

#[test]
fn given_max_edit_length_zero_returns_normally_if_the_strings_are_identical() {
    let options = PatchOptions {
        max_edit_length: Some(0),
        ..PatchOptions::default()
    };
    assert_eq!(
        changes_to_xml(
            &diff_lines("foo\nbar\nbaz\nqux\n", "foo\nbar\nbaz\nqux\n", &options).unwrap()
        ),
        "foo\nbar\nbaz\nqux\n"
    );
}

#[test]
fn given_max_edit_length_zero_terminates_early_if_there_is_even_a_single_change() {
    let options = PatchOptions {
        max_edit_length: Some(0),
        ..PatchOptions::default()
    };
    assert!(diff_lines("foo\nbar\nbaz\nqux\n", "fox\nbar\nbaz\nqux\n", &options).is_none());
}
