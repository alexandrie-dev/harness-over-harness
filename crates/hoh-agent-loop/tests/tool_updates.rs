//! Port of tool-updates.spec.ts: the loop logs tool changes as developer messages and carries
//! session tool history to the runtime; step-time tool activation (`when` conditions).
//!
//! `Object.isFrozen(request.toolHistory)` has no Rust counterpart (values are immutable once owned
//! by the adapter); presence of the history is asserted instead.
mod support_s17;

use hoh_agent::{
    Agent, AgentPreStep, AgentRequestError, AssembleModelSelection, CancelOptions, PreStepDecision,
    RequestErrorAction,
};
use hoh_llm::GenerateOptions;
use hoh_session::AgentCancelCause;
use hoh_system_prompt::{PromptSection, PromptText};
use hoh_tools::{ResolvedToolSet, ToolActivationContext, ToolRegistrationOptions};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use support_s17::*;

async fn harness(adapter: Arc<dyn hoh_llm::LlmAdapter>) -> (Stack, Arc<dyn Agent>) {
    let stack = stack(
        adapter,
        StackOptions {
            system_prompt: json!({ "personaPrefix": "", "personaSuffix": "" }),
            ..StackOptions::default()
        },
    )
    .await;
    let agent = stack.create_agent("tool-updates", "model").await;
    (stack, agent)
}

async fn send(agent: &Arc<dyn Agent>, text: &str) {
    send_and_wait(agent, text).await;
}

fn tool(stack: &Stack, name: &str) -> hoh_kernel::Disposer {
    stack
        .tools
        .register(
            &stack.ctx,
            text_tool(name, &format!("{name} tool"), "done"),
            ToolRegistrationOptions::default(),
        )
        .expect("tool registers")
}

fn developer_events(agent: &Arc<dyn Agent>) -> Vec<Value> {
    events_of(agent, "developer/message")
}

fn header_events(agent: &Arc<dyn Agent>) -> Vec<Value> {
    events_of(agent, "request/header")
}

/// `request.messages.filter(role === 'developer').map(message => message.content)`.
fn developer_content(request: &GenerateOptions) -> Vec<Value> {
    request
        .messages
        .iter()
        .filter(|message| message.role() == "developer")
        .map(|message| serde_json::to_value(message.content()).expect("content"))
        .collect()
}

fn requests(adapter: &MockAdapter) -> Vec<GenerateOptions> {
    lock(&adapter.requests).iter().map(clone_request).collect()
}

fn clone_request(options: &GenerateOptions) -> GenerateOptions {
    options.clone()
}

fn system_content(request: &GenerateOptions) -> Option<Value> {
    request
        .messages
        .iter()
        .find(|message| message.role() == "system")
        .map(|message| serde_json::to_value(message.content()).expect("content"))
}

fn names_of(request: &GenerateOptions) -> Option<Vec<String>> {
    tool_names(request)
}

// ------------------------------------------------------------------ tool update emission

