use hoh_jsdiff::{
    Hunk, PatchOptions, StructuredPatch, create_patch, create_two_files_patch, format_patch,
    structured_patch,
};

fn with_context(context: usize) -> PatchOptions {
    PatchOptions {
        context,
        ..PatchOptions::default()
    }
}

const OLD_FILE: &str = "value\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\nremove value\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\nremove value\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\nvalue\ncontext\ncontext";
const NEW_FILE: &str = "new value\nnew value 2\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\ncontext\nadd value\ncontext\ncontext\ncontext\ncontext\nnew value\nnew value 2\ncontext\ncontext";

mod source_create_patch {
    use super::*;

    fn patch(old: &str, new: &str) -> String {
        create_patch(
            "test",
            old,
            new,
            Some("header1"),
            Some("header2"),
            &PatchOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn should_handle_files_with_the_last_line_changed() {
        assert_eq!(
            patch("line2\nline3\nline5\n", "line2\nline3\nline4\nline5\n"),
            "Index: test\n===================================================================\n--- test\theader1\n+++ test\theader2\n@@ -1,3 +1,4 @@\n line2\n line3\n+line4\n line5\n"
        );
        assert_eq!(
            patch("line2\nline3\nline4\n", "line2\nline3\nline4\nline5\n"),
            "Index: test\n===================================================================\n--- test\theader1\n+++ test\theader2\n@@ -1,3 +1,4 @@\n line2\n line3\n line4\n+line5\n"
        );
        assert_eq!(
            patch(
                "line1\nline2\nline3\nline4\n",
                "line1\nline2\nline3\nline44\n"
            ),
            "Index: test\n===================================================================\n--- test\theader1\n+++ test\theader2\n@@ -1,4 +1,4 @@\n line1\n line2\n line3\n-line4\n+line44\n"
        );
        assert_eq!(
            patch(
                "line1\nline2\nline3\nline4\n",
                "line1\nline2\nline3\nline44\nline5\n"
            ),
            "Index: test\n===================================================================\n--- test\theader1\n+++ test\theader2\n@@ -1,4 +1,5 @@\n line1\n line2\n line3\n-line4\n+line44\n+line5\n"
        );
    }

    #[test]
    fn should_output_no_newline_at_end_of_file_message_on_new_missing_nl() {
        assert_eq!(
            patch("line1\nline2\nline3\nline4\n", "line1\nline2\nline3\nline4"),
            "Index: test\n===================================================================\n--- test\theader1\n+++ test\theader2\n@@ -1,4 +1,4 @@\n line1\n line2\n line3\n-line4\n+line4\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn should_output_no_newline_at_end_of_file_message_on_old_missing_nl() {
        assert_eq!(
            patch("line1\nline2\nline3\nline4", "line1\nline2\nline3\nline4\n"),
            "Index: test\n===================================================================\n--- test\theader1\n+++ test\theader2\n@@ -1,4 +1,4 @@\n line1\n line2\n line3\n-line4\n\\ No newline at end of file\n+line4\n"
        );
    }

    #[test]
    fn should_output_no_newline_at_end_of_file_message_on_both_missing_nl() {
        assert_eq!(
            patch("line1\nline2\nline3\nline4", "line1\nline2\nline3\nline44"),
            "Index: test\n===================================================================\n--- test\theader1\n+++ test\theader2\n@@ -1,4 +1,4 @@\n line1\n line2\n line3\n-line4\n\\ No newline at end of file\n+line44\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn should_get_the_no_newline_position_right_in_issue_531() {
        assert_eq!(
            create_patch(
                "a.txt",
                "1st line.\n2nd line.\n3rd line.",
                "Z11 thing.\nA New thing.\n2nd line.\nNEW LINE.\n3rd line.\n\nSOMETHING ELSE.",
                None,
                None,
                &with_context(3),
            )
            .unwrap(),
            "Index: a.txt\n===================================================================\n--- a.txt\n+++ a.txt\n@@ -1,3 +1,7 @@\n-1st line.\n+Z11 thing.\n+A New thing.\n 2nd line.\n-3rd line.\n\\ No newline at end of file\n+NEW LINE.\n+3rd line.\n+\n+SOMETHING ELSE.\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn should_output_no_newline_at_end_of_file_message_on_context_missing_nl() {
        assert_eq!(
            patch("line11\nline2\nline3\nline4", "line1\nline2\nline3\nline4"),
            "Index: test\n===================================================================\n--- test\theader1\n+++ test\theader2\n@@ -1,4 +1,4 @@\n-line11\n+line1\n line2\n line3\n line4\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn should_output_no_no_newline_at_end_of_file_message_on_empty_file() {
        assert_eq!(
            patch("", "line1\nline2\nline3\nline4"),
            "Index: test\n===================================================================\n--- test\theader1\n+++ test\theader2\n@@ -0,0 +1,4 @@\n+line1\n+line2\n+line3\n+line4\n\\ No newline at end of file\n"
        );
        assert_eq!(
            patch("line1\nline2\nline3\nline4", ""),
            "Index: test\n===================================================================\n--- test\theader1\n+++ test\theader2\n@@ -1,4 +0,0 @@\n-line1\n-line2\n-line3\n-line4\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn should_not_output_no_newline_at_end_of_file_message_when_eof_outside_hunk() {
        assert_eq!(
            patch(
                "line11\nline2\nline3\nline4\nline4\nline4\nline4",
                "line1\nline2\nline3\nline4\nline4\nline4\nline4"
            ),
            "Index: test\n===================================================================\n--- test\theader1\n+++ test\theader2\n@@ -1,5 +1,5 @@\n-line11\n+line1\n line2\n line3\n line4\n line4\n"
        );
    }

    #[test]
    fn should_generate_a_patch_with_default_context_size() {
        assert_eq!(
            create_patch(
                "testFileName",
                OLD_FILE,
                NEW_FILE,
                Some("Old Header"),
                Some("New Header"),
                &PatchOptions::default(),
            )
            .unwrap(),
            "Index: testFileName\n===================================================================\n--- testFileName\tOld Header\n+++ testFileName\tNew Header\n@@ -1,5 +1,6 @@\n-value\n+new value\n+new value 2\n context\n context\n context\n context\n@@ -7,9 +8,8 @@\n context\n context\n context\n context\n-remove value\n context\n context\n context\n context\n@@ -17,20 +17,21 @@\n context\n context\n context\n context\n-remove value\n context\n context\n context\n context\n context\n context\n context\n context\n+add value\n context\n context\n context\n context\n-value\n+new value\n+new value 2\n context\n context\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn should_generate_a_patch_with_context_size_0() {
        assert_eq!(
            create_patch(
                "testFileName",
                OLD_FILE,
                NEW_FILE,
                Some("Old Header"),
                Some("New Header"),
                &with_context(0),
            )
            .unwrap(),
            "Index: testFileName\n===================================================================\n--- testFileName\tOld Header\n+++ testFileName\tNew Header\n@@ -1,1 +1,2 @@\n-value\n+new value\n+new value 2\n@@ -11,1 +11,0 @@\n-remove value\n@@ -21,1 +20,0 @@\n-remove value\n@@ -29,0 +29,1 @@\n+add value\n@@ -34,1 +34,2 @@\n-value\n+new value\n+new value 2\n"
        );
    }

    #[test]
    fn should_generate_a_patch_with_context_size_2() {
        assert_eq!(
            create_patch(
                "testFileName",
                OLD_FILE,
                NEW_FILE,
                Some("Old Header"),
                Some("New Header"),
                &with_context(2),
            )
            .unwrap(),
            "Index: testFileName\n===================================================================\n--- testFileName\tOld Header\n+++ testFileName\tNew Header\n@@ -1,3 +1,4 @@\n-value\n+new value\n+new value 2\n context\n context\n@@ -9,5 +10,4 @@\n context\n context\n-remove value\n context\n context\n@@ -19,5 +19,4 @@\n context\n context\n-remove value\n context\n context\n@@ -28,9 +27,11 @@\n context\n context\n+add value\n context\n context\n context\n context\n-value\n+new value\n+new value 2\n context\n context\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn should_output_headers_only_for_identical_files() {
        assert_eq!(
            create_patch(
                "testFileName",
                OLD_FILE,
                OLD_FILE,
                Some("Old Header"),
                Some("New Header"),
                &PatchOptions::default(),
            )
            .unwrap(),
            "Index: testFileName\n===================================================================\n--- testFileName\tOld Header\n+++ testFileName\tNew Header\n"
        );
    }

    #[test]
    fn should_omit_headers_if_undefined() {
        assert_eq!(
            create_patch(
                "testFileName",
                OLD_FILE,
                OLD_FILE,
                None,
                None,
                &PatchOptions::default(),
            )
            .unwrap(),
            "Index: testFileName\n===================================================================\n--- testFileName\n+++ testFileName\n"
        );
    }

    #[test]
    fn should_safely_handle_empty_inputs() {
        assert_eq!(
            create_patch("testFileName", "", "", None, None, &PatchOptions::default(),).unwrap(),
            "Index: testFileName\n===================================================================\n--- testFileName\n+++ testFileName\n"
        );
    }

    #[test]
    fn should_omit_index_with_multiple_file_names() {
        assert_eq!(
            create_two_files_patch("foo", "bar", "", "", None, None, &PatchOptions::default(),)
                .unwrap(),
            "===================================================================\n--- foo\n+++ bar\n"
        );
    }

    #[test]
    fn should_respect_max_edit_length() {
        let options = PatchOptions {
            max_edit_length: Some(1),
            ..PatchOptions::default()
        };
        assert!(
            create_patch(
                "test",
                "line1\nline2\nline3\n",
                "lineX\nlineY\nlineZ\nline42\n",
                Some("header1"),
                Some("header2"),
                &options,
            )
            .is_none()
        );
    }

    #[test]
    fn should_not_crash_when_encountering_an_enormous_hunk() {
        let new = "foo\n".repeat(1_000_000);
        let expected = format!(
            "===================================================================\n--- foo\n+++ bar\n@@ -0,0 +1,1000000 @@\n{}",
            "+foo\n".repeat(1_000_000)
        );
        assert_eq!(
            create_two_files_patch("foo", "bar", "", &new, None, None, &PatchOptions::default(),)
                .unwrap(),
            expected
        );
    }

    #[test]
    fn ignore_whitespace_false() {
        assert_eq!(
            create_patch(
                "testFileName",
                "line   \n line",
                "line\nline",
                None,
                None,
                &PatchOptions::default(),
            )
            .unwrap(),
            "Index: testFileName\n===================================================================\n--- testFileName\n+++ testFileName\n@@ -1,2 +1,2 @@\n-line   \n- line\n\\ No newline at end of file\n+line\n+line\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn strip_trailing_cr_false() {
        assert_eq!(
            create_two_files_patch(
                "foo",
                "bar",
                "line\nline",
                "line\r\nline",
                None,
                None,
                &PatchOptions::default(),
            )
            .unwrap(),
            "===================================================================\n--- foo\n+++ bar\n@@ -1,2 +1,2 @@\n-line\n+line\r\n line\n\\ No newline at end of file\n"
        );
    }
}

mod source_structured_patch {
    use super::*;

    #[test]
    fn should_handle_files_with_the_last_line_changed() {
        assert_eq!(
            structured_patch(
                "oldfile",
                "newfile",
                "line2\nline3\nline4\n",
                "line2\nline3\nline5",
                Some("header1"),
                Some("header2"),
                &PatchOptions::default(),
            )
            .unwrap(),
            StructuredPatch {
                old_file_name: "oldfile".to_owned(),
                new_file_name: "newfile".to_owned(),
                old_header: Some("header1".to_owned()),
                new_header: Some("header2".to_owned()),
                hunks: vec![Hunk {
                    old_start: 1,
                    old_lines: 3,
                    new_start: 1,
                    new_lines: 3,
                    lines: vec![
                        " line2".to_owned(),
                        " line3".to_owned(),
                        "-line4".to_owned(),
                        "+line5".to_owned(),
                        "\\ No newline at end of file".to_owned(),
                    ],
                }],
            }
        );
    }

    #[test]
    fn sets_old_header_and_new_header_to_undefined_if_they_are_not_provided() {
        let patch = structured_patch(
            "oldfile",
            "newfile",
            "line2\nline3\nline4\n",
            "line2\nline3\nline5",
            None,
            None,
            &PatchOptions::default(),
        )
        .unwrap();
        assert_eq!(patch.old_header, None);
        assert_eq!(patch.new_header, None);
        assert_eq!(patch.hunks[0].lines[4], "\\ No newline at end of file");
    }

    #[test]
    fn given_max_edit_length_terminates_early() {
        let options = PatchOptions {
            max_edit_length: Some(1),
            ..PatchOptions::default()
        };
        assert!(
            structured_patch(
                "oldfile",
                "newfile",
                "line2\nline3\nline4\n",
                "line2\nline3\nline5",
                Some("header1"),
                Some("header2"),
                &options,
            )
            .is_none()
        );
    }
}

mod source_format_patch {
    use super::*;

    #[test]
    fn should_generate_a_patch_string_from_an_input_diff() {
        let patch = structured_patch(
            "oldfile",
            "newfile",
            "line2\nline3\nline4\n",
            "line2\nline3\nline5",
            Some("header1"),
            Some("header2"),
            &PatchOptions::default(),
        )
        .unwrap();
        assert_eq!(
            format_patch(&patch),
            create_two_files_patch(
                "oldfile",
                "newfile",
                "line2\nline3\nline4\n",
                "line2\nline3\nline5",
                Some("header1"),
                Some("header2"),
                &PatchOptions::default(),
            )
            .unwrap()
        );
    }

    #[test]
    fn should_include_all_headers_with_include_headers() {
        let patch = StructuredPatch {
            old_file_name: "oldfile".to_owned(),
            old_header: Some("old-timestamp".to_owned()),
            new_file_name: "newfile".to_owned(),
            new_header: Some("new-timestamp".to_owned()),
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines: vec!["-old line".to_owned(), "+new line".to_owned()],
            }],
        };
        assert_eq!(
            format_patch(&patch),
            "===================================================================\n--- oldfile\told-timestamp\n+++ newfile\tnew-timestamp\n@@ -1,1 +1,1 @@\n-old line\n+new line\n"
        );
    }

    #[test]
    fn should_not_mutate_hunk_objects() {
        let patch = StructuredPatch {
            old_file_name: "a/file.txt".to_owned(),
            new_file_name: "b/file.txt".to_owned(),
            old_header: Some(String::new()),
            new_header: Some(String::new()),
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 0,
                new_start: 1,
                new_lines: 0,
                lines: Vec::new(),
            }],
        };
        let _ = format_patch(&patch);
        assert_eq!(patch.hunks[0].old_start, 1);
        assert_eq!(patch.hunks[0].new_start, 1);
    }
}
