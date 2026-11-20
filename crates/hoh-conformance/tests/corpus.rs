use hoh_conformance::{compare, corpus, read};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn refs() -> PathBuf {
    std::env::var_os("HOH_CONFORMANCE_REFS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness"))
}

#[test]
fn every_historical_and_current_fixture_reads_strictly_under_the_catalog() {
    let mut failures = Vec::new();
    let mut count = 0;
    for s in corpus::discover(&refs()).unwrap() {
        for file in s.fixtures().unwrap() {
            count += 1;
            if let Err(f) = compare::strict_catalog(&read(&file).unwrap()) {
                failures.push(format!(
                    "{}/{} {}: {}",
                    s.suite,
                    s.name,
                    file.display(),
                    f.reason
                ));
            }
        }
    }
    assert!(count > 242, "every role of every scenario is read: {count}");
    // The corpus must be readable except for fixtures whose rejection is itself pinned below.
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn manifest_expected_inventory_and_patches_entire_corpus() {
    let all = corpus::discover(&refs()).unwrap();
    assert_eq!(all.len(), 242);
    let expected =
        read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/EXPECTED.tsv"))
            .unwrap();
    let expected: Vec<_> = expected
        .lines()
        .skip(1)
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .collect();
    assert_eq!(expected.len(), 242);
    for s in &all {
        let row = expected
            .iter()
            .find(|r| r[0] == s.suite && r[1] == s.name)
            .unwrap();
        assert_eq!(row[2], s.profile());
        assert_eq!(row[3], s.composition());
        assert!(!row[6].is_empty());
        for source in corpus::patch_sources(&all, s, &refs()).unwrap() {
            hoh_conformance::yaml::materialize(&read(&source).unwrap(), &source, &refs()).unwrap();
        }
        assert_eq!(
            s.fixtures().unwrap().len(),
            s.expected_logs().unwrap().len()
        );
    }
}

#[cfg(unix)]
#[test]
fn bin_false_all_three_suites_are_failures_without_runner_errors() {
    // `--runs` deliberately goes through a symlink (as a target dir linked into tmpfs does): the
    // oracle's paths must not depend on how the runs folder is spelled.
    let base = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let base_path = std::fs::canonicalize(base.path()).unwrap();
    std::fs::create_dir(base_path.join("real")).unwrap();
    // The link name and its target differ in length, as macOS `/var` -> `/private/var` does: the
    // fixed-length cwd text is wrong unless the runs folder is canonicalised before it is built.
    std::os::unix::fs::symlink(base_path.join("real"), base_path.join("l")).unwrap();
    let root = tempfile::tempdir_in(base_path.join("l")).unwrap();
    assert_ne!(
        std::fs::canonicalize(root.path())
            .unwrap()
            .as_os_str()
            .len(),
        root.path().as_os_str().len(),
        "the runs folder must be spelled with a path of another length than its canonical form"
    );
    let results = root.path().join("false.tsv");
    let output = Command::new(env!("CARGO_BIN_EXE_hoh-conformance"))
        .arg("--hoh")
        .arg("/bin/false")
        .arg("--refs")
        .arg(refs())
        .arg("--runs")
        .arg(root.path())
        .arg("--results")
        .arg(&results)
        .args(["--jobs", "2"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "scenarios: 0 of 242 pass, 242 fail, 0 runner errors"
    );
    let rows = read(&results).unwrap();
    assert_eq!(rows.lines().count(), 242);
    for row in rows.lines() {
        let fields: Vec<_> = row.split('\t').collect();
        assert_eq!(fields[2], "fail");
        assert!(["launch", "exit"].contains(&fields[3]), "{row}");
    }
}

mod strict_catalog {
    use super::refs;
    use hoh_conformance::{compare, corpus, read};
    use serde_json::{Value, json};

    fn recorded(name: &str) -> Vec<Value> {
        let all = corpus::discover(&refs()).unwrap();
        let s = all
            .iter()
            .find(|s| s.suite == "session" && s.name == name)
            .unwrap();
        read(&s.fixtures().unwrap()[0])
            .unwrap()
            .lines()
            .map(|l| hoh_util_values::js_json_parse(l).unwrap())
            .collect()
    }
    fn text(rows: &[Value]) -> String {
        rows.iter()
            .map(|r| format!("{}\n", hoh_util_values::js_json_stringify(r)))
            .collect()
    }
    fn probe(rows: &[Value]) -> String {
        compare::strict_catalog(&text(rows)).unwrap_err().reason
    }

    #[test]
    fn a_recorded_current_journal_passes() {
        let rows = recorded("bash-tool-turn");
        assert_eq!(rows[0]["version"], 4);
        compare::strict_catalog(&text(&rows)).unwrap();
    }

    #[test]
    fn a_recorded_historical_journal_is_migrated_and_passes() {
        let rows = recorded("text-turn");
        assert!(rows[0]["version"].as_u64().unwrap() < 4);
        compare::strict_catalog(&text(&rows)).unwrap();
    }

    #[test]
    fn an_unknown_event_type_is_rejected_with_its_line() {
        let mut rows = recorded("bash-tool-turn");
        rows[3]["type"] = json!("no/such-event");
        let reason = probe(&rows);
        eprintln!("unknown event: {reason}");
        assert!(reason.starts_with("session snapshot line "), "{reason}");
        assert!(reason.contains("no/such-event"), "{reason}");
    }

    #[test]
    fn a_broken_relationship_is_rejected() {
        let mut rows = recorded("bash-tool-turn");
        let result = rows
            .iter_mut()
            .find(|r| r["type"] == "tool/result")
            .unwrap();
        result["data"]["message"]["toolCallId"] = json!("call-that-was-never-made");
        result["data"]["message"]["source"]["callId"] = json!("call-that-was-never-made");
        let reason = probe(&rows);
        eprintln!("broken relationship: {reason}");
        assert!(reason.starts_with("session snapshot line "), "{reason}");
    }

    #[test]
    fn a_newer_format_version_is_rejected_by_name() {
        let mut rows = recorded("bash-tool-turn");
        rows[0]["version"] = json!(9);
        let reason = probe(&rows);
        eprintln!("newer version: {reason}");
        assert!(reason.contains("v9"), "{reason}");
        assert!(reason.contains("v4"), "{reason}");
    }

    #[test]
    fn mixed_projected_and_complete_rows_and_bad_json_are_rejected() {
        let mut rows = recorded("bash-tool-turn");
        rows[1]["seq"] = json!(0);
        rows[1]["time"] = json!(0);
        let reason = probe(&rows);
        assert!(
            reason.contains("cannot mix projected and complete body rows"),
            "{reason}"
        );
        let header = hoh_util_values::js_json_stringify(&recorded("bash-tool-turn")[0]);
        let reason = compare::strict_catalog(&format!("{header}\nnot json\n"))
            .unwrap_err()
            .reason;
        assert_eq!(reason, "session snapshot line 2 contains invalid JSON");
        assert_eq!(
            compare::strict_catalog("").unwrap_err().reason,
            "session snapshot must start with a session header"
        );
    }
}
