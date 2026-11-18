mod common;
use common::*;
#[test]
fn reference_failures_follow_container_then_sources_then_range_endpoints() {
    use hoh_session_format_v3_to_v4::references::remap_v3_references;
    let mut row = e("feedback/record", 1, Value::Null);
    row.insert("surfaceOp".into(), Value::Null);
    row.insert("sourceEventSeqs".into(), Value::Null);
    assert_eq!(
        remap_v3_references(row.clone(), 2, &[0])
            .unwrap_err()
            .to_string(),
        "V3 event reference container must be an object"
    );
    row.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":null,"endSeq":null}),
    );
    assert_eq!(
        remap_v3_references(row.clone(), 2, &[0])
            .unwrap_err()
            .to_string(),
        "V3 event references must be an array"
    );
    row.insert("sourceEventSeqs".into(), json!([0]));
    assert_eq!(
        remap_v3_references(row, 2, &[0]).unwrap_err().to_string(),
        "V3 source event reference must be a non-negative safe integer"
    );
}
#[test]
fn child_order_uses_utf16_and_numeric_creation_times() {
    let evidence = |id: &str, time: Value| json!({"childId":id,"childCreatedAt":time,"descriptorCount":0,"descriptor":null});
    let a = migrate(
        vec![],
        h(3, "sort"),
        Some(0),
        vec![
            evidence("\u{e000}", json!(2)),
            evidence("\u{10000}", json!(2.0)),
            evidence("first", json!(1.0)),
        ],
    )
    .unwrap();
    assert_eq!(
        a.events
            .iter()
            .map(|e| e["data"]["childId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["first", "\u{10000}", "\u{e000}"]
    );
}
#[test]
fn unexpected_header_index_keys_precede_other_own_keys() {
    let mut header = h(4, "keys");
    header.insert("x".into(), true.into());
    header.insert("2".into(), true.into());
    header.insert("1".into(), true.into());
    assert_eq!(
        assert_released_v4_header(&header).unwrap_err().to_string(),
        "format v4 header has unexpected field 1"
    );
}
#[test]
fn ptc_deep_strict_arguments_distinguish_nested_negative_zero() {
    let start = json!({"rootCallId":"root","parentCallId":"root","subCallId":"child","name":"read","arguments":{"nested":[-0.0]}});
    let end = json!({"rootCallId":"root","parentCallId":"root","subCallId":"child","name":"read","arguments":{"nested":[0]}});
    err(
        native(vec![
            ev("turn/start", json!({"turn":1})),
            ev("tool/ptc-dispatch-start", start),
            ev("tool/ptc-dispatch", end),
        ]),
        "PTC dispatch does not match its start",
    );
}
#[test]
fn historical_catalog_extension_names_keep_javascript_value_identity() {
    let catalog = |id: &str, name: Value| {
        ev(
            "subagent/catalog",
            json!({"version":0,"childId":id,"childCreatedAt":2,"mode":"one-shot","name":name}),
        )
    };
    let rows = vec![
        catalog("first", json!(1)),
        catalog("second", json!("1")),
        catalog("third", json!({"same":true})),
        catalog("fourth", json!({"same":true})),
    ];
    assert!(native(rows).is_ok());
    err(
        native(vec![
            catalog("first", json!(1)),
            catalog("second", json!(1)),
        ]),
        "duplicate catalog child name 1",
    );
}
