//! The cases of loop.spec.ts that run with a toolset: the tool round trip, steering and context
//! injection while a tool executes, concluding tools and the per-step extension point.
mod common;

use common::*;
use hoh_agent::{Agent, AgentPreStep, AgentTurnStopping, PreStepDecision};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, create_user_message};
use hoh_session::SessionEventEmitted;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn message_of(source: &str, text: &str) -> hoh_llm::UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new(source),
    })
}

fn requests_json(env: &Env, index: usize) -> Value {
    serde_json::to_value(&lock(&env.adapter.requests)[index].messages).expect("messages")
}

#[tokio::test]
async fn round_trips_tool_calls_model_requests_executes_and_result_reaches_the_next_request() {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("c1", "echo", &json!({"text": "ping"}), Some("calling echo")),
        text_response("done"),
    ]);
    let env = harness(adapter.clone()).await;
    register_tool(
        &env,
        content_tool(
            "echo",
            json!({"text": {"type": "string"}}),
            false,
            |args, _| async move {
                Ok(vec![ContentBlock::text(format!(
                    "echo: {}",
                    args["text"].as_str().unwrap_or_default()
                ))])
            },
        ),
    );
    let agent = create_agent(&env, "a1").await;

    send_and_wait(&agent, "use the tool").await;

    // Two model calls happened (tool-call step, then final step).
    assert_eq!(adapter.request_count(), 2);
    let second = requests_json(&env, 1);
    let tool = second
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("the second request carries the tool result");
    assert_eq!(tool["toolCallId"], "c1");
    assert_eq!(tool["isError"], false);
    assert_eq!(
        tool["content"],
        json!([{"type": "text", "text": "echo: ping"}])
    );
    let types = event_types(&agent);
    assert!(types.contains(&"tool/call".to_owned()));
    assert!(types.contains(&"tool/result".to_owned()));
}

#[tokio::test]
async fn injects_steering_between_steps_and_continues_the_turn() {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("c1", "slow", &json!({}), None),
        text_response("addressed the steering"),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;
    let steered = agent.clone();
    register_tool(
        &env,
        content_tool("slow", json!({}), false, move |_, _| {
            let agent = steered.clone();
            async move {
                // Steer while the turn is running (during tool execution).
                agent.steer(message_of("user", "change of plans"))?;
                Ok(vec![ContentBlock::text("tool done")])
            }
        }),
    );

    send_and_wait(&agent, "start").await;

    let events = agent.session().snapshot_events(None, None);
    let steering = events
        .iter()
        .find(|e| {
            e.event_type() == "user/message"
                && e.data()["content"].to_string().contains("change of plans")
        })
        .expect("steering recorded");
    // The entered batch is appended after the second step opens and before its request derives
    // history.
    let second_step = events
        .iter()
        .filter(|e| e.event_type() == "step/start")
        .nth(1)
        .expect("a second step");
    assert!(steering.seq() > second_step.seq());
    assert!(
        requests_json(&env, 1)
            .to_string()
            .contains("change of plans")
    );
}

#[tokio::test]
async fn defers_inject_during_tool_execution_until_after_the_tool_result() {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("c1", "noticer", &json!({}), Some("calling")),
        text_response("done"),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;
    let visible_during_tool = Arc::new(AtomicBool::new(true));
    {
        let (subject, flag) = (agent.clone(), visible_during_tool.clone());
        register_tool(
            &env,
            content_tool("noticer", json!({}), false, move |_, _| {
                let (agent, flag) = (subject.clone(), flag.clone());
                async move {
                    tokio::task::yield_now().await;
                    agent.inject(message_of("x", "mid-turn notice"))?;
                    agent.inject(message_of("x", "second notice"))?;
                    flag.store(
                        events_of(&agent, "user/message")
                            .iter()
                            .any(|e| e["data"]["source"]["kind"] == "x"),
                        Ordering::SeqCst,
                    );
                    Ok(vec![ContentBlock::text("ok")])
                }
            }),
        );
    }

    send_and_wait(&agent, "go").await;

    assert!(!visible_during_tool.load(Ordering::SeqCst));
    // The injection stays in the open turn, but its user-role context cannot split the assistant
    // tool call from the provider's tool-result message.
    assert_eq!(events_of(&agent, "turn/start").len(), 1);
    let events = agent.session().snapshot_events(None, None);
    let result = events
        .iter()
        .find(|e| e.event_type() == "tool/result")
        .unwrap();
    let contexts: Vec<_> = events
        .iter()
        .filter(|e| e.event_type() == "user/message" && e.data()["source"]["kind"] == "x")
        .collect();
    assert_eq!(contexts.len(), 2);
    assert!(result.seq() < contexts[0].seq());
    let texts: Vec<_> = contexts
        .iter()
        .map(|e| e.data()["content"][0]["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(texts, ["mid-turn notice", "second notice"]);

    let second = requests_json(&env, 1);
    let messages = second.as_array().unwrap();
    let result_index = messages.iter().position(|m| m["role"] == "tool").unwrap();
    let context_indexes: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            let text = m["content"].to_string();
            text.contains("mid-turn notice") || text.contains("second notice")
        })
        .map(|(index, _)| index)
        .collect();
    assert_eq!(context_indexes.len(), 2);
    assert!(context_indexes.iter().all(|index| *index > result_index));
}

