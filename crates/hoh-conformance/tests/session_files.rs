//! `suite.spec.ts`: `writerSnapshotName`, `sessionFixtureNames` and the Session generation filename helpers.
use hoh_conformance::session_files::*;

const MAX_SAFE_PLUS_ONE: f64 = 9_007_199_254_740_992.0;

fn names(list: &[&str]) -> Result<Vec<String>, String> {
    session_fixture_names(list)
}

/// :493
#[test]
fn keeps_native_writer_expectations_outside_historical_replay_selection() {
    assert_eq!(writer_snapshot_name(0.0).unwrap(), "writer.expected.jsonl");
    assert_eq!(
        writer_snapshot_name(2.0).unwrap(),
        "writer.2.expected.jsonl"
    );
    assert_eq!(
        names(&[
            "session.v1.jsonl",
            "writer.expected.jsonl",
            "writer.1.expected.jsonl"
        ])
        .unwrap(),
        ["session.v1.jsonl"]
    );
}

/// :500 (four invalid roles)
#[test]
fn rejects_invalid_writer_snapshot_roles() {
    for index in [-1.0, -0.0, 0.5, MAX_SAFE_PLUS_ONE] {
        assert_eq!(
            writer_snapshot_name(index).unwrap_err(),
            "writer snapshot index must be a non-negative safe integer"
        );
    }
}

/// :506
#[test]
fn selects_the_highest_generation_for_each_contiguous_role_while_ignoring_other_files() {
    assert_eq!(
        names(&[
            "stdout.expected.jsonl",
            "session.2.jsonl",
            "session.jsonl",
            "session.1.jsonl",
            "session.v2.jsonl",
            "session.v1.jsonl",
            "session.1.v3.jsonl",
            "input.json",
        ])
        .unwrap(),
        ["session.v2.jsonl", "session.1.v3.jsonl", "session.2.jsonl"]
    );
}

/// :519, :523, :527
#[test]
fn accepts_a_primary_only_scenario_and_rejects_a_missing_primary_or_a_gap() {
    assert_eq!(names(&["session.jsonl"]).unwrap(), ["session.jsonl"]);
    assert_eq!(
        names(&["session.1.jsonl"]).unwrap_err(),
        "missing parent session fixture"
    );
    assert!(
        names(&["session.jsonl", "session.2.jsonl"])
            .unwrap_err()
            .contains("expected index 1, found session.2.jsonl")
    );
}

/// :532 (seven invalid names)
#[test]
fn rejects_invalid_fixture_names() {
    for name in [
        "session.0.jsonl",
        "session.child.jsonl",
        "session.01.jsonl",
        "session.v0.jsonl",
        "session.V1.jsonl",
        "session.v01.jsonl",
        "session.1.v0.jsonl",
    ] {
        assert_eq!(
            names(&["session.jsonl", name]).unwrap_err(),
            format!("invalid session fixture name: {name}")
        );
    }
}

/// :548
#[test]
fn rejects_duplicate_role_generations() {
    assert_eq!(
        names(&["session.jsonl", "session.1.jsonl", "session.1.jsonl"]).unwrap_err(),
        "duplicate session fixture generation: session.1.jsonl"
    );
}

/// :555
#[test]
fn renders_canonical_parent_child_raw_and_compressed_names() {
    assert_eq!(session_fixture_name(0.0, 0.0).unwrap(), "session.jsonl");
    assert_eq!(session_fixture_name(0.0, 2.0).unwrap(), "session.v2.jsonl");
    assert_eq!(session_fixture_name(3.0, 0.0).unwrap(), "session.3.jsonl");
    assert_eq!(
        session_fixture_name(3.0, 2.0).unwrap(),
        "session.3.v2.jsonl"
    );
    assert_eq!(
        persisted_session_filename(0.0, false).unwrap(),
        "session.jsonl"
    );
    assert_eq!(
        persisted_session_filename(2.0, true).unwrap(),
        "session.v2.jsonl.zstd"
    );
}

/// :564 (five invalid components)
#[test]
fn rejects_invalid_numeric_filename_components() {
    let renders: [Result<String, String>; 5] = [
        session_fixture_name(-1.0, 0.0),
        session_fixture_name(-0.0, 0.0),
        session_fixture_name(0.5, 0.0),
        session_fixture_name(0.0, -1.0),
        persisted_session_filename(MAX_SAFE_PLUS_ONE, false),
    ];
    for render in renders {
        assert!(render.unwrap_err().contains("non-negative safe integer"));
    }
}