#[tokio::test]
async fn logs_additions_bound_to_the_new_header_and_removals_without_one_after_the_first_header() {
    let adapter = MockAdapter::chunks((0..4).map(|_| text_response("ok")).collect());
    let (stack, agent) = harness(adapter.clone()).await;
    let dispose_search = tool(&stack, "search");
    send(&agent, "first").await;
    assert!(developer_events(&agent).is_empty());

    tool(&stack, "fetch");
    send(&agent, "second").await;
    let developer = developer_events(&agent);
    let addition = &developer[0];
    assert_match_object(
        addition,
        &json!({
            "surfaceOp": "append",
            "data": {
                "turn": 2, "step": 1, "headerSeq": header_events(&agent)[1]["seq"],
                "message": {
                    "role": "developer", "source": { "kind": "tool-registry" },
                    "content": [{ "type": "tool-addition", "toolName": "fetch" }],
                },
            },
        }),
    );

    dispose_search.dispose().await;
    send(&agent, "third").await;
    let removal = developer_events(&agent)[1].clone();
    assert!(removal["data"].get("headerSeq").is_none());
    assert_eq!(
        removal["data"]["message"]["content"],
        json!([{ "type": "tool-removal", "toolName": "search" }])
    );

    send(&agent, "fourth").await;
    assert_eq!(developer_events(&agent).len(), 2);
    let all = requests(&adapter);
    assert_eq!(
        serde_json::to_value(all.last().expect("requests").tool_history.as_ref()).expect("history"),
        *agent.session().tool_history().expect("tool history")
    );
    let update_counts: Vec<_> = all
        .iter()
        .map(|request| {
            request
                .tool_history
                .as_ref()
                .map(|history| history.updates.len())
        })
        .collect();
    assert_eq!(update_counts, [Some(0), Some(1), Some(2), Some(2)]);
    for request in &all {
        assert!(request.tool_history.is_some());
        assert!(developer_content(request).is_empty());
        assert!(
            !request
                .tools
                .iter()
                .flatten()
                .any(|schema| schema.defer_loading == Some(true))
        );
    }
    let tools: Vec<_> = all.iter().map(names_of).collect();
    let expect: Vec<Option<Vec<String>>> = [
        vec!["search"],
        vec!["fetch", "search"],
        vec!["fetch"],
        vec!["fetch"],
    ]
    .into_iter()
    .map(|names| Some(names.into_iter().map(str::to_owned).collect()))
    .collect();
    assert_eq!(tools, expect);
    stack.close().await;
}

#[tokio::test]
async fn sends_deferred_declarations_and_recorded_update_blocks_on_an_in_history_route() {
    let mock = MockAdapter::chunks((0..3).map(|_| text_response("ok")).collect());
    let mut adapter = ModelAdapter::new(mock.clone());
    adapter.tool_update = Some("in-history".into());
    let (stack, agent) = harness(Arc::new(adapter)).await;
    tool(&stack, "search");
    send(&agent, "first").await;
    let dispose_fetch = tool(&stack, "fetch");
    send(&agent, "second").await;
    dispose_fetch.dispose().await;
    send(&agent, "third").await;

    let reasons: Vec<_> = header_events(&agent)
        .iter()
        .map(|event| {
            (
                event["data"]["reason"].clone(),
                event["data"].get("startsSeries").cloned(),
            )
        })
        .collect();
    assert_eq!(
        reasons,
        [
            (json!("initial"), None),
            (json!("change"), None),
            (json!("change"), None)
        ]
    );
    let all = requests(&mock);
    let (first, second, third) = (&all[0], &all[1], &all[2]);
    assert_eq!(names_of(first), Some(vec!["search".to_owned()]));
    let second_tools = serde_json::to_value(&second.tools).expect("tools");
    let parameters = second_tools[0]["parameters"].clone();
    assert_eq!(
        second_tools,
        json!([
            { "name": "search", "description": "search tool", "parameters": parameters },
            { "name": "fetch", "description": "fetch tool",
              "parameters": second_tools[1]["parameters"], "deferLoading": true },
        ])
    );
    assert_eq!(
        developer_content(second),
        [json!([{ "type": "tool-addition", "toolName": "fetch" }])]
    );
    assert_eq!(third.tools, second.tools);
    assert_eq!(
        developer_content(third),
        [
            json!([{ "type": "tool-addition", "toolName": "fetch" }]),
            json!([{ "type": "tool-removal", "toolName": "fetch" }]),
        ]
    );
    let header = agent.session().request_header().expect("header");
    let header_tools: Vec<_> = header["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|schema| schema["name"].as_str().expect("name").to_owned())
        .collect();
    assert_eq!(header_tools, ["search"]);
    stack.close().await;
}

// ------------------------------------------------------------------ step-time tool activation

#[tokio::test]
async fn uses_the_step_model_selection_for_asynchronous_conditions_and_rejects_a_historical_name_after_removal()
 {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("allowed", "conditional", &json!({}), None),
        text_response("done"),
        tool_call_response("removed", "conditional", &json!({}), None),
        text_response("done"),
    ]);
    let (stack, agent) = harness(adapter.clone()).await;
    let selected_models = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    select_model(&agent, "enabled");
    let body_calls = calls.clone();
    let definition = fixture_tool(
        "conditional",
        "Available on one model.",
        json!({}),
        move |_, exec| {
            let calls = body_calls.clone();
            async move {
                let tools_agent = exec.agent.as_ref().expect("exec agent");
                let header = tools_agent.session().request_header().expect("header");
                assert_eq!(header["config"]["model"], "enabled");
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![hoh_llm::ContentBlock::text("done")])
            }
        },
    );
    let seen = selected_models.clone();
    stack
        .tools
        .register(
            &stack.ctx,
            definition,
            ToolRegistrationOptions {
                when: Some(Arc::new(move |context: ToolActivationContext| {
                    let seen = seen.clone();
                    Box::pin(async move {
                        lock(&seen).push(context.model_selection.model.clone());
                        tokio::task::yield_now().await;
                        Ok(context.model_selection.model == "enabled")
                    })
                })),
            },
        )
        .expect("conditional registers");
    send(&agent, "enabled").await;
    select_model(&agent, "disabled");
    send(&agent, "try the historical name").await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let selected = lock(&selected_models).clone();
    assert_eq!(selected, ["enabled", "enabled", "disabled", "disabled"]);
    let models: Vec<_> = requests(&adapter).iter().map(|r| r.model.clone()).collect();
    assert_eq!(models, selected);
    assert_eq!(
        developer_events(&agent).last().expect("developer")["data"]["message"]["content"],
        json!([{ "type": "tool-removal", "toolName": "conditional" }])
    );
    let result = events_of(&agent, "tool/result")
        .into_iter()
        .find(|event| event["data"]["message"]["toolCallId"] == "removed")
        .expect("removed result");
    assert_match_object(
        &result["data"],
        &json!({ "error": { "code": "UNKNOWN_TOOL" } }),
    );
    stack.close().await;
}

