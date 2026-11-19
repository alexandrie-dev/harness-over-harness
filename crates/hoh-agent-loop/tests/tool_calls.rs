//! Port of tool-calls.spec.ts: scheduler ordering, barriers, the rolling pool and cancellation with
//! deterministic gated tools. Failure-quiescence cases that replace private pipeline methods are
//! deferred (see PORT_NOTES.md).
mod common;

use common::*;
use hoh_agent::{Agent, CancelOptions};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, UserMessage, create_user_message};
use hoh_session::{AgentCancelCause, SessionEventEmitted};
use hoh_tools::{
    PostToolDecision, PreToolDecision, TOOL_ABORTED_BEFORE_DISPATCH, ToolErrorInfo,
    ToolRegistrationOptions, ToolsPostExecute, ToolsPreExecute, ToolsResult,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn go(agent: &Arc<dyn Agent>) {
    agent.followup(user_message("go")).expect("followup");
}

fn id_calls(name: &str, ids: &[&str]) -> Vec<StreamChunk> {
    let calls: Vec<(String, Value)> = ids
        .iter()
        .map(|id| (format!("c{id}"), json!({ "id": id })))
        .collect();
    let borrowed: Vec<(&str, &str, Value)> = calls
        .iter()
        .map(|(call, args)| (call.as_str(), name, args.clone()))
        .collect();
    multi_call(&borrowed)
}

use hoh_llm::StreamChunk;

fn context_message(text: &str) -> UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("p"),
    })
}

fn user_cancel(agent: &Arc<dyn Agent>) {
    agent
        .cancel(AgentCancelCause::User, CancelOptions::default())
        .expect("cancel");
}

fn result_summary(agent: &Arc<dyn Agent>) -> Vec<Value> {
    events_of(agent, "tool/result")
        .iter()
        .map(|event| {
            json!({
                "callId": event["data"]["message"]["toolCallId"],
                "isError": event["data"]["message"]["isError"],
                "error": event["data"]["error"],
            })
        })
        .collect()
}

// ---------------------------------------------------------------- grouping and barriers

#[tokio::test]
async fn runs_parallel_safe_siblings_concurrently() {
    let adapter = MockAdapter::chunks(vec![id_calls("p", &["1", "2", "3"]), text_response("done")]);
    let env = harness(adapter).await;
    let gated = Gated::parallel("p");
    register_tool(&env, gated.tool.clone());
    let agent = create_agent(&env, "a1").await;

    go(&agent);
    until(|| gated.started().len() == 3).await;
    assert_eq!(gated.started(), ["1", "2", "3"]);
    for id in ["1", "2", "3"] {
        gated.release(id);
    }
    agent.when_idle().await;
    assert_eq!(events_of(&agent, "tool/result").len(), 3);
}

#[tokio::test]
async fn an_exclusive_call_between_two_parallel_safe_calls_forms_a_barrier() {
    let adapter = MockAdapter::chunks(vec![
        multi_call(&[
            ("c1", "r", json!({"id": "A1"})),
            ("c2", "w", json!({"id": "A2"})),
            ("c3", "r", json!({"id": "A3"})),
        ]),
        text_response("done"),
    ]);
    let env = harness(adapter).await;
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let read_log = order.clone();
    register_tool(
        &env,
        content_tool("r", id_param(), true, move |args, _| {
            let log = read_log.clone();
            async move {
                let id = args["id"].as_str().unwrap().to_owned();
                lock(&log).push(format!("r-start-{id}"));
                lock(&log).push(format!("r-end-{id}"));
                Ok(vec![ContentBlock::text("r")])
            }
        }),
    );
    let write_log = order.clone();
    register_tool(
        &env,
        content_tool("w", id_param(), false, move |args, _| {
            let log = write_log.clone();
            async move {
                lock(&log).push(format!("w-{}", args["id"].as_str().unwrap()));
                Ok(vec![ContentBlock::text("w")])
            }
        }),
    );
    let agent = create_agent(&env, "a1").await;
    send_and_wait(&agent, "go").await;

    assert_eq!(
        *lock(&order),
        ["r-start-A1", "r-end-A1", "w-A2", "r-start-A3", "r-end-A3"]
    );
}

