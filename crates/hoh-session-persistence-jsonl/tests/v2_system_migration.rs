mod common;
use common::*;
use hoh_session::{Session, SessionLogOffset, SessionSeedEventState};
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::JsonlCompression;
use serde_json::json;
#[tokio::test]
async fn persists_current_system_appends_after_the_protected_head_without_converting_them_to_user_messages()
 {
    let f = Fixture::new(JsonlCompression::None).await;
    let mut rows = vec![
        event("turn/start", 0, json!({"turn":1})),
        event("step/start", 1, json!({"turn":1,"step":1})),
    ];
    for (seq, role, id, text) in [
        (2, "system", "head", "head prompt"),
        (3, "user", "question", "question"),
        (4, "system", "context", "tail context"),
    ] {
        let message = json!({"id":id,"role":role,"content":[{"type":"text","text":text}],"source":{"kind":if role=="system" {"system-prompt"}else{"user"}}});
        let mut row = event(
            if role == "system" {
                "system/message"
            } else {
                "user/message"
            },
            seq,
            if role == "system" {
                json!({"turn":1,"step":1,"message":message})
            } else {
                message
            },
        );
        row["surfaceOp"] = "append".into();
        rows.push(row);
    }
    rows.push(event("step/end", 5, json!({"turn":1,"step":1})));
    rows.push(event(
        "turn/end",
        6,
        json!({"turn":1,"reason":{"kind":"completed"}}),
    ));
    let m = meta("v2-system-migration");
    let w = f.backend.create(&m, None).await.unwrap();
    w.append(&arcs(&rows), None).await.unwrap();
    w.flush(None).await.unwrap();
    w.close().await.unwrap();
    let r = f
        .backend
        .open(&m.id, SessionAccess::Read, None)
        .await
        .unwrap();
    let result = r.read(None, None, None).await.unwrap();
    assert_eq!(values(&result.events), rows);
    let session = Session::from_restore(
        m.id.clone(),
        &result.events,
        serde_json::to_value(r.header()).unwrap(),
        SessionLogOffset::default(),
        SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert_eq!(
        session
            .surface()
            .unwrap()
            .nodes()
            .unwrap()
            .iter()
            .map(|n| n.get())
            .collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    let messages = session.derive_messages().unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["head", "question", "context"]
    );
    assert_eq!(
        messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["system", "user", "system"]
    );
    r.close().await.unwrap();
    f.finish().await;
}
