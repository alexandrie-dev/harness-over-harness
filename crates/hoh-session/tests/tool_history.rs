mod common;
use common::*;
use hoh_session::*;
use serde_json::json;
use std::sync::Arc;
fn add(s: &Session, name: &str, h: SessionSeq) -> Arc<SessionEvent> {
    developer(
        s,
        json!([{"type":"tool-addition","toolName":name}]),
        Some(h),
    )
    .unwrap()
}
fn remove(s: &Session, name: &str) -> Arc<SessionEvent> {
    developer(s, json!([{"type":"tool-removal","toolName":name}]), None).unwrap()
}
#[test]
fn returns_frozen_active_declarations_with_no_updates_for_an_initial_header() {
    let s = session();
    request(&s, vec![tool("search")], "initial");
    let h = s.tool_history().unwrap();
    assert_eq!(*h, json!({"tools":[tool("search")],"updates":[]}));
    assert!(Arc::ptr_eq(&h, &s.tool_history().unwrap()));
}
#[test]
fn resolves_additions_from_their_referenced_header_and_keeps_earlier_snapshots_unchanged() {
    let s = session();
    request(&s, vec![tool("search")], "initial");
    let initial = s.tool_history().unwrap();
    let h = request(&s, vec![tool("search"), tool("fetch")], "change");
    let added = add(&s, "fetch", h.seq());
    let after = s.tool_history().unwrap();
    request(&s, vec![tool("search")], "change");
    let removed = remove(&s, "fetch");
    assert_eq!(
        *s.tool_history().unwrap(),
        json!({"tools":[tool("search")],"updates":[{"messageId":added.data()["message"]["id"],"additions":[tool("fetch")]},{"messageId":removed.data()["message"]["id"],"additions":[]}]})
    );
    assert_eq!(initial["updates"], json!([]));
    assert_eq!(after["updates"].as_array().unwrap().len(), 1);
}
#[test]
fn restarts_declarations_at_an_explicit_series_start() {
    for marker in [false, true] {
        let s = session();
        request(&s, vec![tool("search")], "initial");
        let h = request(&s, vec![tool("search"), tool("fetch")], "change");
        add(&s, "fetch", h.seq());
        s.tool_history().unwrap();
        let mut d = json!({"header":{"config":{"provider":"mock","model":"mock"},"tools":[tool("search"),tool("fetch")]},"reason":if marker { "change" }else { "series" }});
        if marker {
            d["startsSeries"] = json!(true);
        }
        s.append("request/header", d, None).unwrap();
        assert_eq!(
            *s.tool_history().unwrap(),
            json!({"tools":[tool("search"),tool("fetch")],"updates":[]})
        );
    }
}
#[test]
fn restarts_declarations_when_a_retained_name_returns_with_a_changed_definition() {
    let s = session();
    request(&s, vec![tool("search"), tool("fetch")], "initial");
    request(&s, vec![tool("search")], "change");
    remove(&s, "fetch");
    let mut changed = tool("fetch");
    changed["description"] = json!("Fetch v2");
    let h = request(&s, vec![tool("search"), changed.clone()], "change");
    let a = add(&s, "fetch", h.seq());
    assert_eq!(
        *s.tool_history().unwrap(),
        json!({"tools":[tool("search"),changed.clone()],"updates":[{"messageId":a.data()["message"]["id"],"additions":[changed]}]})
    );
}
#[test]
fn activates_deferred_baseline_tools_through_folded_history_on_both_routes() {
    use hoh_llm::{DeveloperMessage, RequestMessage, ToolHistory, ToolSchema};
    let s = session();
    let mut deferred = tool("fetch");
    deferred["deferLoading"] = json!(true);
    let tools = vec![tool("search"), deferred.clone()];
    let h = request(&s, tools.clone(), "initial");
    let first=developer(&s,json!([{"type":"tool-addition","toolName":"search"},{"type":"tool-addition","toolName":"fetch"}]),Some(h.seq())).unwrap();
    let second = add(&s, "fetch", h.seq());
    let history: ToolHistory =
        serde_json::from_value((*s.tool_history().unwrap()).clone()).unwrap();
    assert_eq!(history.updates.len(), 2);
    let messages = [first, second]
        .into_iter()
        .map(|e| {
            RequestMessage::from(
                serde_json::from_value::<DeveloperMessage>(e.data()["message"].clone()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let tools = tools
        .into_iter()
        .map(|t| serde_json::from_value::<ToolSchema>(t).unwrap())
        .collect::<Vec<_>>();
    for mode in ["addition-only", "in-history"] {
        let projected =
            hoh_llm::project_tool_updates(&messages, Some(&tools), Some(mode), Some(&history))
                .unwrap();
        assert_eq!(projected.messages.len(), 1);
    }
}
#[test]
fn continues_the_series_when_a_retained_name_is_restored_unchanged() {
    let s = session();
    request(&s, vec![tool("search"), tool("fetch")], "initial");
    request(&s, vec![tool("search")], "change");
    remove(&s, "fetch");
    let h = request(&s, vec![tool("search"), tool("fetch")], "change");
    add(&s, "fetch", h.seq());
    assert_eq!(
        s.tool_history().unwrap()["updates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn falls_back_to_complete_active_declarations_while_a_header_lacks_its_update_record() {
    let s = session();
    request(&s, vec![tool("search")], "initial");
    let h = request(&s, vec![tool("search"), tool("fetch")], "change");
    assert_eq!(
        s.tool_history().unwrap()["tools"],
        json!([tool("search"), tool("fetch")])
    );
    add(&s, "fetch", h.seq());
    assert_eq!(s.tool_history().unwrap()["tools"], json!([tool("search")]));
    request(&s, vec![tool("search")], "change");
    assert_eq!(s.tool_history().unwrap()["updates"], json!([]));
}
#[test]
fn reconstructs_inherited_history_on_restore_and_continues_from_live_events() {
    let s = session();
    request(&s, vec![tool("search")], "initial");
    let h = request(&s, vec![tool("search"), tool("fetch")], "change");
    add(&s, "fetch", h.seq());
    let r = Session::from_restore(
        s.id().clone(),
        &s.snapshot_events(None, None),
        (*s.header()).clone(),
        offset(0),
        SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert_eq!(r.tool_history().unwrap(), s.tool_history().unwrap());
    request(&r, vec![tool("search")], "change");
    remove(&r, "fetch");
    assert_eq!(
        r.tool_history().unwrap()["updates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        s.tool_history().unwrap()["updates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn refuses_an_addition_whose_referenced_header_lacks_the_definition() {
    let mut p = hoh_session::tool_history::ToolHistoryProjection::new();
    p.apply(&event("request/header",0,json!({"header":{"config":{"provider":"mock","model":"mock"},"tools":[tool("search")]},"reason":"initial"}))).unwrap();
    let mut d = data("developer", "");
    d["headerSeq"] = json!(0);
    d["message"]["content"] = json!([{"type":"tool-addition","toolName":"fetch"}]);
    error(
        p.apply(&event("developer/message", 1, d)),
        "tool history: missing definition for fetch",
    );
}
