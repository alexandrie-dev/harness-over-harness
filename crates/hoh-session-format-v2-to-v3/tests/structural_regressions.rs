mod common;
use common::*;
use hoh_session_format::*;
use serde_json::{Value, json};
fn timed(rows: Vec<SessionFormatEvent>) -> Vec<SessionFormatEvent> {
    let mut rows = dense(rows);
    for (i, row) in rows.iter_mut().enumerate() {
        row.insert("time".into(), (-100 + i as i64 * 7).into());
    }
    rows
}
fn step_open(turn: u64) -> Vec<SessionFormatEvent> {
    vec![
        ev("turn/start", json!({"turn":turn})),
        ev("step/start", json!({"turn":turn,"step":1})),
    ]
}
fn step_close(turn: u64, step: u64) -> Vec<SessionFormatEvent> {
    vec![
        ev("step/end", json!({"turn":turn,"step":step})),
        ev(
            "turn/end",
            json!({"turn":turn,"reason":{"kind":"completed"}}),
        ),
    ]
}
fn turn(number: u64, prompt: &str) -> Vec<SessionFormatEvent> {
    let mut rows = step_open(number);
    rows.extend([user(&format!("user-{number}")), request(Some(prompt))]);
    rows.extend(step_close(number, 1));
    rows
}
fn visible(role: &str, text: &str) -> Value {
    json!({"role":role,"content":[{"type":"text","text":text}]})
}
fn carry(index: usize, mut message: Value) -> SessionFormatEvent {
    match index {
        0 => surface("user/message", message),
        1 => {
            message["role"] = "assistant".into();
            message["source"] = model();
            surface(
                "assistant/message",
                json!({"turn":1,"step":1,"stream":[],"message":message}),
            )
        }
        2 => {
            message["source"] = json!({"kind":"tool","callId":"call"});
            message["content"] = json!([{"type":"tool-result","toolCallId":"call","content":[]}]);
            surface("tool/result", json!({"turn":1,"step":1,"message":message}))
        }
        3 => ev(
            "agent/inbox/spliced",
            json!({"target":"next-turn","start":0,"inserted":[message]}),
        ),
        4 => {
            message["source"] = json!({"kind":"plugin","plugin":"dsh-session-title-llm"});
            title(2, json!([message]))
        }
        _ => unreachable!(),
    }
}
fn tool_prefix() -> Vec<SessionFormatEvent> {
    vec![
        assistant(
            json!([{"type":"tool-call","id":"call","name":"read","arguments":"{}"}]),
            json!([]),
        ),
        ev(
            "tool/call",
            json!({"turn":1,"step":1,"callId":"call","name":"read","arguments":"{}"}),
        ),
    ]
}
#[test]
fn preserves_every_request_across_step_and_turn_changes_repeated_text_clear_and_reintroduction() {
    let mut input = opening();
    input.extend([
        user("a"),
        request(Some("alpha")),
        closing()[0].clone(),
        ev("step/start", json!({"turn":1,"step":2})),
        user("b"),
        request(Some("beta")),
        request(Some("beta")),
    ]);
    input.extend(step_close(1, 2));
    input.extend(step_open(2));
    input.extend([user("c"), request(None), request(Some("alpha"))]);
    input.extend(step_close(2, 1));
    let input = timed(input);
    let before = json!(input);
    let a = migrate(input.clone()).unwrap();
    let ua = visible("user", "a");
    let ub = visible("user", "b");
    let uc = visible("user", "c");
    assert_eq!(
        requests(&a.events, 3),
        json!([
            [visible("system", "alpha"), ua],
            [visible("system", "beta"), ua, ub],
            [visible("system", "beta"), ua, ub],
            [ua, ub, uc],
            [visible("system", "alpha"), ua, ub, uc]
        ])
    );
    let rows: Vec<_> = systems(&a)
        .into_iter()
        .map(|row| {
            json!([
                row["seq"],
                row["time"],
                row["data"]["turn"],
                row["data"]["step"],
                row["surfaceOp"],
                row.get("sourceEventSeqs")
            ])
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            json!([2, input[1]["time"], 1, 1, "append", null]),
            json!([4,input[3]["time"],1,1,{"op":"replace","startSeq":2,"endSeq":2},[2]]),
            json!([9,input[7]["time"],1,2,{"op":"replace","startSeq":4,"endSeq":4},[4]]),
            json!([17,input[14]["time"],2,1,{"op":"replace","startSeq":9,"endSeq":9},[9]]),
            json!([19,input[15]["time"],2,1,{"op":"replace","startSeq":17,"endSeq":17},[17]])
        ]
    );
    assert_eq!(
        a.events
            .iter()
            .filter(|e| e["type"] == "user/message")
            .map(|e| e["data"].clone())
            .collect::<Vec<_>>(),
        vec![user_data("a"), user_data("b"), user_data("c")]
    );
    assert_eq!(
        a.events
            .iter()
            .filter(|e| e["type"] != "system/message")
            .map(|e| (&e["type"], &e["time"]))
            .collect::<Vec<_>>(),
        input
            .iter()
            .map(|e| (&e["type"], &e["time"]))
            .collect::<Vec<_>>()
    );
    for e in a.events.iter().filter(|e| e["type"] == "request/header") {
        assert_eq!(
            e["data"],
            json!({"reason":"initial","header":{"config":{"provider":"mock","model":"mock"}}})
        );
    }
    assert_eq!(round_trip(a.clone()).unwrap(), a);
    assert_eq!(json!(input), before);
}
#[test]
fn keeps_generated_identities_stable_across_reads_but_distinct_across_session_ids_and_source_anchors()
 {
    let mut input = opening();
    input.extend([
        request(Some("alpha")),
        request(Some("beta")),
        request(Some("alpha")),
    ]);
    let input = timed(input);
    let ids = |a: &SessionFormatArtifact| {
        systems(a)
            .iter()
            .map(|e| e["data"]["message"]["id"].clone())
            .collect::<Vec<_>>()
    };
    let original = ids(&migrate(input.clone()).unwrap());
    assert_eq!(
        original
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        4
    );
    assert_eq!(ids(&migrate(input.clone()).unwrap()), original);
    let other = restore(raw(input.clone(), h(2, "another-session"), None).unwrap()).unwrap();
    assert!(ids(&other).iter().all(|id| !original.contains(id)));
    let mut changed = input;
    for e in &mut changed {
        e.insert("time".into(), (e["time"].as_i64().unwrap() + 1000).into());
    }
    assert_eq!(ids(&migrate(changed).unwrap()), original);
}
#[test]
fn refuses_generated_head_ids_in_later_message_carriers() {
    for index in 0..5 {
        let mut prefix = opening();
        prefix.extend([user("human"), request(Some("prompt"))]);
        if index == 2 {
            prefix.extend(tool_prefix());
        }
        let prefix = timed(prefix);
        let a = migrate(prefix.clone()).unwrap();
        let id = systems(&a)[0]["data"]["message"]["id"].as_str().unwrap();
        let ordinary = carry(index, user_data("ordinary"));
        let mut control = prefix.clone();
        control.push(ordinary.clone());
        control.extend(closing());
        let control = migrate(timed(control)).unwrap();
        let mut expected = ordinary["data"].clone();
        if index == 4 {
            expected["messageSeqs"] = json!([3]);
        }
        assert_eq!(control.events[control.events.len() - 3]["data"], expected);
        round_trip(control).unwrap();
        let mut bad = prefix;
        bad.push(carry(index, user_data(id)));
        bad.extend(closing());
        let bad = timed(bad);
        let before = json!(bad);
        let error = migrate(bad.clone()).unwrap_err();
        assert!(error.is_unsupported_migration());
        assert!(error.to_string().contains("source message id collides"));
        assert_eq!(json!(bad), before);
    }
}
#[test]
fn maps_the_last_inherited_marker_independently_of_later_local_prompt_insertions() {
    for cut in [None, Some(13)] {
        let mut rows = turn(1, "grandparent");
        rows.push(ev("session/end-seed", json!({"inherited":true})));
        rows.extend(turn(2, "parent"));
        rows.push(ev("session/end-seed", json!({"inherited":true})));
        rows.extend(turn(3, "local"));
        rows.push(ev("session/end-seed", json!({})));
        let input = timed(rows);
        let a = restore(raw(input.clone(), seeded_header(), cut).unwrap()).unwrap();
        assert_eq!(a.inherited_event_count, 16);
        let markers: Vec<_> = a
            .events
            .iter()
            .filter(|e| e["type"] == "session/end-seed")
            .cloned()
            .collect();
        let expected: Vec<_> = [(6, 8), (13, 16), (20, 24)]
            .into_iter()
            .map(|(old, new)| {
                let mut e = input[old].clone();
                e.insert("seq".into(), new.into());
                e
            })
            .collect();
        assert_eq!(markers, expected);
        assert_eq!(
            systems(&a)
                .iter()
                .map(|e| e["seq"].as_u64().unwrap() < a.inherited_event_count)
                .collect::<Vec<_>>(),
            vec![true, true, true, false]
        );
        assert_eq!(
            requests(&a.events, 3),
            json!([
                [visible("system", "grandparent"), visible("user", "user-1")],
                [
                    visible("system", "parent"),
                    visible("user", "user-1"),
                    visible("user", "user-2")
                ],
                [
                    visible("system", "local"),
                    visible("user", "user-1"),
                    visible("user", "user-2"),
                    visible("user", "user-3")
                ]
            ])
        );
        assert_eq!(round_trip(a.clone()).unwrap(), a);
    }
}
#[test]
fn keeps_a_zero_length_inherited_prefix_when_the_first_step_and_all_generated_systems_are_local() {
    let mut input = vec![ev("session/end-seed", json!({"inherited":true}))];
    input.extend(turn(1, "local"));
    let input = timed(input);
    let a = restore(raw(input.clone(), seeded_header(), Some(0)).unwrap()).unwrap();
    assert_eq!(a.inherited_event_count, 0);
    assert_eq!(a.events[0], input[0]);
    assert_eq!(
        systems(&a)
            .iter()
            .map(|e| e["seq"].clone())
            .collect::<Vec<_>>(),
        vec![json!(3), json!(5)]
    );
    assert_eq!(round_trip(a.clone()).unwrap(), a);
}
#[test]
fn remaps_references_on_both_sides_of_a_prompt_insertion_without_reinterpreting_same_session_captures_or_framed_title_input()
 {
    let mut recalled = user_data("capture");
    recalled["source"] = json!({"kind":"session-reference","form":"recall","version":1,"references":[{"sessionId":"identity","label":"same-session","capturedThroughSeq":2,"capturedFormatVersion":2,"compacted":false,"originalMessages":1,"retainedMessages":1,"omittedMessages":0,"omittedBytes":0,"truncated":false,"inputIndex":0}]});
    let mut framed = user_data("title-input");
    framed["source"] = json!({"kind":"plugin","plugin":"dsh-session-title-llm"});
    framed["content"] = json!([{"type":"text","text":"Generate the session title from this JSON array of human messages:\n[{\"seq\":2,\"text\":\"a\"},{\"seq\":4,\"text\":\"b\"}]"}]);
    let mut capture = surface("user/message", recalled.clone());
    capture.insert("sourceEventSeqs".into(), json!([2, 4]));
    let mut title = title(2, json!([framed]));
    title["data"]["messageSeqs"] = json!([2, 4]);
    title["data"]["system"] = "title prompt".into();
    let mut input = opening();
    input.extend([
        user("a"),
        request(Some("prompt")),
        user("b"),
        capture,
        ev(
            "command/run",
            json!({"commandId":"command","name":"recall","source":{"kind":"user"}}),
        ),
        ev(
            "command/done",
            json!({"commandId":"command","kind":"success","sourceEventSeq":5}),
        ),
        ev(
            "session/title",
            json!({"title":"title","messageSeqs":[2,4],"source":{"kind":"fallback"}}),
        ),
        title,
    ]);
    let input = timed(input);
    let a = migrate(input.clone()).unwrap();
    let mut expected = input[5].clone();
    expected.insert("seq".into(), 7.into());
    expected.insert("sourceEventSeqs".into(), json!([3, 6]));
    assert_eq!(a.events[7], expected);
    assert_eq!(a.events[7]["data"], recalled);
    assert_eq!(
        a.events[9]["data"],
        json!({"commandId":"command","kind":"success","sourceEventSeq":7})
    );
    assert_eq!(
        a.events[10]["data"],
        json!({"title":"title","messageSeqs":[3,6],"source":{"kind":"fallback"}})
    );
    let mut expected = input[9]["data"].clone();
    expected["messageSeqs"] = json!([3, 6]);
    assert_eq!(a.events[11]["data"], expected);
    assert_eq!(round_trip(a.clone()).unwrap(), a);
}
#[test]
fn refuses_unknown_content_in_messages_even_when_the_event_is_ignorable() {
    for index in [0, 3, 4] {
        let mut message = user_data("future");
        message["content"] = json!([{"type":"future-block","sourceEventSeq":2}]);
        let mut row = carry(index, message);
        row.insert("ignorable".into(), true.into());
        let mut rows = opening();
        rows.extend([user("human"), row]);
        err(
            migrate(timed(rows)),
            "cannot safely transform unclassified message content",
        );
    }
}
#[test]
fn audits_nested_tool_result_content_instead_of_preserving_unknown_blocks_as_opaque_tool_json() {
    let result = |content: Value| {
        let mut row = surface(
            "tool/result",
            json!({"turn":1,"step":1,"message":{"id":"result","role":"user","source":{"kind":"tool","callId":"call"},"content":[{"type":"tool-result","toolCallId":"call","content":content}]}}),
        );
        row.insert("ignorable".into(), true.into());
        row
    };
    let mut prefix = opening();
    prefix.push(request(Some("prompt")));
    prefix.extend(tool_prefix());
    let ordinary = result(json!([{"type":"text","text":"file contents"}]));
    let mut rows = prefix.clone();
    rows.push(ordinary.clone());
    rows.extend(closing());
    let a = migrate(timed(rows)).unwrap();
    assert_eq!(a.events[a.events.len() - 3]["data"], ordinary["data"]);
    round_trip(a).unwrap();
    prefix.push(result(json!([{"type":"future-block","sourceEventSeq":2}])));
    prefix.extend(closing());
    err(
        migrate(timed(prefix)),
        "cannot safely transform unclassified message content",
    );
}
