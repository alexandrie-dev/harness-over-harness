#![allow(dead_code)]
use hoh_session::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
pub fn session() -> Session {
    Session::create("test".into(), None, None, None, None).unwrap()
}
pub fn seq(n: u64) -> SessionSeq {
    SessionSeq::new(n).unwrap()
}
pub fn offset(n: u64) -> SessionLogOffset {
    SessionLogOffset::new(n).unwrap()
}
pub fn header(id: &str) -> Value {
    json!({"version":4,"id":id,"createdAt":1,"isSeeded":false})
}
pub fn message(role: &str, text: &str) -> Value {
    let source = match role {
        "assistant" => json!({"kind":"model","provider":"mock","model":"mock"}),
        "system" => json!({"kind":"system-prompt"}),
        "tool" => json!({"kind":"tool","callId":"call"}),
        _ => json!({"kind":"user"}),
    };
    let mut m = json!({"id":format!("{role}-{text}"),"role":role,"content":if text.is_empty(){json!([])}else{json!([{"type":"text","text":text}])},"source":source});
    if role == "tool" {
        m["toolCallId"] = json!("call");
        m["isError"] = json!(false);
    }
    m
}
pub fn data(role: &str, text: &str) -> Value {
    if role == "user" {
        message(role, text)
    } else {
        let mut d = json!({"turn":1,"step":1,"message":message(role,text)});
        if role == "assistant" {
            d["stream"] = json!([]);
        }
        d
    }
}
pub fn event(t: &str, n: u64, d: Value) -> Arc<SessionEvent> {
    Arc::new(SessionEvent::from_value(
        json!({"type":t,"seq":n,"time":n,"data":d}),
    ))
}
pub fn surface_event(role: &str, n: u64, text: &str) -> Arc<SessionEvent> {
    let t = if role == "tool" {
        "tool/result".to_string()
    } else {
        format!("{role}/message")
    };
    let mut e = event(&t, n, data(role, text)).as_value().clone();
    e["surfaceOp"] = json!("append");
    Arc::new(SessionEvent::from_value(e))
}
pub fn patch(e: &SessionEvent, key: &str, value: Value) -> Arc<SessionEvent> {
    let mut v = e.as_value().clone();
    v[key] = value;
    Arc::new(SessionEvent::from_value(v))
}
pub fn user(s: &Session, text: &str) -> Arc<SessionEvent> {
    s.append(
        "user/message",
        data("user", text),
        Some(SurfaceIntent::append()),
    )
    .unwrap()
}
pub fn replace(
    s: &Session,
    start: u64,
    end: u64,
    sources: &[u64],
    text: &str,
) -> Result<Arc<SessionEvent>> {
    s.append(
        "user/message",
        data("user", text),
        Some(SurfaceIntent::replace(
            seq(start),
            seq(end),
            sources.iter().copied().map(seq).collect(),
        )),
    )
}
pub fn nodes(s: &Session) -> Vec<u64> {
    s.surface()
        .unwrap()
        .nodes()
        .unwrap()
        .into_iter()
        .map(u64::from)
        .collect()
}
pub fn texts(s: &Session) -> Vec<String> {
    s.derive_messages()
        .unwrap()
        .iter()
        .map(|m| m["content"][0]["text"].as_str().unwrap_or("").to_owned())
        .collect()
}
pub fn error<T>(r: Result<T>, text: &str) {
    match r {
        Ok(_) => panic!("expected error containing {text}"),
        Err(e) => assert!(e.to_string().contains(text), "expected {text}, got {e}"),
    }
}
pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}
pub fn open_calls(started: bool) -> Vec<Arc<SessionEvent>> {
    let mut assistant = data("assistant", "");
    assistant["message"]["content"] =
        json!([{"type":"tool-call","id":"call","name":"bash","arguments":"{}"}]);
    let mut a = event("assistant/message", 2, assistant).as_value().clone();
    a["surfaceOp"] = json!("append");
    let mut events = vec![
        event("turn/start", 0, json!({"turn":1})),
        event("step/start", 1, json!({"turn":1,"step":1})),
        Arc::new(SessionEvent::from_value(a)),
    ];
    if started {
        events.push(event(
            "tool/call",
            3,
            json!({"turn":1,"step":1,"callId":"call","name":"bash","arguments":"{}"}),
        ));
    }
    events
}
pub fn tool(name: &str) -> Value {
    json!({"name":name,"description":name,"parameters":{}})
}
pub fn request(s: &Session, tools: Vec<Value>, reason: &str) -> Arc<SessionEvent> {
    let mut h = json!({"config":{"provider":"mock","model":"mock"}});
    if !tools.is_empty() {
        h["tools"] = json!(tools);
    }
    s.append("request/header", json!({"header":h,"reason":reason}), None)
        .unwrap()
}
pub fn developer(
    s: &Session,
    blocks: Value,
    reference: Option<SessionSeq>,
) -> Result<Arc<SessionEvent>> {
    let mut d = data("developer", "");
    d["message"]["content"] = blocks;
    if let Some(r) = reference {
        d["headerSeq"] = json!(r);
    }
    s.append("developer/message", d, Some(SurfaceIntent::append()))
}
pub async fn mounted() -> (hoh_kernel::Kernel, hoh_kernel::Context, SessionStore) {
    let k = hoh_kernel::Kernel::start();
    let ctx = k.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let store = (*ctx.get::<Sessions>().unwrap()).clone();
    (k, ctx, store)
}
