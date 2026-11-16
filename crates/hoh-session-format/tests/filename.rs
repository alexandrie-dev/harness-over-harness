//! Port of `tests/filename.spec.ts`.

use hoh_session_format::{parse_session_format_log_filename, session_format_log_filename};

#[test]
fn names_version_zero_without_a_generation_component_and_later_generations_with_vn() {
    assert_eq!(session_format_log_filename(0).expect("v0"), "session.jsonl");
    assert_eq!(
        session_format_log_filename(1).expect("v1"),
        "session.v1.jsonl"
    );
    assert_eq!(
        session_format_log_filename(27).expect("v27"),
        "session.v27.jsonl"
    );
    // -1, -0 and 1.5 of the source are not representable as `u64`.
    let error = session_format_log_filename(9_007_199_254_740_992).unwrap_err();
    assert!(error.to_string().contains("non-negative safe integer"));
}

#[test]
fn parses_only_canonical_raw_generation_names() {
    assert_eq!(parse_session_format_log_filename("session.jsonl"), Some(0));
    assert_eq!(
        parse_session_format_log_filename("session.v1.jsonl"),
        Some(1)
    );
    assert_eq!(
        parse_session_format_log_filename("session.v27.jsonl"),
        Some(27)
    );
    assert_eq!(
        parse_session_format_log_filename("session.v9007199254740992.jsonl"),
        None
    );
    assert_eq!(
        parse_session_format_log_filename("session.v9007199254740991.jsonl"),
        Some(9_007_199_254_740_991)
    );
    assert_eq!(
        parse_session_format_log_filename("session.v99999999999999999999999.jsonl"),
        None
    );
    for name in [
        "session.v0.jsonl",
        "session.v01.jsonl",
        "session.V1.jsonl",
        "session.v1.backup.jsonl",
        "session.migration.deadbeef.tmp.jsonl",
        "session.v1.jsonl.zstd",
        "session.1.jsonl",
        "session.v1.jsonl\n",
        "session.v.jsonl",
        "session.v٣.jsonl",
        "",
    ] {
        assert_eq!(parse_session_format_log_filename(name), None, "{name:?}");
    }
}