#[tokio::test]
async fn rejects_pending_calls_whose_original_registration_was_replaced() {
    let adapter = MockAdapter::chunks(vec![
        multi_call(&[
            ("c1", "replace", json!({"id": "0"})),
            ("c2", "x", json!({"id": "1"})),
            ("c3", "x", json!({"id": "2"})),
        ]),
        text_response("done"),
    ]);
    let env = harness(adapter).await;
    let replacement = Gated::exclusive("x");
    let dispose_safe = Arc::new(Mutex::new(Some(register_tool(
        &env,
        content_tool("x", id_param(), true, |args, _| async move {
            Ok(vec![ContentBlock::text(format!(
                "old-{}",
                args["id"].as_str().unwrap()
            ))])
        }),
    ))));
    let (tools, ctx, replacement_tool) =
        (env.tools.clone(), env.ctx.clone(), replacement.tool.clone());
    register_tool(
        &env,
        content_tool("replace", id_param(), false, move |_, _| {
            let (tools, ctx, replacement_tool) =
                (tools.clone(), ctx.clone(), replacement_tool.clone());
            let disposer = lock(&dispose_safe).take();
            async move {
                if let Some(disposer) = disposer {
                    disposer.dispose().await;
                }
                tools
                    .register(&ctx, replacement_tool, ToolRegistrationOptions::default())
                    .expect("replacement registers");
                Ok(vec![ContentBlock::text("replaced")])
            }
        }),
    );
    let agent = create_agent(&env, "a1").await;

    send_and_wait(&agent, "go").await;
    assert!(replacement.started().is_empty());
    let codes: Vec<_> = result_codes(&agent).into_iter().skip(1).collect();
    assert_eq!(
        codes,
        [
            ("c2".to_owned(), Some("UNKNOWN_TOOL".to_owned())),
            ("c3".to_owned(), Some("UNKNOWN_TOOL".to_owned()))
        ]
    );
}

#[tokio::test]
async fn drains_started_calls_and_rejects_unstarted_calls_after_replacement() {
    let adapter = MockAdapter::chunks(vec![id_calls("x", &["1", "2", "3"]), text_response("done")]);
    let env = harness_limits(adapter, Some(2), None).await;
    let initial = Gated::parallel("x");
    let replacement = Gated::exclusive("x");
    let dispose_initial = Arc::new(Mutex::new(Some(register_tool(&env, initial.tool.clone()))));
    {
        let (tools, ctx, replacement_tool) =
            (env.tools.clone(), env.ctx.clone(), replacement.tool.clone());
        env.ctx
            .on_contained::<ToolsResult>(move |event| {
                if event.exec.call_id.as_str() != "c1" {
                    return Ok(());
                }
                if let Some(disposer) = lock(&dispose_initial).take() {
                    drop(disposer.dispose());
                }
                tools.register(
                    &ctx,
                    replacement_tool.clone(),
                    ToolRegistrationOptions::default(),
                )?;
                Ok(())
            })
            .expect("listener");
    }
    let agent = create_agent(&env, "a1").await;

    go(&agent);
    until(|| initial.started().len() == 2).await;
    initial.release("1");
    until(|| result_codes(&agent).iter().any(|(id, _)| id == "c1")).await;
    settle(5).await;
    assert!(replacement.started().is_empty());
    initial.release("2");
    agent.when_idle().await;
    assert!(replacement.started().is_empty());
    assert_eq!(
        result_codes(&agent)
            .last()
            .and_then(|(_, code)| code.clone()),
        Some("UNKNOWN_TOOL".to_owned())
    );
}

// ---------------------------------------------------------------- model-order results

#[tokio::test]
async fn commits_tool_results_in_model_order_when_a_later_call_settles_first() {
    let adapter = MockAdapter::chunks(vec![id_calls("p", &["1", "2"]), text_response("done")]);
    let env = harness(adapter).await;
    let gated = Gated::parallel("p");
    register_tool(&env, gated.tool.clone());
    let agent = create_agent(&env, "a1").await;

    go(&agent);
    until(|| gated.started().len() == 2).await;
    gated.release("2");
    settle(5).await;
    assert!(events_of(&agent, "tool/result").is_empty());
    gated.release("1");
    agent.when_idle().await;

    let ids: Vec<_> = result_codes(&agent).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["c1", "c2"]);
}

