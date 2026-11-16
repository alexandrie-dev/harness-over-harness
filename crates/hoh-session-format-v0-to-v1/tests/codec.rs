mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v0_to_v1::*;
use serde_json::{Value, json};
fn full_header() -> Value {
    json!({"type":"session","version":1,"id":"codec","createdAt":1,"cwd":"/work","parentSession":"parent","seedLength":0,"origin":"subagent","delegationDepth":1,"agentPreset":"default"})
}
fn decode_artifact(
    h: &Value,
    rows: &[Value],
    recovery: SessionFormatRecovery,
) -> Result<SessionFormatArtifact, SessionFormatError> {
    let mut decoder = RELEASED_V1_SESSION_FORMAT_CODEC.create_decoder(h, recovery)?;
    let mut context = SessionFormatEventCollector::new();
    for row in rows {
        decoder.decode_row(row.clone(), &mut context)?;
    }
    Ok(SessionFormatArtifact {
        header: decoder.header().clone(),
        inherited_event_count: decoder.finish(&mut context)?,
        events: context.values,
    })
}
#[test]
fn decodes_every_physical_header_field_and_seeded_zero_cut() {
    let decoded = decode_artifact(&full_header(), &[], SessionFormatRecovery::Strict).unwrap();
    let expected = json!({"version":1,"id":"codec","createdAt":1,"cwd":"/work","parentSession":"parent","isSeeded":true,"origin":"subagent","delegationDepth":1,"agentPreset":"default"});
    assert_eq!(Value::Object(decoded.header.clone()), expected);
    assert_eq!(decoded.inherited_event_count, 0);
    assert!(decoded.events.is_empty());
    let mut h = full_header();
    h["version"] = 0.into();
    let mut expected = expected;
    expected["version"] = 0.into();
    assert_eq!(
        Value::Object(RELEASED_V0_SESSION_FORMAT_CODEC.decode_header(&h).unwrap()),
        expected
    );
    assert_eq!(
        serde_json::to_string(&decoded.header).unwrap(),
        r#"{"version":1,"id":"codec","createdAt":1,"cwd":"/work","parentSession":"parent","isSeeded":true,"origin":"subagent","delegationDepth":1,"agentPreset":"default"}"#
    );
}
#[test]
fn refuses_malformed_physical_headers() {
    let mut cases = vec![Value::Null];
    for (key, value) in [
        ("extra", json!(true)),
        ("type", json!("other")),
        ("version", json!(0)),
        ("id", json!(1)),
        ("createdAt", json!(-1)),
        ("delegationDepth", json!(-1)),
        ("cwd", json!(1)),
        ("parentSession", json!(1)),
        ("agentPreset", json!(1)),
        ("origin", json!("other")),
    ] {
        let mut h = full_header();
        h[key] = value;
        cases.push(h);
    }
    for h in cases {
        assert!(
            RELEASED_V1_SESSION_FORMAT_CODEC.decode_header(&h).is_err(),
            "{h}"
        );
    }
}
#[derive(Default)]
struct Items {
    events: Vec<SessionFormatEvent>,
    runs: Vec<Box<dyn SessionFormatEventRun>>,
}
impl SessionFormatMigrationContext for Items {
    fn emit_event(&mut self, e: SessionFormatEvent) -> Result<(), SessionFormatError> {
        self.events.push(e);
        Ok(())
    }
    fn emit_run(&mut self, r: Box<dyn SessionFormatEventRun>) -> Result<(), SessionFormatError> {
        self.runs.push(r);
        Ok(())
    }
}
#[test]
fn expands_valid_text_reasoning_and_named_or_unnamed_tool_call_rows_exactly() {
    let cases = [
        (
            json!({"type":"text-chunks","seq0":0,"time0":1,"data":{"turn":1,"step":2,"index":3,"dt":[2],"texts":["a","b"]}}),
            json!([
            {"type":"assistant/chunk","seq":0,"time":1,"data":{"turn":1,"step":2,"chunk":{"type":"text-delta","index":3,"text":"a"}}},
            {"type":"assistant/chunk","seq":1,"time":3,"data":{"turn":1,"step":2,"chunk":{"type":"text-delta","index":3,"text":"b"}}}]),
        ),
        (
            json!({"type":"reasoning-chunks","seq0":0,"time0":4,"data":{"turn":2,"step":3,"index":1,"dt":[1],"texts":["c","d"]}}),
            json!([
            {"type":"assistant/chunk","seq":0,"time":4,"data":{"turn":2,"step":3,"chunk":{"type":"reasoning-delta","index":1,"text":"c"}}},
            {"type":"assistant/chunk","seq":1,"time":5,"data":{"turn":2,"step":3,"chunk":{"type":"reasoning-delta","index":1,"text":"d"}}}]),
        ),
        (
            json!({"type":"tool-call-chunks","seq0":0,"time0":6,"data":{"turn":3,"step":4,"index":2,"id":"call","name":"read","dt":[1],"args":["{","}"]}}),
            json!([
            {"type":"assistant/chunk","seq":0,"time":6,"data":{"turn":3,"step":4,"chunk":{"type":"tool-call-delta","index":2,"id":"call","name":"read","argumentsDelta":"{"}}},
            {"type":"assistant/chunk","seq":1,"time":7,"data":{"turn":3,"step":4,"chunk":{"type":"tool-call-delta","index":2,"id":"call","name":"read","argumentsDelta":"}"}}}]),
        ),
        (
            json!({"type":"tool-call-chunks","seq0":0,"time0":8,"data":{"turn":4,"step":5,"index":0,"id":"call","dt":[],"args":["x"]}}),
            json!([
            {"type":"assistant/chunk","seq":0,"time":8,"data":{"turn":4,"step":5,"chunk":{"type":"tool-call-delta","index":0,"id":"call","argumentsDelta":"x"}}}]),
        ),
    ];
    for (row, expected) in cases {
        let mut d = RELEASED_V1_SESSION_FORMAT_CODEC
            .create_decoder(&full_header(), SessionFormatRecovery::Strict)
            .unwrap();
        let mut c = Items::default();
        d.decode_row(row, &mut c).unwrap();
        assert!(c.events.is_empty());
        assert_eq!(c.runs.len(), 1);
        assert!(is_released_assistant_chunk_run(c.runs[0].as_ref()));
        assert_eq!(
            Value::Array(c.runs.remove(0).expand().map(Value::Object).collect()),
            expected
        );
    }
}
#[test]
fn refuses_malformed_packed_rows() {
    let base = json!({"type":"text-chunks","seq0":0,"time0":1,"data":{"turn":1,"step":0,"index":0,"dt":[],"texts":["a"]}});
    let mut cases = vec![merge(base.clone(), json!({"extra":true}))];
    for (key, value) in [
        ("texts", json!([])),
        ("texts", json!([1])),
        ("dt", json!([1])),
        ("turn", json!("1")),
    ] {
        let mut row = base.clone();
        row["data"][key] = value;
        cases.push(row);
    }
    cases.extend([
        json!({"type":"tool-call-chunks","seq0":0,"time0":1,"data":{"turn":1,"step":0,"index":0,"id":1,"dt":[],"args":["a"]}}),
        json!({"type":"tool-call-chunks","seq0":0,"time0":1,"data":{"turn":1,"step":0,"index":0,"id":"id","name":1,"dt":[],"args":["a"]}}),
        json!({"type":"text-chunks","seq0":0,"time0":9_007_199_254_740_991_u64,"data":{"turn":1,"step":0,"index":0,"dt":[1],"texts":["a","b"]}}),
    ]);
    for row in cases {
        assert!(
            decode_artifact(
                &full_header(),
                std::slice::from_ref(&row),
                SessionFormatRecovery::Strict
            )
            .is_err(),
            "{row}"
        );
    }
}
#[test]
fn refuses_malformed_stored_source_event_ranges() {
    for sources in [
        json!("bad"),
        json!([0, 0]),
        json!([[0]]),
        json!([[2, 1]]),
        json!([[0, 3]]),
        json!([[0, 1], [1, 2]]),
    ] {
        let mut row = surface("user/message", 1, user("u"));
        row["sourceEventSeqs"] = sources;
        assert!(
            decode_artifact(
                &header(1, "codec"),
                &[turn(0, 1), row],
                SessionFormatRecovery::Strict
            )
            .is_err()
        );
    }
}
#[test]
fn streams_the_same_row_atomic_recoverable_prefix() {
    let create = |h: &Value| {
        RELEASED_V1_SESSION_FORMAT_CODEC
            .create_decoder(h, SessionFormatRecovery::Recoverable)
            .unwrap()
    };
    let h = header(1, "codec");
    let mut malformed = create(&h);
    let mut c = Items::default();
    malformed.decode_row(Value::Null, &mut c).unwrap();
    assert!(c.events.is_empty());
    malformed.decode_row(step(0), &mut c).unwrap();
    assert!(c.events.is_empty());
    assert_error(
        malformed.decode_row(end(0, 1), &mut c),
        "released Session row 0 must be a JSON object",
    );
    let mut gap = create(&h);
    gap.decode_row(step(1), &mut c).unwrap();
    assert!(c.events.is_empty());
    let mut terminal = create(&h);
    assert_error(terminal.decode_row(end(1, 1), &mut c), "seq gap");
    let mut seeded = create(&merge(h.clone(), json!({"seedLength":1})));
    assert_error(seeded.finish(&mut c), "inheritedEventCount exceeds");
    let mut packed = create(&h);
    packed.decode_row(json!({"type":"text-chunks","seq0":0,"time0":1,"data":{"turn":1,"step":1,"index":0,"dt":[1],"texts":["a","b"]}}),&mut c).unwrap();
    assert_eq!(c.runs.len(), 1);
    assert_eq!(c.runs.remove(0).expand().count(), 2);
    let mut d = create(&h);
    d.decode_row(turn(0, 1), &mut c).unwrap();
    let mut row = surface("user/message", 1, user("m"));
    row["sourceEventSeqs"] = json!([0]);
    d.decode_row(row, &mut c).unwrap();
    assert_eq!(c.events.last().unwrap()["sourceEventSeqs"], json!([0]));
}
#[test]
fn ignores_decodable_non_terminal_rows_after_first_recoverable_issue() {
    let a = decode_artifact(
        &header(1, "codec"),
        &[turn(0, 1), turn(4, 2), step(1)],
        SessionFormatRecovery::Recoverable,
    )
    .unwrap();
    assert_eq!(values(&a), vec![turn(0, 1)]);
}
#[test]
fn rejects_strict_gaps_and_a_recoverable_gap_row_that_itself_closes_a_turn() {
    assert_error(
        decode_artifact(
            &header(1, "codec"),
            &[turn(1, 1)],
            SessionFormatRecovery::Strict,
        ),
        "seq gap",
    );
    assert_error(
        decode_artifact(
            &header(1, "codec"),
            &[end(1, 1)],
            SessionFormatRecovery::Recoverable,
        ),
        "seq gap",
    );
}
#[test]
fn refuses_overlapping_ranges_after_a_valid_first_range() {
    let mut u = surface("user/message", 4, user("u"));
    u["sourceEventSeqs"] = json!([[0, 1], [1, 2]]);
    assert_error(
        decode_artifact(
            &header(1, "codec"),
            &[turn(0, 1), step(1), step_end(2), end(3, 1), u],
            SessionFormatRecovery::Strict,
        ),
        "strictly increasing",
    );
}

