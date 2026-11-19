use hoh_session::{SESSION_FORMAT_VERSION, SessionError, SessionEvent, SessionHeader};
use hoh_session_persistence::*;
use serde_json::{Value, json};
use std::{error::Error, sync::Arc};

fn meta(id: &str) -> SessionHeader {
    SessionHeader::new(id.into(), 1000)
}
fn event(value: Value) -> SessionEvent {
    SessionEvent::from_value(value)
}
fn start(seq: u64) -> SessionEvent {
    event(json!({"type":"turn/start","seq":seq,"time":1,"data":{"turn":1}}))
}
fn user(seq: u64) -> SessionEvent {
    event(
        json!({"type":"user/message","seq":seq,"time":seq+1,"data":{"id":"stored-user","role":"user","content":[{"type":"text","text":"hi"}],"source":{"kind":"user"}},"surfaceOp":"append"}),
    )
}
fn location() -> Arc<SessionLocation> {
    Arc::new(SessionLocation {
        kind: "jsonl".into(),
        path: "/store/session.jsonl".into(),
    })
}

#[test]
fn accepts_a_bound_header_and_names_both_ids_on_a_mismatch() {
    let m = meta("bound");
    assert_stored_id(&m.id, &m).unwrap();
    assert_eq!(
        assert_stored_id(&"requested".into(), &m)
            .unwrap_err()
            .to_string(),
        "stored session identity mismatch: requested \"requested\", header contains \"bound\""
    );
}
#[test]
fn accepts_the_current_format_version() {
    assert_version(&meta("current"), None).unwrap();
}
#[test]
fn refuses_a_newer_version_with_the_upgrade_direction_and_the_artifact_location() {
    let mut m = meta("newer");
    m.version += 42;
    let location = location();
    let error = assert_version(&m, Some(&location)).unwrap_err();
    let error = error
        .downcast_ref::<SessionFormatUnsupportedError>()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("written by a newer harness — upgrade the harness to open it")
    );
    assert!(
        error
            .to_string()
            .ends_with("(raw log: /store/session.jsonl)")
    );
    assert!(Arc::ptr_eq(error.location.as_ref().unwrap(), &location));
}
#[test]
fn refuses_an_older_version_without_a_location_suffix_when_the_backend_has_no_artifact() {
    let mut m = meta("older");
    m.version -= 1;
    let error = assert_version(&m, None).unwrap_err();
    let error = error
        .downcast_ref::<SessionFormatUnsupportedError>()
        .unwrap();
    assert_eq!(
        error.to_string(),
        session_format_version_refusal("older", SESSION_FORMAT_VERSION - 1)
    );
    assert!(
        error
            .to_string()
            .contains("this build ships no upgrade path for it")
    );
    assert!(!error.to_string().contains("raw log"));
    assert!(error.location.is_none());
}
#[test]
fn states_the_direction_for_newer_and_older_stored_versions() {
    let version = SESSION_FORMAT_VERSION;
    assert_eq!(
        session_format_version_refusal("s", version + 1),
        format!(
            "session \"s\" uses log format v{}, but this harness reads only v{version}: the log was written by a newer harness — upgrade the harness to open it",
            version + 1
        )
    );
    assert_eq!(
        session_format_version_refusal("s", version - 1),
        format!(
            "session \"s\" uses log format v{}, older than the supported v{version}, and this build ships no upgrade path for it",
            version - 1
        )
    );
    assert_eq!(
        session_format_version_refusal("s", u64::MAX),
        format!(
            "session \"s\" uses log format v18446744073709552000, but this harness reads only v{version}: the log was written by a newer harness — upgrade the harness to open it"
        )
    );
}
#[test]
fn adopts_and_freezes_the_events_in_place_returning_the_same_array() {
    let mut events = vec![start(0), user(1)];
    let pointer = events.as_ptr();
    let validated = validate_stored_events(&meta("adopted"), &mut events, None).unwrap();
    assert_eq!(validated.as_ptr(), pointer);
    assert_eq!(validated[1], user(1));
    // SessionEvent has no mutable access to its graph, including before Arc sharing.
    let shared = Arc::new(validated[1].clone());
    assert_eq!(shared.data()["id"], "stored-user");
}
#[test]
fn adopts_a_merge_extended_turn_end_reason_outside_the_closed_built_in_set() {
    let mut events = vec![
        start(0),
        event(
            json!({"type":"turn/end","seq":1,"time":2,"data":{"turn":1,"reason":{"kind":"plugin/custom-outcome"}}}),
        ),
    ];
    validate_stored_events(&meta("extended-reason"), &mut events, None).unwrap();
}
#[test]
fn refuses_an_unknown_event_type_before_adopting_anything() {
    let m = meta("unknown-type");
    let mut events = vec![event(
        json!({"type":"mystery/event","seq":0,"time":1,"data":{}}),
    )];
    let error = validate_stored_events(&m, &mut events, None).unwrap_err();
    assert!(error.is::<SessionFormatUnsupportedError>());
    assert_eq!(
        error.to_string(),
        "session \"unknown-type\" contains event type \"mystery/event\" (seq 0) unknown to this harness and not marked ignorable; refusing to interpret the log — it was likely written by a newer harness"
    );
    let location = location();
    let error = validate_stored_events(&m, &mut events, Some(&location)).unwrap_err();
    let error = error
        .downcast_ref::<SessionFormatUnsupportedError>()
        .unwrap();
    assert!(
        error
            .to_string()
            .ends_with("(raw log: /store/session.jsonl)")
    );
    assert!(Arc::ptr_eq(error.location.as_ref().unwrap(), &location));
    // A malformed earlier known record must not win over the vocabulary pre-scan.
    events.insert(
        0,
        event(json!({"type":"user/message","seq":0,"time":1,"data":{}})),
    );
    assert!(
        validate_stored_events(&m, &mut events, None)
            .unwrap_err()
            .is::<SessionFormatUnsupportedError>()
    );
}
#[test]
fn retains_an_unknown_event_type_its_writer_marked_ignorable() {
    let mut events = vec![event(
        json!({"type":"foreign/telemetry","seq":0,"time":1,"data":{},"ignorable":true}),
    )];
    validate_stored_events(&meta("ignorable-unknown"), &mut events, None).unwrap();
    assert_eq!(events[0].event_type(), "foreign/telemetry");
    assert!(events[0].ignorable());
}
#[test]
fn refuses_the_retired_request_header_fallback_reason_while_accepting_current_headers() {
    let m = meta("retired-reason");
    let mut retired = vec![event(
        json!({"type":"request/header","seq":0,"time":1,"data":{"header":{"config":{"provider":"mock","model":"legacy"}},"reason":"fallback"}}),
    )];
    let error = validate_stored_events(&m, &mut retired, None).unwrap_err();
    assert!(error.is::<SessionFormatUnsupportedError>());
    assert!(error.to_string().contains("session \"retired-reason\" contains a request/header event (seq 0) with the unsupported legacy reason \"fallback\""));
    let mut current = vec![event(
        json!({"type":"request/header","seq":0,"time":1,"data":{"header":{"config":{"provider":"mock","model":"current"}},"reason":"initial"}}),
    )];
    validate_stored_events(&m, &mut current, None).unwrap();
}
#[test]
fn wraps_a_record_validation_failure_as_corruption_with_its_cause() {
    let mut events = vec![event(
        json!({"type":"user/message","seq":0,"time":1,"data":{"role":"user","content":[],"source":{"kind":"user"}},"surfaceOp":"append"}),
    )];
    let error = validate_stored_events(&meta("damaged"), &mut events, None).unwrap_err();
    let error = error
        .downcast_ref::<SessionPersistenceCorruptionError>()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("stored session \"damaged\" failed validation: Error:")
    );
    assert!(error.to_string().contains("lacks an identified message"));
    assert!(error.source().unwrap().is::<SessionError>());
}
#[test]
fn returns_a_detached_lossless_json_snapshot_of_the_batch() {
    let mut original = vec![start(0)];
    let batch = materialize_append_batch(&original).unwrap();
    assert_ne!(original.as_ptr(), batch.as_ptr());
    assert_eq!(original, batch);
    original[0] = event(json!({"type":"turn/start","seq":0,"time":1,"data":{"turn":99}}));
    assert_eq!(batch[0].data()["turn"], 1);
}
#[test]
fn rejects_every_non_json_value_not_only_bigint() {
    // Negative zero is the non-lossless JSON number representable in Value;
    // JS BigInt/undefined/functions/symbols/Map/cycles are excluded by the type.
    let events = vec![event(
        json!({"type":"turn/start","seq":0,"time":1,"data":{"turn":1,"extra":-0.0}}),
    )];
    let error = materialize_append_batch(&events).unwrap_err();
    let error = error.downcast_ref::<SessionError>().unwrap();
    assert_eq!(error.name(), "TypeError");
    assert_eq!(
        error.to_string(),
        "session event batch is not losslessly JSON-serializable because it contains non-JSON-serializable data"
    );
}
#[test]
fn accepts_a_batch_continuing_exactly_at_the_cursor() {
    let events = vec![
        start(6),
        event(
            json!({"type":"turn/end","seq":7,"time":2,"data":{"turn":2,"reason":{"kind":"completed"}}}),
        ),
    ];
    assert_contiguous(&"ok".into(), &events, 6).unwrap();
    assert_contiguous(&"ok".into(), &[], 6).unwrap();
}
#[test]
fn names_the_expected_seq_for_a_stale_batch_and_for_a_mid_batch_gap() {
    assert_eq!(
        assert_contiguous(&"stale".into(), &[start(0)], 6)
            .unwrap_err()
            .to_string(),
        "append seq mismatch for \"stale\": expected 6 at index 0, got 0"
    );
    assert_eq!(
        assert_contiguous(&"gapped".into(), &[start(6), start(8)], 6)
            .unwrap_err()
            .to_string(),
        "append seq mismatch for \"gapped\": expected 7 at index 1, got 8"
    );
}
#[test]
fn carries_stable_names_messages_and_the_session_id() {
    let id: hoh_session::SessionId = "errored".into();
    let not_found = SessionPersistenceNotFoundError::new(id.clone());
    assert_eq!(not_found.name(), "SessionPersistenceNotFoundError");
    assert_eq!(not_found.session_id, id);
    assert_eq!(not_found.to_string(), "session \"errored\" not found");
    let exists = SessionAlreadyExistsError::new(id.clone());
    assert_eq!(exists.name(), "SessionAlreadyExistsError");
    assert_eq!(exists.to_string(), "session \"errored\" already exists");
    let owned = SessionAlreadyOwnedError::new(id.clone());
    assert_eq!(owned.name(), "SessionAlreadyOwnedError");
    assert_eq!(
        owned.to_string(),
        "session \"errored\" is already owned by an active write handle"
    );
    let read_only = SessionReadOnlyError::new(id.clone(), "append");
    assert_eq!(read_only.name(), "SessionReadOnlyError");
    assert_eq!(
        read_only.to_string(),
        "session \"errored\": append is not available on a read handle"
    );
    let lost = SessionOwnershipLostError::new(id.clone());
    assert_eq!(lost.name(), "SessionOwnershipLostError");
    assert_eq!(
        lost.to_string(),
        "session \"errored\": write ownership was lost; close this handle and reopen"
    );
    let closed = SessionHandleClosedError::new(id, "flush");
    assert_eq!(closed.name(), "SessionHandleClosedError");
    assert_eq!(
        closed.to_string(),
        "session \"errored\": flush on a closed handle"
    );
    let cause = hoh_error::SharedError::new(SessionError::Message("detail".into()));
    let corruption = SessionPersistenceCorruptionError::new("bad log", cause.clone().into());
    assert_eq!(corruption.name(), "SessionPersistenceCorruptionError");
    assert_eq!(corruption.source().unwrap().to_string(), "detail");
    assert!(
        hoh_error::SharedError::of(corruption.source().unwrap())
            .unwrap()
            .ptr_eq(&cause)
    );
}
#[test]
fn materializes_detached_headers_and_checks_created_at_and_lossless_metadata() {
    let mut m = meta("header");
    m.extra.insert("extension".into(), json!({"nested":[1,2]}));
    let materialized = materialize_create_header(&m).unwrap();
    m.extra["extension"] = Value::Null;
    assert_eq!(materialized.extra["extension"], json!({"nested":[1,2]}));
    for bad in [-1, hoh_session::MAX_SAFE_INTEGER as i64 + 1] {
        m.created_at = bad;
        assert_eq!(
            materialize_create_header(&m).unwrap_err().to_string(),
            "session metadata createdAt must be a non-negative safe integer"
        );
    }
    m.created_at = 1;
    m.extra.insert("bad".into(), json!(-0.0));
    assert_eq!(
        materialize_create_header(&m).unwrap_err().to_string(),
        "session metadata must be losslessly JSON-serializable"
    );
}
#[test]
fn create_header_validates_the_serialized_created_at_after_extra_is_merged() {
    // `extra` is flattened over the typed field, so the record that is stored
    // carries createdAt -1 even though the typed field is valid.
    let mut m = meta("header-extra");
    m.created_at = 1;
    m.extra.insert("createdAt".into(), json!(-1));
    assert_eq!(
        materialize_create_header(&m).unwrap_err().to_string(),
        "session metadata createdAt must be a non-negative safe integer"
    );
    let error = materialize_create_header(&m).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<SessionError>(),
        Some(SessionError::TypeError(_))
    ));
    // The override can also repair an invalid typed field.
    m.created_at = -1;
    m.extra.insert("createdAt".into(), json!(7));
    assert!(materialize_create_header(&m).is_ok());
}
#[test]
fn revision_and_observation_preserve_wire_shape_and_instance_identity() {
    let first = SessionPersistenceIdentity::new();
    assert_eq!(first, first.clone());
    assert_ne!(first, SessionPersistenceIdentity::new());
    let snapshot = SessionPersistenceSnapshot {
        header: meta("shape"),
        format_status: Some(SessionFormatStatus::MigrationRequired),
        revision: "opaque".into(),
        event_count: None,
        size_bytes: None,
        location: None,
    };
    let value = serde_json::to_value(snapshot).unwrap();
    assert_eq!(value["revision"], "opaque");
    assert_eq!(value["formatStatus"], "migration-required");
    assert!(value.get("eventCount").is_none());
    assert!(value.get("sizeBytes").is_none());
    assert!(value.get("location").is_none());
    let slice = SessionHandleReadResult {
        event_state: hoh_session::SessionSeedEventState::SharedFrozen,
        events: vec![],
    };
    assert_eq!(
        serde_json::to_value(slice).unwrap(),
        json!({"eventState":"shared-frozen","events":[]})
    );
}

#[test]
fn fix2_create_header_returns_the_merged_snapshot() {
    for created in [json!(7), json!(7.0)] {
        let mut m = meta("header-extra");
        m.created_at = -1;
        m.extra.insert("createdAt".into(), created);
        let materialized = materialize_create_header(&m).unwrap();
        assert_eq!(materialized.created_at, 7);
        assert!(!materialized.extra.contains_key("createdAt"));
        assert_eq!(serde_json::to_value(&materialized).unwrap()["createdAt"], 7);
        assert_eq!(m.created_at, -1);
    }
}
