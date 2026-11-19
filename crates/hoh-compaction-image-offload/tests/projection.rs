//! Port of projection.spec.ts: durable image selections, replay and rejection of malformed ones.
mod support;

use hoh_compaction_image_offload::image_offload_projection;
use hoh_session::{
    MessageProjections, Session, SessionEvent, SessionId, SessionLogOffset, SessionSeedEventState,
    SessionSeq, SurfaceIntent, derive_event_message, fold_surface,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::*;

fn projections() -> MessageProjections {
    Arc::new(Mutex::new(vec![image_offload_projection()]))
}

/// `createSession(...)`: `Session.create` with the offload projection installed.
fn create_session(
    id: &str,
    seed: Option<&[Arc<SessionEvent>]>,
    header: Option<Value>,
    inherited: Option<SessionLogOffset>,
) -> hoh_session::Result<Session> {
    Session::create(
        SessionId::from(id),
        seed,
        header,
        inherited,
        Some(projections()),
    )
}

fn input(session: &Session, content: Vec<Value>) -> Arc<SessionEvent> {
    session
        .append(
            "user/message",
            user_message(content),
            Some(SurfaceIntent::append()),
        )
        .expect("user message appends")
}

fn input_images(session: &Session) -> Arc<SessionEvent> {
    input(session, vec![image(), image()])
}

fn offload(session: &Session, targets: Value) -> hoh_session::Result<Arc<SessionEvent>> {
    session.append("image/offload", json!({ "targets": targets }), None)
}

fn marked(session: &Session) -> Vec<bool> {
    let mut result = vec![];
    for message in session.derive_messages().expect("messages derive") {
        for block in message["content"].as_array().expect("content") {
            if block["type"] == "image" {
                result.push(block["offloaded"] == true);
            }
        }
    }
    result
}

fn generations(session: &Session) -> (Vec<SessionSeq>, u64, u64) {
    let surface = session.surface().expect("surface");
    (
        surface.nodes().expect("nodes"),
        surface.replace_generation().expect("replace generation"),
        surface.content_generation().expect("content generation"),
    )
}

fn assert_image_offload_error(result: hoh_session::Result<impl std::fmt::Debug>, what: &str) {
    let error = result.expect_err(what);
    assert!(
        error.to_string().contains("image/offload"),
        "{what}: {error}"
    );
}

#[test]
fn preserves_message_identity_original_data_and_previously_derived_snapshots() {
    let session = create_session("images", None, None, None).unwrap();
    let source = input(
        &session,
        vec![
            text("before"),
            image(),
            image(),
            text("after"),
            text("unchanged"),
            image(),
        ],
    );
    let before = session.derive_messages().unwrap();
    offload(
        &session,
        json!([{ "seq": source.seq(), "imageIndexes": [1] }]),
    )
    .unwrap();
    assert_eq!(marked(&session), [false, true, false]);
    let after = session.derive_messages().unwrap();
    assert_eq!(after[0]["id"], before[0]["id"]);
    assert!(Arc::ptr_eq(
        &after[0],
        &session.derive_event_message(&source).unwrap().unwrap()
    ));
    // `Object.isFrozen` has no counterpart: `Arc<Value>` is immutable by type.
    assert_eq!(after[0]["content"][0], before[0]["content"][0]);
    assert_eq!(after[0]["content"][3], before[0]["content"][3]);
    assert!(!source.as_value().to_string().contains("offloaded"));
    assert!(
        !serde_json::to_string(&before)
            .unwrap()
            .contains("offloaded")
    );
    let (nodes, replace_generation, content_generation) = generations(&session);
    assert_eq!(nodes, [source.seq()]);
    assert_eq!(replace_generation, 0);
    assert_eq!(content_generation, 1);
    let events = session.snapshot_events(None, None);
    assert!(
        session
            .derive_event_message(events.last().unwrap())
            .unwrap()
            .is_none()
    );
    offload(
        &session,
        json!([{ "seq": source.seq(), "imageIndexes": [0, 2] }]),
    )
    .unwrap();
    assert_eq!(marked(&session), [true, true, true]);
    assert_eq!(generations(&session).2, 2);
    assert!(!Arc::ptr_eq(
        &after[0],
        &session.derive_messages().unwrap()[0]
    ));
}

#[test]
fn reconstructs_the_same_selections_through_pure_folding_resume_restore_and_fork() {
    let session = create_session("source", None, None, None).unwrap();
    let source = input_images(&session);
    let before = session.snapshot_events(None, None);
    offload(
        &session,
        json!([{ "seq": source.seq(), "imageIndexes": [0] }]),
    )
    .unwrap();
    let events = session.snapshot_events(None, None);
    let fold = fold_surface(&events, &[image_offload_projection()]).unwrap();
    assert_eq!(
        derive_event_message(&source, Some(&fold.projected_messages)).unwrap(),
        session.derive_messages().unwrap()[0]
    );
    assert_eq!(
        marked(&create_session("before", Some(&before), None, None).unwrap()),
        [false, false]
    );
    assert_eq!(
        marked(&create_session("resume", Some(&events), None, None).unwrap()),
        [true, false]
    );
    let restored = Session::from_restore(
        session.id().clone(),
        &events,
        (*session.header()).clone(),
        SessionLogOffset::default(),
        SessionSeedEventState::SharedFrozen,
        Some(projections()),
    )
    .unwrap();
    assert_eq!(marked(&restored), [true, false]);
    let mut header = (*session.header()).clone();
    header["id"] = json!("child");
    header["parentSession"] = json!(session.id());
    header["isSeeded"] = json!(true);
    let child = create_session(
        "child",
        Some(&events),
        Some(header),
        Some(SessionLogOffset::new(events.len() as u64).unwrap()),
    )
    .unwrap();
    assert_eq!(marked(&child), [true, false]);
    input(&child, vec![image()]);
    assert_eq!(marked(&child), [true, false, false]);
    assert_eq!(marked(&session), [true, false]);
}

#[test]
fn counts_all_images_in_a_tool_result_without_changing_its_call_identity() {
    let session = create_session("tool", None, None, None).unwrap();
    let source = session
        .append(
            "tool/result",
            json!({
                "turn": 1, "step": 1,
                "message": tool_result_message("shot", vec![image(), image()]),
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    offload(
        &session,
        json!([{ "seq": source.seq(), "imageIndexes": [1] }]),
    )
    .unwrap();
    assert_eq!(marked(&session), [false, true]);
    assert_eq!(
        session.derive_event_message(&source).unwrap().unwrap()["source"],
        source.data()["message"]["source"]
    );
}

/// One row of the `it.each` table: the malformed `data` is rejected at append and on replay,
/// without partial application, and the session stays usable.
fn rejects_malformed_selection(data: Value) {
    let session = create_session("invalid", None, None, None).unwrap();
    input_images(&session);
    let events = session.snapshot_events(None, None);
    let candidate = Arc::new(SessionEvent::from_value(
        json!({ "type": "image/offload", "seq": 1, "time": 0, "data": data }),
    ));
    assert_image_offload_error(
        session.append("image/offload", data.clone(), None),
        "append",
    );
    let seeded: Vec<Arc<SessionEvent>> =
        events.iter().cloned().chain([candidate.clone()]).collect();
    assert_image_offload_error(create_session("seed", Some(&seeded), None, None), "seed");
    assert_image_offload_error(fold_surface(&seeded, &[image_offload_projection()]), "fold");
    assert_eq!(*session.snapshot_events(None, None), *events);
    assert_eq!(marked(&session), [false, false]);
    assert_eq!(generations(&session).2, 0);
    offload(&session, json!([{ "seq": 0, "imageIndexes": [1] }])).unwrap();
    assert_eq!(marked(&session), [false, true]);
}

macro_rules! rejects_malformed {
    ($($name:ident: $data:tt,)*) => {$(
        #[test]
        fn $name() {
            rejects_malformed_selection(json!($data));
        }
    )*};
}

rejects_malformed! {
    rejects_malformed_selections_null: null,
    rejects_malformed_selections_empty_object: {},
    rejects_malformed_selections_empty_targets: { "targets": [] },
    rejects_malformed_selections_targets_not_an_array: { "targets": "bad" },
    rejects_malformed_selections_null_target: { "targets": [null] },
    rejects_malformed_selections_empty_image_indexes: { "targets": [{ "seq": 0, "imageIndexes": [] }] },
    rejects_malformed_selections_extra_target_key: { "targets": [{ "seq": 0, "imageIndexes": [0], "extra": true }] },
    rejects_malformed_selections_extra_data_key: { "targets": [{ "seq": 0, "imageIndexes": [0] }], "extra": true },
    rejects_malformed_selections_negative_seq: { "targets": [{ "seq": -1, "imageIndexes": [0] }] },
    rejects_malformed_selections_negative_index: { "targets": [{ "seq": 0, "imageIndexes": [-1] }] },
    rejects_malformed_selections_fractional_index: { "targets": [{ "seq": 0, "imageIndexes": [0.5] }] },
    rejects_malformed_selections_string_index: { "targets": [{ "seq": 0, "imageIndexes": ["0"] }] },
    rejects_malformed_selections_decreasing_indexes: { "targets": [{ "seq": 0, "imageIndexes": [1, 0] }] },
    rejects_malformed_selections_repeated_index: { "targets": [{ "seq": 0, "imageIndexes": [0, 0] }] },
    rejects_malformed_selections_missing_image: { "targets": [{ "seq": 0, "imageIndexes": [2] }] },
    rejects_malformed_selections_not_a_surface_node: { "targets": [{ "seq": 1, "imageIndexes": [0] }] },
    rejects_malformed_selections_duplicate_seq: { "targets": [{ "seq": 0, "imageIndexes": [0] }, { "seq": 0, "imageIndexes": [1] }] },
}

#[test]
fn rejects_repeated_offloads_and_shadowed_or_assistant_targets() {
    let session = create_session("invalid-targets", None, None, None).unwrap();
    let source = input_images(&session);
    offload(
        &session,
        json!([{ "seq": source.seq(), "imageIndexes": [0] }]),
    )
    .unwrap();
    let repeated = offload(
        &session,
        json!([{ "seq": source.seq(), "imageIndexes": [0] }]),
    );
    assert!(
        repeated
            .unwrap_err()
            .to_string()
            .contains("already offloaded")
    );
    session
        .append(
            "user/message",
            user_message(vec![text("summary")]),
            Some(SurfaceIntent::replace(
                source.seq(),
                source.seq(),
                vec![source.seq()],
            )),
        )
        .unwrap();
    let shadowed = offload(
        &session,
        json!([{ "seq": source.seq(), "imageIndexes": [1] }]),
    );
    assert!(shadowed.unwrap_err().to_string().contains("not a current"));
    let assistant = session
        .append(
            "assistant/message",
            json!({
                "turn": 1, "step": 1, "stream": [],
                "message": assistant_message(vec![image()]),
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let rejected = offload(
        &session,
        json!([{ "seq": assistant.seq(), "imageIndexes": [0] }]),
    );
    assert!(
        rejected
            .unwrap_err()
            .to_string()
            .contains("must be user/message")
    );
}
