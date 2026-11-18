mod common;
use common::*;
use hoh_session_format_v3_to_v4::fork_result::assert_v4_fork_result;
fn restore(events: Vec<SessionFormatEvent>) -> R<SessionFormatArtifact> {
    let mut a = fork_artifact();
    a.events = events;
    let k = known(&a.events);
    restore_released_v4_artifact(a, &k)
}
#[test]
fn ignores_non_object_fork_result_data_and_validates_non_object_sources() {
    assert_v4_fork_result(&json!({"type":"tool/result","data":null})).unwrap();
    err(
        assert_v4_fork_result(
            &json!({"type":"tool/result","seq":0,"surfaceOp":"append","data":{"error":{"code":"TOOL_NOT_STARTED","name":"ToolNotStartedError"},"message":{"id":"forked-tool-result-undefined-0","role":"tool","toolCallId":"call-1","isError":true,"source":null,"content":[{"type":"text","text":"not started"}]}}}),
        ),
        "invalid V4 not-started fork result",
    );
}
#[test]
fn persists_original_fork_ids_and_wording_restores_relationships_and_retains_nested_inherited_cuts()
{
    let a = fork_artifact();
    let before = a.clone();
    for row in &a.events {
        assert_v4_row_admission(
            &RELEASED_V4_SESSION_FORMAT_CODEC
                .encode_event(row)
                .unwrap()
                .into(),
            None,
        )
        .unwrap();
    }
    let (events, cut) = round_trip(&a.events, &a.header, a.inherited_event_count).unwrap();
    assert_eq!(cut, 3);
    assert_eq!(events, a.events);
    assert_eq!(restore(events.clone()).unwrap().events, events);
    assert_eq!(a, before);
    assert_eq!(
        a.events[4]["data"]["message"]["id"],
        "forked-tool-result-read-one-4"
    );
    assert!(
        hoh_util_values::js_json_stringify(&a.events[4].clone().into())
            .contains("The parent session may have executed it after the fork point.")
    );
    let mut nested = a.clone();
    nested.header.insert("id".into(), "fork-grandchild".into());
    nested
        .header
        .insert("parentSession".into(), "fork-child".into());
    nested.events.truncate(5);
    nested.events.push(obj(
        json!({"type":"session/end-seed","seq":5,"time":3,"data":{"inherited":true}}),
    ));
    nested.events.push(obj(
        json!({"type":"step/end","seq":6,"time":3,"data":{"turn":1,"step":1}}),
    ));
    nested.events.push(obj(
        json!({"type":"turn/end","seq":7,"time":3,"data":{"turn":1,"reason":{"kind":"forked"}}}),
    ));
    nested.inherited_event_count = 5;
    let known = known(&nested.events);
    assert!(restore_released_v4_artifact(nested, &known).is_ok());
}
#[test]
fn round_trips_a_pruned_fork_result_while_preserving_its_original_message_identity() {
    let mut a = fork_artifact();
    let original = a.events[4].clone();
    a.events.push(e("turn/start", 7, json!({"turn":2})));
    a.events
        .push(e("step/start", 8, json!({"turn":2,"step":1})));
    a.events.push(e(
        "compaction/prune",
        9,
        json!({"shadowedRange":{"start":4,"end":4},"shadowedSeqs":[4],"shadowedTokenCount":0}),
    ));
    let mut replacement = original.clone();
    replacement.insert("seq".into(), 10.into());
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":4,"endSeq":4}),
    );
    replacement.insert("sourceEventSeqs".into(), json!([4]));
    let chars: Vec<_> = FORK_NOT_STARTED_TEXT.chars().collect();
    replacement["data"]["message"]["content"][0]["text"] = format!(
        "{}\n\n[... tool result middle pruned ...]\n\n{}",
        chars[..100].iter().collect::<String>(),
        chars[chars.len() - 100..].iter().collect::<String>()
    )
    .into();
    a.events.push(replacement.clone());
    a.events.push(e("step/end", 11, json!({"turn":2,"step":1})));
    a.events.push(e(
        "turn/end",
        12,
        json!({"turn":2,"reason":{"kind":"completed"}}),
    ));
    assert_eq!(
        replacement["data"]["message"]["id"],
        original["data"]["message"]["id"]
    );
    assert_eq!(replacement["sourceEventSeqs"], json!([4]));
    assert!(hoh_util_values::js_json_stringify(&replacement.clone().into()).contains("pruned"));
    let (events, cut) = round_trip(&a.events, &a.header, a.inherited_event_count).unwrap();
    assert_eq!(cut, a.inherited_event_count);
    assert_eq!(events, a.events);
    assert_eq!(restore(events.clone()).unwrap().events, events);
    for override_ in [
        json!({"sourceEventSeqs":[10]}),
        json!({"data":{"message":{"id":"forked-tool-result-read-one-10"}}}),
    ] {
        let mut invalid = events.clone();
        if let Some(refs) = override_.get("sourceEventSeqs") {
            invalid[10].insert("sourceEventSeqs".into(), refs.clone());
        } else {
            invalid[10]["data"]["message"]["id"] = "forked-tool-result-read-one-10".into();
        }
        assert!(restore(invalid).is_err());
    }
}
#[test]
fn rejects_incomplete_persisted_result_fields_through_v4_restoration() {
    for data in [Value::Null, {
        let mut d = fork_artifact().events[4]["data"].clone();
        d["message"]["source"] = Value::Null;
        d
    }] {
        let mut events = fork_artifact().events;
        events[4].insert("data".into(), data);
        assert!(restore(events).is_err());
    }
}
#[test]
fn rejects_malformed_fork_results() {
    for name in [
        "ID sequence",
        "error name",
        "call ID",
        "error result",
        "source references",
        "content",
    ] {
        let mut events = fork_artifact().events;
        let row = &mut events[4];
        match name {
            "ID sequence" => row["data"]["message"]["id"] = "forked-tool-result-read-one-0".into(),
            "error name" => row["data"]["error"]["name"] = "OtherError".into(),
            "call ID" => row["data"]["message"]["toolCallId"] = "different".into(),
            "error result" => row["data"]["message"]["isError"] = false.into(),
            "source references" => {
                row.insert("sourceEventSeqs".into(), json!([2]));
            }
            "content" => row["data"]["message"]["content"] = json!([]),
            _ => unreachable!(),
        };
        assert!(
            assert_v4_row_admission(&row.clone().into(), None).is_err(),
            "{name}"
        );
        assert!(restore(events).is_err(), "{name}");
    }
}
