mod common;
use common::*;
use hoh_session::surface::SurfaceManager;
use hoh_session::*;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
struct Projection;
impl SessionMessageProjection for Projection {
    fn event_type(&self) -> &str {
        "test/project"
    }
    fn project(
        &self,
        e: &SessionEvent,
        c: SessionMessageProjectionContext<'_>,
    ) -> Result<IndexMap<SessionSeq, Arc<Value>>> {
        if e.data()["text"] == "reject" {
            return Err("rejected decision".into());
        }
        let seq = SessionSeq::new(e.data()["seq"].as_u64().unwrap())?;
        let source = &c.events[(u64::from(seq) - u64::from(c.base_seq)) as usize];
        let original = derive_event_message(source, Some(c.messages)).unwrap();
        let mut m = (*original).clone();
        m["content"] = json!([{"type":"text","text":e.data()["text"]}]);
        Ok(IndexMap::from([(seq, Arc::new(m))]))
    }
}
fn defs() -> MessageProjections {
    Arc::new(Mutex::new(vec![Arc::new(Projection)]))
}
#[test]
fn refuses_required_events_when_no_interpreter_is_supplied() {
    let s = session();
    error(
        s.append("image/offload", json!({}), None),
        "requires a message projection",
    );
    let e = event("image/offload", 0, json!({}));
    error(
        fold_surface(std::slice::from_ref(&e), &[]),
        "requires a message projection",
    );
    error(
        Session::create("test".into(), Some(&[e]), None, None, None),
        "requires a message projection",
    );
    assert_eq!(s.seq(), offset(0));
}
#[test]
fn applies_generic_decisions_atomically_and_replays_them_through_detached_folds() {
    let definitions = defs();
    let s = Session::create("test".into(), None, None, None, Some(definitions.clone())).unwrap();
    let source = user(&s, "original");
    let original = s.derive_messages().unwrap();
    error(
        s.append(
            "test/project",
            json!({"seq":source.seq(),"text":"reject"}),
            None,
        ),
        "rejected decision",
    );
    assert_eq!(s.derive_messages().unwrap(), original);
    s.append(
        "test/project",
        json!({"seq":source.seq(),"text":"projected"}),
        None,
    )
    .unwrap();
    assert_eq!(texts(&s), vec!["projected"]);
    assert_eq!(s.surface().unwrap().replace_generation().unwrap(), 0);
    assert_eq!(s.surface().unwrap().content_generation().unwrap(), 1);
    let f = fold_surface(&s.snapshot_events(None, None), &lock(&definitions)).unwrap();
    assert_eq!(
        derive_event_message(&source, Some(&f.projected_messages)),
        s.derive_messages().unwrap().first().cloned()
    );
    assert_eq!(source.data()["content"][0]["text"], "original");
}
#[test]
fn admits_message_projections_through_array_append() {
    for text in ["projected", "reject"] {
        let s = Session::create("test".into(), None, None, None, Some(defs())).unwrap();
        let out = s.append_batch(&[
            SessionEventInput::new("user/message", data("user", "original"))
                .with_surface(SurfaceIntent::append()),
            SessionEventInput::new("test/project", json!({"seq":0,"text":text})),
        ]);
        if text == "reject" {
            error(out, "rejected decision");
            assert_eq!(s.seq(), offset(0));
            assert!(s.derive_messages().unwrap().is_empty());
        } else {
            out.unwrap();
            assert_eq!(texts(&s), vec!["projected"]);
            assert_eq!(s.surface().unwrap().content_generation().unwrap(), 1);
        }
    }
}
#[tokio::test(start_paused = true)]
async fn registers_by_fiber_and_refuses_cached_or_pending_decisions_after_disposal() {
    let (k, ctx, store) = mounted().await;
    let owner = ctx.child("projection-owner").unwrap();
    let registration = store
        .register_message_projection(&owner, Arc::new(Projection))
        .unwrap();
    error(
        store.register_message_projection(&ctx, Arc::new(Projection)),
        "already registered",
    );
    let live = store
        .create(&ctx, Some("live".into()), Default::default())
        .unwrap();
    let source = user(&live, "original");
    live.append(
        "test/project",
        json!({"seq":source.seq(),"text":"changed"}),
        None,
    )
    .unwrap();
    let before = live.derive_messages().unwrap();
    let child = store
        .fork(&ctx, SessionForkSource::Session(live.clone()), None, None)
        .unwrap();
    let restored = store
        .prepare(
            Some("restore".into()),
            PrepareSessionOptions::Restored(RestoredSessionOptions {
                seed: live.snapshot_events(None, None).to_vec(),
                meta: {
                    let mut h = (*live.header()).clone();
                    h["id"] = json!("restore");
                    h
                },
                inherited_event_count: offset(0),
                event_state: SessionSeedEventState::SharedFrozen,
            }),
        )
        .unwrap();
    registration.dispose().await;
    assert!(lock(&store.message_projections()).is_empty());
    for s in [live, child, restored] {
        error(s.derive_messages(), "was removed or replaced");
        error(s.derive_event_message(&source), "was removed or replaced");
        error(s.surface(), "was removed or replaced");
        assert!(s.append("turn/start", json!({"turn":1}), None).is_err());
    }
    assert_eq!(before[0]["content"][0]["text"], "changed");
    k.shutdown().await;
}
#[test]
fn applies_a_supplied_interpreter_to_a_loaded_window_with_absolute_sequences() {
    let definitions = defs();
    let s = Session::create("test".into(), None, None, None, Some(definitions.clone())).unwrap();
    user(&s, "first");
    let source = user(&s, "original");
    s.append(
        "test/project",
        json!({"seq":source.seq(),"text":"window"}),
        None,
    )
    .unwrap();
    let manager = SurfaceManager::new(
        s.snapshot_events(Some(offset(1)), None).to_vec(),
        offset(1),
        definitions,
    )
    .unwrap();
    assert_eq!(manager.surface().unwrap().nodes, vec![source.seq()]);
    assert_eq!(
        manager.derive_event_message(&source).unwrap().unwrap()["content"][0]["text"],
        "window"
    );
}
#[test]
fn does_not_retain_an_interpreter_for_a_candidate_that_never_committed() {
    let definitions = defs();
    let s = session();
    let source = user(&s, "original");
    let manager = SurfaceManager::new(
        s.snapshot_events(None, None).to_vec(),
        offset(0),
        definitions.clone(),
    )
    .unwrap();
    manager
        .validate_next(&event("test/project", 1, json!({"seq":0,"text":"unused"})))
        .unwrap();
    lock(&definitions).clear();
    assert_eq!(
        *manager.derive_event_message(&source).unwrap().unwrap(),
        *source.data()
    );
}
