mod common;
use common::*;
fn wrapper() -> Value {
    json!({"type":"tool-result","toolCallId":"retired-call","content":[{"type":"text","text":"saved result"}]})
}
fn message(role: &str) -> Value {
    let source = match role {
        "assistant" => json!({"kind":"model","provider":"test","model":"test"}),
        "tool" => json!({"kind":"tool","callId":"call"}),
        _ => json!({"kind":format!("{role}-prompt")}),
    };
    let mut m = json!({"id":role,"role":role,"content":[wrapper()],"source":source});
    if role == "tool" {
        m["toolCallId"] = "call".into();
    }
    m
}
fn user_message() -> Value {
    json!({"id":"user","role":"user","source":{"kind":"user"},"content":[wrapper()]})
}
#[test]
fn refuses_retired_content_in_native_encoding_restoration_and_recoverable_suffixes() {
    let u = user_message();
    let mut rows = vec![ev("user/message", u.clone())];
    for role in ["system", "developer", "assistant"] {
        rows.push(ev(
            &format!("{role}/message"),
            json!({"turn":1,"step":1,"message":message(role)}),
        ));
    }
    rows.extend([
        ev(
            "tool/result",
            json!({"turn":1,"step":1,"message":message("tool")}),
        ),
        ev(
            "agent/inbox/spliced",
            json!({"target":"next-turn","start":0,"inserted":[u]}),
        ),
        ev("session/title-llm-request", json!({"messages":[u]})),
        ev("compaction/summary", json!({"summary":[wrapper()]})),
        ev(
            "compaction/summary",
            json!({"summary":[],"rawOutput":[wrapper()]}),
        ),
        ev("tool/ptc-dispatch", json!({"content":[wrapper()]})),
        ev("team/message/queued", json!({"message":u})),
    ]);
    for tag in ["assistant/message", "assistant/attempt"] {
        for chunk_type in ["block-start", "block-end"] {
            let chunk = if chunk_type == "block-start" {
                json!({"type":chunk_type,"index":0,"blockType":"tool-result"})
            } else {
                json!({"type":chunk_type,"index":0,"block":wrapper()})
            };
            let mut m = message("assistant");
            m["content"] = json!([]);
            rows.push(ev(tag,json!({"turn":1,"step":1,"message":m,"stream":[{"type":"chunk","elapsedMs":0,"chunk":chunk}]})));
        }
    }
    assert_eq!(rows.len(), 15);
    for row in rows {
        let before = row.clone();
        let expected = if row["type"] == "system/message" {
            "rejects retired tool-result"
        } else {
            "released tool-result wrapper"
        };
        err(
            RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row),
            expected,
        );
        err(native(vec![row.clone()]), expected);
        for recovery in [
            SessionFormatRecovery::Strict,
            SessionFormatRecovery::Recoverable,
        ] {
            let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
                .create_decoder(
                    &RELEASED_V4_SESSION_FORMAT_CODEC
                        .encode_header(&h(4, "retired-content"), 0)
                        .unwrap()
                        .into(),
                    recovery,
                )
                .unwrap();
            let mut out = SessionFormatEventCollector::new();
            if recovery == SessionFormatRecovery::Recoverable {
                d.decode_row(Value::Null, &mut out).unwrap();
            }
            err(d.decode_row(row.clone().into(), &mut out), expected);
        }
        assert_eq!(row, before);
    }
}
#[test]
fn refuses_a_v3_user_wrapper_at_the_v4_target_while_leaving_the_released_reader_and_source_intact()
{
    let row = surface("user/message", user_message());
    let before = row.clone();
    let a = artifact(vec![row.clone()], h(3, "retired-content"), 0);
    let historical = hoh_session_format_v2_to_v3::restore_released_v3_artifact(
        a,
        &known(std::slice::from_ref(&row)),
    )
    .unwrap();
    assert_eq!(historical.events, vec![row.clone()]);
    let migrated = migrate(vec![row.clone()], h(3, "retired-content"), Some(0), vec![]).unwrap();
    err(
        restore_released_v4_artifact(migrated, &known(std::slice::from_ref(&row))),
        "released tool-result wrapper",
    );
    assert_eq!(row, before);
}
#[test]
fn interprets_ignorable_developer_content_only_for_a_reader_that_knows_the_event_or_for_encoding() {
    let mut row = ev(
        "developer/message",
        json!({"turn":1,"step":1,"message":message("developer")}),
    );
    row.insert("ignorable".into(), true.into());
    let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
        .create_decoder(
            &RELEASED_V4_SESSION_FORMAT_CODEC
                .encode_header(&h(4, "retired-content"), 0)
                .unwrap()
                .into(),
            SessionFormatRecovery::Strict,
        )
        .unwrap();
    let mut out = SessionFormatEventCollector::new();
    d.decode_row(row.clone().into(), &mut out).unwrap();
    assert_eq!(out.values, vec![row.clone()]);
    assert_v4_row_admission(&row.clone().into(), Some(&HashSet::new())).unwrap();
    err(
        assert_v4_row_admission(
            &row.clone().into(),
            Some(&known(std::slice::from_ref(&row))),
        ),
        "released tool-result wrapper",
    );
    err(
        RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row),
        "released tool-result wrapper",
    );
    let a = artifact(vec![row], h(4, "retired-content"), 0);
    assert_eq!(
        restore_released_v4_artifact(a.clone(), &HashSet::new()).unwrap(),
        a
    );
}
#[test]
fn keeps_tool_arguments_replay_state_schemas_nested_metadata_and_unknown_ignorable_payloads_opaque()
{
    let mut u = user_message();
    u["content"] = json!([{"type":"text","text":"text","metadata":wrapper()}]);
    u["replayState"] = json!({"content":[wrapper()]});
    let mut opaque = ev(
        "future/opaque",
        json!({"content":[wrapper()],"message":user_message(),"stream":[{"type":"chunk","chunk":{"type":"block-end","block":wrapper()}}]}),
    );
    opaque.insert("ignorable".into(), true.into());
    let rows = vec![
        ev("user/message", u),
        ev(
            "tool/call",
            json!({"arguments":hoh_util_values::js_json_stringify(&wrapper()),"metadata":{"content":[wrapper()]}}),
        ),
        ev(
            "tool/ptc-dispatch",
            json!({"content":[{"type":"text","text":"result"}],"arguments":wrapper()}),
        ),
        ev(
            "request/header",
            json!({"header":{"config":{"provider":"test","model":"test"},"tools":[{"name":"example","description":"","parameters":{"type":"object","examples":[wrapper()]}}]}}),
        ),
        opaque.clone(),
    ];
    for row in rows {
        let before = row.clone();
        assert_eq!(
            round_trip(std::slice::from_ref(&row), &h(4, "retired-content"), 0)
                .unwrap()
                .0,
            vec![before.clone()]
        );
        assert_eq!(row, before);
    }
    let migrated = migrate(
        vec![opaque.clone()],
        h(3, "retired-content"),
        Some(0),
        vec![],
    )
    .unwrap();
    opaque.insert("type".into(), "plugin:future/opaque".into());
    assert_eq!(migrated.events, vec![opaque]);
}
