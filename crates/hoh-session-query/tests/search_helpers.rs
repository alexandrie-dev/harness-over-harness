use hoh_session::{SESSION_FORMAT_VERSION, SessionEvent, SessionHeader, SessionId, SessionSeq};
use hoh_session_query::*;
use serde_json::{Value, json};
use std::sync::Arc;

fn event(value: Value) -> Arc<SessionEvent> {
    Arc::new(SessionEvent::from_value(value))
}

fn header(id: &str, created_at: i64) -> SessionHeader {
    SessionHeader::new(SessionId::from(id), created_at)
}

fn user(seq: u64, text: &str, surface: Value) -> Arc<SessionEvent> {
    event(json!({
        "type":"user/message", "seq":seq, "time":seq as i64 + 10,
        "data":{"id":format!("user-{seq}"),"role":"user","content":[{"type":"text","text":text}],"source":{"kind":"user"}},
        "surfaceOp":surface
    }))
}

fn assert_code(error: &hoh_error::BoxError, code: SessionQueryErrorCode) {
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        code.as_str()
    );
}

#[test]
fn extracts_first_party_message_tool_todo_and_failure_detail() {
    let message = event(json!({
        "type":"user/message", "seq":0, "time":1,
        "data":{"id":"user-0","role":"user","content":[
            {"type":"text","text":" visible "},
            {"type":"reasoning","text":"thought"},
            {"type":"tool-call","name":"read","arguments":"{\"path\":\"a\"}"},
            {"type":"future-content","payload":"hidden"}
        ],"source":{"kind":"user"}}, "surfaceOp":"append"
    }));
    assert_eq!(
        extract_session_event_text(&message),
        "visible\nread\n{\"path\":\"a\"}"
    );
    let call = event(
        json!({"type":"tool/call","seq":1,"time":2,"data":{"name":"bash","arguments":"{\"cmd\":\"pwd\"}"}}),
    );
    assert_eq!(extract_session_event_text(&call), "bash\n{\"cmd\":\"pwd\"}");
    let result = event(
        json!({"type":"tool/result","seq":2,"time":3,"data":{"message":{"content":[{"type":"text","text":"failed"}]},"error":{"name":"Oops","code":"E_OOPS"}},"surfaceOp":"append"}),
    );
    assert_eq!(extract_session_event_text(&result), "failed\nOops\nE_OOPS");
    let todo = event(
        json!({"type":"todo/write","seq":3,"time":4,"data":{"todos":[{"status":"in_progress","content":"ship search"}]}}),
    );
    assert_eq!(
        extract_session_event_text(&todo),
        "in_progress\nship search"
    );
}

#[test]
fn extracts_meaningful_turn_outcomes_and_skips_structural_events() {
    let cases = [
        (
            json!({"kind":"error","error":{"message":"boom"}}),
            "error\nboom",
        ),
        (
            json!({"kind":"aborted","reason":{"kind":"user"}}),
            "aborted",
        ),
        (json!({"kind":"max-tokens"}), "max-tokens"),
        (json!({"kind":"interrupted"}), "interrupted"),
        (json!({"kind":"completed"}), ""),
    ];
    for (reason, expected) in cases {
        let event =
            event(json!({"type":"turn/end","seq":0,"time":1,"data":{"turn":1,"reason":reason}}));
        assert_eq!(extract_session_event_text(&event), expected);
    }
    let structural =
        event(json!({"type":"step/start","seq":0,"time":1,"data":{"turn":1,"step":1}}));
    assert_eq!(extract_session_event_text(&structural), "");
}

#[test]
fn javascript_whitespace_is_used_for_extraction() {
    let bom = user(0, "\u{feff}visible\u{feff}", json!("append"));
    assert_eq!(extract_session_event_text(&bom), "visible");
    let nel = user(0, "\u{85}visible\u{85}", json!("append"));
    assert_eq!(extract_session_event_text(&nel), "\u{85}visible\u{85}");
    let control = user(0, "\u{1c}visible\u{1c}", json!("append"));
    assert_eq!(extract_session_event_text(&control), "\u{1c}visible\u{1c}");
}

#[test]
fn classifies_every_event_and_omits_non_semantic_documents() {
    let events = vec![
        user(0, "Hello", json!("append")),
        event(json!({"type":"assistant/attempt","seq":1,"time":11,"data":{"turn":1,"step":1}})),
        event(json!({
            "type":"user/message","seq":2,"time":12,
            "data":{"id":"user-2","role":"user","content":[{"type":"text","text":"replacement"}],"source":{"kind":"user"}},
            "surfaceOp":{"op":"replace","startSeq":0,"endSeq":0},"sourceEventSeqs":[0]
        })),
        event(
            json!({"type":"turn/end","seq":3,"time":13,"data":{"turn":1,"reason":{"kind":"interrupted"}}}),
        ),
    ];
    let records = build_session_event_records(&SessionId::from("session"), &events).unwrap();
    assert_eq!(
        records
            .iter()
            .map(|record| record.surface)
            .collect::<Vec<_>>(),
        vec![
            SessionEventSurface::Shadowed,
            SessionEventSurface::LogOnly,
            SessionEventSurface::Current,
            SessionEventSurface::LogOnly
        ]
    );
    let documents =
        build_session_event_search_documents(&SessionId::from("session"), &events).unwrap();
    assert_eq!(
        documents
            .iter()
            .map(|document| document.record.seq.get())
            .collect::<Vec<_>>(),
        vec![0, 2, 3]
    );
}

