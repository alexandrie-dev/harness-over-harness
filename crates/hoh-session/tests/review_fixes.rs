mod common;
use common::*;
use hoh_session::*;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[test]
fn projection_context_excludes_a_single_candidate_and_includes_all_batch_candidates() {
    struct Inspect(Arc<Mutex<Vec<(u64, usize)>>>);
    impl SessionMessageProjection for Inspect {
        fn event_type(&self) -> &str {
            "test/inspect"
        }
        fn project(
            &self,
            event: &SessionEvent,
            ctx: SessionMessageProjectionContext<'_>,
        ) -> Result<IndexMap<SessionSeq, Arc<Value>>> {
            lock(&self.0).push((event.seq().get(), ctx.events.len()));
            Ok(IndexMap::new())
        }
    }
    let seen = Arc::new(Mutex::new(vec![]));
    let definitions: MessageProjections =
        Arc::new(Mutex::new(vec![Arc::new(Inspect(seen.clone()))]));
    let s = Session::create("test".into(), None, None, None, Some(definitions)).unwrap();
    s.append("test/inspect", json!({}), None).unwrap();
    s.append_batch(&[
        SessionEventInput::new("test/inspect", json!({})),
        SessionEventInput::new("test/inspect", json!({})),
    ])
    .unwrap();
    assert_eq!(*lock(&seen), vec![(0, 0), (1, 3), (2, 3)]);
}

#[tokio::test(start_paused = true)]
#[ignore = "blocked: hoh-kernel Callbacks needs a stop-on-synchronous-error invocation facility"]
async fn raw_creation_veto_stops_the_next_raw_listener() {
    let (k, ctx, store) = mounted().await;
    ctx.on_contained::<SessionCreated>(|_| Err("raw veto".into()))
        .unwrap();
    let seen = Arc::new(Mutex::new(0));
    let out = seen.clone();
    ctx.on_contained::<SessionCreated>(move |_| {
        *lock(&out) += 1;
        Ok(())
    })
    .unwrap();
    error(store.create(&ctx, None, Default::default()), "raw veto");
    assert_eq!(*lock(&seen), 0);
    k.shutdown().await;
}

#[test]
fn batch_snapshots_all_members_before_validating_any_payload() {
    let s = session();
    let result = s.append_batch(&[
        SessionEventInput::new("request/header", json!({"header":{"system":"obsolete"}})),
        SessionEventInput::new("test/lossy", json!(-0.0)),
    ]);
    error(
        result,
        "session batch member 1 (\"test/lossy\") carries non-JSON-serializable data",
    );
    assert_eq!(s.seq(), offset(0));
}

#[test]
fn repair_accepts_the_sources_negative_one_tail_watermark_without_panicking() {
    let tail = Arc::new(SessionEvent::from_value(
        json!({"type":"turn/start","seq":-1,"time":0,"data":{"turn":1}}),
    ));
    let result = std::panic::catch_unwind(|| interrupted_turn_closers(&[tail]));
    assert!(result.is_ok(), "repair must not panic on log input");
    let events = result.unwrap().unwrap();
    assert_eq!(
        hoh_util_values::js_json_stringify(events[0].as_value()),
        "{\"type\":\"turn/end\",\"seq\":0,\"time\":0,\"data\":{\"turn\":1,\"reason\":{\"kind\":\"interrupted\"}}}"
    );
    for (raw, expected) in [
        (json!(-1), 0),
        (json!(null), 1),
        (json!(true), 2),
        (json!("2"), 201),
        (json!("0x"), 1),
    ] {
        let event = Arc::new(SessionEvent::from_value(
            json!({"type":"turn/start","seq":raw,"time":"raw-time","data":{"turn":1}}),
        ));
        let repaired = interrupted_turn_closers(std::slice::from_ref(&event)).unwrap();
        assert_eq!(repaired[0].seq(), seq(expected));
        assert_eq!(repaired[0].as_value()["time"], "raw-time");
        let forked = build_fork_seed(&[event], seq(0)).unwrap();
        assert_eq!(forked[1].as_value()["time"], "raw-time");
    }
    for raw in [json!(-2), json!(0.5), json!(MAX_SAFE_INTEGER)] {
        let event = Arc::new(SessionEvent::from_value(
            json!({"type":"turn/start","seq":raw,"time":null,"data":{"turn":1}}),
        ));
        assert!(interrupted_turn_closers(&[event]).is_err());
    }
}

