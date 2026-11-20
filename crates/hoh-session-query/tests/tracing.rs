use hoh_session::{SessionEvent, SessionHeader, SessionId, SessionSeq};
use hoh_session_query::*;
use serde_json::{Value, json};
use std::sync::Arc;

fn event(value: Value) -> Arc<SessionEvent> {
    Arc::new(SessionEvent::from_value(value))
}

fn user(seq: u64, text: &str, surface: Value, sources: Option<Vec<u64>>) -> Arc<SessionEvent> {
    let mut value = json!({
        "type":"user/message", "seq":seq, "time":seq as i64 + 1,
        "data":{"id":format!("user-{seq}"),"role":"user","content":[{"type":"text","text":text}],"source":{"kind":"user"}},
        "surfaceOp":surface
    });
    if let Some(sources) = sources {
        value["sourceEventSeqs"] = json!(sources);
    }
    event(value)
}

fn record(id: &str, created: i64, parent: Option<&str>) -> SessionRecord {
    let mut header = SessionHeader::new(SessionId::from(id), created);
    header.parent_session = parent.map(SessionId::from);
    SessionRecord {
        header,
        live: true,
        persisted: false,
        format_status: None,
    }
}

fn assert_code(error: &hoh_error::BoxError, code: SessionQueryErrorCode) {
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        code.as_str()
    );
}

#[test]
fn returns_complete_ancestry_and_deterministic_descendants() {
    let records = vec![
        record("root", 0, None),
        record("parent", 1, Some("root")),
        record("target", 2, Some("parent")),
        record("b", 4, Some("target")),
        record("a", 4, Some("target")),
        record("older", 3, Some("target")),
        record("grandchild", 5, Some("a")),
    ];
    let trace = trace_session(&records, &SessionId::from("target")).unwrap();
    assert_eq!(
        trace
            .ancestors
            .iter()
            .map(|r| r.header.id.as_ref())
            .collect::<Vec<_>>(),
        vec!["parent", "root"]
    );
    assert_eq!(
        trace
            .descendants
            .iter()
            .map(|n| n.session.header.id.as_ref())
            .collect::<Vec<_>>(),
        vec!["older", "a", "b"]
    );
    assert_eq!(
        trace.descendants[1].descendants[0].session.header.id,
        SessionId::from("grandchild")
    );
    match trace.completion {
        SessionLineageCompletion::Complete { root } => {
            assert_eq!(root.header.id, SessionId::from("root"))
        }
        SessionLineageCompletion::Incomplete { .. } => panic!("expected complete lineage"),
    }
}

#[test]
fn represents_root_and_unresolved_parent_explicitly() {
    let root = trace_session(&[record("root", 1, None)], &SessionId::from("root")).unwrap();
    assert!(matches!(
        root.completion,
        SessionLineageCompletion::Complete { .. }
    ));
    let partial = trace_session(
        &[record("partial", 2, Some("outside"))],
        &SessionId::from("partial"),
    )
    .unwrap();
    assert!(
        matches!(partial.completion, SessionLineageCompletion::Incomplete { unresolved_parent_id } if unresolved_parent_id.as_ref() == "outside")
    );
}

#[test]
fn rejects_target_connected_cycles_and_missing_targets() {
    let records = vec![record("a", 1, Some("b")), record("b", 2, Some("a"))];
    let cycle = trace_session(&records, &SessionId::from("a")).unwrap_err();
    assert_code(&cycle, SessionQueryErrorCode::InvalidLineage);
    let missing = trace_session(&records, &SessionId::from("missing")).unwrap_err();
    assert_code(&missing, SessionQueryErrorCode::SessionNotFound);
}

#[test]
fn constructs_deep_descendants_iteratively() {
    let mut records = vec![record("deep-0", 0, None)];
    for depth in 1..3_000 {
        records.push(record(
            &format!("deep-{depth}"),
            depth,
            Some(&format!("deep-{}", depth - 1)),
        ));
    }
    let trace = trace_session(&records, &SessionId::from("deep-0")).unwrap();
    let mut node = &trace.descendants[0];
    for depth in 1..3_000 {
        if depth == 2_999 {
            assert_eq!(node.session.header.id, SessionId::from("deep-2999"));
        } else {
            node = &node.descendants[0];
        }
    }
    drop(trace);
}

#[test]
fn descendant_ties_use_locale_collation_stably() {
    let records = vec![
        record("root", 0, None),
        record("z", 1, Some("root")),
        record("é", 1, Some("root")),
        record("a", 1, Some("root")),
        record("A", 1, Some("root")),
        record("-a", 1, Some("root")),
    ];
    let first = trace_session(&records, &SessionId::from("root")).unwrap();
    let second = trace_session(&records, &SessionId::from("root")).unwrap();
    let ids = |trace: &SessionLineageTrace| {
        trace
            .descendants
            .iter()
            .map(|n| n.session.header.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&first), ids(&second));
    assert!(
        ids(&first)
            .iter()
            .position(|id| id.as_ref() == "é")
            .unwrap()
            < ids(&first)
                .iter()
                .position(|id| id.as_ref() == "z")
                .unwrap()
    );
}