#[test]
fn preserves_js_integer_spelling_key_order_and_integral_packed_gaps() {
    let mut row = surface("user/message", 0, user("numbers"));
    row["time"] = json!(1.0);
    row["data"]["source"] = serde_json::from_str(
        r#"{"kind":"future","2":2.0,"1":1.0,"nested":{"z":1.0,"nothing":null}}"#,
    )
    .unwrap();
    let original = row.clone();
    let a = decode_artifact(
        &header(1, "codec"),
        std::slice::from_ref(&row),
        SessionFormatRecovery::Strict,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_string(&a.events[0]["data"]["source"]).unwrap(),
        r#"{"1":1,"2":2,"kind":"future","nested":{"z":1,"nothing":null}}"#
    );
    assert_eq!(a.events[0]["time"].to_string(), "1");
    assert_eq!(row, original);
    let row = json!({"type":"text-chunks","seq0":0.0,"time0":1.0,"data":{"turn":1.0,"step":1.0,"index":0.0,"dt":[2.0,-1.0],"texts":["a","b","c"]}});
    let a = decode_artifact(&header(1, "codec"), &[row], SessionFormatRecovery::Strict).unwrap();
    assert_eq!(
        a.events
            .iter()
            .map(|e| e["time"].clone())
            .collect::<Vec<_>>(),
        vec![json!(1), json!(3), json!(2)]
    );
}