#[tokio::test]
async fn cancels_an_asynchronous_condition_without_committing_the_step_request() {
    let adapter = MockAdapter::chunks(vec![text_response("unused")]);
    let (stack, agent) = harness(adapter.clone()).await;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let (settle_tx, settle_rx) = tokio::sync::oneshot::channel::<bool>();
    let entered_tx = Mutex::new(Some(entered_tx));
    let settle_rx = Arc::new(tokio::sync::Mutex::new(Some(settle_rx)));
    stack
        .tools
        .register(
            &stack.ctx,
            text_tool("conditional", "A pending condition.", "unused"),
            ToolRegistrationOptions {
                when: Some(Arc::new(move |_| {
                    if let Some(entered) = lock(&entered_tx).take() {
                        let _ = entered.send(());
                    }
                    let settle = settle_rx.clone();
                    Box::pin(async move {
                        let receiver = settle.lock().await.take().expect("settle receiver");
                        Ok(receiver.await.unwrap_or(false))
                    })
                })),
            },
        )
        .expect("conditional registers");
    agent
        .followup(user_message("cancel during activation"))
        .expect("followup");
    entered_rx.await.expect("condition entered");
    agent
        .cancel(AgentCancelCause::User, CancelOptions::default())
        .expect("cancel");
    let _ = settle_tx.send(true);
    agent.when_idle().await;
    assert_eq!(adapter.request_count(), 0);
    assert!(
        events_of_types(
            &agent,
            &["request/header", "system/message", "user/message"]
        )
        .is_empty()
    );
    stack.close().await;
}

