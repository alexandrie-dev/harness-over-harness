mod support;

use std::sync::{Arc, Mutex};

use hoh_session::{SessionLogOffset, SessionSeq, SessionSeqCursor};
use hoh_session_projection::ProjectionCheckpointRow;
use serde_json::{Value, json};

use support::*;

#[tokio::test]
async fn serves_null_before_first_title_event() {
    let env = setup().await;
    let s = session(&env, "null");
    assert_eq!(
        env.projections.snapshot(&s, None).unwrap().values["title"].as_ref(),
        &Value::Null
    );
    let row = &env.projections.checkpoint(&s).unwrap()["title"];
    assert_eq!(row.ver, 1);
    assert_eq!(row.seq, SessionSeqCursor::BeforeFirst);
    assert_eq!(row.val, Some(Value::Null));
    let checkpoint = env.projections.checkpoint(&s).unwrap();
    let header: hoh_session::SessionHeader = serde_json::from_value((*s.header()).clone()).unwrap();
    assert!(
        env.projections
            .restore(
                &checkpoint,
                &[],
                SessionLogOffset::default(),
                &header,
                s.inherited_event_count(),
            )
            .is_ok()
    );
    env.shutdown().await;
}

#[tokio::test]
async fn latest_title_wins_and_change_feed_uses_causing_seq() {
    let env = setup().await;
    let s = session(&env, "latest");
    let changes = Arc::new(Mutex::new(vec![]));
    let out = changes.clone();
    env.projections
        .on_changed(
            &env.ctx,
            Arc::new(move |_, key, value, seq| {
                if key == "title" {
                    out.lock().unwrap().push((value, seq));
                }
                Ok(())
            }),
        )
        .unwrap();
    let a = s
        .append(
            "session/title",
            json!({"title":"First","messageSeqs":[],"source":{"kind":"user"}}),
            None,
        )
        .unwrap()
        .seq();
    let b = s
        .append(
            "session/title",
            json!({"title":"Second","messageSeqs":[],"source":{"kind":"user"}}),
            None,
        )
        .unwrap()
        .seq();
    assert_eq!(
        env.projections.snapshot(&s, None).unwrap().values["title"].as_str(),
        Some("Second")
    );
    assert_eq!(
        changes
            .lock()
            .unwrap()
            .iter()
            .map(|(_, seq)| *seq)
            .collect::<Vec<_>>(),
        vec![a, b]
    );
    env.shutdown().await;
}

#[tokio::test]
async fn reads_version_one_string_checkpoint() {
    let env = setup().await;
    let mut checkpoint = env.projections.checkpoint(&session(&env, "cp")).unwrap();
    checkpoint.insert(
        "title".into(),
        ProjectionCheckpointRow {
            ver: 1,
            seq: SessionSeqCursor::Seq(SessionSeq::new(8).unwrap()),
            val: Some(json!("Cached title")),
        },
    );
    assert_eq!(
        env.projections.view_checkpoint(&checkpoint, None).unwrap()["title"].as_str(),
        Some("Cached title")
    );
    env.shutdown().await;
}

#[tokio::test]
async fn folds_titles_already_in_log_when_read_lazily() {
    let env = setup_base().await;
    let s = env
        .sessions
        .create(
            &env.ctx,
            Some("premount".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    s.append(
        "session/title",
        json!({"title":"Pre-mount title","messageSeqs":[],"source":{"kind":"user"}}),
        None,
    )
    .unwrap();
    env.mount_titles(config()).await;
    assert_eq!(
        env.projections.snapshot(&s, None).unwrap().values["title"].as_str(),
        Some("Pre-mount title")
    );
    env.shutdown().await;
}

#[tokio::test]
async fn title_key_is_owned_by_the_title_plugin() {
    let env = setup_base().await;
    let s = env
        .sessions
        .create(
            &env.ctx,
            Some("owned".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    assert!(
        !env.projections
            .snapshot(&s, None)
            .unwrap()
            .values
            .contains_key("title")
    );
    let title_fiber = env.mount_titles(config()).await;
    s.append(
        "session/title",
        json!({"title":"Ephemeral","messageSeqs":[],"source":{"kind":"user"}}),
        None,
    )
    .unwrap();
    assert!(
        env.projections
            .snapshot(&s, None)
            .unwrap()
            .values
            .contains_key("title")
    );
    assert_eq!(
        env.projections.snapshot(&s, None).unwrap().values["title"].as_str(),
        Some("Ephemeral")
    );
    title_fiber.dispose().await;
    assert!(
        !env.projections
            .snapshot(&s, None)
            .unwrap()
            .values
            .contains_key("title")
    );
    env.shutdown().await;
}

#[tokio::test]
async fn thousands_of_inputs_remain_a_bounded_aggregate() {
    let env = setup_base().await;
    let s = env
        .sessions
        .create(
            &env.ctx,
            Some("bounded".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    let mut last = None;
    for index in 0..5000 {
        last = Some(append_human(&s, &format!("message {index}")));
    }
    env.mount_titles(config()).await;
    let row = &env.projections.checkpoint(&s).unwrap()["titleInput"];
    assert_eq!(row.val.as_ref().unwrap()["count"], 5000);
    assert_eq!(row.val.as_ref().unwrap()["first"]["text"], "message 0");
    assert_eq!(row.val.as_ref().unwrap()["lastSeq"], last.unwrap().get());
    env.shutdown().await;
}

#[tokio::test]
async fn rejects_inconsistent_title_input_checkpoint() {
    let env = setup().await;
    let s = session(&env, "invalid");
    let checkpoint = env.projections.checkpoint(&s).unwrap();
    let header: hoh_session::SessionHeader = serde_json::from_value((*s.header()).clone()).unwrap();
    for invalid in [
        json!({"count":0,"lastSeq":null}),
        json!({"first":null,"count":0}),
        json!({"first":null,"count":1,"lastSeq":null}),
        json!({"first":{"seq":1,"text":"first"},"count":1,"lastSeq":null}),
        json!({"first":{"seq":1,"text":"first"},"count":0,"lastSeq":1}),
        json!({"first":{"seq":2,"text":"first"},"count":1,"lastSeq":1}),
    ] {
        let mut malformed = checkpoint.clone();
        malformed.get_mut("titleInput").unwrap().val = Some(invalid);
        let error = env
            .projections
            .restore(
                &malformed,
                &[],
                SessionLogOffset::default(),
                &header,
                s.inherited_event_count(),
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("pair its count")
                || error
                    .to_string()
                    .contains("must contain first, count, and lastSeq"),
            "{error}"
        );
    }
    let mut unknown = checkpoint.clone();
    unknown.get_mut("titleInput").unwrap().val = Some(json!({
        "first": null, "count": 0, "lastSeq": null, "extra": true,
    }));
    assert!(
        env.projections
            .restore(
                &unknown,
                &[],
                SessionLogOffset::default(),
                &header,
                s.inherited_event_count(),
            )
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );
    let mut valid = checkpoint.clone();
    valid.get_mut("titleInput").unwrap().val = Some(json!({
        "first":{"seq":1,"text":"first"},"count":1,"lastSeq":1,
    }));
    assert!(
        env.projections
            .restore(
                &valid,
                &[],
                SessionLogOffset::default(),
                &header,
                s.inherited_event_count(),
            )
            .is_ok()
    );
    env.shutdown().await;
}
