mod common;
use common::*;
use hoh_session::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
#[test]
fn commits_an_ignorable_record_outside_the_model_visible_surface() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    user(&s, "hello");
    let before = s.derive_messages().unwrap();
    let mut d = json!({"count":1});
    let n = append_plugin_record(&s, "plugin:test/state", d.clone()).unwrap();
    d["count"] = json!(2);
    let e = s.event_at(n).unwrap();
    assert_eq!(n, seq(2));
    assert_eq!(e.data(), &json!({"count":1}));
    assert!(e.ignorable());
    assert_eq!(s.derive_messages().unwrap(), before);
    assert_eq!(nodes(&s), vec![1]);
}
#[test]
fn reads_records_back_and_leaves_every_other_event_unread() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    user(&s, "hello");
    let n = append_plugin_record(&s, "plugin:test.bridge/entry_1", json!(["a", 2])).unwrap();
    let events = s.snapshot_events(None, None);
    assert!(plugin_record_of(&events[0]).is_none());
    assert!(plugin_record_of(&events[1]).is_none());
    let record = plugin_record_of(&events[2]).unwrap();
    assert_eq!(record.r#type, "plugin:test.bridge/entry_1");
    assert_eq!(record.seq, n);
    assert_eq!(record.data, json!(["a", 2]));
}
#[test]
fn snapshots_an_interface_typed_request_with_readonly_messages() {
    let s = session();
    let mut request = json!({"config":{"provider":"fixture","model":"fixture"},"messages":[{"role":"user","content":[{"type":"text","text":"Translate this fragment."}]}]});
    let n = append_plugin_record(&s, "plugin:test/request", request.clone()).unwrap();
    request["config"]["model"] = json!("changed");
    assert_eq!(s.event_at(n).unwrap().data()["config"]["model"], "fixture");
    assert!(s.derive_messages().unwrap().is_empty());
}
#[test]
fn does_not_read_a_known_event_even_one_marked_ignorable() {
    for marked in [false, true] {
        let e = event("turn/start", 0, json!({"turn":1}));
        let e = if marked {
            patch(&e, "ignorable", json!(true))
        } else {
            e
        };
        assert!(plugin_record_of(&e).is_none());
    }
}
#[test]
fn rejects_invalid_record_types_without_changing_the_log() {
    for t in [
        "plugin:",
        "plugin:Test/state",
        "plugin:/state",
        "plugin:test/",
        "plugin:test//state",
        "plugin:-test",
        "plugin:test state",
        "turn/start",
    ] {
        let s = session();
        error(
            append_plugin_record(&s, t, json!({})),
            &format!(
                "plugin record type \"{t}\" must be \"plugin:\" followed by lowercase slash-separated segments"
            ),
        );
        assert_eq!(s.seq(), offset(0));
    }
}
#[test]
fn rejects_data_that_is_not_losslessly_json_serializable_without_changing_the_log() {
    let s = session();
    error(
        append_plugin_record(&s, "plugin:test/number", json!(-0.0)),
        "plugin record \"plugin:test/number\" carries non-JSON-serializable data",
    );
    assert_eq!(s.seq(), offset(0));
}
#[tokio::test(start_paused = true)]
async fn publishes_a_record_to_session_event_observers_and_contains_a_reentrant_record() {
    let (k, ctx, store) = mounted().await;
    let s = store
        .create(&ctx, Some("observed-record".into()), Default::default())
        .unwrap();
    ctx.on_contained::<SessionEventEmitted>(|e| {
        append_plugin_record(&e.session, "plugin:test/echo", json!({}))?;
        Ok(())
    })
    .unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let out = seen.clone();
    ctx.on_contained::<SessionEventEmitted>(move |e| {
        lock(&out).push(e.event.clone());
        Ok(())
    })
    .unwrap();
    append_plugin_record(&s, "plugin:test/state", json!({"count":1})).unwrap();
    assert_eq!(lock(&seen).len(), 1);
    assert_eq!(s.seq(), offset(1));
    assert!(ctx.logger().buffer().iter().any(|r|r.message=="session \"observed-record\": session/event listener threw: Error: session append cannot reenter while another append is being published"));
    k.shutdown().await;
}
#[test]
fn carries_records_through_a_snapshot_restore_a_storage_reopen_and_a_fork() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    user(&s, "hello");
    append_plugin_record(&s, "plugin:test/state", json!({"count":1})).unwrap();
    let events = s.snapshot_events(None, None);
    let restored = Session::create("restored".into(), Some(&events), None, None, None).unwrap();
    let reopened = Session::from_restore(
        s.id().clone(),
        &events,
        (*s.header()).clone(),
        offset(0),
        SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    let seed = build_fork_seed(&events, seq(2)).unwrap();
    let child = Session::create("child".into(), Some(&seed), None, None, None).unwrap();
    for copy in [restored, reopened, child] {
        assert_eq!(
            plugin_record_of(&copy.event_at(seq(2)).unwrap()),
            plugin_record_of(&events[2])
        );
        assert_eq!(
            copy.derive_messages().unwrap(),
            s.derive_messages().unwrap()
        );
    }
}
