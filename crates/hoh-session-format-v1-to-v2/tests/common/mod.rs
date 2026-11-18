#![allow(dead_code)]
use hoh_session_format::*;
use hoh_session_format_v1_to_v2::*;
use serde_json::{Map, Value, json};
pub fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}
pub fn header(version: u64, id: &str) -> SessionFormatHeader {
    obj(json!({"version":version,"id":id,"createdAt":1,"isSeeded":false,"delegationDepth":0}))
}
pub fn physical() -> Value {
    json!({"type":"session","version":2,"id":"codec","createdAt":1,"isSeeded":false,"delegationDepth":0})
}
pub fn event(kind: &str, seq: u64, time: i64, data: Value) -> SessionFormatEvent {
    obj(json!({"type":kind,"seq":seq,"time":time,"data":data}))
}
pub fn ev(kind: &str, seq: u64, data: Value) -> SessionFormatEvent {
    event(kind, seq, seq as i64 + 1, data)
}
pub fn artifact(events: Vec<SessionFormatEvent>) -> SessionFormatArtifact {
    SessionFormatArtifact {
        header: header(2, "validation"),
        inherited_event_count: 0,
        events,
    }
}
pub fn feedback(seq: u64) -> SessionFormatEvent {
    ev(
        "feedback/record",
        seq,
        json!({"text":format!("feedback-{seq}")}),
    )
}
pub fn user_data(id: &str, text: &str) -> Value {
    json!({"id":id,"role":"user","content":[{"type":"text","text":text}],"source":{"kind":"user"}})
}
pub fn user(seq: u64, sources: Option<Value>) -> SessionFormatEvent {
    let mut e = ev(
        "user/message",
        seq,
        user_data(&format!("user-{seq}"), "text"),
    );
    e.insert("surfaceOp".into(), "append".into());
    if let Some(v) = sources {
        e.insert("sourceEventSeqs".into(), v);
    }
    e
}
pub fn err<T: std::fmt::Debug>(r: Result<T, SessionFormatError>, expected: &str) {
    let e = r.unwrap_err();
    assert_format_class(&e);
    assert!(
        e.to_string().contains(expected),
        "expected {expected:?}, got {e}"
    );
}
fn assert_format_class(error: &SessionFormatError) {
    // Like instanceof SessionFormatError: includes its UnsupportedMigration subclass.
    assert!(
        !matches!(error, SessionFormatError::Other(_)),
        "expected SessionFormatError, got {error:?}"
    );
}
pub fn err_pattern<T: std::fmt::Debug>(r: Result<T, SessionFormatError>, pattern: &str) {
    let error = r.unwrap_err();
    assert_format_class(&error);
    assert!(
        regex::Regex::new(pattern)
            .unwrap()
            .is_match(&error.to_string()),
        "expected /{pattern}/, got {error}"
    );
}
pub fn decode(
    header: &Value,
    rows: Vec<Value>,
    recovery: SessionFormatRecovery,
) -> Result<SessionFormatArtifact, SessionFormatError> {
    let mut decoder = RELEASED_V2_SESSION_FORMAT_CODEC.create_decoder(header, recovery)?;
    let mut context = SessionFormatEventCollector::new();
    for row in rows {
        decoder.decode_row(row, &mut context)?;
    }
    Ok(SessionFormatArtifact {
        header: decoder.header().clone(),
        inherited_event_count: decoder.finish(&mut context)?,
        events: context.values,
    })
}
pub fn values(events: Vec<SessionFormatEvent>) -> Vec<Value> {
    events.into_iter().map(Value::Object).collect()
}