#[tokio::test]
async fn turn_stopping_can_steer_another_step_the_loop_pattern() {
    let adapter = MockAdapter::chunks(vec![
        text_response("step 1"),
        text_response("step 2"),
        text_response("step 3"),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;
    let steps = Arc::new(AtomicUsize::new(0));
    {
        let (session, steps) = (agent.session().clone(), steps.clone());
        env.ctx
            .on_contained::<SessionEventEmitted>(move |emitted| {
                if emitted.session == session && emitted.event.event_type() == "step/end" {
                    steps.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            })
            .expect("listener");
    }
    {
        let steps = steps.clone();
        env.ctx
            .on_serial::<AgentTurnStopping, (), _>(move |event| {
                let (subject, steps) = (event.agent.clone(), steps.clone());
                async move {
                    if steps.load(Ordering::SeqCst) < 3 {
                        subject.steer(message_of("loop-test", "continue"))?;
                    }
                    Ok(None)
                }
            })
            .expect("listener");
    }

    send_and_wait(&agent, "go").await;

    assert_eq!(steps.load(Ordering::SeqCst), 3);
    assert_eq!(adapter.request_count(), 3);
}

#[tokio::test]
async fn a_tool_can_conclude_the_turn_despite_owing_a_follow_up_request() {
    let adapter = MockAdapter::chunks(vec![tool_call_response(
        "c1",
        "echo",
        &json!({"text": "x"}),
        None,
    )]);
    let env = harness(adapter.clone()).await;
    register_tool(
        &env,
        content_tool(
            "echo",
            json!({"text": {"type": "string"}}),
            false,
            |args, exec| async move {
                exec.conclude_turn();
                Ok(vec![ContentBlock::text(
                    args["text"].as_str().unwrap_or_default(),
                )])
            },
        ),
    );
    let agent = create_agent(&env, "a1").await;

    send_and_wait(&agent, "go").await;

    // Only one model call despite the tool call requesting a follow-up; the tool still executes
    // and durably records its result.
    assert_eq!(adapter.request_count(), 1);
    assert_eq!(events_of(&agent, "tool/result").len(), 1);
}

#[tokio::test]
async fn continues_for_steering_that_arrived_during_a_concluding_tool_step() {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("c1", "finalize", &json!({}), None),
        text_response("next turn reply"),
    ]);
    let env = harness(adapter.clone()).await;
    let agent = create_agent(&env, "a1").await;
    let steered = agent.clone();
    register_tool(
        &env,
        content_tool("finalize", json!({}), false, move |_, exec| {
            let agent = steered.clone();
            async move {
                // Steering lands while the concluding tool is still executing.
                agent.steer(message_of("user", "late steering"))?;
                exec.conclude_turn();
                Ok(vec![ContentBlock::text("final")])
            }
        }),
    );

    send_and_wait(&agent, "go").await;

    assert_eq!(adapter.request_count(), 2);
    assert_eq!(events_of(&agent, "turn/end").len(), 1);
    assert!(requests_json(&env, 1).to_string().contains("late steering"));
}

#[tokio::test]
async fn pre_step_fires_once_per_proposed_step_before_the_step_is_opened() {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("c1", "echo", &json!({}), Some("calling echo")),
        text_response("done"),
    ]);
    let env = harness(adapter).await;
    register_tool(
        &env,
        content_tool("echo", json!({}), false, |_, _| async {
            Ok(vec![ContentBlock::text("echoed")])
        }),
    );
    let agent = create_agent(&env, "a1").await;
    let fires = Arc::new(Mutex::new(Vec::<(u64, u64)>::new()));
    {
        let (subject, fires) = (agent.clone(), fires.clone());
        env.ctx
            .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
                let (subject, fires) = (subject.clone(), fires.clone());
                async move {
                    if event.agent.id() == subject.id() {
                        lock(&fires).push((event.turn, event.step));
                    }
                    next.run().await
                }
            })
            .expect("listener");
    }

    send_and_wait(&agent, "go").await;

    assert_eq!(*lock(&fires), [(1, 1), (1, 2)]);
}

#[tokio::test]
async fn pre_step_fires_before_its_step_boundary_opens() {
    let adapter = MockAdapter::chunks(vec![text_response("ok")]);
    let env = harness(adapter).await;
    let agent = create_agent(&env, "a1").await;
    let boundary_open = Arc::new(AtomicBool::new(true));
    {
        let (subject, flag) = (agent.clone(), boundary_open.clone());
        env.ctx
            .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
                let (subject, flag) = (subject.clone(), flag.clone());
                async move {
                    if event.agent.id() == subject.id() {
                        flag.store(
                            event_types(&subject).last().map(String::as_str) == Some("step/start"),
                            Ordering::SeqCst,
                        );
                    }
                    next.run().await
                }
            })
            .expect("listener");
    }

    send_and_wait(&agent, "go").await;

    assert!(!boundary_open.load(Ordering::SeqCst));
}

fn _unused(_: &Arc<dyn Agent>) {}