#[tokio::test]
async fn keeps_one_model_selection_tool_set_and_prompt_across_retries_without_admitting_users_again()
 {
    let adapter = MockAdapter::chunks(vec![
        vec![chunk(json!({"type": "finish", "reason": {
            "kind": "error",
            "failure": { "code": "SERVER", "message": "retry", "status": 503 },
        }}))],
        text_response("done"),
    ]);
    let (stack, agent) = harness(adapter.clone()).await;
    let retried = Arc::new(AtomicBool::new(false));
    let conditions = Arc::new(AtomicUsize::new(0));
    let admissions = Arc::new(AtomicUsize::new(0));
    let counted = admissions.clone();
    stack
        .ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |_, next| {
            counted.fetch_add(1, Ordering::SeqCst);
            next.run()
        })
        .expect("pre-step listener");
    select_model(&agent, "first");
    let (flag, retrying_agent) = (retried.clone(), agent.clone());
    stack
        .ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(move |_, next| {
            let (flag, agent) = (flag.clone(), retrying_agent.clone());
            async move {
                next.run().await?;
                flag.store(true, Ordering::SeqCst);
                select_model(&agent, "second");
                Ok(Some(RequestErrorAction::Retry))
            }
        })
        .expect("request-error listener");
    let (condition_count, flag) = (conditions.clone(), retried.clone());
    stack
        .tools
        .register(
            &stack.ctx,
            text_tool("conditional", "Retry-dependent availability.", "unused"),
            ToolRegistrationOptions::when_sync(move |_| {
                condition_count.fetch_add(1, Ordering::SeqCst);
                flag.load(Ordering::SeqCst)
            }),
        )
        .expect("conditional registers");
    let (tools, section_agent) = (stack.tools.clone(), agent.clone());
    stack
        .system_prompt
        .section(
            &stack.ctx,
            PromptSection {
                name: "retry:guidance".into(),
                order: 0.0,
                complete: true,
                text: PromptText::provider(move |context| {
                    let model = context
                        .extension::<AssembleModelSelection>()
                        .map(|selection| selection.0.model.clone())
                        .unwrap_or_default();
                    let schemas = tools.schemas(
                        Some(&section_agent.scope_key()),
                        context.extension::<ResolvedToolSet>(),
                    )?;
                    let names: Vec<_> = schemas.iter().map(|tool| tool.name.as_str()).collect();
                    Ok(format!("{model}:{}", names.join(",")))
                }),
                ..PromptSection::default()
            },
        )
        .expect("section registers");
    send(&agent, "retry once").await;
    let all = requests(&adapter);
    let models: Vec<_> = all.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(models, ["first", "first"]);
    let tools: Vec<_> = all
        .iter()
        .map(|r| r.tools.clone().unwrap_or_default())
        .collect();
    assert!(tools.iter().all(Vec::is_empty) && tools.len() == 2);
    let prompts: Vec<_> = all.iter().map(system_content).collect();
    let expected = json!([{ "type": "text", "text": "first:" }]);
    assert_eq!(prompts, [Some(expected.clone()), Some(expected)]);
    assert_eq!(
        (
            conditions.load(Ordering::SeqCst),
            admissions.load(Ordering::SeqCst)
        ),
        (1, 1)
    );
    let users = events_of(&agent, "user/message")
        .into_iter()
        .filter(|event| event["data"]["source"]["kind"] == "user")
        .count();
    assert_eq!(users, 1);
    stack.close().await;
}

#[tokio::test]
async fn reports_condition_failures_without_admitting_a_partial_request() {
    let adapter = MockAdapter::chunks(vec![text_response("unused")]);
    let (stack, agent) = harness(adapter.clone()).await;
    stack
        .tools
        .register(
            &stack.ctx,
            text_tool(
                "conditional",
                "An unavailable activation dependency.",
                "unused",
            ),
            ToolRegistrationOptions {
                when: Some(Arc::new(|_| {
                    Box::pin(async { Err("credential storage unavailable".into()) })
                })),
            },
        )
        .expect("conditional registers");
    send(&agent, "inspect").await;
    assert_eq!(adapter.request_count(), 0);
    assert!(
        events_of_types(
            &agent,
            &["request/header", "system/message", "user/message"]
        )
        .is_empty()
    );
    let end = events_of(&agent, "turn/end")
        .into_iter()
        .next()
        .expect("turn end");
    assert_match_object(
        &end["data"],
        &json!({ "reason": { "kind": "error", "error": { "code": "TOOL_ACTIVATION_FAILED" } } }),
    );
    stack.close().await;
}

