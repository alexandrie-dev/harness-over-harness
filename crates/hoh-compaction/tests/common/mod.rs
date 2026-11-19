#![allow(dead_code)]

use hoh_session::{Session, SessionEvent, SessionSeq, SurfaceIntent};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn session(id: &str) -> Session {
    Session::create(id.into(), None, None, None, None).expect("session")
}

pub fn append_user(session: &Session, text: &str) -> Arc<SessionEvent> {
    session
        .append(
            "user/message",
            json!({
                "content": [{"type": "text", "text": text}],
                "source": {"kind": "user"},
                "role": "user",
                "id": format!("user-{text}"),
            }),
            Some(SurfaceIntent::append()),
        )
        .expect("append user")
}

pub fn append_assistant_calls(session: &Session, ids: &[&str]) -> Arc<SessionEvent> {
    let content = ids
        .iter()
        .map(|id| json!({"type":"tool-call","id":id,"name":id,"arguments":"{}"}))
        .collect::<Vec<_>>();
    session
        .append(
            "assistant/message",
            json!({
                "stream": [],
                "turn": 1,
                "step": 1,
                "message": {
                    "role": "assistant",
                    "content": content,
                    "source": {"kind":"model","provider":"mock","model":"mock"},
                    "id": format!("assistant-{}", ids.join("-")),
                },
            }),
            Some(SurfaceIntent::append()),
        )
        .expect("append assistant")
}

pub fn append_tool_result(session: &Session, id: &str) -> Arc<SessionEvent> {
    session
        .append(
            "tool/result",
            json!({
                "turn": 1,
                "step": 1,
                "message": {
                    "role": "tool",
                    "source": {"kind":"tool","callId":id},
                    "toolCallId": id,
                    "content": [],
                    "isError": false,
                    "id": format!("tool-{id}"),
                },
            }),
            Some(SurfaceIntent::append()),
        )
        .expect("append tool result")
}

pub fn seq(value: u64) -> SessionSeq {
    SessionSeq::new(value).expect("safe seq")
}

pub fn event_data(event: &SessionEvent) -> Value {
    event.data().clone()
}