#[tokio::test]
async fn derived_history_pairs_calls_in_model_order() {
    let adapter = MockAdapter::chunks(vec![id_calls("p", &["1", "2"]), text_response("done")]);
    let env = harness(adapter).await;
    let gated = Gated::parallel("p");
    register_tool(&env, gated.tool.clone());
    let agent = create_agent(&env, "a1").await;
    go(&agent);
    until(|| gated.started().len() == 2).await;
    gated.release("2");
    gated.release("1");
    agent.when_idle().await;

    let messages = agent.session().derive_messages().expect("messages");
    let tool_ids: Vec<_> = messages
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["toolCallId"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(tool_ids, ["c1", "c2"]);
}

// ---------------------------------------------------------------- rolling pool

async fn pool_peak(executor_limit: u64, loop_limit: Option<u64>) -> (u64, usize) {
    let calls: Vec<(String, Value)> = (0..12).map(|n| (format!("c{n}"), json!({}))).collect();
    let borrowed: Vec<(&str, &str, Value)> = calls
        .iter()
        .map(|(id, args)| (id.as_str(), "overlap", args.clone()))
        .collect();
    let adapter = MockAdapter::chunks(vec![multi_call(&borrowed), text_response("done")]);
    let env = harness_limits(adapter, loop_limit, Some(executor_limit)).await;
    let (active, peak) = (Arc::new(Mutex::new(0u64)), Arc::new(Mutex::new(0u64)));
    let (a, p) = (active.clone(), peak.clone());
    register_tool(
        &env,
        content_tool("overlap", json!({}), true, move |_, _| {
            let (a, p) = (a.clone(), p.clone());
            async move {
                {
                    let mut active = lock(&a);
                    *active += 1;
                    let mut peak = lock(&p);
                    *peak = (*peak).max(*active);
                }
                // Let the scheduler start every available sibling before any body settles.
                settle(5).await;
                *lock(&a) -= 1;
                Ok(vec![ContentBlock::text("done")])
            }
        }),
    );
    let agent = create_agent(&env, "override").await;
    send_and_wait(&agent, "go").await;
    assert_eq!(*lock(&active), 0);
    let results = events_of(&agent, "tool/result").len();
    let peak = *lock(&peak);
    env.close().await;
    (peak, results)
}

#[tokio::test]
async fn the_loop_limit_overrides_a_smaller_executor_limit() {
    assert_eq!(pool_peak(1, Some(2)).await, (2, 12));
}

#[tokio::test]
async fn the_loop_limit_caps_a_larger_executor_limit() {
    assert_eq!(pool_peak(3, Some(1)).await, (1, 12));
}

#[tokio::test]
async fn the_default_loop_limit_is_ten() {
    assert_eq!(pool_peak(2, None).await, (10, 12));
}

#[tokio::test]
async fn rejects_invalid_global_max_parallel_tool_calls_at_plugin_load() {
    for limit in [json!(0), json!(1.5)] {
        let outcome = try_harness(
            MockAdapter::chunks(vec![]),
            json!({ "agents": [], "maxParallelToolCalls": limit }),
            None,
            None,
        )
        .await;
        assert!(outcome.is_err(), "limit {limit} was accepted");
    }
}

#[tokio::test]
async fn starts_at_most_the_cap_replenishing_as_calls_settle() {
    let adapter = MockAdapter::chunks(vec![
        id_calls("p", &["1", "2", "3", "4"]),
        text_response("done"),
    ]);
    let env = harness_limits(adapter, Some(2), None).await;
    let gated = Gated::parallel("p");
    register_tool(&env, gated.tool.clone());
    let agent = create_agent(&env, "a1").await;

    go(&agent);
    until(|| gated.started().len() == 2).await;
    settle(5).await;
    assert_eq!(gated.started(), ["1", "2"]);
    gated.release("1");
    until(|| gated.started().len() == 3).await;
    assert_eq!(gated.started(), ["1", "2", "3"]);
    let order: Vec<String> = agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|e| matches!(e.event_type(), "tool/call" | "tool/result"))
        .map(|e| {
            if e.event_type() == "tool/call" {
                format!("tool/call:{}", e.data()["callId"].as_str().unwrap())
            } else {
                format!(
                    "tool/result:{}",
                    e.data()["message"]["toolCallId"].as_str().unwrap()
                )
            }
        })
        .take(4)
        .collect();
    assert_eq!(
        order,
        [
            "tool/call:c1",
            "tool/call:c2",
            "tool/result:c1",
            "tool/call:c3"
        ]
    );
    gated.release("2");
    gated.release("3");
    until(|| gated.started().len() == 4).await;
    gated.release("4");
    agent.when_idle().await;
    let ids: Vec<_> = result_codes(&agent).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["c1", "c2", "c3", "c4"]);
}

