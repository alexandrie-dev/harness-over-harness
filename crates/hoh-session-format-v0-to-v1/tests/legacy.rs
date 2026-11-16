mod common;
use common::*;
use hoh_session_format::SessionFormatMigration;
use hoh_session_format_v0_to_v1::*;
use serde_json::{Value, json};

#[test]
fn restores_pre_identity_user_assistant_and_replacement_tool_result_identities() {
    let rows = vec![
        turn(0, 1),
        surface(
            "user/message",
            1,
            json!({"content":[{"type":"text","text":"hi"}],"source":{"kind":"user"}}),
        ),
        step(2),
        surface(
            "assistant/message",
            3,
            json!({"turn":1,"step":1,"content":[{"type":"tool-call","id":"call-1","name":"read","arguments":"{}"}],"provenance":{"provider":"mock","model":"mock"}}),
        ),
        event(
            "tool/call",
            4,
            json!({"turn":1,"step":1,"callId":"call-1","name":"read","arguments":"{}"}),
        ),
        merge(
            surface(
                "tool/result",
                5,
                json!({"turn":1,"step":1,"callId":"call-1","content":[{"type":"text","text":"full"}],"isError":false}),
            ),
            json!({"sourceEventSeqs":[4]}),
        ),
        merge(
            event(
                "tool/result",
                6,
                json!({"turn":1,"step":1,"callId":"call-1","content":[{"type":"text","text":"pruned"}],"isError":false}),
            ),
            json!({"sourceEventSeqs":[5],"surfaceOp":{"op":"replace","start":5,"end":5}}),
        ),
        step_end(7),
        end(8, 1),
    ];
    let events = values(&migrate(&rows).unwrap());
    assert_eq!(events[1]["data"]["id"], "legacy-message:legacy:1");
    assert_eq!(events[1]["data"]["role"], "user");
    for (seq, id, role) in [(3, 3, "assistant"), (5, 5, "user"), (6, 5, "user")] {
        assert_eq!(
            events[seq]["data"]["message"]["id"],
            format!("legacy-message:legacy:{id}")
        );
        assert_eq!(events[seq]["data"]["message"]["role"], role);
    }
}
#[test]
fn normalizes_wrapped_flat_steering_and_every_accepted_old_turn_ending() {
    let reasons = vec![
        json!({"kind":"completed"}),
        json!({"kind":"error","step":1,"failure":{"message":"provider","code":"SERVER"}}),
        json!({"kind":"aborted"}),
        json!({"kind":"disposed"}),
        json!({"kind":"error","step":1,"message":"thrown"}),
        json!({"kind":"error","step":0,"failure":{"message":"detailed","code":"RATE_LIMIT","status":429,"providerRetryAfterMs":1000,"requestId":"request-1"}}),
        json!({"kind":"error","step":0,"message":"coded","code":"CODED"}),
    ];
    let expected = [
        json!({"kind":"completed"}),
        json!({"kind":"error","error":{"message":"provider","code":"SERVER"}}),
        json!({"kind":"aborted","reason":{"kind":"legacy"}}),
        json!({"kind":"aborted","reason":{"kind":"disposed"}}),
        json!({"kind":"error","error":{"message":"thrown","code":"UNKNOWN"}}),
        json!({"kind":"error","error":{"message":"detailed","code":"RATE_LIMIT","status":429,"providerRetryAfterMs":1000,"requestId":"request-1"}}),
        json!({"kind":"error","error":{"message":"coded","code":"CODED"}}),
    ];
    let mut rows = Vec::new();
    for (i, reason) in reasons.into_iter().enumerate() {
        rows.push(event(
            "turn/start",
            rows.len() as u64,
            json!({"turn":i+1,"trigger":{"kind":if i==1 {"retry"} else {"message"}}}),
        ));
        if i == 0 {
            rows.push(surface(
                "steering/message",
                1,
                json!({"turn":1,"message":user("wrapped")}),
            ));
            rows.push(surface("steering/message",2,json!({"turn":1,"content":[{"type":"text","text":"flat"}],"source":{"kind":"user"}})));
        }
        rows.push(event(
            "turn/end",
            rows.len() as u64,
            json!({"turn":i+1,"reason":reason}),
        ));
    }
    let events = values(&migrate(&rows).unwrap());
    let starts: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "turn/start")
        .map(|e| e["data"].clone())
        .collect();
    assert_eq!(
        starts,
        (1..=7).map(|i| json!({"turn":i})).collect::<Vec<_>>()
    );
    assert_eq!(events[1]["type"], "user/message");
    assert_eq!(events[1]["data"]["id"], "wrapped");
    assert_eq!(events[2]["type"], "user/message");
    assert_eq!(events[2]["data"]["id"], "legacy-message:legacy:2");
    assert_eq!(events[2]["data"]["role"], "user");
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "turn/end")
            .map(|e| e["data"].clone())
            .collect::<Vec<_>>(),
        expected
            .into_iter()
            .enumerate()
            .map(|(i, r)| json!({"turn":i+1,"reason":r}))
            .collect::<Vec<_>>()
    );
}
#[test]
fn refuses_malformed_legacy_branches() {
    let cases = [
        event("turn/start", 0, json!({"turn":1,"trigger":null})),
        surface(
            "steering/message",
            0,
            json!({"turn":1,"content":[],"source":{"kind":"user"},"extra":true}),
        ),
        surface("steering/message", 0, Value::Null),
        event(
            "turn/end",
            0,
            json!({"turn":1,"reason":{"kind":"completed","extra":true}}),
        ),
        event("turn/end", 0, json!({"turn":1,"reason":null})),
        event(
            "turn/end",
            0,
            json!({"turn":1,"step":1,"reason":{"kind":"completed"}}),
        ),
        event(
            "turn/end",
            0,
            json!({"turn":1,"reason":{"kind":"aborted","extra":true}}),
        ),
        event(
            "turn/end",
            0,
            json!({"turn":1,"reason":{"kind":"disposed","extra":true}}),
        ),
        event(
            "turn/end",
            0,
            json!({"turn":1,"reason":{"kind":"error","step":-1,"message":"bad"}}),
        ),
        event(
            "turn/end",
            0,
            json!({"turn":1,"reason":{"kind":"error","step":0,"message":"bad","code":1}}),
        ),
    ];
    for e in cases {
        let error = migrate(&[e]).unwrap_err().to_string();
        assert!(
            [
                "malformed",
                "must be a JSON object",
                "unexpected member",
                "non-negative"
            ]
            .iter()
            .any(|text| error.contains(text)),
            "{error}"
        );
    }
}
#[test]
fn classifies_retired_types_as_unsupported_migration() {
    for (kind, data) in [
        ("request/header-delta", json!({"config":{"model":"legacy"}})),
        ("mode/set", json!({"mode":"plan"})),
    ] {
        assert!(
            migrate(&[event(kind, 0, data)])
                .unwrap_err()
                .is_unsupported_migration()
        );
    }
}
#[test]
fn classifies_retired_request_header_fallback_as_unsupported_migration() {
    assert!(
        migrate(&[event(
            "request/header",
            0,
            json!({"header":{"config":{"model":"legacy"}},"reason":"fallback"})
        )])
        .unwrap_err()
        .is_unsupported_migration()
    );
}
#[test]
fn leaves_current_message_and_turn_end_variants_unchanged() {
    let rows = vec![
        turn(0, 1),
        surface("user/message", 1, user("current")),
        event(
            "turn/end",
            2,
            json!({"turn":1,"reason":{"kind":"aborted","reason":{"kind":"user"}}}),
        ),
        turn(3, 2),
        event(
            "turn/end",
            4,
            json!({"turn":2,"reason":{"kind":"error","error":{"message":"x","code":"X"}}}),
        ),
    ];
    assert_eq!(values(&migrate(&rows).unwrap()), rows);
}
#[test]
fn refuses_wrong_version_headers_and_additional_malformed_branches() {
    assert_error(
        SESSION_FORMAT_V0_TO_V1.migrate_header(logical(1, "x")),
        "expected format v0",
    );
    assert_error(
        migrate(&[event(
            "turn/start",
            0,
            json!({"turn":0,"trigger":{"kind":"message"}}),
        )]),
        "malformed",
    );
    assert_error(
        migrate(&[event(
            "turn/end",
            0,
            json!({"turn":1,"reason":{"kind":"error","step":0,"failure":{"message":1,"code":"X"}}}),
        )]),
        "malformed",
    );
    for d in [
        json!({"turn":1,"step":0,"callId":1,"content":[],"isError":false}),
        json!({"turn":1,"step":0,"callId":"call","content":[]}),
    ] {
        assert!(migrate(&[surface("tool/result", 0, d)]).is_err());
    }
    let future = event(
        "turn/end",
        1,
        json!({"turn":1,"reason":{"kind":"future-reason"}}),
    );
    assert_eq!(
        values(&migrate(&[turn(0, 1), future.clone()]).unwrap())[1],
        future
    );
}
#[test]
fn refuses_legacy_replacement_without_imported_identity() {
    let row = merge(
        event(
            "tool/result",
            1,
            json!({"turn":1,"step":0,"callId":"call","content":[],"isError":false}),
        ),
        json!({"sourceEventSeqs":[0],"surfaceOp":{"op":"replace","start":0,"end":0}}),
    );
    assert_error(migrate(&[turn(0, 1), row]), "without identity");
}
#[test]
fn normalizes_legacy_request_header_message_prefix() {
    let mut r = request(1);
    r["data"]["header"]["messagePrefix"] =
        json!([{"role":"user","content":[{"type":"text","text":"obsolete"}]}]);
    let events = values(&migrate(&[turn(0, 1), r.clone(), end(2, 1)]).unwrap());
    assert!(events[1]["data"]["header"].get("messagePrefix").is_none());
    r["data"]["header"]["messagePrefix"] = "bad".into();
    assert_error(migrate(&[turn(0, 1), r]), "messagePrefix");
    for (turn, kind) in [(0, json!("completed")), (1, json!(1))] {
        assert_error(
            migrate(&[event(
                "turn/end",
                0,
                json!({"turn":turn,"reason":{"kind":kind}}),
            )]),
            "malformed",
        );
    }
}
#[test]
fn refuses_invalid_legacy_relationship_facts_instead_of_rewriting() {
    let human = surface("user/message", 1, user("human"));
    let checkpoint = merge(
        surface(
            "user/message",
            2,
            user_source(
                "compact",
                json!({"kind":"plugin","plugin":"compact","compactionId":"orphan"}),
            ),
        ),
        json!({"sourceEventSeqs":[1],"surfaceOp":{"op":"replace","start":1,"end":1}}),
    );
    assert_error(
        migrate(&[turn(0, 1), human.clone(), checkpoint]),
        "compaction checkpoint",
    );
    assert_error(
        migrate(&[
            turn(0, 1),
            human,
            event(
                "session/title",
                2,
                json!({"title":"Pinned","messageSeqs":[1],"source":{"kind":"user"}}),
            ),
        ]),
        "empty exactly",
    );
}

#[test]
fn legacy_replacement_does_not_truncate_or_saturate_an_invalid_start() {
    let mut st = stage("legacy");
    transform(st.as_mut(), surface("user/message", 0, user("zero"))).unwrap();
    transform(st.as_mut(), surface("user/message", 2, user("two"))).unwrap();
    for start in [json!(-1), json!(2.5), json!(9_007_199_254_740_994.0)] {
        let row = merge(
            event(
                "tool/result",
                3,
                json!({"turn":1,"step":1,"callId":"call","content":[],"isError":false}),
            ),
            json!({"surfaceOp":{"op":"replace","start":start,"end":0}}),
        );
        let error = transform(st.as_mut(), row).unwrap_err();
        assert_eq!(
            error.to_string(),
            "tool/result 3 replacement cites a message without identity"
        );
    }
    let row = merge(
        event(
            "tool/result",
            3,
            json!({"turn":1,"step":1,"callId":"call","content":[],"isError":false}),
        ),
        json!({"surfaceOp":{"op":"replace","start":2.0,"end":2}}),
    );
    assert_eq!(
        transform(st.as_mut(), row).unwrap()["data"]["message"]["id"],
        "two"
    );
}