#[test]
fn applies_every_session_clause_with_or_values() {
    let parent = SessionId::from("parent");
    let mut a = header("a", 10);
    a.cwd = Some("/a".into());
    a.parent_session = Some(parent.clone());
    let records = vec![
        SessionRecord {
            header: a,
            live: true,
            persisted: false,
            format_status: None,
        },
        SessionRecord {
            header: header("b", 20),
            live: false,
            persisted: true,
            format_status: None,
        },
    ];
    let filtered = filter_session_results(
        &records,
        &[
            SessionResultFilter::Id(vec![SessionId::from("a"), SessionId::from("x")]),
            SessionResultFilter::Cwd(vec![Some("/a".into()), None]),
            SessionResultFilter::CreatedAt(SessionResultRange {
                from: Some(5.0),
                to: Some(15.0),
            }),
            SessionResultFilter::Parent(vec![Some(parent), None]),
            SessionResultFilter::Availability(vec![SessionAvailability::Live]),
        ],
    )
    .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].header.id, SessionId::from("a"));
}

#[test]
fn applies_metadata_and_safe_literal_text_clauses() {
    let documents = vec![SessionEventSearchDocument {
        record: SessionEventRecord {
            session_id: SessionId::from("s"),
            seq: SessionSeq::new(0).unwrap(),
            r#type: "user/message".into(),
            time: 10,
            surface: SessionEventSurface::Shadowed,
        },
        text: "Hello\n(AI)+".into(),
    }];
    let result = filter_session_event_documents(
        &documents,
        &[
            SessionEventResultFilter::Seq(SessionResultRange {
                from: Some(0.0),
                to: Some(1.0),
            }),
            SessionEventResultFilter::Time(SessionResultRange {
                from: Some(9.0),
                to: Some(11.0),
            }),
            SessionEventResultFilter::Type(vec!["user/message".into()]),
            SessionEventResultFilter::Surface(vec![SessionEventSurface::Shadowed]),
            SessionEventResultFilter::Text("hello   (ai)+".into()),
        ],
    )
    .unwrap();
    assert_eq!(result.len(), 1);
    assert!(
        compile_session_text_filter("CAFÉ")
            .unwrap()
            .is_match("café")
    );
}

#[test]
fn literal_filter_uses_javascript_whitespace_set() {
    assert!(
        compile_session_text_filter("a\u{feff}b")
            .unwrap()
            .is_match("A\nb")
    );
    assert!(
        !compile_session_text_filter("a b")
            .unwrap()
            .is_match("a\u{85}b")
    );
    assert!(
        !compile_session_text_filter("a b")
            .unwrap()
            .is_match("a\u{1c}b")
    );
}

#[test]
fn rejects_empty_text_and_malformed_ranges() {
    let empty = compile_session_text_filter(" \n ").unwrap_err();
    assert_code(&empty, SessionQueryErrorCode::InvalidFilter);
    for range in [
        SessionResultRange {
            from: Some(f64::NAN),
            to: None,
        },
        SessionResultRange {
            from: None,
            to: Some(f64::INFINITY),
        },
        SessionResultRange {
            from: Some(2.0),
            to: Some(1.0),
        },
    ] {
        let error = materialize_session_result_filters(&[SessionResultFilter::CreatedAt(range)])
            .unwrap_err();
        assert_code(&error, SessionQueryErrorCode::InvalidFilter);
    }
}

#[test]
fn filters_are_owned() {
    let mut values = vec![SessionId::from("a")];
    let owned =
        materialize_session_result_filters(&[SessionResultFilter::Id(values.clone())]).unwrap();
    values[0] = SessionId::from("b");
    assert_eq!(
        owned,
        vec![SessionResultFilter::Id(vec![SessionId::from("a")])]
    );

    let mut event_values = vec!["user/message".to_owned()];
    let event_owned = materialize_session_event_result_filters(&[
        SessionEventResultFilter::Type(event_values.clone()),
        SessionEventResultFilter::Text("later validation".into()),
    ])
    .unwrap();
    event_values[0] = "tool/result".into();
    assert_eq!(
        event_owned,
        vec![
            SessionEventResultFilter::Type(vec!["user/message".into()]),
            SessionEventResultFilter::Text("later validation".into()),
        ]
    );
}

#[test]
fn header_compatibility_uses_default_delegation_depth() {
    let mut a = header("a", 10);
    let mut b = a.clone();
    a.delegation_depth = None;
    b.delegation_depth = Some(0);
    assert_session_headers_compatible(&a, &b).unwrap();
    b.cwd = Some("/other".into());
    let error = assert_session_headers_compatible(&a, &b).unwrap_err();
    assert_code(&error, SessionQueryErrorCode::SourceConflict);
    assert_eq!(
        error.to_string(),
        "session source headers conflict for session \"a\""
    );
}

#[test]
fn public_constants_match_source() {
    assert_eq!(SESSION_QUERY_READ_WINDOW_MAX, 50);
    assert_eq!(SESSION_QUERY_DEFAULT_PERSISTED_INSPECT_CONCURRENCY, 4);
    assert_eq!(SESSION_QUERY_DEFAULT_PREPARED_SESSION_CACHE_SIZE, 5);
    assert_eq!(SESSION_FORMAT_VERSION, 4);
}