fn trace_events() -> Vec<Arc<SessionEvent>> {
    vec![
        event(json!({"type":"turn/start","seq":0,"time":1,"data":{"turn":1}})),
        event(json!({"type":"step/start","seq":1,"time":2,"data":{"turn":1,"step":1}})),
        event(json!({"type":"assistant/attempt","seq":2,"time":3,"data":{"turn":1,"step":1}})),
        user(3, "original", json!("append"), Some(vec![2])),
        user(
            4,
            "summary one",
            json!({"op":"replace","startSeq":3,"endSeq":3}),
            Some(vec![3, 2]),
        ),
        user(5, "context", json!("append"), None),
        event(json!({"type":"step/end","seq":6,"time":7,"data":{"turn":1,"step":1}})),
        event(json!({"type":"step/start","seq":7,"time":8,"data":{"turn":1,"step":2}})),
        user(
            8,
            "summary two",
            json!({"op":"replace","startSeq":4,"endSeq":4}),
            Some(vec![2, 4]),
        ),
    ]
}

#[test]
fn returns_replacement_and_source_links_in_contract_order() {
    let events = trace_events();
    let original = trace_event(
        &SessionId::from("trace"),
        &events,
        SessionSeq::new(3).unwrap(),
    )
    .unwrap();
    assert_eq!(original.replaced_by.unwrap().get(), 4);
    assert_eq!(
        original
            .replacement_chain
            .iter()
            .map(|s| s.get())
            .collect::<Vec<_>>(),
        vec![4, 8]
    );
    assert_eq!(
        original
            .source_event_seqs
            .iter()
            .map(|s| s.get())
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(
        original
            .derived_event_seqs
            .iter()
            .map(|s| s.get())
            .collect::<Vec<_>>(),
        vec![4]
    );
    let summary = trace_event(
        &SessionId::from("trace"),
        &events,
        SessionSeq::new(4).unwrap(),
    )
    .unwrap();
    assert_eq!(
        summary
            .replaced_event_seqs
            .iter()
            .map(|s| s.get())
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(
        summary
            .source_event_seqs
            .iter()
            .map(|s| s.get())
            .collect::<Vec<_>>(),
        vec![3, 2]
    );
    assert_eq!(
        summary
            .derived_event_seqs
            .iter()
            .map(|s| s.get())
            .collect::<Vec<_>>(),
        vec![8]
    );
}

#[test]
fn trace_results_are_fresh_owned_arrays() {
    let events = trace_events();
    let mut first = trace_event(
        &SessionId::from("trace"),
        &events,
        SessionSeq::new(3).unwrap(),
    )
    .unwrap();
    first.replacement_chain.clear();
    let second = trace_event(
        &SessionId::from("trace"),
        &events,
        SessionSeq::new(3).unwrap(),
    )
    .unwrap();
    assert_eq!(second.replacement_chain.len(), 2);
}

#[test]
fn checks_target_existence_before_surface_analysis() {
    let malformed = vec![user(
        0,
        "bad",
        json!({"op":"replace","startSeq":9,"endSeq":9}),
        None,
    )];
    let error = trace_event(
        &SessionId::from("s"),
        &malformed,
        SessionSeq::new(7).unwrap(),
    )
    .unwrap_err();
    assert_code(&error, SessionQueryErrorCode::EventNotFound);
}

#[test]
fn rejects_invalid_surface_for_existing_target() {
    let cases = vec![
        vec![event(
            json!({"type":"turn/start","seq":0,"time":1,"data":{"turn":1},"sourceEventSeqs":[0]}),
        )],
        vec![event(
            json!({"type":"user/message","seq":0,"time":1,"data":{"id":"u","role":"user","content":[],"source":{"kind":"user"}},"surfaceOp":"append","sourceEventSeqs":"invalid"}),
        )],
        vec![user(0, "empty", json!("append"), Some(vec![]))],
        vec![
            user(0, "duplicate", json!("append"), None),
            user(1, "duplicate", json!("append"), Some(vec![0, 0])),
        ],
        vec![event(
            json!({"type":"user/message","seq":0,"time":1,"data":{"id":"u","role":"user","content":[],"source":{"kind":"user"}},"surfaceOp":"append","sourceEventSeqs":[-1]}),
        )],
        vec![
            user(0, "future", json!("append"), Some(vec![1])),
            user(1, "later", json!("append"), None),
        ],
        vec![
            user(0, "base", json!("append"), None),
            user(
                1,
                "replacement",
                json!({"op":"replace","startSeq":0,"endSeq":0}),
                None,
            ),
        ],
        vec![
            event(json!({"type":"assistant/attempt","seq":0,"time":1,"data":{"turn":1,"step":1}})),
            user(1, "base", json!("append"), None),
            user(
                2,
                "replacement",
                json!({"op":"replace","startSeq":1,"endSeq":1}),
                Some(vec![0]),
            ),
        ],
        vec![user(
            0,
            "bad range",
            json!({"op":"replace","startSeq":9,"endSeq":9}),
            None,
        )],
    ];
    for malformed in cases {
        let error = trace_event(
            &SessionId::from("s"),
            &malformed,
            SessionSeq::new(0).unwrap(),
        )
        .unwrap_err();
        assert_code(&error, SessionQueryErrorCode::InvalidSurface);
    }
}