#[tokio::test]
async fn closes_the_step_tool_set_after_a_rejected_proposal_and_after_its_step() {
    let adapter = MockAdapter::chunks(vec![text_response("done")]);
    let (stack, agent) = harness(adapter.clone()).await;
    stack
        .tools
        .register(
            &stack.ctx,
            text_tool("conditional", "Enabled.", "unused"),
            ToolRegistrationOptions::when_sync(|_| true),
        )
        .expect("conditional registers");
    let assembled = Arc::new(Mutex::new(Vec::<(ResolvedToolSet, usize)>::new()));
    let (captured, section_agent) = (assembled.clone(), agent.clone());
    stack
        .system_prompt
        .section(
            &stack.ctx,
            PromptSection {
                name: "capture:tool-set".into(),
                order: 0.0,
                text: PromptText::provider(move |context| {
                    if let Some(tool_set) = context.extension::<ResolvedToolSet>() {
                        let admitted = tool_set.view(Some(&section_agent.scope_key())).len();
                        lock(&captured).push((tool_set.clone(), admitted));
                    }
                    Ok(String::new())
                }),
                ..PromptSection::default()
            },
        )
        .expect("section registers");
    let reject = Arc::new(AtomicBool::new(true));
    let rejecting = reject.clone();
    stack
        .ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |_, next| {
            let reject = rejecting.clone();
            async move {
                let decision = next.run().await?;
                Ok(if reject.load(Ordering::SeqCst) {
                    PreStepDecision::Reject
                } else {
                    decision
                })
            }
        })
        .expect("pre-step listener");
    send(&agent, "rejected").await;
    reject.store(false, Ordering::SeqCst);
    send(&agent, "accepted").await;
    assert_eq!(adapter.request_count(), 1);
    {
        let assembled = lock(&assembled);
        let admitted: Vec<_> = assembled.iter().map(|(_, admitted)| *admitted).collect();
        assert_eq!(admitted, [1, 1]);
        let after: Vec<_> = assembled
            .iter()
            .map(|(set, _)| set.view(Some(&agent.scope_key())).len())
            .collect();
        assert_eq!(after, [0, 0]);
    }
    stack.close().await;
}

#[tokio::test]
async fn shares_the_selected_set_with_prompt_guidance() {
    let adapter = MockAdapter::chunks(vec![text_response("done")]);
    let (stack, agent) = harness(adapter.clone()).await;
    stack
        .tools
        .register(
            &stack.ctx,
            text_tool("conditional", "Disabled.", "unused"),
            ToolRegistrationOptions::when_sync(|_| false),
        )
        .expect("conditional registers");
    let tools = stack.tools.clone();
    stack
        .system_prompt
        .section(
            &stack.ctx,
            PromptSection {
                name: "conditional:guidance".into(),
                order: 0.0,
                text: PromptText::provider(move |context| {
                    Ok(
                        if tools
                            .get(
                                "conditional",
                                context.scope.as_ref(),
                                context.extension::<ResolvedToolSet>(),
                            )
                            .is_none()
                        {
                            String::new()
                        } else {
                            "Conditional guidance".to_owned()
                        },
                    )
                }),
                ..PromptSection::default()
            },
        )
        .expect("section registers");
    send(&agent, "inspect").await;
    let all = requests(&adapter);
    assert!(all[0].tools.clone().unwrap_or_default().is_empty());
    let messages = serde_json::to_string(&all[0].messages).expect("messages");
    assert!(!messages.contains("Conditional guidance"));
    stack.close().await;
}
