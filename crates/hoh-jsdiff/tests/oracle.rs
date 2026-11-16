//! Compares the port with what jsdiff 9.0.0 itself returned for generated pairs of texts.
//! The fixture is written by `fixtures/gen-jsdiff-oracle.mjs` from the library's source.

use hoh_jsdiff::{
    Change, Hunk, PatchOptions, create_two_files_patch, diff_lines, structured_patch,
};
use serde_json::Value;

fn text<'a>(record: &'a Value, key: &str) -> &'a str {
    record[key].as_str().unwrap()
}

fn options(record: &Value) -> PatchOptions {
    PatchOptions {
        context: usize::try_from(record["context"].as_u64().unwrap()).unwrap(),
        timeout_ms: None,
        max_edit_length: record["max"]
            .as_u64()
            .map(|limit| usize::try_from(limit).unwrap()),
    }
}

fn expected_hunks(value: &Value) -> Option<Vec<Hunk>> {
    let number = |value: &Value| usize::try_from(value.as_u64().unwrap()).unwrap();
    value.as_array().map(|hunks| {
        hunks
            .iter()
            .map(|hunk| Hunk {
                old_start: number(&hunk[0]),
                old_lines: number(&hunk[1]),
                new_start: number(&hunk[2]),
                new_lines: number(&hunk[3]),
                lines: hunk[4]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|line| line.as_str().unwrap().to_owned())
                    .collect(),
            })
            .collect()
    })
}

fn expected_changes(value: &Value) -> Option<Vec<Change>> {
    value.as_array().map(|changes| {
        changes
            .iter()
            .map(|change| Change {
                value: change[0].as_str().unwrap().to_owned(),
                added: change[1].as_bool().unwrap(),
                removed: change[2].as_bool().unwrap(),
                count: usize::try_from(change[3].as_u64().unwrap()).unwrap(),
            })
            .collect()
    })
}

#[test]
fn port_matches_jsdiff_on_generated_pairs() {
    let mut patches = 0;
    let mut limited = 0;
    let mut formatted = 0;
    for (index, line) in include_str!("fixtures/jsdiff_oracle.jsonl")
        .lines()
        .enumerate()
    {
        let record: Value = serde_json::from_str(line).unwrap();
        let (old, new) = (text(&record, "old"), text(&record, "new"));
        let options = options(&record);
        let place = format!(
            "record {}: old {old:?}, new {new:?}, {options:?}",
            index + 1
        );

        if record.get("text").is_some() {
            let header = |key: &str| record[key].as_str();
            let actual = create_two_files_patch(
                text(&record, "oldName"),
                text(&record, "newName"),
                old,
                new,
                header("oldHeader"),
                header("newHeader"),
                &options,
            );
            assert_eq!(actual.as_deref(), record["text"].as_str(), "{place}");
            formatted += 1;
            continue;
        }

        let actual = structured_patch("", "", old, new, None, None, &options);
        assert_eq!(
            actual.map(|patch| patch.hunks),
            expected_hunks(&record["hunks"]),
            "hunks, {place}"
        );
        let diff_options = PatchOptions {
            context: PatchOptions::default().context,
            ..options
        };
        assert_eq!(
            diff_lines(old, new, &diff_options),
            expected_changes(&record["changes"]),
            "changes, {place}"
        );
        patches += 1;
        if record["hunks"].is_null() {
            limited += 1;
        }
    }
    assert_eq!((patches, formatted), (1500, 60));
    assert!(limited > 20, "only {limited} records hit the edit limit");
}