#[test]
fn appends_one_hundred_thousand_events_within_fifteen_seconds() {
    // Debug build, at least one available 2 GHz x86-64 core; no prolonged scheduling pause.
    let s = session();
    let start = std::time::Instant::now();
    for i in 0..100_000 {
        if i % 2 == 0 {
            s.append(
                "user/message",
                json!({"id":"small","role":"user","source":{"kind":"user"},"content":[]}),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
        } else {
            s.append("test/small", json!({"i":i}), None).unwrap();
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(15),
            "100k appends exceeded 15 s at {i}"
        );
    }
    assert_eq!(s.seq(), offset(100_000));
    assert_eq!(s.surface().unwrap().nodes().unwrap().len(), 50_000);
}

#[test]
fn depth_4096_append_seed_fork_derive_and_drop_on_the_application_stack() {
    std::thread::Builder::new()
        .name("session-depth-4096".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let mut content = json!("leaf");
            for _ in 0..4096 {
                content = Value::Array(vec![content]);
            }
            let mut message = message("user", "deep");
            message["extension"] = content;
            let s = session();
            s.append("user/message", message, Some(SurfaceIntent::append()))
                .unwrap();
            let seed = s.snapshot_events(None, None);
            let restored =
                Session::create("restored".into(), Some(&seed), None, None, None).unwrap();
            let fork_seed = build_fork_seed(&seed, seq(0)).unwrap();
            let mut h = header("child");
            h["isSeeded"] = json!(true);
            let child = Session::create(
                "child".into(),
                Some(&fork_seed),
                Some(h),
                Some(offset(1)),
                None,
            )
            .unwrap();
            assert_eq!(restored.derive_messages().unwrap().len(), 1);
            assert_eq!(child.derive_messages().unwrap().len(), 1);
            assert!(hoh_util_values::deep_equal_json(
                seed[0].data(),
                restored.event_at(seq(0)).unwrap().data()
            ));
        })
        .expect("spawn the application-sized stack")
        .join()
        .expect("depth 4096 operations and drops must succeed");
}

#[test]
fn validates_header_version_as_a_javascript_number() {
    for version in [json!(4), Value::from(4.0)] {
        let mut h = header("test");
        h["version"] = version;
        Session::from_restore(
            "test".into(),
            &[],
            h,
            offset(0),
            SessionSeedEventState::Detached,
            None,
        )
        .unwrap();
    }
    for (version, expected) in [
        (json!(4.5), "session header version must be 4, got 4.5"),
        (json!("4"), "session header version must be 4, got 4"),
    ] {
        let mut h = header("test");
        h["version"] = version;
        let result = Session::from_restore(
            "test".into(),
            &[],
            h,
            offset(0),
            SessionSeedEventState::Detached,
            None,
        );
        assert_eq!(result.expect_err("invalid version").to_string(), expected);
    }
}

#[test]
fn persisted_integral_float_version_is_normalized_by_the_shared_parser() {
    let h = hoh_util_values::js_json_parse(
        r#"{"version":4.0,"id":"test","createdAt":1,"isSeeded":false}"#,
    )
    .unwrap();
    Session::from_restore(
        "test".into(),
        &[],
        h,
        offset(0),
        SessionSeedEventState::Detached,
        None,
    )
    .unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_failed_append_releases_planning_views_before_detach_allows_reentry() {
    let (kernel, ctx, store) = mounted().await;
    let session = store
        .prepare(Some("rebound".into()), Default::default())
        .unwrap();
    let detach = store.enter(&ctx, session.clone()).unwrap();
    store.announce(&session).unwrap();
    ctx.on_dispatch(move |_, name, payload| {
        if name == "session/event" {
            let event = payload.downcast_ref::<SessionEventEmitted>().unwrap();
            if event.event.event_type() == "test/rejected" {
                detach.detach();
                event.validation.reject("reject before commit".into());
            }
        }
    })
    .unwrap();
    let rebound = SessionStore::new();
    let owner = ctx.clone();
    let retained = Arc::new(Mutex::new(None));
    let output = retained.clone();
    let new_store = rebound.clone();
    let once = Arc::new(std::sync::atomic::AtomicBool::new(false));
    ctx.on_contained::<SessionDisposed>(move |event| {
        if !once.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let detach = new_store.enter(&owner, event.session.clone())?;
            new_store.announce(&event.session)?;
            event.session.append("test/rebound", json!({}), None)?;
            *lock(&output) = Some(detach);
        }
        Ok(())
    })
    .unwrap();
    error(
        session.append("test/rejected", json!({}), None),
        "reject before commit",
    );
    assert_eq!(session.seq(), offset(1));
    assert_eq!(
        session.event_at(seq(0)).unwrap().event_type(),
        "test/rebound"
    );
    lock(&retained).take().unwrap().detach();
    kernel.shutdown().await;
}
