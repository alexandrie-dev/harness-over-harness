mod common;
use common::*;
use hoh_session::*;
use serde_json::json;
use std::sync::Arc;
fn closed(s: &Session, turn: u64, text: &str) {
    s.append("turn/start", json!({"turn":turn}), None).unwrap();
    user(s, text);
    s.append(
        "turn/end",
        json!({"turn":turn,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
}
fn open_turn_source(ctx: &hoh_kernel::Context, store: &SessionStore, id: &str) -> Session {
    let source = store
        .create(ctx, Some(id.into()), Default::default())
        .unwrap();
    closed(&source, 1, "first");
    source
        .append("turn/start", json!({"turn":2}), None)
        .unwrap();
    source
}
fn append_tool_request(source: &Session, id: &str) {
    source
        .append("step/start", json!({"turn":2,"step":1}), None)
        .unwrap();
    let mut message = message("assistant", "");
    message["content"] = json!([{"type":"tool-call","id":id,"name":"bash","arguments":"{}"}]);
    source
        .append(
            "assistant/message",
            json!({"stream":[],"turn":2,"step":1,"message":message}),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
}
fn event_bytes(event: &SessionEvent, expected: serde_json::Value) {
    assert_eq!(
        hoh_util_values::js_json_stringify(event.as_value()),
        hoh_util_values::js_json_stringify(&expected)
    );
}
fn marker_bytes(child: &Session, parent: &Session) {
    let seq = parent.seq().get();
    let time = parent.snapshot_events(None, None).last().unwrap().time();
    event_bytes(
        &child.event_at(common::seq(seq)).unwrap(),
        json!({"type":"session/end-seed","seq":seq,"time":time,"data":{"inherited":true}}),
    );
    assert_eq!(child.inherited_event_count(), parent.seq());
}
fn forked_turn_end_bytes(event: &SessionEvent, seq: u64, time: i64) {
    event_bytes(
        event,
        json!({"type":"turn/end","seq":seq,"time":time,"data":{"turn":2,"reason":{"kind":"forked"}}}),
    );
}
#[tokio::test(start_paused = true)]
async fn forks_an_empty_live_session_as_an_independent_empty_seeded_session() {
    let (k, ctx, store) = mounted().await;
    let p = store
        .create(
            &ctx,
            None,
            CreateSessionOptions {
                meta: Some(json!({"cwd":"/workspace"})),
                ..Default::default()
            },
        )
        .unwrap();
    let c = store
        .fork(
            &ctx,
            SessionForkSource::Session(p),
            None,
            Some("child".into()),
        )
        .unwrap();
    assert_eq!(c.inherited_event_count(), offset(0));
    assert_eq!(c.header()["cwd"], "/workspace");
    assert_eq!(c.header()["isSeeded"], true);
    assert!(c.header().get("parentSession").is_none());
    assert_eq!(c.own_events().len(), 1);
    event_bytes(
        &c.own_events()[0],
        json!({"type":"session/end-seed","seq":0,"time":c.own_events()[0].time(),"data":{"inherited":true}}),
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn forks_the_latest_completed_boundary_by_default_into_detached_frozen_seed_events() {
    let (k, ctx, store) = mounted().await;
    let p = store
        .create(
            &ctx,
            Some("parent".into()),
            CreateSessionOptions {
                meta: Some(json!({"cwd":"/workspace"})),
                ..Default::default()
            },
        )
        .unwrap();
    closed(&p, 1, "hello");
    let c = store
        .fork(
            &ctx,
            SessionForkSource::Id(p.id().clone()),
            None,
            Some("child".into()),
        )
        .unwrap();
    assert_eq!(
        c.snapshot_events(None, Some(c.inherited_event_count())),
        p.snapshot_events(None, None)
    );
    assert!(!Arc::ptr_eq(
        &c.event_at(seq(1)).unwrap(),
        &p.event_at(seq(1)).unwrap()
    ));
    assert_eq!(c.inherited_event_count(), p.seq());
    assert!(c.header().get("parentSession").is_none());
    assert_eq!(texts(&c), vec!["hello"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn includes_stable_log_only_events_appended_after_a_closed_turn() {
    let (k, ctx, store) = mounted().await;
    let p = store.create(&ctx, None, Default::default()).unwrap();
    closed(&p, 1, "hello");
    p.append("test/log-only", json!({"value":"after execution"}), None)
        .unwrap();
    let c = store
        .fork(&ctx, SessionForkSource::Session(p.clone()), None, None)
        .unwrap();
    assert_eq!(
        c.event_at(seq(3)).unwrap().data(),
        &json!({"value":"after execution"})
    );
    assert_eq!(c.inherited_event_count(), p.seq());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn forks_from_an_earlier_turn_boundary_even_when_the_source_currently_has_an_open_tail() {
    let (k, ctx, store) = mounted().await;
    let p = store.create(&ctx, None, Default::default()).unwrap();
    closed(&p, 1, "first");
    closed(&p, 2, "second");
    p.append("turn/start", json!({"turn":3}), None).unwrap();
    user(&p, "open 3");
    let c = store
        .fork(
            &ctx,
            SessionForkSource::Session(p.clone()),
            Some(seq(2)),
            None,
        )
        .unwrap();
    assert_eq!(c.inherited_event_count(), offset(3));
    assert_eq!(texts(&c), vec!["first"]);
    assert_eq!(
        c.snapshot_events(None, Some(offset(3))),
        p.snapshot_events(None, Some(offset(3)))
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn accepts_every_turn_end_reason_as_an_explicit_fork_boundary() {
    let (k, ctx, store) = mounted().await;
    for reason in [
        json!({"kind":"completed"}),
        json!({"kind":"aborted","reason":{"kind":"user"}}),
        json!({"kind":"error","error":{"message":"model failed","code":"UNKNOWN"}}),
        json!({"kind":"aborted","reason":{"kind":"disposed"}}),
        json!({"kind":"max-tokens"}),
        json!({"kind":"interrupted"}),
    ] {
        let p = store.create(&ctx, None, Default::default()).unwrap();
        p.append("turn/start", json!({"turn":1}), None).unwrap();
        let end = p
            .append("turn/end", json!({"turn":1,"reason":reason}), None)
            .unwrap();
        let c = store
            .fork(
                &ctx,
                SessionForkSource::Session(p.clone()),
                Some(end.seq()),
                None,
            )
            .unwrap();
        assert_eq!(c.inherited_event_count(), p.seq());
        assert_eq!(c.event_at(end.seq()).unwrap().data(), end.data());
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn marks_a_bracket_the_child_inherited_from_a_still_running_parent() {
    let (k, ctx, store) = mounted().await;
    let p = store.create(&ctx, None, Default::default()).unwrap();
    closed(&p, 1, "work");
    let open = p
        .append("test/bracket-open", json!({"id":"op-1"}), None)
        .unwrap();
    let c = store
        .fork(&ctx, SessionForkSource::Session(p.clone()), None, None)
        .unwrap();
    assert_eq!(p.snapshot_events(None, None).last(), Some(&open));
    assert_eq!(
        c.snapshot_events(None, None).last().unwrap().event_type(),
        "session/end-seed"
    );
    assert_eq!(u64::from(c.first_live_seq()), u64::from(open.seq()) + 2);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_invalid_boundaries_before_creating_a_child() {
    let (k, ctx, store) = mounted().await;
    let p = store
        .create(&ctx, Some("empty".into()), Default::default())
        .unwrap();
    let e = store
        .fork(
            &ctx,
            SessionForkSource::Session(p),
            Some(seq(0)),
            Some("child".into()),
        )
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "fork boundary 0 does not exist in session \"empty\" (last seq: none)"
    );
    assert_eq!(
        e.downcast_ref::<SessionForkError>().unwrap().code,
        SessionForkErrorCode::InvalidBoundary
    );
    assert!(store.get(&"child".into()).is_none());
    let p = store
        .create(&ctx, Some("parent".into()), Default::default())
        .unwrap();
    closed(&p, 1, "");
    let e = store
        .fork(
            &ctx,
            SessionForkSource::Session(p.clone()),
            Some(seq(p.seq().get())),
            Some("past-end".into()),
        )
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "fork boundary 3 does not exist in session \"parent\" (last seq: 2)"
    );
    assert_eq!(
        e.downcast_ref::<SessionForkError>().unwrap().code,
        SessionForkErrorCode::InvalidBoundary
    );
    assert!(store.get(&"past-end".into()).is_none());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_an_unknown_live_session_id() {
    let (k, ctx, store) = mounted().await;
    let e = store
        .fork(&ctx, SessionForkSource::Id("missing".into()), None, None)
        .unwrap_err();
    assert_eq!(e.to_string(), "session \"missing\" not found");
    assert_eq!(
        e.downcast_ref::<SessionForkError>().unwrap().code,
        SessionForkErrorCode::SessionNotFound
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_a_detached_session_object_that_is_not_live_in_sessions() {
    let (k, ctx, store) = mounted().await;
    let e = store
        .fork(
            &ctx,
            SessionForkSource::Session(
                Session::create("detached".into(), None, None, None, None).unwrap(),
            ),
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(e.to_string(), "session \"detached\" not found");
    assert_eq!(
        e.downcast_ref::<SessionForkError>().unwrap().code,
        SessionForkErrorCode::SessionNotFound
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_a_stale_session_object_whose_id_is_live_on_a_different_instance() {
    let (k, ctx, store) = mounted().await;
    store
        .create(&ctx, Some("same-id".into()), Default::default())
        .unwrap();
    let e = store
        .fork(
            &ctx,
            SessionForkSource::Session(
                Session::create("same-id".into(), None, None, None, None).unwrap(),
            ),
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "session \"same-id\" is not the live store instance"
    );
    assert_eq!(
        e.downcast_ref::<SessionForkError>().unwrap().code,
        SessionForkErrorCode::SessionNotLive
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_a_child_session_id_that_is_already_live_with_a_typed_fork_error() {
    let (k, ctx, store) = mounted().await;
    let p = store.create(&ctx, None, Default::default()).unwrap();
    store
        .create(&ctx, Some("child".into()), Default::default())
        .unwrap();
    let e = store
        .fork(
            &ctx,
            SessionForkSource::Session(p),
            None,
            Some("child".into()),
        )
        .unwrap_err();
    assert_eq!(e.to_string(), "session \"child\" already exists");
    assert_eq!(
        e.downcast_ref::<SessionForkError>().unwrap().code,
        SessionForkErrorCode::SessionAlreadyExists
    );
    assert!(hoh_error::HarnessError::of(e.as_ref()).is_none());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_a_duplicate_child_session_id_before_validating_the_boundary() {
    let (k, ctx, store) = mounted().await;
    let p = store.create(&ctx, None, Default::default()).unwrap();
    store
        .create(&ctx, Some("child".into()), Default::default())
        .unwrap();
    let e = store
        .fork(
            &ctx,
            SessionForkSource::Session(p),
            Some(seq(100)),
            Some("child".into()),
        )
        .unwrap_err();
    assert_eq!(e.to_string(), "session \"child\" already exists");
    assert_eq!(
        e.downcast_ref::<SessionForkError>().unwrap().code,
        SessionForkErrorCode::SessionAlreadyExists
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn closes_an_empty_open_turn_with_a_synthetic_forked_turn_end_outside_the_inherited_prefix() {
    let (k, ctx, store) = mounted().await;
    let p = open_turn_source(&ctx, &store, "open-empty");
    let c = store
        .fork(
            &ctx,
            SessionForkSource::Session(p.clone()),
            None,
            Some("open-empty-child".into()),
        )
        .unwrap();
    assert_eq!(
        c.snapshot_events(None, Some(p.seq())),
        p.snapshot_events(None, None)
    );
    assert_eq!(c.own_events().len(), 2);
    marker_bytes(&c, &p);
    forked_turn_end_bytes(
        c.own_events().last().unwrap(),
        p.seq().get() + 1,
        p.snapshot_events(None, None).last().unwrap().time(),
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn closes_only_the_turn_when_the_cut_lands_on_a_closed_step_boundary() {
    let (k, ctx, store) = mounted().await;
    let p = open_turn_source(&ctx, &store, "open-step-closed");
    p.append("step/start", json!({"turn":2,"step":1}), None)
        .unwrap();
    p.append(
        "assistant/message",
        json!({"stream":[],"turn":2,"step":1,"message":message("assistant","step done")}),
        Some(SurfaceIntent::append()),
    )
    .unwrap();
    p.append("step/end", json!({"turn":2,"step":1}), None)
        .unwrap();
    assert!(
        !p.snapshot_events(None, None)
            .iter()
            .any(|e| e.event_type() == "session/end-seed")
    );
    let c = store
        .fork(&ctx, SessionForkSource::Session(p.clone()), None, None)
        .unwrap();
    assert_eq!(c.own_events().len(), 2);
    marker_bytes(&c, &p);
    forked_turn_end_bytes(
        &c.own_events()[1],
        p.seq().get() + 1,
        p.snapshot_events(None, None).last().unwrap().time(),
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn answers_an_assistant_call_cut_before_its_tool_call_with_a_forked_not_started_error_result()
{
    let (k, ctx, store) = mounted().await;
    let p = open_turn_source(&ctx, &store, "open-not-started");
    append_tool_request(&p, "call-ns");
    assert!(
        !p.snapshot_events(None, None)
            .iter()
            .any(|e| e.event_type() == "session/end-seed")
    );
    let c = store
        .fork(&ctx, SessionForkSource::Session(p.clone()), None, None)
        .unwrap();
    let own = c.own_events();
    assert_eq!(
        own.iter().map(|e| e.event_type()).collect::<Vec<_>>(),
        ["session/end-seed", "tool/result", "step/end", "turn/end"]
    );
    marker_bytes(&c, &p);
    assert_eq!(
        own[1].data()["error"],
        json!({"name":"ToolNotStartedError","code":TOOL_NOT_STARTED})
    );
    assert_eq!(
        own[1].data()["message"]["id"],
        "forked-tool-result-call-ns-7"
    );
    assert_eq!(own[1].data()["message"]["source"]["callId"], "call-ns");
    let derived = c.derive_messages().unwrap();
    let message = derived.last().unwrap();
    assert_eq!(message["role"], "tool");
    assert_eq!(message["source"], json!({"kind":"tool","callId":"call-ns"}));
    assert_eq!(message["toolCallId"], "call-ns");
    assert_eq!(message["isError"], true);
    assert_eq!(message["content"][0]["type"], "text");
    let time = p.snapshot_events(None, None).last().unwrap().time();
    event_bytes(
        &own[2],
        json!({"type":"step/end","seq":8,"time":time,"data":{"turn":2,"step":1}}),
    );
    forked_turn_end_bytes(&own[3], 9, time);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn answers_a_dispatched_call_cut_before_its_result_with_an_unknown_outcome_error_citing_the_call()
 {
    let (k, ctx, store) = mounted().await;
    let p = open_turn_source(&ctx, &store, "open-unknown");
    append_tool_request(&p, "call-uo");
    let call = p
        .append(
            "tool/call",
            json!({"turn":2,"step":1,"callId":"call-uo","name":"bash","arguments":"{}"}),
            None,
        )
        .unwrap();
    assert!(
        !p.snapshot_events(None, None)
            .iter()
            .any(|e| e.event_type() == "session/end-seed")
    );
    let c = store
        .fork(&ctx, SessionForkSource::Session(p.clone()), None, None)
        .unwrap();
    let own = c.own_events();
    assert_eq!(
        own.iter().map(|e| e.event_type()).collect::<Vec<_>>(),
        ["session/end-seed", "tool/result", "step/end", "turn/end"]
    );
    marker_bytes(&c, &p);
    assert_eq!(
        own[1].data()["error"],
        json!({"name":"ToolOutcomeUnknownError","code":TOOL_OUTCOME_UNKNOWN})
    );
    assert_eq!(own[1].source_event_seqs(), Some(&json!([call.seq()])));
    event_bytes(
        &own[2],
        json!({"type":"step/end","seq":9,"time":call.time(),"data":{"turn":2,"step":1}}),
    );
    forked_turn_end_bytes(&own[3], 10, call.time());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn omits_a_failed_assistant_attempt_from_derived_history() {
    let (k, ctx, store) = mounted().await;
    let p = open_turn_source(&ctx, &store, "open-mid-stream");
    p.append("step/start", json!({"turn":2,"step":1}), None)
        .unwrap();
    p.append(
        "assistant/attempt",
        json!({"turn":2,"step":1,"stream":[]}),
        None,
    )
    .unwrap();
    let c = store
        .fork(&ctx, SessionForkSource::Session(p.clone()), None, None)
        .unwrap();
    assert!(
        c.derive_messages()
            .unwrap()
            .iter()
            .all(|message| message["role"] != "assistant")
    );
    assert_eq!(
        c.own_events()
            .iter()
            .map(|e| e.event_type())
            .collect::<Vec<_>>(),
        ["session/end-seed", "step/end", "turn/end"]
    );
    marker_bytes(&c, &p);
    let time = p.snapshot_events(None, None).last().unwrap().time();
    forked_turn_end_bytes(c.own_events().last().unwrap(), p.seq().get() + 2, time);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn preserves_a_closed_step_with_an_unanswered_call() {
    for ended in [false, true] {
        let (k, ctx, store) = mounted().await;
        let p = open_turn_source(&ctx, &store, "dangling-closed-step");
        append_tool_request(&p, "call-dangling");
        p.append(
            "tool/call",
            json!({"turn":2,"step":1,"callId":"call-dangling","name":"bash","arguments":"{}"}),
            None,
        )
        .unwrap();
        p.append("step/end", json!({"turn":2,"step":1}), None)
            .unwrap();
        if ended {
            p.append("turn/end", json!({"turn":2,"reason":{"kind":"error","error":{"message":"scheduler failed","code":"UNKNOWN"}}}), None).unwrap();
        }
        assert!(
            !p.snapshot_events(None, None)
                .iter()
                .any(|e| e.event_type() == "session/end-seed")
        );
        let copied = p.snapshot_events(None, None);
        let c = store
            .fork(&ctx, SessionForkSource::Session(p.clone()), None, None)
            .unwrap();
        assert_eq!(c.snapshot_events(None, Some(p.seq())), copied);
        assert_eq!(c.own_events().len(), if ended { 1 } else { 2 });
        marker_bytes(&c, &p);
        if !ended {
            forked_turn_end_bytes(
                &c.own_events()[1],
                p.seq().get() + 1,
                copied.last().unwrap().time(),
            );
        }
        assert_eq!(c.derive_messages().unwrap(), p.derive_messages().unwrap());
        k.shutdown().await;
    }
}
#[tokio::test(start_paused = true)]
async fn leaves_a_plugin_bracket_left_open_at_the_cut_untouched() {
    let (k, ctx, store) = mounted().await;
    let p = open_turn_source(&ctx, &store, "open-bracket");
    p.append("test/bracket-open", json!({"id":"op-live"}), None)
        .unwrap();
    let c = store
        .fork(&ctx, SessionForkSource::Session(p.clone()), None, None)
        .unwrap();
    assert_eq!(
        c.own_events()
            .iter()
            .map(|e| e.event_type())
            .collect::<Vec<_>>(),
        ["session/end-seed", "turn/end"]
    );
    marker_bytes(&c, &p);
    forked_turn_end_bytes(
        &c.own_events()[1],
        p.seq().get() + 1,
        p.snapshot_events(None, None).last().unwrap().time(),
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn derives_the_original_message_for_a_cut_before_the_replacement() {
    replacement_cut(false).await;
}
#[tokio::test(start_paused = true)]
async fn derives_the_compacted_surface_for_a_cut_after_the_replacement() {
    replacement_cut(true).await;
}
async fn replacement_cut(after: bool) {
    let (k, ctx, store) = mounted().await;
    let p = store.create(&ctx, None, Default::default()).unwrap();
    closed(&p, 1, "original request");
    p.append("turn/start", json!({"turn":2}), None).unwrap();
    replace(&p, 1, 1, &[1], "summary of earlier work").unwrap();
    let boundary = if after { 4 } else { 3 };
    let time = p.event_at(seq(boundary)).unwrap().time();
    let c = store
        .fork(
            &ctx,
            SessionForkSource::Session(p),
            Some(seq(boundary)),
            None,
        )
        .unwrap();
    assert_eq!(
        texts(&c),
        vec![if after {
            "summary of earlier work"
        } else {
            "original request"
        }]
    );
    event_bytes(
        &c.own_events()[0],
        json!({"type":"session/end-seed","seq":boundary+1,"time":time,"data":{"inherited":true}}),
    );
    forked_turn_end_bytes(c.own_events().last().unwrap(), boundary + 2, time);
    k.shutdown().await;
}
