use hoh_conformance::{compare, corpus, read, records};
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
fn historical_inputs_use_available_migration_then_name_the_missing_edge() {
    for s in corpus::discover(&refs()).unwrap() {
        for file in s.fixtures().unwrap() {
            let raw = read(&file).unwrap();
            let version = records(&raw).unwrap()[0]["version"].as_u64().unwrap_or(0);
            if version < 4 {
                let f = compare::migration_gate(&raw).unwrap_err();
                assert_eq!(f.phase, "input");
                let next = version.max(1);
                assert_eq!(
                    f.reason,
                    format!("migration v{next}->v{} not ported", next + 1),
                    "{}/{} {}",
                    s.suite,
                    s.name,
                    file.display()
                );
            }
        }
    }
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