#[tokio::test]
async fn max_parallel_tool_calls_of_one_is_fully_serial() {
    let adapter = MockAdapter::chunks(vec![id_calls("p", &["1", "2"]), text_response("done")]);
    let env = harness_limits(adapter, Some(1), None).await;
    let gated = Gated::parallel("p");
    register_tool(&env, gated.tool.clone());
    let agent = create_agent(&env, "a1").await;
    go(&agent);
    until(|| gated.started().len() == 1).await;
    settle(5).await;
    assert_eq!(gated.started(), ["1"]);
    gated.release("1");
    until(|| gated.started().len() == 2).await;
    gated.release("2");
    agent.when_idle().await;
}

// ---------------------------------------------------------------- middleware

#[tokio::test]
async fn pre_and_post_execute_observe_model_call_order() {
    let adapter = MockAdapter::chunks(vec![id_calls("p", &["1", "2", "3"]), text_response("done")]);
    let env = harness(adapter).await;
    let gated = Gated::parallel("p");
    register_tool(&env, gated.tool.clone());
    let pre = Arc::new(Mutex::new(Vec::<String>::new()));
    let post = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = pre.clone();
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| {
            let sink = sink.clone();
            async move {
                lock(&sink).push(event.exec.call_id.to_string());
                next.run().await
            }
        })
        .expect("pre listener");
    let sink = post.clone();
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, next| {
            let sink = sink.clone();
            async move {
                lock(&sink).push(event.exec.call_id.to_string());
                next.run().await
            }
        })
        .expect("post listener");
    let agent = create_agent(&env, "a1").await;

    go(&agent);
    until(|| gated.started().len() == 3).await;
    for id in ["3", "2", "1"] {
        gated.release(id);
    }
    agent.when_idle().await;

    assert_eq!(*lock(&pre), ["c1", "c2", "c3"]);
    assert_eq!(*lock(&post), ["c1", "c2", "c3"]);
}

fn texts_of_non_user_messages(agent: &Arc<dyn Agent>) -> Vec<String> {
    events_of(agent, "user/message")
        .iter()
        .filter(|e| e["data"]["source"]["kind"] != "user")
        .map(|e| e["data"]["content"][0]["text"].as_str().unwrap().to_owned())
        .collect()
}

fn accept_with_context(text: String) -> PostToolDecision {
    PostToolDecision::Accept {
        content: None,
        value: None,
        additional_contexts: Some(vec![context_message(&text)]),
    }
}

#[tokio::test]
async fn injects_additional_contexts_in_model_call_order_not_settlement_order() {
    let adapter = MockAdapter::chunks(vec![id_calls("p", &["1", "2"]), text_response("done")]);
    let env = harness_limits(adapter, Some(2), None).await;
    let gated = Gated::parallel("p");
    register_tool(&env, gated.tool.clone());
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, _next| {
            let text = format!("ctx-{}", event.exec.call_id);
            async move { Ok(accept_with_context(text)) }
        })
        .expect("post listener");
    let agent = create_agent(&env, "a1").await;

    go(&agent);
    until(|| gated.started().len() == 2).await;
    gated.release("2");
    gated.release("1");
    agent.when_idle().await;

    assert_eq!(texts_of_non_user_messages(&agent), ["ctx-c1", "ctx-c2"]);
    let log = event_types(&agent);
    let last_result = log.iter().rposition(|t| t == "tool/result").unwrap();
    let first_context = agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .position(|e| e.event_type() == "user/message" && e.data()["source"]["kind"] != "user")
        .unwrap();
    assert!(last_result < first_context);
}