/// :574
#[test]
fn parses_canonical_fixture_and_persistence_names_without_admitting_lookalikes() {
    assert_eq!(
        parse_session_fixture_name("session.2.v3.jsonl").unwrap(),
        Some(SessionFixtureFile {
            index: 2,
            version: 3,
            name: "session.2.v3.jsonl".into()
        })
    );
    assert_eq!(parse_session_fixture_name("notes.jsonl").unwrap(), None);
    let huge = format!("session.{}.jsonl", "9".repeat(400));
    assert!(
        parse_session_fixture_name(&huge)
            .unwrap_err()
            .contains("invalid session fixture name")
    );
    assert_eq!(
        parse_persisted_session_filename("session.jsonl"),
        Some(PersistedSessionFile {
            version: 0,
            zstd: false,
            name: "session.jsonl".into()
        })
    );
    assert_eq!(
        parse_persisted_session_filename("session.v4.jsonl.zstd"),
        Some(PersistedSessionFile {
            version: 4,
            zstd: true,
            name: "session.v4.jsonl.zstd".into()
        })
    );
    assert_eq!(parse_persisted_session_filename("session.1.jsonl"), None);
    assert_eq!(
        parse_persisted_session_filename(&format!("session.v{}.jsonl", "9".repeat(400))),
        None
    );
}

/// :585
#[test]
fn selects_one_highest_persisted_generation_per_directory_and_compression() {
    let paths = [
        "b/session.v1.jsonl",
        "b/session.jsonl",
        "a/session.jsonl",
        "a/session.v2.jsonl",
        "a/session.v3.jsonl.zstd",
        "a/session.v2.20260101.backup.jsonl",
        "noise.jsonl",
    ];
    assert_eq!(
        latest_persisted_session_paths(&paths, false),
        ["a/session.v2.jsonl", "b/session.v1.jsonl"]
    );
    assert_eq!(
        latest_persisted_session_paths(&paths, true),
        ["a/session.v3.jsonl.zstd"]
    );
}

/// :599
#[test]
fn validates_filename_and_header_generation_equality() {
    let v0 = "{\"type\":\"session\",\"version\":0}\n";
    let v1 = "{\"type\":\"session\",\"version\":1}\n";
    assert_eq!(session_header_version(v1, "fixture").unwrap(), 1);
    assert_eq!(
        assert_session_fixture_version("session.jsonl", v0).unwrap(),
        0
    );
    assert_eq!(
        assert_persisted_session_version("session.v1.jsonl", v1).unwrap(),
        1
    );
    let error = |r: Result<u64, String>| r.unwrap_err();
    assert!(
        error(assert_session_fixture_version("notes.jsonl", v0))
            .contains("not a session fixture name")
    );
    assert_eq!(
        assert_session_fixture_version("session.jsonl", "{\"type\":\"session\"}\n").unwrap(),
        0
    );
    assert!(
        error(assert_session_fixture_version(
            "session.v1.jsonl",
            "{\"type\":\"session\"}\n"
        ))
        .contains("a versionless projected Session header is format v0")
    );
    assert!(
        error(assert_session_fixture_version("session.jsonl", ""))
            .contains("session fixture is empty")
    );
    assert!(
        error(assert_session_fixture_version("session.jsonl", "{"))
            .contains("session header contains invalid JSON")
    );
    assert!(
        error(assert_persisted_session_version("session.1.jsonl", v0))
            .contains("not a canonical Session persistence filename")
    );
    assert!(
        error(assert_session_fixture_version("session.v1.jsonl", v0))
            .contains("filename declares Session format v1, header declares v0")
    );
    assert!(
        error(assert_persisted_session_version("session.jsonl", v1))
            .contains("filename declares Session format v0, header declares v1")
    );
}

/// :618 (eight malformed headers)
#[test]
fn rejects_malformed_headers() {
    for (content, message) in [
        ("", "session fixture is empty"),
        ("{", "session header contains invalid JSON"),
        ("[]", "first record must be a Session header"),
        (
            "{\"type\":\"event\",\"version\":0}",
            "first record must be a Session header",
        ),
        (
            "{\"type\":\"session\"}",
            "version must be a non-negative safe integer",
        ),
        (
            "{\"type\":\"session\",\"version\":-1}",
            "version must be a non-negative safe integer",
        ),
        (
            "{\"type\":\"session\",\"version\":-0}",
            "version must be a non-negative safe integer",
        ),
        (
            "{\"type\":\"session\",\"version\":1.5}",
            "version must be a non-negative safe integer",
        ),
    ] {
        let error = session_header_version(content, "bad.jsonl").unwrap_err();
        assert!(error.contains(message), "{content}: {error}");
    }
}
