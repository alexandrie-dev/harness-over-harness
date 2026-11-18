mod common;
use common::*;
use hoh_session::repair::{OpenTurnCloseCause, open_turn_closers};
use hoh_session::*;
use serde_json::json;
use std::sync::Arc;
const CAUSES: [OpenTurnCloseCause; 2] =
    [OpenTurnCloseCause::Interrupted, OpenTurnCloseCause::Forked];
fn kinds(events: &[Arc<SessionEvent>]) -> Vec<&str> {
    events.iter().map(|e| e.event_type()).collect()
}
#[test]
fn observes_a_live_step_without_earlier_history_and_retains_repairs_until_their_commits_arrive() {
    let mut r = ToolCallRecovery::default();
    assert!(r.results().unwrap().is_empty());
    r.observe(&event("step/start", 20, json!({"turn":3,"step":2})));
    let mut d = data("assistant", "");
    d["turn"] = json!(3);
    d["step"] = json!(2);
    d["message"]["content"] = json!([{"type":"tool-call","id":"started","name":"bash","arguments":"{}"},{"type":"tool-call","id":"pending","name":"bash","arguments":"{}"}]);
    r.observe(&event("assistant/message", 21, d));
    let call = patch(
        &event(
            "tool/call",
            22,
            json!({"turn":3,"step":2,"callId":"started","name":"bash","arguments":"{}"}),
        ),
        "time",
        json!(120),
    );
    r.observe(&call);
    let results = r.results().unwrap();
    assert_eq!(results.len(), 2);
    for (i, (id, code)) in [
        ("started", TOOL_OUTCOME_UNKNOWN),
        ("pending", TOOL_NOT_STARTED),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(results[i].seq(), seq(23 + i as u64));
        assert_eq!(results[i].time(), 120);
        assert_eq!(results[i].data()["message"]["toolCallId"], id);
        assert_eq!(results[i].data()["error"]["code"], code);
        assert_eq!(results[i].data()["turn"], 3);
        assert_eq!(results[i].data()["step"], 2);
    }
    assert_eq!(results[0].source_event_seqs(), Some(&json!([22])));
    assert!(results[1].source_event_seqs().is_none());
    assert_eq!(r.results().unwrap(), results);
    for (i, e) in results.iter().enumerate() {
        r.observe(e);
        assert_eq!(r.results().unwrap(), results[i + 1..]);
    }
}
#[test]
fn does_not_acknowledge_a_pending_call_with_a_reused_id_result() {
    for (replacement, turn, step) in [(true, 1, 1), (false, 2, 1), (false, 1, 2)] {
        let mut r = ToolCallRecovery::default();
        for e in open_calls(false) {
            r.observe(&e);
        }
        let mut d = data("tool", "old");
        d["turn"] = json!(turn);
        d["step"] = json!(step);
        let mut e = event("tool/result", 3, d).as_value().clone();
        e["surfaceOp"] = if replacement {
            json!({"op":"replace","startSeq":0,"endSeq":0})
        } else {
            json!("append")
        };
        r.observe(&SessionEvent::from_value(e));
        assert_eq!(r.results().unwrap().len(), 1);
    }
}
#[test]
fn returns_nothing_for_a_balanced_log() {
    let e = vec![
        event("turn/start", 0, json!({"turn":1})),
        event(
            "turn/end",
            1,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    for c in CAUSES {
        assert!(open_turn_closers(&e, c).unwrap().is_empty());
    }
}
#[test]
fn returns_nothing_for_an_empty_log() {
    for c in CAUSES {
        assert!(open_turn_closers(&[], c).unwrap().is_empty());
    }
}
#[test]
fn closes_an_open_turn_with_no_open_step() {
    for c in CAUSES {
        let out = open_turn_closers(&[event("turn/start", 0, json!({"turn":1}))], c).unwrap();
        assert_eq!(kinds(&out), vec!["turn/end"]);
        assert_eq!(
            out[0].as_value(),
            &json!({"type":"turn/end","seq":1,"time":0,"data":{"turn":1,"reason":{"kind":c.kind()}}})
        );
    }
}
#[test]
fn closes_an_open_step_before_the_turn() {
    let e = open_calls(false);
    for c in CAUSES {
        let out = open_turn_closers(&e[..2], c).unwrap();
        assert_eq!(kinds(&out), vec!["step/end", "turn/end"]);
        assert_eq!(
            out.iter().map(|e| u64::from(e.seq())).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }
}
#[test]
fn marks_an_assistant_tool_request_with_no_recorded_call_as_not_started() {
    for c in CAUSES {
        let out = open_turn_closers(&open_calls(false), c).unwrap();
        assert_eq!(kinds(&out), vec!["tool/result", "step/end", "turn/end"]);
        let result = &out[0];
        assert_eq!(result.seq(), seq(3));
        assert_eq!(
            result.data()["message"]["id"],
            format!("{}-tool-result-call-3", c.kind())
        );
        assert_eq!(result.data()["message"]["role"], "tool");
        assert_eq!(result.data()["message"]["toolCallId"], "call");
        assert_eq!(
            result.data()["message"]["source"],
            json!({"kind":"tool","callId":"call"})
        );
        assert_eq!(result.data()["message"]["isError"], true);
        assert_eq!(result.data()["error"]["code"], TOOL_NOT_STARTED);
        assert!(result.source_event_seqs().is_none());
    }
}
#[test]
fn does_not_synthesize_a_result_for_a_tool_call_that_already_has_one() {
    let mut e = open_calls(false);
    e.push(surface_event("tool", 3, "ok"));
    for c in CAUSES {
        assert_eq!(
            kinds(&open_turn_closers(&e, c).unwrap()),
            vec!["step/end", "turn/end"]
        );
    }
}
#[test]
fn preserves_an_unanswered_assistant_call_after_the_owning_step_already_closed() {
    let mut e = open_calls(false);
    for event in &mut e {
        let mut value = event.as_value().clone();
        value["data"]["turn"] = json!(2);
        *event = Arc::new(SessionEvent::from_value(value));
    }
    e.push(event("step/end", 3, json!({"turn":2,"step":1})));
    for c in CAUSES {
        let out = open_turn_closers(&e, c).unwrap();
        assert_eq!(kinds(&out), vec!["turn/end"]);
        assert_eq!(
            hoh_util_values::js_json_stringify(out[0].as_value()),
            hoh_util_values::js_json_stringify(
                &json!({"type":"turn/end","seq":4,"time":3,"data":{"turn":2,"reason":{"kind":c.kind()}}})
            )
        );
    }
}
#[test]
fn synthesizes_results_only_for_the_still_open_turn_not_a_committed_earlier_turn() {
    let mut e = open_calls(false);
    e.push(event("step/end", 3, json!({"turn":1,"step":1})));
    e.push(event(
        "turn/end",
        4,
        json!({"turn":1,"reason":{"kind":"completed"}}),
    ));
    e.push(event("turn/start", 5, json!({"turn":2})));
    let mut d = data("assistant", "");
    d["turn"] = json!(2);
    d["message"]["content"] =
        json!([{"type":"tool-call","id":"new-call","name":"bash","arguments":"{}"}]);
    e.push(event("assistant/message", 6, d));
    for c in CAUSES {
        let out = open_turn_closers(&e, c).unwrap();
        assert_eq!(out[0].data()["message"]["toolCallId"], "new-call");
        assert_eq!(
            out.iter()
                .filter(|e| e.event_type() == "tool/result")
                .count(),
            1
        );
    }
}
#[test]
fn synthesizes_a_result_for_each_of_multiple_unanswered_calls_in_log_order() {
    let mut e = open_calls(false);
    let mut d = e[2].data().clone();
    d["message"]["content"] = json!([{"type":"tool-call","id":"call-a","name":"bash","arguments":"{}"},{"type":"tool-call","id":"call-b","name":"bash","arguments":"{}"}]);
    e[2] = patch(&e[2], "data", d);
    let mut result = data("tool", "");
    result["message"]["toolCallId"] = json!("call-a");
    result["message"]["source"]["callId"] = json!("call-a");
    let result = patch(&surface_event("tool", 3, ""), "data", result);
    e.push(result);
    for c in CAUSES {
        let out = open_turn_closers(&e, c).unwrap();
        assert_eq!(out[0].data()["message"]["toolCallId"], "call-b");
        assert_eq!(kinds(&out), vec!["tool/result", "step/end", "turn/end"]);
    }
}
#[test]
fn synthesized_tool_result_carries_surface_op_and_source_event_seqs_when_tool_call_was_logged() {
    for c in CAUSES {
        let out = open_turn_closers(&open_calls(true), c).unwrap();
        let e = &out[0];
        assert_eq!(e.surface_op(), Some(&json!("append")));
        assert_eq!(e.source_event_seqs(), Some(&json!([3])));
        assert_eq!(
            e.data()["error"],
            json!({"name":"ToolOutcomeUnknownError","code":TOOL_OUTCOME_UNKNOWN})
        );
        let t = e.data()["message"]["content"][0]["text"].as_str().unwrap();
        assert!(t.contains("retry only if the operation is read-only or idempotent"));
        assert!(t.contains("first verify external state or ask the user"));
    }
}
#[test]
fn handles_tool_call_without_a_matching_assistant_message_entry_gracefully() {
    let mut e = open_calls(true);
    e.remove(2);
    for c in CAUSES {
        assert_eq!(
            kinds(&open_turn_closers(&e, c).unwrap()),
            vec!["step/end", "turn/end"]
        );
    }
}
#[test]
fn pins_its_exact_model_visible_text_for_each_cause_and_start_state() {
    let texts = [
        (
            OpenTurnCloseCause::Interrupted,
            false,
            "The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed.",
        ),
        (
            OpenTurnCloseCause::Interrupted,
            true,
            "The tool call was interrupted after it was recorded, but no result was durably recorded. Its outcome is unknown. Decide whether to retry from the tool semantics: retry only if the operation is read-only or idempotent; if it may have side effects, first verify external state or ask the user. Do not retry blindly.",
        ),
        (
            OpenTurnCloseCause::Forked,
            false,
            "The history inherited by this branch has no record of this tool call starting. The parent session may have executed it after the fork point. Decide whether to retry from the tool semantics: retry only if the operation is read-only or idempotent; if it may have side effects, first verify external state or ask the user. Do not retry blindly.",
        ),
    ];
    for (cause, started, text) in &texts[..3] {
        let out = open_turn_closers(&open_calls(*started), *cause).unwrap();
        assert_eq!(out[0].data()["message"]["content"][0]["text"], *text);
    }
    let out = open_turn_closers(&open_calls(true), OpenTurnCloseCause::Forked).unwrap();
    assert_eq!(
        out[0].data()["message"]["content"][0]["text"],
        "The history inherited by this branch records this tool call starting but does not include its result. The parent session may have completed it after the fork point. Decide whether to retry from the tool semantics: retry only if the operation is read-only or idempotent; if it may have side effects, first verify external state or ask the user. Do not retry blindly."
    );
}
#[test]
fn shares_the_error_codes_across_causes() {
    for c in CAUSES {
        for started in [false, true] {
            let out = open_turn_closers(&open_calls(started), c).unwrap();
            assert_eq!(
                out[0].data()["error"]["code"],
                if started {
                    TOOL_OUTCOME_UNKNOWN
                } else {
                    TOOL_NOT_STARTED
                }
            );
        }
    }
}
#[test]
fn interrupted_turn_closers_is_the_public_interrupted_cause_entry() {
    let e = open_calls(true);
    assert_eq!(
        interrupted_turn_closers(&e).unwrap(),
        open_turn_closers(&e, OpenTurnCloseCause::Interrupted).unwrap()
    );
}
