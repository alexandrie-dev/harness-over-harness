mod common;
use common::*;
use hoh_session::*;
use hoh_util_values::{js_json_parse, js_json_stringify};
use serde_json::json;
use std::sync::{Arc, Mutex};
#[test]
fn append_envelope_and_surface_keys_follow_the_sources_construction_order() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    let e=s.append("user/message",json!({"content":[{"type":"text","text":"x"}],"source":{"kind":"user"},"role":"user","id":"m"}),Some(SurfaceIntent { source_event_seqs:Some(vec![seq(0)]),surface_op:Some(SurfaceOp::append()) })).unwrap();
    let mut v = e.as_value().clone();
    v["time"] = json!(0);
    assert_eq!(
        js_json_stringify(&v),
        "{\"type\":\"user/message\",\"seq\":1,\"time\":0,\"data\":{\"content\":[{\"type\":\"text\",\"text\":\"x\"}],\"source\":{\"kind\":\"user\"},\"role\":\"user\",\"id\":\"m\"},\"sourceEventSeqs\":[0],\"surfaceOp\":\"append\"}"
    );
}
#[tokio::test(start_paused = true)]
async fn session_header_keeps_every_optional_fields_source_position() {
    let (k, ctx, store) = mounted().await;
    let s=store.create(&ctx,Some("child".into()),CreateSessionOptions { meta:Some(json!({"createdAt":123,"cwd":"/work","parentSession":"parent","origin":"subagent","delegationDepth":2,"agentPreset":"p"})),..Default::default() }).unwrap();
    assert_eq!(
        js_json_stringify(&s.header()),
        "{\"version\":4,\"id\":\"child\",\"createdAt\":123,\"cwd\":\"/work\",\"parentSession\":\"parent\",\"isSeeded\":false,\"origin\":\"subagent\",\"delegationDepth\":2,\"agentPreset\":\"p\"}"
    );
    k.shutdown().await;
}
#[test]
fn restored_unknown_stream_and_message_fields_and_key_order_survive_byte_for_byte() {
    let text = "{\"type\":\"assistant/message\",\"seq\":0,\"time\":1,\"data\":{\"turn\":1,\"step\":1,\"stream\":[{\"type\":\"chunk\",\"time\":1,\"chunk\":{\"extensionFirst\":null,\"type\":\"finish\",\"reason\":\"stop\",\"extensionLast\":[true]}}],\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"answer\",\"extra\":null}],\"role\":\"assistant\",\"source\":{\"kind\":\"model\",\"provider\":\"mock\",\"model\":\"mock\",\"extra\":{\"x\":1}},\"id\":\"m\",\"extension\":null},\"extension\":{\"later\":2,\"earlier\":1}},\"surfaceOp\":\"append\"}";
    let e = Arc::new(SessionEvent::from_value(js_json_parse(text).unwrap()));
    let s = Session::from_restore(
        "test".into(),
        &[e],
        header("test"),
        offset(0),
        SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert_eq!(
        js_json_stringify(s.event_at(seq(0)).unwrap().as_value()),
        text
    );
    let m = s.derive_messages().unwrap();
    assert!(m[0].get("extension").is_some());
    assert_eq!(m[0]["content"][0]["extra"], serde_json::Value::Null);
}
#[test]
fn recovery_envelopes_preserve_their_distinct_surface_key_order_and_message_key_order() {
    let events = interrupted_turn_closers(&open_calls(true)).unwrap();
    let e = events[0].as_value();
    assert_eq!(
        e.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![
            "type",
            "seq",
            "time",
            "data",
            "surfaceOp",
            "sourceEventSeqs"
        ]
    );
    assert_eq!(
        e["data"]["message"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["id", "role", "toolCallId", "isError", "source", "content"]
    );
    assert_eq!(
        e["data"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["turn", "step", "message", "error"]
    );
    assert_eq!(
        e["data"]["error"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["name", "code"]
    );
}
#[test]
fn numeric_json_remains_javascript_json_at_the_session_boundary() {
    let s = session();
    let e = s
        .append(
            "test/numbers",
            json!({"whole":1.0,"large":1e20,"tiny":1e-7}),
            None,
        )
        .unwrap();
    assert_eq!(
        js_json_stringify(e.data()),
        "{\"whole\":1,\"large\":100000000000000000000,\"tiny\":1e-7}"
    );
    assert_eq!(
        serde_json::to_value(SessionSeqCursor::BeforeFirst).unwrap(),
        json!(-1)
    );
    assert!(serde_json::from_value::<SessionSeqCursor>(json!(-2)).is_err());
}
#[tokio::test(start_paused = true)]
async fn batch_listener_snapshots_are_fixed_before_the_first_member_is_published() {
    let (k, ctx, store) = mounted().await;
    let s = store.create(&ctx, None, Default::default()).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let out = calls.clone();
    let registrant = ctx.clone();
    ctx.on_contained::<SessionEventEmitted>(move |e| {
        if e.event.seq() == seq(0) {
            let out = out.clone();
            registrant.on_contained::<SessionEventEmitted>(move |e| {
                lock(&out).push(e.event.seq());
                Ok(())
            })?;
        }
        Ok(())
    })
    .unwrap();
    s.append_batch(&[
        SessionEventInput::new("turn/start", json!({"turn":1})),
        SessionEventInput::new("step/start", json!({"turn":1,"step":1})),
    ])
    .unwrap();
    assert!(lock(&calls).is_empty());
    s.append("step/end", json!({"turn":1,"step":1}), None)
        .unwrap();
    assert_eq!(*lock(&calls), vec![seq(2)]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn synchronous_creation_veto_stops_later_listener_bodies() {
    let (k, ctx, store) = mounted().await;
    SessionStore::on_created(&ctx, |_| Err("veto".into())).unwrap();
    let calls = Arc::new(Mutex::new(0));
    let out = calls.clone();
    SessionStore::on_created(&ctx, move |_| {
        *lock(&out) += 1;
        Ok(())
    })
    .unwrap();
    let out = calls.clone();
    SessionStore::on_created_async(&ctx, move |_| {
        let out = out.clone();
        async move {
            *lock(&out) += 1;
            Ok(())
        }
    })
    .unwrap();
    error(
        store.create(&ctx, Some("rejected".into()), Default::default()),
        "veto",
    );
    assert_eq!(*lock(&calls), 0);
    assert!(store.get(&"rejected".into()).is_none());
    k.shutdown().await;
}
#[test]
fn per_event_and_cached_message_derivation_share_immutable_identity() {
    let s = session();
    let e = user(&s, "message");
    let one = s.derive_event_message(&e).unwrap().unwrap();
    let many = s.derive_messages().unwrap();
    assert!(Arc::ptr_eq(&one, &many[0]));
}

#[test]
fn seed_and_header_validation_keep_the_sources_constructor_and_restore_precedence() {
    let seed = vec![event("turn/start", 5, json!({"turn":1}))];
    let mut h = header("test");
    h["version"] = json!(5);
    h["extra"] = json!(-0.0);
    error(
        Session::create("test".into(), Some(&seed), Some(h.clone()), None, None),
        "has seq 5 (expected 0)",
    );
    error(
        Session::from_restore(
            "test".into(),
            &seed,
            h,
            offset(0),
            SessionSeedEventState::Detached,
            None,
        ),
        "session header version must be 4",
    );
}

#[test]
fn a_prevalidated_message_projection_is_interpreted_once_when_admitted() {
    use hoh_session::surface::SurfaceManager;
    use indexmap::IndexMap;
    struct Counting(Arc<Mutex<u64>>);
    impl SessionMessageProjection for Counting {
        fn event_type(&self) -> &str {
            "test/count"
        }
        fn project(
            &self,
            _: &SessionEvent,
            c: SessionMessageProjectionContext<'_>,
        ) -> Result<IndexMap<SessionSeq, Arc<serde_json::Value>>> {
            *lock(&self.0) += 1;
            Ok(IndexMap::from([(
                seq(0),
                derive_event_message(&c.events[0], Some(c.messages)).unwrap(),
            )]))
        }
    }
    let calls = Arc::new(Mutex::new(0));
    let definitions: MessageProjections =
        Arc::new(Mutex::new(vec![Arc::new(Counting(calls.clone()))]));
    let mut manager = SurfaceManager::new(
        vec![surface_event("user", 0, "source")],
        offset(0),
        definitions,
    )
    .unwrap();
    let candidate = event("test/count", 1, json!({}));
    manager.validate_next(&candidate).unwrap();
    assert_eq!(*lock(&calls), 1);
    assert_eq!(manager.surface().unwrap().content_generation, 0);
    manager.commit_batch(&[candidate]).unwrap();
    assert_eq!(*lock(&calls), 1);
    assert_eq!(manager.surface().unwrap().content_generation, 1);
}
