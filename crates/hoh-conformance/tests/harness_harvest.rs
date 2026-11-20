//! `harness.spec.ts` 1352–1406: harvesting the persisted logs of a run.
use hoh_conformance::runner::harvest;
use serde_json::{Value, json};
use std::{fs, path::Path};

fn write(root: &Path, file: &str, lines: &[Value]) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(path, text).unwrap();
}

const SID: &str = "11111111-0000-4000-8000-000000000000";

/// :1352
#[test]
fn harvests_all_logs_primary_first_children_by_created_at_then_id_skipping_filesystem_noise() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("stray.txt"), "noise").unwrap();
    fs::create_dir_all(root.join("b1")).unwrap();
    fs::write(root.join("b1/stray.txt"), "noise").unwrap();
    let child = |id: &str| json!({"type":"session","version":0,"id":id,"createdAt":500,"parentSession":SID});
    write(
        root,
        "b1/aa-child-c/session.jsonl",
        &[child("cccccccc-0000-4000-8000-000000000000")],
    );
    write(
        root,
        "b1/bb-parent/session.jsonl",
        &[json!({"type":"session","version":0,"id":"superseded-parent","createdAt":1})],
    );
    write(
        root,
        "b1/bb-parent/session.v1.jsonl",
        &[json!({"type":"session","version":1,"id":SID,"createdAt":900})],
    );
    write(
        root,
        "b1/cc-child-a/session.jsonl",
        &[child("aaaaaaaa-0000-4000-8000-000000000000")],
    );
    // Missing id/createdAt fall back to ''/0, so the orphan child is the earliest.
    write(
        root,
        "b2/orphan/session.jsonl",
        &[json!({"type":"session","version":0,"parentSession":SID})],
    );

    let logs = harvest(root, "acp").unwrap();
    let ids: Vec<(String, u64)> = logs
        .iter()
        .map(|log| {
            let header: Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
            (
                header["id"].as_str().unwrap_or_default().to_owned(),
                header["createdAt"].as_u64().unwrap_or(0),
            )
        })
        .collect();
    assert_eq!(
        ids,
        vec![
            (SID.to_owned(), 900),
            (String::new(), 0),
            ("aaaaaaaa-0000-4000-8000-000000000000".to_owned(), 500),
            ("cccccccc-0000-4000-8000-000000000000".to_owned(), 500),
        ]
    );
    assert!(logs[1].contains(SID));
}

/// :1381
#[test]
fn rejects_an_empty_persisted_generation() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "b/empty/session.jsonl", &[]);
    let error = harvest(dir.path(), "acp").unwrap_err();
    assert!(
        error.contains("session.jsonl: session fixture is empty"),
        "{error}"
    );
}

/// :1389
#[test]
fn rejects_a_persisted_filename_header_generation_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "b/mismatch/session.v1.jsonl",
        &[json!({"type":"session","version":0})],
    );
    let error = harvest(dir.path(), "acp").unwrap_err();
    assert!(
        error.contains("filename declares Session format v1, header declares v0"),
        "{error}"
    );
}

/// :1399
#[test]
fn yields_no_logs_when_the_sessions_root_vanished() {
    let dir = tempfile::tempdir().unwrap();
    assert!(harvest(&dir.path().join("gone"), "acp").unwrap().is_empty());
}
