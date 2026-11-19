use hoh_protocol::{
    SessionAssistantStreamFrame, SessionAssistantStreamNotification, validate_notification,
};
use serde_json::{Value, json};

fn validates(value: Value) -> SessionAssistantStreamNotification {
    validate_notification("session.assistant-stream", &value).expect("valid stream frame");
    serde_json::from_value(value).expect("typed stream frame")
}

#[test]
fn validates_start_text_delta_and_terminal_frames() {
    let start = validates(json!({
        "sessionId":"s", "frame":{
            "type":"start", "attemptId":"s:1", "revision":1, "turn":2, "step":3
        }
    }));
    assert!(matches!(
        start.frame,
        SessionAssistantStreamFrame::Start { .. }
    ));

    let chunk = validates(json!({
        "sessionId":"s", "frame":{
            "type":"chunk", "attemptId":"s:1", "revision":2, "index":0,
            "time":1700000000000_i64, "chunk":{"type":"text-delta", "text":"при"}
        }
    }));
    assert_eq!(
        serde_json::to_value(chunk).unwrap(),
        json!({
            "sessionId":"s", "frame":{
                "type":"chunk", "attemptId":"s:1", "revision":2, "index":0,
                "time":1700000000000_i64, "chunk":{"type":"text-delta", "text":"при"}
            }
        })
    );

    for outcome in ["committed", "abandoned"] {
        validates(json!({
            "sessionId":"s", "frame":{
                "type":"end", "attemptId":"s:1", "revision":3, "index":1,
                "outcome":outcome
            }
        }));
    }
}

#[test]
fn rejects_incomplete_or_non_text_frames() {
    for invalid in [
        json!({"sessionId":"s", "frame":{"type":"start", "attemptId":"s:1", "revision":1, "turn":1}}),
        json!({"sessionId":"s", "frame":{"type":"chunk", "attemptId":"s:1", "revision":2, "index":0, "time":1, "chunk":{"type":"reasoning-delta", "text":"secret"}}}),
        json!({"sessionId":"s", "frame":{"type":"end", "attemptId":"s:1", "revision":3, "index":1, "outcome":"cancelled"}}),
    ] {
        assert!(
            validate_notification("session.assistant-stream", &invalid).is_err(),
            "accepted {invalid}"
        );
    }
}
