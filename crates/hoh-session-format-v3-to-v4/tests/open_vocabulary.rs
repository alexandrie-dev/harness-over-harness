mod common;
use common::*;
#[test]
fn keeps_unknown_ignorable_payloads_and_coordinates_opaque_to_every_native_semantic_pass() {
    let mut rows: Vec<_> = [
        "tool/result",
        "system/message",
        "request/header",
        "session/title-llm-request",
        "subagent/catalog",
        "session-log-deepseek/delivery-accepted",
        "compaction/start",
        "compaction/end",
        "session/end-seed",
    ]
    .iter()
    .map(|tag| {
        let mut e = ev(tag, Value::Null);
        e.insert("ignorable".into(), true.into());
        e
    })
    .collect();
    rows.push(ev("turn/start", json!({"turn":1})));
    rows.push(ev("turn/end", json!({"turn":1})));
    let a = artifact(dense(rows), h(4, "open-vocabulary"), 0);
    let k = ["turn/start", "turn/end"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let before = hoh_util_values::js_json_stringify(
        &json!({"header":a.header,"events":a.events,"inheritedEventCount":0}),
    );
    assert_eq!(restore_released_v4_artifact(a.clone(), &k).unwrap(), a);
    assert_eq!(
        hoh_util_values::js_json_stringify(
            &json!({"header":a.header,"events":a.events,"inheritedEventCount":0})
        ),
        before
    );
    assert_eq!(a.events.last().unwrap()["seq"], 10);
}
#[test]
fn does_not_let_an_opaque_end_seed_clear_an_owned_compaction() {
    let mut seed = ev("session/end-seed", json!({}));
    seed.insert("ignorable".into(), true.into());
    let rows = dense(vec![
        ev(
            "compaction/start",
            json!({"compactionId":"owner","turn":null}),
        ),
        seed,
        ev("turn/start", json!({"turn":1})),
    ]);
    let k = ["compaction/start", "turn/start"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    err(
        restore_released_v4_artifact(artifact(rows, h(4, "open-vocabulary"), 0), &k),
        "crosses an open compaction",
    );
}
#[test]
fn does_not_reinterpret_an_unknown_ignorable_user_row_through_a_known_title_citation() {
    let mut row = ev("user/message", Value::Null);
    row.insert("ignorable".into(), true.into());
    let rows = dense(vec![
        row,
        ev(
            "session/title",
            json!({"title":"title","source":{"kind":"generated"},"messageSeqs":[0]}),
        ),
    ]);
    let k = ["session/title"].into_iter().map(str::to_owned).collect();
    err(
        restore_released_v4_artifact(artifact(rows, h(4, "open-vocabulary"), 0), &k),
        "earlier human user/message",
    );
}
#[test]
fn still_validates_known_events_marked_ignorable() {
    let mut row = ev("tool/result", Value::Null);
    row.insert("ignorable".into(), true.into());
    err(native(vec![row]), "data must be an object");
}
#[test]
fn still_validates_unknown_envelopes_and_refuses_unknown_required_types() {
    let row = ev("external/event", Value::Null);
    err(
        restore_released_v4_artifact(
            artifact(vec![row.clone()], h(4, "open-vocabulary"), 0),
            &HashSet::new(),
        ),
        "unknown event type",
    );
    let mut row = row;
    row.insert("ignorable".into(), true.into());
    row.insert("seq".into(), 3.into());
    err(
        restore_released_v4_artifact(
            artifact(vec![row], h(4, "open-vocabulary"), 0),
            &HashSet::new(),
        ),
        "not dense",
    );
}
