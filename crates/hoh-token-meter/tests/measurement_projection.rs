use hoh_session::{SessionEvent, SessionHeader, SessionLogOffset};
use hoh_token_meter::{MeasurementState, advance_measurement, measurement_projection_definition};
use serde_json::{Value, json};

fn event(seq: u64, kind: &str, data: Value) -> SessionEvent {
    SessionEvent::from_value(json!({"seq":seq,"time":seq,"type":kind,"data":data}))
}

#[test]
fn keeps_stream_decode_errors_local_with_the_source_error_text() {
    let mut state = MeasurementState::default();
    state =
        advance_measurement(&state, &event(0, "step/start", json!({"turn":1,"step":1}))).unwrap();
    state = advance_measurement(
        &state,
        &event(1, "request/header", json!({"reason":"test","header":{}})),
    )
    .unwrap();
    let error = advance_measurement(
        &state,
        &event(
            2,
            "assistant/message",
            json!({
                "turn":1,"step":1,"usage":{"inputTokens":1,"outputTokens":1},
                "stream":[{"type":"unsupported"}],
                "message":{"role":"assistant","content":[],"source":{"kind":"model","provider":"p","model":"m"},"id":"a"}
            }),
        ),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(error, "unreachable variant in assembleAssistantStream");
}

#[test]
fn current_and_successful_call_headers_remain_distinct_references() {
    let mut state = MeasurementState::default();
    for candidate in [
        event(0, "step/start", json!({"turn":1,"step":1})),
        event(
            1,
            "request/header",
            json!({"reason":"first","header":{"config":{"provider":"p","model":"a"}}}),
        ),
        event(
            2,
            "assistant/message",
            json!({
                "turn":1,"step":1,"usage":{"inputTokens":10,"outputTokens":2},"stream":[],
                "message":{"role":"assistant","content":[],"source":{"kind":"model","provider":"p","model":"a"},"id":"a"}
            }),
        ),
        event(3, "step/end", json!({"turn":1,"step":1})),
        event(
            4,
            "request/header",
            json!({"reason":"next","header":{"config":{"provider":"p","model":"b"}}}),
        ),
    ] {
        state = advance_measurement(&state, &candidate).unwrap();
    }
    assert_eq!(state.header_seq.unwrap().get(), 4);
    assert_eq!(state.anchor.as_ref().unwrap().header_seq.unwrap().get(), 1);
    let definition = measurement_projection_definition();
    let encoded = serde_json::to_value(&state).unwrap();
    assert_eq!(
        (definition.state_schema)(Some(&encoded)).unwrap().as_ref(),
        &state
    );
}

#[test]
fn checkpoint_state_size_is_independent_of_unrelated_message_count() {
    let definition = measurement_projection_definition();
    let header = SessionHeader::new("fixed".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    let before = serde_json::to_value(state.as_ref()).unwrap();
    for seq in 0..100 {
        state = (definition.apply)(
            &state,
            &event(
                seq,
                "user/message",
                json!({
                    "role":"user","content":[{"type":"text","text":"x"}],
                    "source":{"kind":"user"},"id":format!("u{seq}")
                }),
            ),
        )
        .unwrap();
    }
    assert_eq!(serde_json::to_value(state.as_ref()).unwrap(), before);
}
