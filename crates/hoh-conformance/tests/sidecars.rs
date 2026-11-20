//! `suite.spec.ts` 640–940: scenario skipping, stdout variants, fixture context, header/prompt/schema
//! extraction and the sidecar formats.
use hoh_conformance::{
    compare::restore_pinned_tool_schemas,
    normalize::Context,
    sidecars::*,
    storage::{system_prompt_precedes_requests, unknown_tool_call_ids},
};
use serde_json::{Value, json};
use std::path::PathBuf;

fn ctx(cwd: &str, ids: &[&str]) -> Context {
    Context {
        cwd: cwd.into(),
        aliases: Vec::new(),
        session_ids: ids.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn pinned() -> SnapshotScenario {
    SnapshotScenario {
        name: "pinned".into(),
        recorded: true,
        pins_native_windows_stdout: true,
        ..Default::default()
    }
}

/// :640
#[test]
fn adds_the_native_sidecar_after_the_shared_golden_on_windows() {
    assert_eq!(
        stdout_expected_variants(&pinned(), "win32"),
        vec![
            ("stdout.expected.jsonl", "canonical"),
            ("stdout.expected.windows.jsonl", "native")
        ]
    );
}

/// :647
#[test]
fn keeps_only_the_shared_golden_on_other_platforms_or_without_the_declaration() {
    let shared = vec![("stdout.expected.jsonl", "canonical")];
    assert_eq!(stdout_expected_variants(&pinned(), "linux"), shared);
    let off = SnapshotScenario {
        pins_native_windows_stdout: false,
        ..pinned()
    };
    assert_eq!(stdout_expected_variants(&off, "win32"), shared);
}

fn authored() -> SnapshotScenario {
    SnapshotScenario {
        name: "authored".into(),
        ..Default::default()
    }
}

/// :668
#[test]
fn skips_authored_scenarios_only_while_recording() {
    assert!(scenario_skipped(&authored(), true, "linux", None));
    assert!(!scenario_skipped(&authored(), false, "linux", None));
}

/// :673
#[test]
fn skips_a_retained_generation_while_recording_but_replays_it() {
    let retained = SnapshotScenario {
        recorded: true,
        retained_generation: true,
        ..authored()
    };
    assert!(scenario_skipped(&retained, true, "linux", None));
    assert!(!scenario_skipped(&retained, false, "linux", None));
}

/// :678
#[test]
fn skips_posix_only_scenarios_on_windows_and_nowhere_else() {
    let posix = SnapshotScenario {
        posix_only: true,
        ..authored()
    };
    assert!(scenario_skipped(&posix, false, "win32", None));
    assert!(!scenario_skipped(&posix, false, "linux", None));
    assert!(!scenario_skipped(&posix, false, "darwin", None));
    assert!(!scenario_skipped(&authored(), false, "win32", None));
}

/// :685
#[test]
fn skips_pwsh_only_scenarios_when_the_host_lacks_pwsh() {
    let pwsh = SnapshotScenario {
        pwsh_only: true,
        ..authored()
    };
    assert!(scenario_skipped(&pwsh, false, "linux", Some(false)));
    assert!(!scenario_skipped(&pwsh, false, "win32", Some(true)));
    assert!(!scenario_skipped(&pwsh, false, "linux", Some(true)));
    assert!(!scenario_skipped(&authored(), false, "linux", Some(false)));
}

/// :694
#[test]
fn fixture_context_reads_the_header_id_and_cwd() {
    let c = fixture_context(
        "{\"type\":\"session\",\"id\":\"abc\",\"cwd\":\"/rec\"}\n{\"type\":\"turn/start\"}\n",
    )
    .unwrap();
    assert_eq!(
        (c.session_ids, c.cwd),
        (vec!["abc".to_owned()], "/rec".to_owned())
    );
}

/// :699
#[test]
fn fixture_context_yields_no_session_ids_without_a_string_id() {
    assert!(
        fixture_context("{\"type\":\"session\",\"cwd\":\"/rec\"}\n")
            .unwrap()
            .session_ids
            .is_empty()
    );
}

/// :703
#[test]
fn fixture_context_falls_back_to_an_impossible_sentinel_cwd() {
    let c = fixture_context("{\"type\":\"session\",\"id\":\"abc\"}\n").unwrap();
    assert_eq!(c.cwd, "\0no-cwd\0");
    assert_ne!(c.cwd, "");
}

/// :709
#[test]
fn fixture_context_treats_an_empty_fixture_as_an_empty_header() {
    let c = fixture_context("").unwrap();
    assert!(c.session_ids.is_empty());
    assert_eq!(c.cwd, "\0no-cwd\0");
}

fn header(tool: &str) -> String {
    json!({"type":"request/header","seq":0,"time":9,
        "data":{"header":{"config":{"model":"m"},"tools":[{"name":tool}]},"reason":"initial"}})
    .to_string()
}

/// :719
#[test]
fn extracts_every_request_header_payload_in_log_order_normalized() {
    let id = "11111111-2222-4333-8444-555555555555";
    let log = format!(
        "{}\n{}\n{}\n{}\n",
        json!({"type":"session","id":id,"createdAt":5,"cwd":"/w"}),
        header("one"),
        json!({"type":"turn/start","seq":1,"time":9,"data":{"turn":1}}),
        header("two")
    );
    assert_eq!(
        normalized_headers(&log, &ctx("/w", &[id])).unwrap(),
        vec![
            json!({"config":{"model":"m"},"tools":[{"name":"one"}]}),
            json!({"config":{"model":"m"},"tools":[{"name":"two"}]}),
        ]
    );
}

/// :730
#[test]
fn yields_nothing_for_a_log_without_header_events() {
    let log = format!("{}\n", json!({"type":"session","id":"a","createdAt":5}));
    assert!(
        normalized_headers(&log, &ctx("/w", &[]))
            .unwrap()
            .is_empty()
    );
}

fn system_message(seq: u32, data: Value) -> String {
    json!({"type":"system/message","seq":seq,"time":9,"data":data}).to_string()
}

/// :739
#[test]
fn extracts_every_system_message_text_and_reads_empty_content_as_no_prompt() {
    let log = [
        r#"{"type":"session","id":"a","createdAt":5,"cwd":"/w"}"#.to_owned(),
        system_message(0, json!({"turn":1,"step":1,"message":{"role":"system","content":[{"type":"text","text":"work in /w"}]}})),
        r#"{"type":"request/header","seq":1,"time":9,"data":{"header":{"tools":[]},"reason":"initial"}}"#.to_owned(),
        system_message(2, json!({"turn":1,"step":2,"message":{"role":"system","content":[]}})),
        system_message(3, json!({"turn":1,"step":3,"message":{"role":"system","content":[{"type":"text","text":"replaced"}]}})),
        String::new(),
    ]
    .join("\n");
    assert_eq!(
        normalized_system_prompts(&log, &ctx("/w", &[])).unwrap(),
        vec!["work in {{cwd}}", "", "replaced"]
    );
}

/// :751
#[test]
fn omits_system_messages_without_a_text_block() {
    let log = [
        r#"{"type":"session","id":"a","createdAt":5,"cwd":"/w"}"#.to_owned(),
        system_message(0, json!({"turn":1,"step":1})),
        system_message(1, json!({"turn":1,"step":1,"message":{"role":"system","content":"text"}})),
        system_message(2, json!({"turn":1,"step":1,"message":{"role":"system","content":[{"type":"image","data":"x"}]}})),
        system_message(3, json!({"turn":1,"step":1,"message":{"role":"system","content":[{"type":"text","text":"kept"}]}})),
        String::new(),
    ]
    .join("\n");
    assert_eq!(
        normalized_system_prompts(&log, &ctx("/w", &[])).unwrap(),
        vec!["kept"]
    );
}

const SESSION: &str = r#"{"type":"session","id":"a","createdAt":5,"cwd":"/w"}"#;
const SYSTEM: &str = r#"{"type":"system/message","seq":0,"time":9,"data":{"turn":1,"step":1,"message":{"role":"system","content":[]}}}"#;
const HEADER: &str = r#"{"type":"request/header","seq":1,"time":9,"data":{"header":{"tools":[]},"reason":"initial"}}"#;

/// :769
#[test]
fn accepts_a_log_whose_first_request_header_follows_a_system_message() {
    assert!(
        system_prompt_precedes_requests(&format!("{SESSION}\n{SYSTEM}\n{HEADER}\n{HEADER}\n"))
            .unwrap()
    );
}

/// :773
#[test]
fn accepts_a_log_without_any_request_header() {
    assert!(system_prompt_precedes_requests(&format!("{SESSION}\n")).unwrap());
    assert!(system_prompt_precedes_requests(&format!("{SESSION}\n{SYSTEM}\n")).unwrap());
}

/// :778
#[test]
fn rejects_a_request_header_with_no_preceding_system_message() {
    assert!(!system_prompt_precedes_requests(&format!("{SESSION}\n{HEADER}\n")).unwrap());
    assert!(!system_prompt_precedes_requests(&format!("{SESSION}\n{HEADER}\n{SYSTEM}\n")).unwrap());
}

/// :785
#[test]
fn extracts_normalized_schema_arrays_preserves_empty_headers_and_omits_invalid_fields() {
    let log = [
        SESSION,
        r#"{"type":"request/header","seq":0,"time":9,"data":{"header":{"tools":[{"name":"read","description":"work in /w"}]}}}"#,
        r#"{"type":"request/header","seq":1,"time":9,"data":{"header":{}}}"#,
        r#"{"type":"request/header","seq":2,"time":9,"data":{"header":{"tools":null}}}"#,
        r#"{"type":"request/header","seq":3,"time":9,"data":{"header":null}}"#,
        r#"{"type":"request/header","seq":4,"time":9,"data":{"header":"invalid"}}"#,
        "",
    ]
    .join("\n");
    assert_eq!(
        normalized_tool_schemas(&log, &ctx("/w", &[])).unwrap(),
        vec![
            vec![json!({"name":"read","description":"work in {{cwd}}"})],
            vec![]
        ]
    );
}

/// :803
#[test]
fn adds_a_missing_terminal_newline_without_changing_an_existing_one() {
    assert_eq!(format_system_prompt_snapshot("prompt", &[]), "prompt\n");
    assert_eq!(format_system_prompt_snapshot("prompt\n", &[]), "prompt\n");
}

/// :808
#[test]
fn renders_readable_changed_prompt_sections() {
    assert_eq!(
        format_system_prompt_snapshot("prompt", &["new\nlines"]),
        "prompt\n\n<!-- system/message change 1 -->\n\nnew\nlines\n"
    );
}

/// :813
#[test]
fn does_not_double_the_newline_of_a_changed_prompt() {
    assert_eq!(
        format_system_prompt_snapshot("prompt\n", &["changed\n"]),
        "prompt\n\n<!-- system/message change 1 -->\n\nchanged\n"
    );
}

/// :820
#[test]
fn splits_a_formatted_sidecar_back_into_its_initial_prompt_and_each_replacement() {
    let snapshot =
        format_system_prompt_snapshot("initial\n", &["first change", "second\nchange\n"]);
    assert_eq!(
        parse_system_prompt_snapshot(&snapshot).unwrap(),
        (
            "initial\n".to_owned(),
            vec!["first change\n".to_owned(), "second\nchange\n".to_owned()]
        )
    );
}

/// :828
#[test]
fn reads_a_single_prompt_sidecar_as_having_no_changes() {
    assert_eq!(
        parse_system_prompt_snapshot("prompt\n").unwrap(),
        ("prompt\n".to_owned(), vec![])
    );
}

const LABEL: &str = "plain-turn/system-prompt.1.expected.md";

/// :836
#[test]
fn accepts_a_distinct_non_empty_canonical_child_prompt() {
    assert_eq!(
        assert_child_system_prompt_snapshot(
            "SYS PROMPT\n\nCHILD GUIDANCE\n",
            "SYS PROMPT\n",
            LABEL
        ),
        Ok(())
    );
}

/// :842
#[test]
fn rejects_an_empty_or_non_canonical_child_prompt() {
    let empty = assert_child_system_prompt_snapshot("\n", "SYS PROMPT\n", LABEL).unwrap_err();
    assert!(empty.contains("non-empty prompt"), "{empty}");
    let bare =
        assert_child_system_prompt_snapshot("CHILD GUIDANCE", "SYS PROMPT\n", LABEL).unwrap_err();
    assert!(bare.contains("end in a newline"), "{bare}");
}

/// :849 — the class pin is read from the committed `pin-turn` fixture.
#[test]
fn rejects_a_child_prompt_that_duplicates_its_class_pin() {
    let refs = std::env::var_os("HOH_REFS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness"));
    let path = refs
        .join("packages/test-support/session-snapshot/tests/fixtures/suite/pin-turn/system-prompt.expected.md");
    let class_snapshot = std::fs::read_to_string(path).unwrap();
    let (initial, changes) = parse_system_prompt_snapshot(&class_snapshot).unwrap();
    assert_eq!(changes.len(), 1);
    let err = assert_child_system_prompt_snapshot(&initial, &initial, LABEL).unwrap_err();
    assert!(err.contains("must differ from its class pin"), "{err}");
}

/// :862
#[test]
fn counts_changed_request_headers_ignoring_anchors_blanks_and_other_lines() {
    let change = json!({"type":"request/header","seq":2,"time":9,"data":{"reason":"change"}});
    let anchor = json!({"type":"request/header","seq":0,"time":9,"data":{"reason":"initial"}});
    let other = json!({"type":"turn/start","seq":1,"time":9,"data":{}});
    assert_eq!(
        header_change_count(&format!("{anchor}\n{other}\n\n{change}\n{change}\n")).unwrap(),
        2
    );
    assert_eq!(header_change_count(&format!("{anchor}\n")).unwrap(), 0);
}

fn snapshot_initial() -> Vec<Value> {
    vec![json!({"name":"read","description":"Read a file."})]
}
fn snapshot_changes() -> Vec<Vec<Value>> {
    vec![vec![json!({"name":"grep","description":"Search files."})]]
}

/// :877
#[test]
fn formats_and_parses_canonical_structured_json() {
    let formatted = format_tool_schemas_snapshot(&snapshot_initial(), &snapshot_changes());
    let expected = json!({"initial": snapshot_initial(), "changes": snapshot_changes()});
    assert_eq!(
        formatted,
        format!("{}\n", serde_json::to_string_pretty(&expected).unwrap())
    );
    assert_eq!(
        parse_tool_schemas_snapshot(&formatted).unwrap(),
        (snapshot_initial(), snapshot_changes())
    );
}

/// :883
#[test]
fn rejects_invalid_top_level_and_field_shapes() {
    for bad in ["null", "\"invalid\"", "[]"] {
        let err = parse_tool_schemas_snapshot(bad).unwrap_err();
        assert!(err.contains("must be an object"), "{bad}: {err}");
    }
    for bad in [
        r#"{"initial":{},"changes":[]}"#,
        r#"{"initial":[],"changes":{}}"#,
        r#"{"initial":[],"changes":[{}]}"#,
    ] {
        let err = parse_tool_schemas_snapshot(bad).unwrap_err();
        assert!(err.contains("array-valued"), "{bad}: {err}");
    }
}

fn restore(mut header: Value, schemas: Value) -> Result<Value, String> {
    restore_pinned_tool_schemas(&mut header, &schemas).map(|()| header)
}

/// :892
#[test]
fn restores_initial_schemas_into_the_pinned_header_token() {
    let initial = Value::Array(snapshot_initial());
    assert_eq!(
        restore(
            json!({"system":"{{system}}","tools":"{{tools}}"}),
            initial.clone()
        )
        .unwrap(),
        json!({"system":"{{system}}","tools":initial})
    );
}

/// :897
#[test]
fn preserves_absent_tools_only_for_an_empty_sidecar() {
    let header = json!({"config":{"model":"test"}});
    assert_eq!(restore(header.clone(), json!([])).unwrap(), header);
    let err = restore(header, Value::Array(snapshot_initial())).unwrap_err();
    assert!(err.contains("must equal"), "{err}");
}

/// :903
#[test]
fn restores_ordered_tool_names_only_when_they_match_the_sidecar() {
    let schemas = json!([{"name":"read"},{"name":"write"}]);
    assert_eq!(
        restore(json!({"tools":["read","write"]}), schemas.clone()).unwrap(),
        json!({"tools": schemas})
    );
    for tools in [
        json!(["write", "read"]),
        json!(["read"]),
        json!(["read", "missing"]),
        json!([1, "write"]),
    ] {
        let err = restore(json!({"tools": tools}), schemas.clone()).unwrap_err();
        assert!(err.contains("must equal"), "{err}");
    }
    for schema in [
        json!(null),
        json!("read"),
        json!({}),
        json!({"name":"other"}),
    ] {
        let err = restore(json!({"tools":["read"]}), json!([schema])).unwrap_err();
        assert!(err.contains("must equal"), "{err}");
    }
}

/// :914
#[test]
fn rejects_invalid_headers_and_a_missing_tool_token() {
    let initial = Value::Array(snapshot_initial());
    for header in [json!(null), json!("invalid"), json!([])] {
        let err = restore(header, initial.clone()).unwrap_err();
        assert!(err.contains("must be an object"), "{err}");
    }
    let err = restore(json!({"tools":[]}), initial).unwrap_err();
    assert!(err.contains("must equal"), "{err}");
}

/// :923
#[test]
fn returns_structured_unknown_tool_call_ids_and_ignores_other_results() {
    let log = [
        r#"{"type":"tool/result","data":{"message":{"source":{"kind":"tool","callId":"missing"}},"error":{"code":"UNKNOWN_TOOL"}}}"#,
        r#"{"type":"tool/result","data":{"message":{"source":{"kind":"tool","callId":"failed"}},"error":{"code":"EXECUTION_FAILED"}}}"#,
        r#"{"type":"tool/result","data":null}"#,
        r#"{"type":"tool/result","data":"invalid"}"#,
        r#"{"type":"tool/result","data":{"error":null}}"#,
        r#"{"type":"tool/result","data":{"error":"invalid"}}"#,
        r#"{"type":"assistant/message","data":{"error":{"code":"UNKNOWN_TOOL"}}}"#,
        r#"{"type":"tool/result","data":{"error":{"code":"UNKNOWN_TOOL"}}}"#,
        "",
    ]
    .join("\n");
    assert_eq!(
        unknown_tool_call_ids(&log).unwrap(),
        vec!["missing", "<missing callId>"]
    );
}

/// :938
#[test]
fn returns_no_failures_for_ordinary_tool_results() {
    let log = "{\"type\":\"tool/result\",\"data\":{\"message\":{\"source\":{\"kind\":\"tool\",\"callId\":\"ok\"}}}}\n";
    assert!(unknown_tool_call_ids(log).unwrap().is_empty());
}