#[tokio::test]
async fn orders_pre_execute_denials_and_errors_without_dispatching_them() {
    let adapter = MockAdapter::chunks(vec![id_calls("p", &["1", "2", "3"]), text_response("done")]);
    let env = harness(adapter).await;
    let gated = Gated::parallel("p");
    register_tool(&env, gated.tool.clone());
    let post = Arc::new(Mutex::new(Vec::<String>::new()));
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| async move {
            match event.exec.call_id.as_str() {
                "c2" => Ok(PreToolDecision::Deny {
                    reason: "blocked by policy".into(),
                    info: Some(ToolErrorInfo {
                        name: "AutoReviewDeniedError".into(),
                        code: "AUTO_REVIEW_DENIED".into(),
                        reason: Some("exact scope was not authorized".into()),
                    }),
                }),
                "c3" => Err("pre exploded".into()),
                _ => next.run().await,
            }
        })
        .expect("pre listener");
    let sink = post.clone();
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, next| {
            let sink = sink.clone();
            async move {
                lock(&sink).push(event.exec.call_id.to_string());
                next.run().await
            }
        })
        .expect("post listener");
    let agent = create_agent(&env, "a1").await;

    go(&agent);
    until(|| gated.started().len() == 1).await;
    gated.release("1");
    agent.when_idle().await;

    assert_eq!(gated.started(), ["1"]);
    assert_eq!(*lock(&post), ["c1", "c2"]);
    let results = events_of(&agent, "tool/result");
    let ids: Vec<_> = results
        .iter()
        .map(|e| {
            e["data"]["message"]["toolCallId"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(ids, ["c1", "c2", "c3"]);
    let text = |index: usize| {
        results[index]["data"]["message"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(text(1).contains("blocked by policy"));
    assert_eq!(
        results[1]["data"]["error"],
        json!({
            "name": "AutoReviewDeniedError",
            "code": "AUTO_REVIEW_DENIED",
            "reason": "exact scope was not authorized"
        })
    );
    assert!(text(2).contains("pre exploded"));
}

// ---------------------------------------------------------------- abort handling

fn aborted_before_dispatch(call: &str) -> Value {
    json!({
        "callId": call,
        "isError": true,
        "error": { "name": "AbortError", "code": TOOL_ABORTED_BEFORE_DISPATCH },
    })
}

#[tokio::test]
async fn starts_no_calls_when_the_signal_is_already_aborted_before_a_parallel_group() {
    let adapter = MockAdapter::chunks(vec![
        id_calls("p", &["1", "2"]),
        text_response("should never be requested"),
    ]);
    let env = harness(adapter).await;
    let gated = Gated::parallel("p");
    register_tool(&env, gated.tool.clone());
    let agent = create_agent(&env, "a1").await;
    {
        let (session, victim) = (agent.session().clone(), agent.clone());
        env.ctx
            .on_contained::<SessionEventEmitted>(move |emitted| {
                if emitted.session == session && emitted.event.event_type() == "assistant/message" {
                    user_cancel(&victim);
                }
                Ok(())
            })
            .expect("listener");
    }

    send_and_wait(&agent, "go").await;

    assert!(gated.started().is_empty());
    assert_eq!(call_ids(&agent, "tool/call"), ["c1", "c2"]);
    assert_eq!(
        result_summary(&agent),
        [aborted_before_dispatch("c1"), aborted_before_dispatch("c2")]
    );
}

#[tokio::test]
async fn skips_dispatch_and_stops_starting_siblings_when_abort_fires_during_pre_execute() {
    let adapter = MockAdapter::chunks(vec![
        id_calls("p", &["1", "2"]),
        text_response("should never be requested"),
    ]);
    let env = harness(adapter).await;
    let gated = Gated::parallel("p");
    register_tool(&env, gated.tool.clone());
    let agent = create_agent(&env, "a1").await;
    {
        let victim = agent.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| {
                let victim = victim.clone();
                async move {
                    if event.exec.call_id.as_str() == "c1" {
                        user_cancel(&victim);
                    }
                    next.run().await
                }
            })
            .expect("listener");
    }

    send_and_wait(&agent, "go").await;

    assert!(gated.started().is_empty());
    assert_eq!(call_ids(&agent, "tool/call"), ["c1", "c2"]);
    assert_eq!(
        result_summary(&agent),
        [aborted_before_dispatch("c1"), aborted_before_dispatch("c2")]
    );
}

#[tokio::test]
async fn stops_replenishing_after_abort_commits_started_results_and_parks_contexts() {
    let adapter = MockAdapter::chunks(vec![
        id_calls("p", &["1", "2", "3", "4"]),
        text_response("after wake"),
    ]);
    let env = harness_limits(adapter, Some(2), None).await;
    let gated = Gated::parallel("p");
    register_tool(&env, gated.tool.clone());
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, _next| {
            let text = format!("ctx-{}", event.exec.call_id);
            async move { Ok(accept_with_context(text)) }
        })
        .expect("post listener");
    let agent = create_agent(&env, "a1").await;

    go(&agent);
    until(|| gated.started().len() == 2).await;
    user_cancel(&agent);
    gated.release("1");
    gated.release("2");
    agent.when_idle().await;

    assert_eq!(gated.started(), ["1", "2"]);
    assert_eq!(call_ids(&agent, "tool/call"), ["c1", "c2", "c3", "c4"]);
    let ids: Vec<_> = result_codes(&agent).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["c1", "c2", "c3", "c4"]);
    assert_eq!(
        result_summary(&agent)[2..],
        [aborted_before_dispatch("c3"), aborted_before_dispatch("c4")]
    );
    let settled: Vec<_> = event_types(&agent)
        .into_iter()
        .filter(|t| t == "tool/result" || t == "user/message")
        .collect();
    // The accepted contexts are parked in the inbox, not yet in the log.
    assert_eq!(
        settled,
        [
            "user/message",
            "tool/result",
            "tool/result",
            "tool/result",
            "tool/result"
        ]
    );
    let parked: Vec<_> = agent
        .inbox()
        .next_step()
        .iter()
        .map(|m| m.content[0].text_value().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(parked, ["ctx-c1", "ctx-c2"]);

    send_and_wait(&agent, "wake").await;
    assert_eq!(texts_of_non_user_messages(&agent), ["ctx-c1", "ctx-c2"]);
}

#[tokio::test]
async fn does_not_run_an_exclusive_barrier_after_a_parallel_group_aborts() {
    let adapter = MockAdapter::chunks(vec![
        multi_call(&[
            ("c1", "p", json!({"id": "1"})),
            ("c2", "p", json!({"id": "2"})),
            ("c3", "x", json!({"id": "3"})),
        ]),
        text_response("should never be requested"),
    ]);
    let env = harness_limits(adapter, Some(2), None).await;
    let gated = Gated::parallel("p");
    let exclusive = Arc::new(Mutex::new(Vec::<String>::new()));
    register_tool(&env, gated.tool.clone());
    let log = exclusive.clone();
    register_tool(
        &env,
        content_tool("x", id_param(), false, move |args, _| {
            let log = log.clone();
            async move {
                lock(&log).push(args["id"].as_str().unwrap().to_owned());
                Ok(vec![ContentBlock::text("x")])
            }
        }),
    );
    let agent = create_agent(&env, "a1").await;

    go(&agent);
    until(|| gated.started().len() == 2).await;
    user_cancel(&agent);
    gated.release("1");
    gated.release("2");
    agent.when_idle().await;

    assert!(lock(&exclusive).is_empty());
    assert_eq!(call_ids(&agent, "tool/call"), ["c1", "c2", "c3"]);
    let last = events_of(&agent, "tool/result").pop().expect("a result");
    assert_eq!(last["data"]["message"]["source"]["kind"], "tool");
    assert_eq!(last["data"]["message"]["source"]["callId"], "c3");
    assert_eq!(
        last["data"]["message"]["content"][0]["text"],
        "Error: tool call aborted before dispatch"
    );
    assert_eq!(last["data"]["message"]["isError"], true);
    assert_eq!(
        last["data"]["error"],
        json!({ "name": "AbortError", "code": TOOL_ABORTED_BEFORE_DISPATCH })
    );
}
