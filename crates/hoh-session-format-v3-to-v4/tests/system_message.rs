mod common;
use common::*;
fn attachment() -> Value {
    json!({"attachmentId":"sha256:image","mediaType":"image/png","bytes":1,"width":1,"height":1})
}
fn message(content: Value) -> Value {
    json!({"id":"system","role":"system","source":{"kind":"system-prompt"},"content":content})
}
fn row(data: Value) -> SessionFormatEvent {
    e("system/message", 2, data)
}
fn restored(data: Value) -> R<SessionFormatArtifact> {
    let mut rows = opening();
    let mut row = row(data);
    row.insert("surfaceOp".into(), "append".into());
    rows.push(row);
    let (rows, _) = round_trip(&dense(rows), &h(4, "system-fields"), 0)?;
    native(rows)
}
#[test]
fn preserves_required_fields_and_extra_json_on_known_and_extensible_block_types() {
    let mut named = attachment();
    named["name"] = "name".into();
    named["originalDimensions"] = json!({"width":1,"height":2});
    named["extension"] = true.into();
    let content = json!([{"type":"text","text":"","extension":{"keep":true}},{"type":"reasoning","text":"reasoning"},{"type":"tool-call","id":"call","name":"read","arguments":"{}"},{"type":"file","extension":true},{"type":"image","attachment":attachment()},{"type":"image","attachment":named}]);
    let data = json!({"turn":1,"step":1,"message":message(content),"extra":true});
    assert_eq!(restored(data.clone()).unwrap().events[2]["data"], data);
}
#[test]
fn rejects_malformed_fields_before_a_recoverable_scanner_can_discard_the_row() {
    let mut contents = vec![
        Value::Null,
        json!([null]),
        json!([{}]),
        json!([{"type":""}]),
        json!([{"type":"text","text":1}]),
        json!([{"type":"tool-call","id":""}]),
        json!([{"type":"tool-call","id":"id","name":""}]),
        json!([{"type":"tool-call","id":"id","name":"name","arguments":null}]),
        json!([{"type":"tool-result","toolCallId":"call","content":[]}]),
    ];
    let mut invalid_attachments = vec![Value::Null];
    for (key, value) in [
        ("mediaType", json!("text/plain")),
        ("attachmentId", json!("")),
        ("bytes", json!(-1)),
        ("width", json!(0)),
        ("height", json!(0)),
        ("name", json!(1)),
        ("originalDimensions", Value::Null),
        ("originalDimensions", json!({"width":0,"height":1})),
        ("originalDimensions", json!({"width":1,"height":0})),
    ] {
        let mut a = attachment();
        a[key] = value;
        invalid_attachments.push(a);
    }
    for a in invalid_attachments {
        contents.push(json!([{"type":"image","attachment":a}]));
    }
    let mut invalid = vec![
        Value::Null,
        json!({"turn":0,"step":1}),
        json!({"turn":1,"step":0}),
        json!({"turn":1,"step":1,"message":null}),
    ];
    let mut m = message(json!([]));
    m["id"] = "".into();
    invalid.push(json!({"turn":1,"step":1,"message":m}));
    let mut m = message(json!([]));
    m["role"] = "user".into();
    invalid.push(json!({"turn":1,"step":1,"message":m}));
    for c in contents {
        invalid.push(json!({"turn":1,"step":1,"message":message(c)}));
    }
    for data in invalid {
        assert!(restored(data.clone()).is_err());
        let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
            .create_decoder(
                &RELEASED_V4_SESSION_FORMAT_CODEC
                    .encode_header(&h(4, "system-fields"), 0)
                    .unwrap()
                    .into(),
                SessionFormatRecovery::Recoverable,
            )
            .unwrap();
        let mut out = SessionFormatEventCollector::new();
        d.decode_row(Value::Null, &mut out).unwrap();
        assert!(d.decode_row(row(data).into(), &mut out).is_err());
    }
}
