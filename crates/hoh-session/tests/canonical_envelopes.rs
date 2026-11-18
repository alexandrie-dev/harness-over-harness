mod common;
use common::*;
use hoh_session::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
fn req(d: Value) -> Arc<SessionEvent> {
    event("request/header", 0, d)
}
fn accept(path: usize, e: Arc<SessionEvent>) -> Result<()> {
    let s = session();
    match path {
        0 => {
            s.append_batch(&[SessionEventInput::new(e.event_type(), e.data().clone())
                .with_options(e.as_value().clone())])?;
        }
        1 => {
            Session::create("test".into(), Some(&[e]), None, None, None)?;
        }
        2 => {
            Session::from_restore(
                "test".into(),
                &[e],
                header("test"),
                offset(0),
                SessionSeedEventState::Detached,
                None,
            )?;
        }
        3 => {
            adopt_session_event((*e).clone())?;
        }
        4 => {
            snapshot_session_event(&e)?;
        }
        _ => {
            let opts = SurfaceIntent {
                source_event_seqs: e
                    .source_event_seqs()
                    .map(|v| serde_json::from_value(v.clone()).unwrap()),
                surface_op: e
                    .surface_op()
                    .map(|v| serde_json::from_value(v.clone()).unwrap()),
            };
            s.append(e.event_type(), e.data().clone(), Some(opts))?;
        }
    }
    Ok(())
}
fn invalid() -> Vec<(Arc<SessionEvent>, &'static str)> {
    let mut cases = vec![];
    for value in [Value::Null, json!([]), json!(1), json!("invalid")] {
        cases.push((req(value.clone()), "data must be an object"));
        cases.push((
            req(json!({"header":value,"reason":"initial"})),
            "header must be an object",
        ));
        cases.push((
            patch(&surface_event("tool", 0, ""), "data", value),
            "data must be an object",
        ));
    }
    cases.push((req(json!({"reason":"initial"})), "header must be an object"));
    for optional in [json!({"tools":[]}), json!({"adapterDefaults":{}})] {
        let mut h = json!({"config":{"provider":"mock","model":"mock","maxTokens":1}});
        h.as_object_mut()
            .unwrap()
            .extend(optional.as_object().unwrap().clone());
        cases.push((
            req(json!({"header":h,"reason":"initial"})),
            "must omit empty",
        ));
    }
    for system in [
        json!(""),
        json!(" "),
        json!("system"),
        Value::Null,
        json!([]),
        json!({}),
    ] {
        cases.push((req(json!({"header":{"config":{"provider":"mock","model":"mock"},"system":system},"reason":"initial"})),"must omit header.system"));
    }
    let mut d = data("tool", "");
    d["error"] = json!({"name":"ToolError","code":"FAILED"});
    cases.push((
        patch(&surface_event("tool", 0, ""), "data", d),
        "error requires",
    ));
    for m in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"isError":"true"}),
        json!({"content":[{"isError":true}]}),
    ] {
        cases.push((
            patch(
                &surface_event("tool", 0, ""),
                "data",
                json!({"turn":1,"step":1,"message":m,"error":{"name":"ToolError","code":"FAILED"}}),
            ),
            "error requires",
        ));
    }
    cases
}
#[test]
fn restores_system_prompts_only_with_system_prompt_producer_attribution() {
    let good = surface_event("system", 0, "prompt");
    for path in [1, 2, 3, 4] {
        accept(path, good.clone()).unwrap();
        for source in [
            json!({"kind":"runtime-context"}),
            json!({"kind":"plugin","plugin":"@deepseek-ai/dsh-system-prompt"}),
            json!({"kind":"user"}),
        ] {
            let mut d = good.data().clone();
            d["message"]["source"] = source;
            error(
                accept(path, patch(&good, "data", d)),
                "system-prompt source",
            );
        }
    }
}
#[test]
fn rejects_invalid_payloads_with_a_located_error_on_every_entry_path() {
    for path in 0..6 {
        for (e, expected) in invalid() {
            let before = e.clone();
            let err = accept(path, e.clone()).unwrap_err().to_string();
            assert!(err.contains(expected), "path {path}: {err}");
            assert!(err.contains("seq 0") || err.contains("index 0"), "{err}");
            assert_eq!(before, e);
        }
    }
}
#[test]
fn accepts_absent_optional_fields_nonempty_fields_and_optional_failure_identity() {
    let mut success = data("tool", "");
    let mut failure = success.clone();
    failure["message"]["isError"] = json!(true);
    let mut with_error = failure.clone();
    with_error["error"] = json!({"name":"ToolError","code":"FAILED"});
    let base = surface_event("tool", 0, "");
    let events = vec![
        req(
            json!({"header":{"config":{"provider":"mock","model":"mock","maxTokens":1}},"reason":"initial"}),
        ),
        req(
            json!({"header":{"config":{"provider":"mock","model":"mock","maxTokens":1},"extension":{"nested":true},"tools":[tool("tool")],"adapterDefaults":{"maxTokens":true}},"reason":"initial"}),
        ),
        patch(&base, "data", success.take()),
        patch(&base, "data", failure),
        patch(&base, "data", with_error),
    ];
    for path in 0..6 {
        for e in &events {
            accept(path, e.clone()).unwrap();
        }
    }
}
#[test]
fn preserves_nested_header_source_and_data_extras_without_normalizing_accepted_events() {
    let a = req(
        json!({"header":{"config":{"provider":"mock","model":"mock"},"extra":{"nested":[true,null]}},"reason":"initial","extra":["retained"]}),
    );
    let b = surface_event("system", 0, "prompt");
    let mut d = b.data().clone();
    d["message"]["source"]["extra"] = json!({"nested":true});
    d["extra"] = json!(["retained"]);
    let b = patch(&b, "data", d);
    for e in [a, b] {
        for path in 0..6 {
            accept(path, e.clone()).unwrap();
        }
        let s = Session::create(
            "test".into(),
            Some(std::slice::from_ref(&e)),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(s.event_at(seq(0)).unwrap(), e);
    }
}
#[test]
fn adopts_an_assistant_message_and_freezes_its_identified_message() {
    let e = surface_event("assistant", 0, "answer");
    let adopted = adopt_session_event((*e).clone()).unwrap();
    assert_eq!(adopted, *e);
    assert_eq!(snapshot_session_event(&e).unwrap(), *e);
}
#[test]
fn rejects_a_non_object_restored_request_config() {
    for config in [Value::Null, json!("invalid")] {
        let e = req(json!({"header":{"config":config},"reason":"initial"}));
        for path in [1, 2] {
            error(accept(path, e.clone()), "lacks provider/model");
        }
    }
}
#[test]
fn updates_the_frozen_request_header_cache_only_for_accepted_snapshots() {
    let s = session();
    request(&s, vec![tool("tool")], "initial");
    let first = s.request_header().unwrap();
    assert!(Arc::ptr_eq(&first, &s.request_header().unwrap()));
    error(s.append("request/header",json!({"header":{"config":{"provider":"mock","model":"mock"},"system":""},"reason":"change"}),None),"must omit header.system");
    assert!(Arc::ptr_eq(&first, &s.request_header().unwrap()));
    request(&s, vec![], "change");
    assert!(s.request_header().unwrap().get("tools").is_none());
    assert_eq!(first["tools"][0]["name"], "tool");
}
#[tokio::test(start_paused = true)]
async fn prepares_owned_restored_events_without_publication() {
    let (k, ctx, store) = mounted().await;
    let calls = Arc::new(Mutex::new(0));
    let c = calls.clone();
    SessionStore::on_created(&ctx, move |_| {
        *lock(&c) += 1;
        Ok(())
    })
    .unwrap();
    for state in [
        SessionSeedEventState::Detached,
        SessionSeedEventState::SharedFrozen,
    ] {
        let e =
            req(json!({"header":{"config":{"provider":"mock","model":"mock"}},"reason":"initial"}));
        let prepared = store
            .prepare(
                Some("test".into()),
                PrepareSessionOptions::Restored(RestoredSessionOptions {
                    seed: vec![e.clone()],
                    meta: header("test"),
                    inherited_event_count: offset(0),
                    event_state: state,
                }),
            )
            .unwrap();
        assert!(Arc::ptr_eq(&prepared.event_at(seq(0)).unwrap(), &e));
        assert!(store.get(prepared.id()).is_none());
        assert_eq!(*lock(&calls), 0);
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_publish_rejected_appends_or_seeds_or_change_derived_state() {
    let (k, ctx, store) = mounted().await;
    let s = store
        .create(&ctx, Some("test".into()), Default::default())
        .unwrap();
    let seen = Arc::new(Mutex::new(0));
    let out = seen.clone();
    ctx.on_contained::<SessionEventEmitted>(move |_| {
        *lock(&out) += 1;
        Ok(())
    })
    .unwrap();
    let before = s.snapshot_events(None, None);
    for (e, _) in invalid() {
        assert!(
            s.append_batch(&[SessionEventInput::new(e.event_type(), e.data().clone())
                .with_options(e.as_value().clone())])
                .is_err()
        );
        assert!(
            store
                .create(
                    &ctx,
                    Some("bad".into()),
                    CreateSessionOptions {
                        seed: Some(vec![e]),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert!(store.get(&"bad".into()).is_none());
        assert!(Arc::ptr_eq(&before, &s.snapshot_events(None, None)));
        assert_eq!(s.seq(), offset(0));
        assert!(s.derive_messages().unwrap().is_empty());
        assert!(s.request_header().is_none());
    }
    assert_eq!(*lock(&seen), 0);
    k.shutdown().await;
}
#[test]
fn reports_malformed_restored_event_envelopes_without_type_error() {
    for value in [Value::Null, json!([]), json!(1), json!("invalid")] {
        error(
            Session::from_restore(
                "test".into(),
                &[Arc::new(SessionEvent::from_value(value))],
                header("test"),
                offset(0),
                SessionSeedEventState::Detached,
                None,
            ),
            "seed event at index 0 has an invalid event envelope",
        );
    }
}
#[test]
fn rejects_noncanonical_replace_keys_on_all_entry_paths() {
    for op in [
        json!({"op":"replace","start":0,"end":0}),
        json!({"op":"replace","start":0,"endSeq":0}),
        json!({"op":"replace","startSeq":0,"end":0}),
        json!({"op":"replace","startSeq":0,"endSeq":0,"start":0}),
        json!({"op":"replace","startSeq":0,"endSeq":0,"end":0}),
        json!({"op":"replace","startSeq":0,"endSeq":0,"extra":true}),
        json!({"op":"replace","startSeq":-1,"endSeq":0}),
    ] {
        let e = patch(&surface_event("user", 0, ""), "surfaceOp", op);
        for path in 0..5 {
            error(accept(path, e.clone()), "invalid replace surfaceOp");
        }
    }
}
#[test]
fn requires_markers_and_forbids_non_surface_and_assistant_source_event_references() {
    for path in 0..6 {
        let a = event("user/message", 0, data("user", ""));
        error(accept(path, a), "requires a surfaceOp marker");
        let a = event("turn/start", 0, json!({"turn":1}));
        error(
            accept(path, patch(&a, "surfaceOp", json!("append"))),
            "not surface-eligible",
        );
        error(
            accept(path, patch(&a, "sourceEventSeqs", json!([0]))),
            "not surface-eligible",
        );
        error(
            accept(
                path,
                patch(
                    &surface_event("assistant", 0, ""),
                    "sourceEventSeqs",
                    json!([0]),
                ),
            ),
            "cannot carry sourceEventSeqs",
        );
    }
}
#[test]
fn retains_opaque_ignorable_metadata_without_deriving_messages() {
    for t in [
        "extension/event",
        "tool/code-dispatch",
        "tool/code-dispatch-start",
    ] {
        let mut v = event(t, 0, json!({"nested":{"retained":true}}))
            .as_value()
            .clone();
        v["ignorable"] = json!(true);
        v["surfaceOp"] = json!({"opaque":["not","placement"]});
        v["sourceEventSeqs"] = json!({"opaque":[null,true]});
        let e = Arc::new(SessionEvent::from_value(v));
        for path in 1..5 {
            accept(path, e.clone()).unwrap();
        }
        let s = Session::create(
            "test".into(),
            Some(std::slice::from_ref(&e)),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(s.event_at(seq(0)), Some(e));
        assert!(nodes(&s).is_empty());
        assert!(s.derive_messages().unwrap().is_empty());
    }
}
#[test]
fn rejects_known_log_only_metadata_even_when_ignorable() {
    for t in [
        "turn/start",
        "assistant/attempt",
        "request/context",
        "session/title",
        "tool/ptc-dispatch",
    ] {
        for key in ["surfaceOp", "sourceEventSeqs"] {
            let mut e = event(t, 0, json!({"turn":1,"step":1,"stream":[]}))
                .as_value()
                .clone();
            e["ignorable"] = json!(true);
            e[key] = if key == "surfaceOp" {
                json!("append")
            } else {
                json!([0])
            };
            let e = Arc::new(SessionEvent::from_value(e));
            for path in 0..6 {
                error(accept(path, e.clone()), "not surface-eligible");
            }
        }
    }
}
#[test]
fn requires_system_placement_and_preserves_system_data_and_source_event_references_on_head_replacements()
 {
    let s = session();
    let mut d = data("system", "head");
    d["extra"] = json!({"nested":true});
    let head = s
        .append("system/message", d.clone(), Some(SurfaceIntent::append()))
        .unwrap();
    d["message"] = message("system", "next");
    let next = s
        .append(
            "system/message",
            d.clone(),
            Some(SurfaceIntent::replace(
                head.seq(),
                head.seq(),
                vec![head.seq()],
            )),
        )
        .unwrap();
    assert_eq!(next.data(), &d);
    assert_eq!(nodes(&s), vec![1]);
    assert_eq!(*s.derive_messages().unwrap()[0], d["message"]);
}
#[test]
fn adoption_rejects_current_or_future_replacement_endpoints() {
    for endpoint in [1, 2] {
        let e = surface_event("user", 1, "");
        let e = patch(
            &e,
            "surfaceOp",
            json!({"op":"replace","startSeq":endpoint,"endSeq":0}),
        );
        error(
            adopt_session_event((*e).clone()),
            "must reference earlier events",
        );
        error(snapshot_session_event(&e), "must reference earlier events");
    }
}
#[test]
fn adoption_validates_only_local_metadata_not_referenced_history_or_endpoint_order() {
    let e = surface_event("user", 12, "");
    let e = patch(
        &e,
        "surfaceOp",
        json!({"op":"replace","startSeq":11,"endSeq":4}),
    );
    let e = patch(&e, "sourceEventSeqs", json!([11, 4]));
    assert_eq!(adopt_session_event((*e).clone()).unwrap(), *e);
    assert_eq!(snapshot_session_event(&e).unwrap(), *e);
}
