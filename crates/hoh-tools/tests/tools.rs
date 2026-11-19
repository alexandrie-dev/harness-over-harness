//! Port of the runtime, pipeline and DSL cases of tests/tools.spec.ts. Cases that need the agent loop,
//! JavaScript-only values (Proxy, getters, symbols, non-cloneable objects),
//! TypeScript types or later mutation of a registered definition are listed in PORT_NOTES.md.

mod common;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use common::*;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Kernel, Next};
use hoh_llm::{ContentBlock, MessageSource};
use hoh_timeout::AbortController;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, PostToolDecision, PreToolDecision, ToolArgsError,
    ToolContextUnavailableError, ToolErrorInfo, ToolExecution, ToolExecutionResult,
    ToolNotFoundError, ToolOutputError, ToolRegistrationOptions, ToolRuntime, ToolsExecute,
    ToolsPostExecute, ToolsPreExecute, ToolsResult, define_tool, validate_args,
};
use hoh_user_approval::{ApprovalOutcome, ApprovalRequestEvent, DisplayReason};
use serde_json::{Value, json};

fn error_text(result: &ToolExecutionResult) -> String {
    result.text()
}

fn info(result: &ToolExecutionResult) -> Option<(String, String)> {
    result
        .error()
        .and_then(|error| error.info.as_ref())
        .map(|info| (info.name.clone(), info.code.clone()))
}

fn named(name: &str, code: &str) -> Option<(String, String)> {
    Some((name.to_owned(), code.to_owned()))
}

fn string_tool(
    name: &str,
    body: impl Fn() -> Result<Value, BoxError> + Send + Sync + 'static,
) -> hoh_tools::ToolDefinition {
    define_tool(DefineToolOptions::new(
        name,
        "",
        json!({}),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, value| vec![text(value.as_str().unwrap_or(""))]),
            presentation_meta: None,
        },
        move |_, _| {
            let result = body();
            Box::pin(async move { result })
        },
    ))
    .expect("tool defines")
}

fn user_message(text: &str) -> hoh_llm::UserMessage {
    hoh_llm::create_user_message(hoh_llm::NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("test"),
    })
}

// ---------------------------------------------------------------- ToolRuntime

#[tokio::test]
async fn rejects_removed_execution_configuration_during_plugin_mounting() {
    for config in [
        json!({ "mode": "ptc" }),
        json!({ "maxParallelSubCalls": 4 }),
    ] {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<hoh_system_prompt::SystemPrompt>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let fiber = ctx.plugin::<ToolRuntime>(config.clone());
        let settled = match fiber {
            Ok(fiber) => fiber.settled().await,
            Err(error) => Err(error),
        };
        assert!(settled.is_err(), "{config}");
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn settles_accepted_calls_after_the_actual_native_executor_fiber_unloads() {
    let env = setup_without_executor().await;
    let provider = env.ctx.plugin::<NativeExecutorPlugin>(json!({})).unwrap();
    provider.settled().await.unwrap();
    register(&env, echo_tool());
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    {
        let (entered, release) = (entered.clone(), release.clone());
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| {
                let (entered, release) = (entered.clone(), release.clone());
                async move {
                    if event.exec.call_id.as_str() == "first" {
                        entered.notify_one();
                        release.notified().await;
                    }
                    next.run().await
                }
            })
            .unwrap();
    }
    let batch = || {
        hoh_tools::ToolBatchInput::new(
            vec![
                model_call("first", "echo", r#"{"text":"first"}"#),
                model_call("second", "echo", r#"{"text":"second"}"#),
            ],
            live_signal(),
        )
    };
    let tools = env.tools.clone();
    let pending = tokio::spawn({
        let tools = tools.clone();
        let input = batch();
        async move { tools.execute(input).await }
    });
    entered.notified().await;
    provider.dispose().await;
    let rejected = tools.execute(batch()).await.expect_err("executor is gone");
    assert!(
        rejected
            .to_string()
            .contains("selected tool executor is unavailable")
    );
    release.notify_one();
    let results = pending.await.unwrap().unwrap();
    let values: Vec<_> = results
        .results
        .iter()
        .map(|outcome| (outcome.result.is_error(), outcome.result.value().cloned()))
        .collect();
    assert_eq!(
        values,
        vec![
            (false, Some(json!("first"))),
            (false, Some(json!("second")))
        ]
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn preserves_deferred_loading_through_registry_and_prompt_schema_projection() {
    let env = setup().await;
    register(
        &env,
        content_tool_with(
            "deferred",
            json!({}),
            |_, _| async { Ok(vec![]) },
            |options| {
                options.defer_loading = true;
            },
        ),
    );
    assert_eq!(
        env.tools.schemas(None, None).unwrap()[0].defer_loading,
        Some(true)
    );
    let assembly = env
        .prompt
        .assemble(hoh_system_prompt::AssembleContext::default())
        .await
        .unwrap();
    assert_eq!(assembly.tools[0].defer_loading, Some(true));
}

#[tokio::test]
async fn registers_tools_exposes_schemas_and_feeds_the_system_prompt_assembly() {
    let env = setup().await;
    register(&env, echo_tool());
    let schemas = env.tools.schemas(None, None).unwrap();
    assert_eq!(schemas.len(), 1);
    assert_eq!(
        serde_json::to_value(&schemas[0]).unwrap(),
        json!({
            "name": "echo",
            "description": "echo arguments back",
            "parameters": { "type": "object", "properties": { "text": { "type": "string" } } },
        })
    );
    let assembly = env
        .prompt
        .assemble(hoh_system_prompt::AssembleContext::default())
        .await
        .unwrap();
    let names: Vec<_> = assembly
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(names, vec!["echo"]);
}

#[tokio::test]
async fn schemas_drops_host_callbacks_and_the_timeout_budget() {
    let env = setup().await;
    register(
        &env,
        content_tool_with(
            "present",
            json!({ "x": { "type": "string", "required": true } }),
            |_, _| async { Ok(vec![]) },
            |options| {
                options.timeout_ms = Some(5_000.0);
                options.finalize_content =
                    Some(Arc::new(|_, result| Some(result.content().to_vec())));
                options.present_call = Some(Arc::new(|args| {
                    Some(hoh_tools::ToolCallView::Generic(
                        hoh_tools::GenericCallView {
                            title: args["x"].as_str().unwrap().into(),
                            kind: None,
                            raw_input: None,
                            content: None,
                            locations: None,
                        },
                    ))
                }));
            },
        ),
    );
    let schema = serde_json::to_value(&env.tools.schemas(None, None).unwrap()[0]).unwrap();
    let mut keys: Vec<_> = schema.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, vec!["description", "name", "parameters"]);
}

#[tokio::test]
async fn runs_the_captured_content_projector_before_policy_and_keeps_replacement_authoritative() {
    let env = setup().await;
    let mut tool = string_tool("projected", || Ok(json!("canonical")));
    tool.project_content = Some(Arc::new(|_, _| Some(vec![text("prepared")])));
    register(&env, tool);
    let seen = Arc::new(Mutex::new(Vec::new()));
    {
        let seen = seen.clone();
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, next| {
                let seen = seen.clone();
                async move {
                    next.run().await?;
                    lock(&seen).push(event.result.text());
                    Ok(PostToolDecision::Accept {
                        content: Some(vec![text("policy result")]),
                        value: None,
                        additional_contexts: None,
                    })
                }
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("projection", "projected", json!({}))).await;
    assert_eq!(*lock(&seen), vec!["prepared".to_owned()]);
    assert_eq!(result.content(), &[text("policy result")]);
    assert_eq!(result.value(), Some(&json!("canonical")));
}

#[tokio::test]
async fn normalizes_a_throwing_content_projector_without_running_post_execute() {
    let env = setup().await;
    let mut tool = echo_tool();
    tool.project_content = Some(Arc::new(|_, _| panic!("projection failed")));
    register(&env, tool);
    let post_calls = Arc::new(AtomicUsize::new(0));
    {
        let post_calls = post_calls.clone();
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |_, next| {
                post_calls.fetch_add(1, Ordering::SeqCst);
                async move { next.run().await }
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("projection-error", "echo", json!({}))).await;
    assert!(result.is_error());
    assert!(error_text(&result).contains("projection failed"));
    assert_eq!(post_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn executes_a_tool_and_returns_its_content() {
    let env = setup().await;
    register(&env, echo_tool());
    let observed = Arc::new(Mutex::new(None));
    {
        let observed = observed.clone();
        env.ctx
            .on_contained::<ToolsResult>(move |event| {
                *lock(&observed) = Some(event.result.clone());
                Ok(())
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({ "text": "hi" }))).await;
    assert_eq!(result.content(), &[text("hi")]);
    assert_eq!(result.value(), Some(&json!("hi")));
    assert!(!result.is_error() && result.meta().is_none());
    assert_eq!(**lock(&observed).as_ref().expect("observed"), result);
}

#[tokio::test]
async fn projects_presentation_metadata_and_omits_meta_when_not_declared() {
    let env = setup().await;
    let mut with_meta = string_tool("meta-tool", || Ok(json!("ok")));
    with_meta.output.presentation_meta = Some(Arc::new(|_, _| {
        Ok(json!({ "diffs": [{ "path": "a", "oldText": null, "newText": "x" }] }))
    }));
    register(&env, with_meta);
    register(&env, string_tool("no-meta-tool", || Ok(json!("ok"))));
    let result = execute_tool_fixture(&env.ctx, input("c1", "meta-tool", json!({}))).await;
    assert_eq!(
        result.meta(),
        Some(&json!({ "diffs": [{ "path": "a", "oldText": null, "newText": "x" }] }))
    );
    assert_eq!(result.value(), Some(&json!("ok")));
    let plain = execute_tool_fixture(&env.ctx, input("c1", "no-meta-tool", json!({}))).await;
    assert!(plain.meta().is_none());
}

#[tokio::test]
async fn normalizes_a_contract_violating_result_before_final_notification() {
    let env = setup().await;
    let observed_error = Arc::new(Mutex::new(None));
    {
        let observed_error = observed_error.clone();
        env.ctx
            .on_contained::<ToolsResult>(move |event| {
                *lock(&observed_error) = Some(event.result.is_error());
                Ok(())
            })
            .unwrap();
    }
    let mut tool = string_tool("bad-meta", || Ok(json!("ok")));
    // Negative zero is the lossy JSON value a `Value` can carry.
    tool.output.presentation_meta = Some(Arc::new(|_, _| Ok(json!(-0.0))));
    register(&env, tool);
    let result = execute_tool_fixture(&env.ctx, input("bad-meta", "bad-meta", json!({}))).await;
    assert!(result.is_error());
    assert!(error_text(&result).starts_with("Error:"));
    assert_eq!(
        info(&result),
        named("ToolOutputError", "INVALID_TOOL_OUTPUT")
    );
    assert_eq!(*lock(&observed_error), Some(true));
}

#[tokio::test]
async fn finalizes_errors_discovered_while_snapshotting_non_content_result_fields() {
    let env = setup().await;
    let finalize_calls = Arc::new(AtomicUsize::new(0));
    let mut tool = string_tool("throwing-meta", || Ok(json!("body")));
    tool.output.presentation_meta = Some(Arc::new(|_, _| Ok(json!(-0.0))));
    {
        let finalize_calls = finalize_calls.clone();
        tool.finalize_content = Some(Arc::new(move |_, result| {
            finalize_calls.fetch_add(1, Ordering::SeqCst);
            let block = result.content().first()?;
            let truncated: String = block.text_value()?.chars().take(32).collect();
            Some(vec![text(&truncated)])
        }));
    }
    register(&env, tool);
    let result =
        execute_tool_fixture(&env.ctx, input("throwing-meta", "throwing-meta", json!({}))).await;
    assert!(result.is_error());
    let body = error_text(&result);
    assert!(body.starts_with("Error: tool \"throwing-meta\""));
    assert_eq!(body.encode_utf16().count(), 32);
    assert_eq!(finalize_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn normalizes_a_throwing_final_content_callback_without_invoking_it_again() {
    let env = setup().await;
    let finalize_calls = Arc::new(AtomicUsize::new(0));
    let mut tool = echo_tool();
    {
        let finalize_calls = finalize_calls.clone();
        tool.finalize_content = Some(Arc::new(move |_, _| {
            finalize_calls.fetch_add(1, Ordering::SeqCst);
            panic!("finalizer violated its total contract")
        }));
    }
    register(&env, tool);
    let result =
        execute_tool_fixture(&env.ctx, input("throwing-finalizer", "echo", json!({}))).await;
    assert_eq!(
        result.content(),
        &[text("Error: finalizer violated its total contract")]
    );
    assert!(result.is_error());
    let error = result.error().unwrap();
    assert_eq!(error.message, "finalizer violated its total contract");
    assert!(error.info.is_none());
    assert_eq!(finalize_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_lossy_and_schema_mismatched_body_values_before_post_execute() {
    let env = setup().await;
    let json_tool = define_tool(DefineToolOptions::new(
        "lossy-output",
        "lossy",
        json!({}),
        DefineToolOutput {
            schema: json!({ "type": "json" }),
            render: Arc::new(|_, _| vec![]),
            presentation_meta: None,
        },
        |_, _| Box::pin(async { Ok(json!(-0.0)) }),
    ))
    .unwrap();
    register(&env, json_tool);
    let mismatch = define_tool(DefineToolOptions::new(
        "wrong-output",
        "wrong schema",
        json!({}),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, _| vec![]),
            presentation_meta: None,
        },
        |_, _| Box::pin(async { Ok(json!(42)) }),
    ))
    .unwrap();
    register(&env, mismatch);
    let lossy = execute_tool_fixture(&env.ctx, input("lossy", "lossy-output", json!({}))).await;
    let wrong = execute_tool_fixture(&env.ctx, input("mismatch", "wrong-output", json!({}))).await;
    assert_eq!(
        info(&lossy),
        named("ToolOutputError", "INVALID_TOOL_OUTPUT")
    );
    assert!(error_text(&lossy).contains("not lossless JSON"));
    assert_eq!(
        info(&wrong),
        named("ToolOutputError", "INVALID_TOOL_OUTPUT")
    );
    assert!(error_text(&wrong).contains("\"value\" must be a string"));
}

#[tokio::test]
async fn contains_a_throwing_output_projector_as_one_failed_call() {
    for projector in ["render", "presentationMeta"] {
        let env = setup().await;
        let mut tool = string_tool(&format!("throwing-{projector}"), || Ok(json!("ok")));
        if projector == "render" {
            tool.output.render = Arc::new(|_, _| panic!("renderer exploded"));
        } else {
            tool.output.presentation_meta = Some(Arc::new(|_, _| panic!("metadata exploded")));
        }
        register(&env, tool);
        let result = execute_tool_fixture(
            &env.ctx,
            input(projector, &format!("throwing-{projector}"), json!({})),
        )
        .await;
        assert!(result.is_error());
        let message = &result.error().unwrap().message;
        let expected = if projector == "render" {
            "renderer exploded"
        } else {
            "metadata exploded"
        };
        assert!(message.contains(expected), "{message}");
        assert_eq!(
            info(&result),
            named("ToolOutputError", "INVALID_TOOL_OUTPUT")
        );
        assert!(result.value().is_none());
    }
}

#[tokio::test]
async fn contains_a_non_lossless_output_projection_as_one_failed_call() {
    for projector in ["render", "presentationMeta"] {
        let env = setup().await;
        let mut tool = string_tool(&format!("lossy-{projector}"), || Ok(json!("ok")));
        if projector == "render" {
            tool.output.render = Arc::new(|_, _| {
                let mut block = ContentBlock::text("x");
                block.data.insert("bad".into(), json!(-0.0));
                Ok(vec![block])
            });
        } else {
            tool.output.presentation_meta = Some(Arc::new(|_, _| Ok(json!({ "bad": -0.0 }))));
        }
        register(&env, tool);
        let result = execute_tool_fixture(
            &env.ctx,
            input(projector, &format!("lossy-{projector}"), json!({})),
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .error()
                .unwrap()
                .message
                .contains("non-lossless JSON")
        );
        assert_eq!(
            info(&result),
            named("ToolOutputError", "INVALID_TOOL_OUTPUT")
        );
    }
}

#[tokio::test]
async fn keeps_value_and_meta_through_content_replacement_and_recomputes_both_after_value_replacement()
 {
    let env = setup().await;
    let tool = define_tool(DefineToolOptions {
        output: DefineToolOutput {
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "text": { "type": "string", "required": true } },
            }),
            render: Arc::new(|_, value| {
                vec![text(&format!("render:{}", value["text"].as_str().unwrap()))]
            }),
            presentation_meta: Some(Arc::new(|_, value| json!({ "projected": value["text"] }))),
        },
        ..DefineToolOptions::new(
            "projected",
            "projected",
            json!({}),
            DefineToolOutput {
                schema: json!({ "type": "string" }),
                render: Arc::new(|_, _| vec![]),
                presentation_meta: None,
            },
            |_, _| Box::pin(async { Ok(json!({ "text": "body" })) }),
        )
    })
    .unwrap();
    register(&env, tool);
    let replacement = Arc::new(Mutex::new("content"));
    {
        let replacement = replacement.clone();
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |_, _next| {
                let mode = *lock(&replacement);
                async move {
                    Ok(if mode == "content" {
                        PostToolDecision::Accept {
                            content: Some(vec![text("policy content")]),
                            value: None,
                            additional_contexts: None,
                        }
                    } else {
                        PostToolDecision::Accept {
                            content: None,
                            value: Some(json!({ "text": "policy value" })),
                            additional_contexts: Some(vec![user_message("value context")]),
                        }
                    })
                }
            })
            .unwrap();
    }
    let content = execute_tool_fixture(&env.ctx, input("content", "projected", json!({}))).await;
    *lock(&replacement) = "value";
    let value = execute_tool_fixture(&env.ctx, input("value", "projected", json!({}))).await;
    assert_eq!(content.value(), Some(&json!({ "text": "body" })));
    assert_eq!(content.content(), &[text("policy content")]);
    assert_eq!(content.meta(), Some(&json!({ "projected": "body" })));
    assert_eq!(value.value(), Some(&json!({ "text": "policy value" })));
    assert_eq!(value.content(), &[text("render:policy value")]);
    assert_eq!(value.meta(), Some(&json!({ "projected": "policy value" })));
    let contexts = value
        .additional_contexts()
        .expect("value context rides along");
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].content, vec![text("value context")]);
}

#[tokio::test]
async fn fails_a_post_execute_decision_that_replaces_both_projections_or_supplies_an_invalid_value()
{
    let both = setup().await;
    register(&both, echo_tool());
    both.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Ok(PostToolDecision::Accept {
                content: Some(vec![text("also replacement")]),
                value: Some(json!("replacement")),
                additional_contexts: None,
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&both.ctx, input("both", "echo", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(
        result.error().unwrap().message,
        "tools/post-execute accept decision cannot replace both value and content"
    );

    let invalid = setup().await;
    register(&invalid, echo_tool());
    invalid
        .ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Ok(PostToolDecision::Accept {
                content: None,
                value: Some(json!(1)),
                additional_contexts: None,
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&invalid.ctx, input("invalid", "echo", json!({}))).await;
    assert_eq!(
        result.error().unwrap().info.as_ref().unwrap().code,
        "INVALID_TOOL_OUTPUT"
    );
    assert!(result.value().is_none());
}

#[tokio::test]
async fn turns_a_post_execute_block_into_a_valueless_failure_with_its_identity() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Ok(PostToolDecision::Block {
                feedback: vec![text("blocked by policy")],
                info: Some(ToolErrorInfo::new("HookBlockedError", "HOOK_BLOCKED")),
                additional_contexts: None,
            })
        })
        .unwrap();
    let result = execute_tool_fixture(
        &env.ctx,
        input("block", "echo", json!({ "text": "secret" })),
    )
    .await;
    assert!(result.is_error() && result.value().is_none());
    assert_eq!(result.content(), &[text("blocked by policy")]);
    let error = result.error().unwrap();
    assert_eq!(error.message, "blocked by policy");
    assert_eq!(info(&result), named("HookBlockedError", "HOOK_BLOCKED"));
}

#[tokio::test]
async fn replaces_a_canonical_value_without_manufacturing_additional_context() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Ok(PostToolDecision::Accept {
                content: None,
                value: Some(json!("replacement")),
                additional_contexts: None,
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("replace-value", "echo", json!({}))).await;
    assert!(!result.is_error());
    assert_eq!(result.value(), Some(&json!("replacement")));
    assert_eq!(result.content(), &[text("replacement")]);
    assert!(result.additional_contexts().is_none() && result.meta().is_none());
}

#[tokio::test]
async fn derives_a_stable_failure_message_from_non_text_or_empty_block_feedback() {
    let cases = [
        (vec![], "tool result blocked by post-execute policy"),
        (
            vec![ContentBlock::reasoning("private rationale")],
            "[reasoning content]",
        ),
    ];
    for (feedback, message) in cases {
        let env = setup().await;
        register(&env, echo_tool());
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |_, _| {
                let feedback = feedback.clone();
                async move {
                    Ok(PostToolDecision::Block {
                        feedback,
                        info: None,
                        additional_contexts: None,
                    })
                }
            })
            .unwrap();
        let result =
            execute_tool_fixture(&env.ctx, input("block-message", "echo", json!({}))).await;
        assert_eq!(result.error().unwrap().message, message);
    }
}

#[tokio::test]
async fn contains_a_non_json_post_execute_failure_projection_as_a_safe_final_error() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            let mut block = ContentBlock::text("blocked");
            block.data.insert("invalid".into(), json!(-0.0));
            Ok(PostToolDecision::Block {
                feedback: vec![block],
                info: None,
                additional_contexts: None,
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("invalid-block", "echo", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(
        result.error().unwrap().message,
        "tool result must be losslessly JSON-serializable"
    );
}

#[tokio::test]
async fn rejects_value_replacement_on_a_failed_dispatch() {
    let env = setup().await;
    register(
        &env,
        string_tool("throw-before-replace", || Err("body failed".into())),
    );
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Ok(PostToolDecision::Accept {
                content: None,
                value: Some(json!("replacement")),
                additional_contexts: None,
            })
        })
        .unwrap();
    let result = execute_tool_fixture(
        &env.ctx,
        input("failed-replace", "throw-before-replace", json!({})),
    )
    .await;
    assert_eq!(
        result.error().unwrap().message,
        "tools/post-execute cannot replace the value of a failed result"
    );
}

#[tokio::test]
async fn fails_value_replacement_when_the_owning_tool_disappears_before_post_policy_resolves() {
    let env = setup().await;
    let dispose = Arc::new(Mutex::new(Some(register(&env, echo_tool()))));
    {
        let dispose = dispose.clone();
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |_, _| {
                let disposer = lock(&dispose).take();
                async move {
                    if let Some(disposer) = disposer {
                        disposer.dispose().await;
                    }
                    Ok(PostToolDecision::Accept {
                        content: None,
                        value: Some(json!("replacement")),
                        additional_contexts: None,
                    })
                }
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("post-disposed", "echo", json!({}))).await;
    assert_eq!(result.error().unwrap().message, "unknown tool \"echo\"");
    assert_eq!(info(&result), named("ToolNotFoundError", "UNKNOWN_TOOL"));
}

#[tokio::test]
async fn normalizes_wrapper_authored_failure_metadata_and_contexts() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, _| async {
            let mut failure = hoh_tools::ToolExecutionFailure::new(
                hoh_tools::ToolFailure {
                    message: "wrapped failure".into(),
                    info: None,
                },
                vec![text("wrapper content")],
            );
            failure.set_meta(Some(json!({ "wrapped": true })));
            failure.set_additional_contexts(Some(vec![user_message("wrapper context")]));
            Ok(ToolExecutionResult::Failure(failure))
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("wrapper-failure", "echo", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(result.error().unwrap().message, "wrapped failure");
    assert_eq!(result.content(), &[text("wrapper content")]);
    assert_eq!(result.meta(), Some(&json!({ "wrapped": true })));
    assert_eq!(
        result.additional_contexts().unwrap()[0].content,
        vec![text("wrapper context")]
    );
}

#[tokio::test]
async fn fails_wrapper_authored_success_normalization_when_the_owning_tool_disappears() {
    let env = setup().await;
    let dispose = Arc::new(Mutex::new(Some(register(&env, echo_tool()))));
    {
        let dispose = dispose.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |_, _| {
                let disposer = lock(&dispose).take();
                async move {
                    if let Some(disposer) = disposer {
                        disposer.dispose().await;
                    }
                    Ok(ToolExecutionResult::Success(
                        hoh_tools::ToolExecutionSuccess::new(json!("replacement"), vec![]),
                    ))
                }
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("wrapper-disposed", "echo", json!({}))).await;
    assert_eq!(result.error().unwrap().message, "unknown tool \"echo\"");
    assert_eq!(info(&result), named("ToolNotFoundError", "UNKNOWN_TOOL"));
}

#[tokio::test]
async fn projects_presentation_metadata_for_native_and_nested_composite_dispatches() {
    let env = setup().await;
    let mut tool = echo_tool();
    tool.name = "meta-suppression".into();
    tool.output.presentation_meta = Some(Arc::new(|_, _| Ok(json!({ "card": true }))));
    register(&env, tool);
    let captured = Arc::new(Mutex::new(None));
    let capture = {
        let captured = captured.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| {
                *lock(&captured) = Some(event.exec.token);
                async move { next.run().await }
            })
            .unwrap()
    };
    let direct =
        execute_tool_fixture(&env.ctx, input("direct", "meta-suppression", json!({}))).await;
    capture.dispose().await;
    let mut nested_input = input("nested", "meta-suppression", json!({}));
    nested_input.parent = *lock(&captured);
    let nested = execute_tool_fixture(&env.ctx, nested_input).await;
    assert_eq!(direct.meta(), Some(&json!({ "card": true })));
    assert_eq!(nested.meta(), Some(&json!({ "card": true })));
    assert_eq!(nested.value(), Some(&json!("")));
}

#[tokio::test]
async fn carries_a_nested_conclusion_on_the_nested_result_for_its_composite_to_forward() {
    let env = setup().await;
    let mut terminal = echo_tool();
    terminal.name = "terminal-nested".into();
    terminal.execute = Arc::new(|_, exec: Arc<ToolExecution>| {
        exec.conclude_turn();
        Box::pin(async { Ok(json!("terminal")) })
    });
    register(&env, terminal);
    let mut composite = echo_tool();
    composite.name = "composite".into();
    let call = Arc::new(AtomicUsize::new(0));
    {
        let ctx = env.ctx.clone();
        composite.execute = Arc::new(move |_, exec: Arc<ToolExecution>| {
            let n = call.fetch_add(1, Ordering::SeqCst) + 1;
            let ctx = ctx.clone();
            Box::pin(async move {
                let mut nested_input = input(&format!("nested-{n}"), "terminal-nested", json!({}));
                nested_input.signal = exec.signal();
                nested_input.parent = Some(exec.token);
                let nested = execute_tool_fixture(&ctx, nested_input).await;
                if nested.concludes_turn() {
                    exec.conclude_turn();
                }
                Ok(json!(if nested.is_error() {
                    "nested failed, composite recovered"
                } else {
                    "nested succeeded"
                }))
            })
        });
    }
    register(&env, composite);
    // A policy converts the nested success into an error: the failed result carries no marker.
    let veto = env
        .ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(
            |event, next: Next<PostToolDecision>| async move {
                if event.exec.name != "terminal-nested" {
                    return next.run().await;
                }
                Ok(PostToolDecision::Block {
                    feedback: vec![text("nested success rejected")],
                    info: None,
                    additional_contexts: None,
                })
            },
        )
        .unwrap();
    let recovered =
        execute_tool_fixture(&env.ctx, input("composite-vetoed", "composite", json!({}))).await;
    assert!(!recovered.is_error());
    assert!(!recovered.concludes_turn());
    veto.dispose().await;
    let concluded =
        execute_tool_fixture(&env.ctx, input("composite-ok", "composite", json!({}))).await;
    assert!(!concluded.is_error());
    assert!(concluded.concludes_turn());
}

#[tokio::test]
async fn returns_is_error_results_for_unknown_tools_and_throwing_tools() {
    let env = setup().await;
    register(&env, string_tool("boom", || Err("exploded".into())));
    let unknown = execute_tool_fixture(&env.ctx, input("c1", "nope", json!({}))).await;
    assert!(unknown.is_error());
    assert_eq!(
        unknown.content()[0].text_value(),
        Some("Error: unknown tool \"nope\"")
    );
    assert_eq!(unknown.error().unwrap().message, "unknown tool \"nope\"");
    assert_eq!(info(&unknown), named("ToolNotFoundError", "UNKNOWN_TOOL"));
    let thrown = execute_tool_fixture(&env.ctx, input("c2", "boom", json!({}))).await;
    assert!(thrown.is_error());
    assert_eq!(thrown.content()[0].text_value(), Some("Error: exploded"));
}

#[tokio::test]
async fn records_tool_context_unavailable_error_thrown_by_a_tool_body() {
    let env = setup().await;
    register(
        &env,
        string_tool("needs-agent", || {
            Err(Box::new(ToolContextUnavailableError::new(
                "needs-agent requires a calling agent",
            )))
        }),
    );
    let result =
        execute_tool_fixture(&env.ctx, input("needs-agent", "needs-agent", json!({}))).await;
    assert_eq!(
        result.content(),
        &[text("Error: needs-agent requires a calling agent")]
    );
    assert_eq!(
        info(&result),
        named("ToolContextUnavailableError", "TOOL_CONTEXT_UNAVAILABLE")
    );
}

#[test]
fn tool_not_found_error_carries_a_stable_message_and_code() {
    let error: HarnessError = ToolNotFoundError::new("ghost", None);
    assert_eq!(error.name(), "ToolNotFoundError");
    assert_eq!(error.code(), "UNKNOWN_TOOL");
    assert_eq!(error.message(), "unknown tool \"ghost\"");
    assert_eq!(
        ToolNotFoundError::new("ghost", Some("use run_code")).message(),
        "unknown tool \"ghost\": use run_code"
    );
}

// ------------------------------------------------------- pre-execute and approval

#[tokio::test]
async fn lets_a_pre_execute_listener_deny_a_call_permission_pattern() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|event, next| async move {
            if event.exec.name == "echo" {
                return Ok(PreToolDecision::Deny {
                    reason: "denied by policy".into(),
                    info: Some(ToolErrorInfo {
                        name: "AutoReviewDeniedError".into(),
                        code: "AUTO_REVIEW_DENIED".into(),
                        reason: Some(" raw\nreason ".into()),
                    }),
                });
            }
            next.run().await
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({ "text": "hi" }))).await;
    assert!(result.is_error());
    assert_eq!(
        result.content()[0].text_value(),
        Some("Error: denied by policy")
    );
    let error = result.error().unwrap();
    assert_eq!(error.message, "denied by policy");
    assert_eq!(
        error.info,
        Some(ToolErrorInfo {
            name: "AutoReviewDeniedError".into(),
            code: "AUTO_REVIEW_DENIED".into(),
            reason: Some(" raw\nreason ".into()),
        })
    );
}

#[tokio::test]
async fn an_ask_decision_degrades_to_deny_when_no_approval_seam_is_mounted() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|_, _| async {
            Ok(PreToolDecision::Ask {
                reason: Some("needs approval".into()),
                display_reason: None,
                details: None,
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({ "text": "hi" }))).await;
    assert!(result.is_error());
    assert_eq!(
        result.content()[0].text_value(),
        Some("Error: needs approval")
    );
}

#[tokio::test]
async fn an_ask_decision_with_no_reason_degrades_to_deny_with_a_default_message() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|_, _| async {
            Ok(PreToolDecision::Ask {
                reason: None,
                display_reason: None,
                details: None,
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({ "text": "hi" }))).await;
    assert!(result.is_error());
    assert_eq!(
        result.content()[0].text_value(),
        Some("Error: tool \"echo\" requires approval (not yet supported)")
    );
    assert_eq!(
        info(&result),
        named("ApprovalError", "APPROVAL_UNAVAILABLE")
    );
}

async fn ask_env(outcome: Option<ApprovalOutcome>) -> (Env, Arc<Answerer>) {
    let env = setup().await;
    register(&env, echo_tool());
    let answerer = Answerer::new(outcome.unwrap_or(ApprovalOutcome::Unavailable));
    if outcome.is_some() {
        mount_approval(&env, answerer.clone()).await;
    }
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|_, _| async {
            let mut display_reason = DisplayReason::new("Allow it?");
            display_reason.insert("zh", "允许吗？");
            Ok(PreToolDecision::Ask {
                reason: Some("hook wants a human".into()),
                display_reason: Some(display_reason),
                details: None,
            })
        })
        .unwrap();
    (env, answerer)
}

/// `details` of an `Ask` reaches the approval request as `ApprovalRequest.details` (the answerers'
/// `reason` is untouched), and the `Ask` ends the waterfall: a hook behind it never runs.
#[tokio::test]
async fn ask_details_reach_the_approval_request_and_the_ask_ends_the_waterfall() {
    let env = setup().await;
    register(&env, echo_tool());
    let answerer = Answerer::new(ApprovalOutcome::AllowedOnce);
    mount_approval(&env, answerer.clone()).await;
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|_, _| async {
            Ok(PreToolDecision::Ask {
                reason: Some("what will happen".into()),
                display_reason: Some(DisplayReason::new("Allow it?")),
                details: Some("Команда: rm x".into()),
            })
        })
        .unwrap();
    let behind = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = behind.clone();
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |_, next| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move { next.run().await }
        })
        .unwrap();
    let mut request = input("c1", "echo", json!({ "text": "hi" }));
    request.agent = Some(TestAgent::open(&env.ctx));
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert!(!result.is_error());
    assert_eq!(
        *lock(&answerer.seen_details),
        vec![Some("Команда: rm x".to_owned())]
    );
    assert_eq!(
        lock(&answerer.seen)[0].2.as_deref(),
        Some("what will happen")
    );
    assert_eq!(
        behind.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a hook behind the Ask must not run"
    );
}

#[tokio::test]
async fn ask_routing_dispatches_when_the_answerer_grants_allowed_once() {
    let (env, answerer) = ask_env(Some(ApprovalOutcome::AllowedOnce)).await;
    let mut request = input("c1", "echo", json!({ "text": "hi" }));
    request.agent = Some(TestAgent::open(&env.ctx));
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert!(!result.is_error());
    assert_eq!(result.content(), &[text("hi")]);
    assert_eq!(
        *lock(&answerer.seen),
        vec![(
            "echo".to_owned(),
            call_id("c1"),
            Some("hook wants a human".to_owned()),
            Some({
                let mut reason = DisplayReason::new("Allow it?");
                reason.insert("zh", "允许吗？");
                reason
            })
        )]
    );
}

#[tokio::test]
async fn ask_routing_denies_with_the_user_rejection_reason_on_rejected() {
    let (env, _) = ask_env(Some(ApprovalOutcome::Rejected)).await;
    let mut request = input("c1", "echo", json!({}));
    request.agent = Some(TestAgent::open(&env.ctx));
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert_eq!(
        result.content()[0].text_value(),
        Some("Error: the user rejected tool \"echo\"")
    );
    assert_eq!(info(&result), named("ApprovalError", "APPROVAL_REJECTED"));
}

#[tokio::test]
async fn ask_routing_denies_with_the_cancellation_reason_on_cancelled() {
    let (env, _) = ask_env(Some(ApprovalOutcome::Cancelled)).await;
    let mut request = input("c1", "echo", json!({}));
    request.agent = Some(TestAgent::open(&env.ctx));
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert_eq!(
        result.content()[0].text_value(),
        Some("Error: approval for tool \"echo\" was cancelled")
    );
    assert_eq!(info(&result), named("ApprovalError", "APPROVAL_CANCELLED"));
}

#[tokio::test]
async fn ask_routing_denies_with_the_no_channel_reason_when_nobody_answers() {
    let (env, _) = ask_env(Some(ApprovalOutcome::Unavailable)).await;
    let mut request = input("c1", "echo", json!({}));
    request.agent = Some(TestAgent::open(&env.ctx));
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert_eq!(
        result.content()[0].text_value(),
        Some("Error: tool \"echo\" requires approval, but no approval channel is available")
    );
    assert_eq!(
        info(&result),
        named("ApprovalError", "APPROVAL_UNAVAILABLE")
    );
}

#[tokio::test]
async fn ask_routing_denies_an_agent_less_execution_without_asking() {
    let (env, answerer) = ask_env(Some(ApprovalOutcome::AllowedOnce)).await;
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({}))).await;
    assert!(lock(&answerer.seen).is_empty());
    assert_eq!(
        result.content()[0].text_value(),
        Some(
            "Error: tool \"echo\" requires approval, but the call has no agent to route it through"
        )
    );
    assert_eq!(
        info(&result),
        named("ApprovalError", "APPROVAL_UNAVAILABLE")
    );
}

#[tokio::test]
async fn ask_routing_returns_aborted_before_dispatch_when_caller_cancellation_overtakes_approval() {
    let env = setup().await;
    let dispatched = Arc::new(AtomicUsize::new(0));
    {
        let dispatched = dispatched.clone();
        let mut probe = echo_tool();
        probe.name = "approval-probe".into();
        probe.execute = Arc::new(move |_, _| {
            dispatched.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(json!("")) })
        });
        register(&env, probe);
    }
    // The real ApprovalService dispatches its owner event; this answerer blocks until released.
    struct Gate {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    let gate = Arc::new(Gate {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    mount_approval_service(&env).await;
    env.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>({
            let gate = gate.clone();
            move |_, _| {
                let gate = gate.clone();
                async move {
                    gate.entered.notify_one();
                    gate.release.notified().await;
                    Ok(ApprovalOutcome::AllowedOnce.into())
                }
            }
        })
        .unwrap();
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|_, _| async {
            Ok(PreToolDecision::Ask {
                reason: None,
                display_reason: None,
                details: None,
            })
        })
        .unwrap();
    let controller = Arc::new(AbortController::new());
    let mut request = input("approval-cancelled", "approval-probe", json!({}));
    request.agent = Some(TestAgent::open(&env.ctx));
    request.signal = controller.signal();
    let ctx = env.ctx.clone();
    let pending = tokio::spawn(async move { execute_tool_fixture(&ctx, request).await });
    gate.entered.notified().await;
    controller.abort();
    gate.release.notify_one();
    let result = pending.await.unwrap();
    assert!(result.is_error());
    assert_eq!(
        info(&result),
        named("AbortError", "ABORTED_BEFORE_DISPATCH")
    );
    assert_eq!(dispatched.load(Ordering::SeqCst), 0);
}

// ----------------------------------------------------------- post-execute and wrappers

#[tokio::test]
async fn a_post_execute_listener_can_replace_the_result_content_and_attach_contexts() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Ok(PostToolDecision::Accept {
                content: Some(vec![text("rewritten")]),
                value: None,
                additional_contexts: Some(vec![user_message("extra")]),
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({ "text": "hi" }))).await;
    assert!(!result.is_error());
    assert_eq!(result.content(), &[text("rewritten")]);
    assert_eq!(result.value(), Some(&json!("hi")));
    assert_eq!(result.additional_contexts().unwrap().len(), 1);
}

#[tokio::test]
async fn a_block_decision_can_also_attach_additional_contexts() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Ok(PostToolDecision::Block {
                feedback: vec![text("nope")],
                info: None,
                additional_contexts: Some(vec![user_message("why")]),
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({ "text": "hi" }))).await;
    assert!(result.is_error());
    assert_eq!(
        result.additional_contexts().unwrap()[0].content,
        vec![text("why")]
    );
}

#[tokio::test]
async fn preserves_tool_deferred_execute_wrapper_and_post_execute_contexts_in_order() {
    let env = setup().await;
    let mut tool = echo_tool();
    tool.execute = Arc::new(|_, exec: Arc<ToolExecution>| {
        exec.defer_context(user_message("tool context"));
        Box::pin(async { Ok(json!("ok")) })
    });
    register(&env, tool);
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, next| async move {
            // A spread copy in the source: an edited copy, never the frozen original.
            let mut result = next.run().await?.clone();
            if let ToolExecutionResult::Success(success) = &mut result {
                success.set_additional_contexts(Some(vec![user_message("wrapper context")]));
            }
            Ok(result)
        })
        .unwrap();
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Ok(PostToolDecision::Accept {
                content: None,
                value: None,
                additional_contexts: Some(vec![user_message("post context")]),
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({}))).await;
    let texts: Vec<_> = result
        .additional_contexts()
        .unwrap()
        .iter()
        .map(|message| message.content[0].text_value().unwrap().to_owned())
        .collect();
    assert_eq!(
        texts,
        vec!["tool context", "wrapper context", "post context"]
    );
}

#[tokio::test]
async fn composes_pre_and_post_waterfalls_around_dispatch_and_runs_tools_execute_in_between() {
    let env = setup().await;
    register(&env, echo_tool());
    let order = Arc::new(Mutex::new(Vec::new()));
    {
        let order = order.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |_, next| {
                lock(&order).push("pre");
                async move { next.run().await }
            })
            .unwrap();
    }
    {
        let order = order.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |_, next| {
                lock(&order).push("execute-before");
                let order = order.clone();
                async move {
                    let result = next.run().await;
                    lock(&order).push("execute-after");
                    result
                }
            })
            .unwrap();
    }
    {
        let order = order.clone();
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |_, next| {
                lock(&order).push("post");
                async move { next.run().await }
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({ "text": "x" }))).await;
    assert!(!result.is_error());
    assert_eq!(
        *lock(&order),
        vec!["pre", "execute-before", "execute-after", "post"]
    );
}

#[tokio::test]
async fn skips_dispatch_when_caller_cancellation_arrives_while_pre_execute_awaits() {
    let env = setup().await;
    let dispatched = Arc::new(AtomicUsize::new(0));
    let mut tool = echo_tool();
    {
        let dispatched = dispatched.clone();
        tool.execute = Arc::new(move |_, _| {
            dispatched.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(json!("")) })
        });
    }
    register(&env, tool);
    let controller = Arc::new(AbortController::new());
    {
        let controller = controller.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |_, next| {
                controller.abort();
                async move { next.run().await }
            })
            .unwrap();
    }
    let mut request = input("c1", "echo", json!({}));
    request.signal = controller.signal();
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert!(result.is_error());
    assert_eq!(
        info(&result),
        named("AbortError", "ABORTED_BEFORE_DISPATCH")
    );
    assert_eq!(dispatched.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn maps_an_explicit_pre_execute_cancellation_to_the_canonical_before_dispatch_result() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|_, _| async {
            Ok(PreToolDecision::Cancel)
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({}))).await;
    assert_eq!(
        result.content(),
        &[text("Error: tool call aborted before dispatch")]
    );
    assert_eq!(
        info(&result),
        named("AbortError", "ABORTED_BEFORE_DISPATCH")
    );
}

#[tokio::test]
async fn replaces_a_late_wrapper_success_with_aborted_and_keeps_tool_owned_failures() {
    let env = setup().await;
    let controller = Arc::new(AbortController::new());
    let mut tool = echo_tool();
    {
        let controller = controller.clone();
        tool.execute = Arc::new(move |_, _| {
            controller.abort();
            Box::pin(async { Ok(json!("done")) })
        });
    }
    register(&env, tool);
    register(&env, string_tool("failing", || Err("tool-owned".into())));
    let mut request = input("c1", "echo", json!({}));
    request.signal = controller.signal();
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert_eq!(info(&result), named("AbortError", "ABORTED"));
    assert_eq!(result.error().unwrap().message, "tool call aborted");

    let controller = Arc::new(AbortController::new());
    let mut failing = string_tool("failing-after-abort", || Err("tool-owned".into()));
    {
        let controller = controller.clone();
        failing.execute = Arc::new(move |_, _| {
            controller.abort();
            Box::pin(async { Err("tool-owned".into()) })
        });
    }
    register(&env, failing);
    let mut request = input("c2", "failing-after-abort", json!({}));
    request.signal = controller.signal();
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert_eq!(result.error().unwrap().message, "tool-owned");
    assert!(result.error().unwrap().info.is_none());
}

#[tokio::test]
async fn keeps_a_harness_error_thrown_after_cancellation_and_classifies_a_signal_reason() {
    let env = setup().await;
    let controller = Arc::new(AbortController::new());
    let mut harness = echo_tool();
    harness.name = "harness-throw".into();
    {
        let controller = controller.clone();
        harness.execute = Arc::new(move |_, _| {
            controller.abort();
            Box::pin(async {
                Err(Box::new(HarnessError::new("own failure", "OWN_CODE")) as BoxError)
            })
        });
    }
    register(&env, harness);
    let mut request = input("c1", "harness-throw", json!({}));
    request.signal = controller.signal();
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert_eq!(result.error().unwrap().message, "own failure");
    assert_eq!(info(&result), named("HarnessError", "OWN_CODE"));

    // A body that throws the signal's own reason gains the ABORTED identity and keeps its message.
    let controller = Arc::new(AbortController::new());
    let mut aborting = echo_tool();
    aborting.name = "reason-throw".into();
    aborting.execute = Arc::new(move |_, exec: Arc<ToolExecution>| {
        Box::pin(async move {
            exec.signal().cancelled().await;
            Err(exec
                .signal()
                .throw_if_aborted()
                .expect_err("aborted")
                .into())
        })
    });
    register(&env, aborting);
    let mut request = input("c2", "reason-throw", json!({}));
    request.signal = controller.signal();
    let aborter = controller.clone();
    let ctx = env.ctx.clone();
    let pending = tokio::spawn(async move { execute_tool_fixture(&ctx, request).await });
    tokio::task::yield_now().await;
    aborter.abort();
    let result = pending.await.unwrap();
    assert_eq!(info(&result), named("AbortError", "ABORTED"));
}

#[tokio::test]
async fn re_fuses_the_caller_signal_with_an_around_dispatch_replacement_for_the_body() {
    let env = setup().await;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let mut tool = echo_tool();
    {
        let observed = observed.clone();
        tool.execute = Arc::new(move |_, exec: Arc<ToolExecution>| {
            let observed = observed.clone();
            Box::pin(async move {
                exec.signal().cancelled().await;
                lock(&observed).push("aborted");
                Ok(json!("done"))
            })
        });
    }
    register(&env, tool);
    let wrapper_controller = AbortController::new();
    {
        let wrapper_signal = wrapper_controller.signal();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |event, next| {
                event.exec.set_signal(wrapper_signal.clone());
                async move { next.run().await }
            })
            .unwrap();
    }
    let caller = AbortController::new();
    let mut request = input("c1", "echo", json!({}));
    request.signal = caller.signal();
    let ctx = env.ctx.clone();
    let pending = tokio::spawn(async move { execute_tool_fixture(&ctx, request).await });
    tokio::task::yield_now().await;
    caller.abort();
    let result = pending.await.unwrap();
    assert_eq!(*lock(&observed), vec!["aborted"]);
    assert_eq!(info(&result), named("AbortError", "ABORTED"));
}

#[tokio::test]
async fn waits_for_an_uncooperative_started_body_before_returning_aborted() {
    let env = setup().await;
    let finished = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let started = Arc::new(tokio::sync::Notify::new());
    let mut tool = echo_tool();
    {
        let (finished, release, started) = (finished.clone(), release.clone(), started.clone());
        tool.execute = Arc::new(move |_, _| {
            let (finished, release, started) = (finished.clone(), release.clone(), started.clone());
            Box::pin(async move {
                started.notify_one();
                release.notified().await;
                finished.fetch_add(1, Ordering::SeqCst);
                Ok(json!("late"))
            })
        });
    }
    register(&env, tool);
    let controller = Arc::new(AbortController::new());
    let mut request = input("c1", "echo", json!({}));
    request.signal = controller.signal();
    let ctx = env.ctx.clone();
    let pending = tokio::spawn(async move { execute_tool_fixture(&ctx, request).await });
    started.notified().await;
    controller.abort();
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    release.notify_one();
    let result = pending.await.unwrap();
    assert_eq!(finished.load(Ordering::SeqCst), 1);
    assert_eq!(info(&result), named("AbortError", "ABORTED"));
}

#[tokio::test]
async fn materializes_a_pre_aborted_call_without_entering_pipeline_phases() {
    let env = setup().await;
    register(&env, echo_tool());
    let entered = Arc::new(AtomicUsize::new(0));
    {
        let entered = entered.clone();
        env.ctx
            .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |_, next| {
                entered.fetch_add(1, Ordering::SeqCst);
                async move { next.run().await }
            })
            .unwrap();
    }
    let observed = Arc::new(AtomicUsize::new(0));
    {
        let observed = observed.clone();
        env.ctx
            .on_contained::<ToolsResult>(move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
    }
    let controller = Arc::new(AbortController::new());
    controller.abort();
    let mut request = input("pre-aborted", "echo", json!({}));
    request.signal = controller.signal();
    let result = execute_tool_fixture(&env.ctx, request).await;
    assert_eq!(
        info(&result),
        named("AbortError", "ABORTED_BEFORE_DISPATCH")
    );
    assert_eq!(entered.load(Ordering::SeqCst), 0);
    assert_eq!(observed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_pre_execute_deny_short_circuits_before_tools_execute() {
    let env = setup().await;
    register(&env, echo_tool());
    let executed = Arc::new(AtomicUsize::new(0));
    {
        let executed = executed.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |_, next| {
                executed.fetch_add(1, Ordering::SeqCst);
                async move { next.run().await }
            })
            .unwrap();
    }
    env.ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|_, _| async {
            Ok(PreToolDecision::Deny {
                reason: "no".into(),
                info: None,
            })
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(executed.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_thrown_tool_is_normalized_inside_tools_execute_and_still_reaches_post_execute() {
    let env = setup().await;
    register(&env, string_tool("boom", || Err("exploded".into())));
    let seen_in_wrapper = Arc::new(Mutex::new(None));
    let post_calls = Arc::new(AtomicUsize::new(0));
    {
        let seen = seen_in_wrapper.clone();
        env.ctx
            .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(move |_, next| {
                let seen = seen.clone();
                async move {
                    let result = next.run().await?;
                    *lock(&seen) =
                        Some((result.is_error(), result.error().map(|e| e.message.clone())));
                    Ok(result)
                }
            })
            .unwrap();
    }
    {
        let post_calls = post_calls.clone();
        env.ctx
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |_, next| {
                post_calls.fetch_add(1, Ordering::SeqCst);
                async move { next.run().await }
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("c1", "boom", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(
        *lock(&seen_in_wrapper),
        Some((true, Some("exploded".to_owned())))
    );
    assert_eq!(post_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_tools_execute_listener_can_short_circuit_dispatch_by_returning_a_result() {
    let env = setup().await;
    let ran = Arc::new(AtomicUsize::new(0));
    let mut tool = echo_tool();
    {
        let ran = ran.clone();
        tool.execute = Arc::new(move |_, _| {
            ran.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(json!("body")) })
        });
    }
    register(&env, tool);
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, _| async {
            Ok(ToolExecutionResult::Success(
                hoh_tools::ToolExecutionSuccess::new(json!("short-circuit"), vec![]),
            ))
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({}))).await;
    assert_eq!(result.value(), Some(&json!("short-circuit")));
    assert_eq!(result.content(), &[text("short-circuit")]);
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn revalidates_a_wrapper_authored_result_that_is_a_copy() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, next| async move {
            let result = next.run().await?;
            // A copy is a new object: it is normalized through the output contract again.
            let mut copy = result.clone();
            if let ToolExecutionResult::Success(success) = &mut copy {
                success.set_value(json!(42));
            }
            Ok(copy)
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(
        info(&result),
        named("ToolOutputError", "INVALID_TOOL_OUTPUT")
    );
}

#[tokio::test]
async fn returns_an_is_error_result_when_a_listener_fails() {
    let env = setup().await;
    register(&env, echo_tool());
    let pre = env
        .ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|_, _| async {
            Err::<PreToolDecision, BoxError>("pre listener failed".into())
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(result.error().unwrap().message, "pre listener failed");
    pre.dispose().await;
    let structured = env
        .ctx
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(|_, _| async {
            Err::<PreToolDecision, BoxError>(Box::new(HarnessError::new("denied", "STRUCTURED")))
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c2", "echo", json!({}))).await;
    assert_eq!(info(&result), named("HarnessError", "STRUCTURED"));
    structured.dispose().await;
    env.ctx
        .on_waterfall::<ToolsExecute, ToolExecutionResult, _>(|_, _| async {
            Err::<ToolExecutionResult, BoxError>("execute listener failed".into())
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c3", "echo", json!({}))).await;
    assert_eq!(result.error().unwrap().message, "execute listener failed");
    assert!(result.error().unwrap().info.is_none());
}

#[tokio::test]
async fn returns_an_is_error_result_when_a_post_execute_listener_throws() {
    let env = setup().await;
    register(&env, echo_tool());
    env.ctx
        .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(|_, _| async {
            Err::<PostToolDecision, BoxError>("post listener failed".into())
        })
        .unwrap();
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(result.error().unwrap().message, "post listener failed");
}

#[tokio::test]
async fn notifies_every_result_observer_with_the_final_outcome_and_contains_failures() {
    let env = setup().await;
    register(&env, echo_tool());
    let seen = Arc::new(Mutex::new(Vec::new()));
    env.ctx
        .on_contained::<ToolsResult>(|_| Err("observer failed".into()))
        .unwrap();
    {
        let seen = seen.clone();
        env.ctx
            .on_contained::<ToolsResult>(move |event| {
                lock(&seen).push(event.result.text());
                Ok(())
            })
            .unwrap();
    }
    let result = execute_tool_fixture(&env.ctx, input("c1", "echo", json!({ "text": "ok" }))).await;
    assert!(!result.is_error());
    assert_eq!(*lock(&seen), vec!["ok".to_owned()]);
    let warned = env.kernel.root().logger().buffer().iter().any(|record| {
        record
            .message
            .contains("tools/result observer failed: observer failed")
    });
    assert!(warned);
}

// -------------------------------------------------------------- registry lifecycle

#[tokio::test]
async fn rejects_a_non_positive_or_non_finite_registration_timeout() {
    let env = setup().await;
    for timeout in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut tool = echo_tool();
        tool.timeout_ms = Some(timeout);
        let error = env
            .tools
            .register(&env.ctx, tool, ToolRegistrationOptions::default())
            .err()
            .expect("rejected");
        assert!(
            error
                .to_string()
                .contains("timeoutMs must be a positive finite number")
        );
    }
}

#[tokio::test]
async fn rejects_duplicate_names_and_unregisters_on_fiber_dispose() {
    let env = setup().await;
    register(&env, echo_tool());
    let error = env
        .tools
        .register(&env.ctx, echo_tool(), ToolRegistrationOptions::default())
        .err()
        .expect("duplicate");
    assert!(
        error
            .to_string()
            .contains("tool \"echo\" is already registered")
    );
    assert!(error.to_string().contains("agent.ctx"));
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn returns_a_callable_disposer_from_register_that_unregisters_the_tool() {
    let env = setup().await;
    let disposer = register(&env, echo_tool());
    assert!(env.tools.get("echo", None, None).is_some());
    disposer.dispose().await;
    assert!(env.tools.get("echo", None, None).is_none());
    assert!(env.tools.schemas(None, None).unwrap().is_empty());
}

#[tokio::test]
async fn reserves_the_run_code_name_and_notifies_tools_change() {
    let env = setup().await;
    let mut tool = echo_tool();
    tool.name = "run_code".into();
    let error = env
        .tools
        .register(&env.ctx, tool, ToolRegistrationOptions::default())
        .err()
        .expect("reserved");
    assert!(
        error
            .to_string()
            .contains("is reserved for the PTC mode presentation transport")
    );
    let changes = Arc::new(AtomicUsize::new(0));
    {
        let changes = changes.clone();
        env.ctx
            .on::<hoh_tools::ToolsChange>(move |_| {
                changes.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
    }
    let disposer = register(&env, echo_tool());
    disposer.dispose().await;
    assert_eq!(changes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn rolls_back_the_tool_entry_when_a_tools_change_listener_throws() {
    let env = setup().await;
    let listener = env
        .ctx
        .on::<hoh_tools::ToolsChange>(|_| panic!("change listener failed"))
        .unwrap();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.tools
            .register(&env.ctx, echo_tool(), ToolRegistrationOptions::default())
    }));
    assert!(matches!(outcome, Ok(Err(_))));
    listener.dispose().await;
    assert!(env.tools.get("echo", None, None).is_none());
    register(&env, echo_tool());
}

// ---------------------------------------------------------- defineTool / schema DSL

#[test]
fn converts_a_parameter_schema_spec_to_standard_json_schema_with_a_required_array() {
    let compiled = hoh_tools::parameter_schema_spec_to_json_schema(&json!({
        "path": { "type": "string", "description": "file path", "required": true },
        "offset": { "type": "integer" },
    }))
    .unwrap();
    assert_eq!(
        Value::Object(compiled),
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "file path" },
                "offset": { "type": "integer" },
            },
            "required": ["path"],
        })
    );
    assert_eq!(
        Value::Object(hoh_tools::parameter_schema_spec_to_json_schema(&json!({})).unwrap()),
        json!({ "type": "object", "properties": {} })
    );
}

#[test]
fn schema_dsl_edge_cases_emit_enum_default_and_omit_absent_keys() {
    let compiled = Value::Object(
        hoh_tools::parameter_schema_spec_to_json_schema(&json!({
            "mode": { "type": "string", "enum": ["a", "b"], "default": "a", "description": "mode" },
            "plain": { "type": "string" },
            "tags": { "type": "array", "items": { "type": "string" } },
            "any": { "type": "array" },
            "nested": {
                "type": "object",
                "additionalProperties": true,
                "properties": { "x": { "type": "number" } },
            },
        }))
        .unwrap(),
    );
    assert_eq!(
        compiled["properties"]["mode"],
        json!({ "type": "string", "description": "mode", "default": "a", "enum": ["a", "b"] })
    );
    assert_eq!(compiled["properties"]["plain"], json!({ "type": "string" }));
    assert_eq!(
        compiled["properties"]["tags"],
        json!({ "type": "array", "items": { "type": "string" } })
    );
    assert_eq!(compiled["properties"]["any"], json!({ "type": "array" }));
    assert!(compiled["properties"]["nested"].get("required").is_none());
    assert!(compiled.get("required").is_none());
}

#[tokio::test]
async fn registry_round_trips_a_define_tool_definition() {
    let env = setup().await;
    let tool = define_tool(DefineToolOptions::new(
        "read",
        "read a file",
        json!({ "path": { "type": "string", "required": true }, "offset": { "type": "integer" } }),
        DefineToolOutput {
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "path": { "type": "string", "required": true } },
            }),
            render: Arc::new(|_, value| {
                vec![text(&format!("read {}", value["path"].as_str().unwrap()))]
            }),
            presentation_meta: None,
        },
        |args, _| Box::pin(async move { Ok(json!({ "path": args["path"] })) }),
    ))
    .unwrap();
    register(&env, tool);
    let schemas = env.tools.schemas(None, None).unwrap();
    assert_eq!(schemas[0].name, "read");
    assert_eq!(
        Value::Object(schemas[0].parameters.clone()),
        json!({
            "type": "object",
            "properties": { "path": { "type": "string" }, "offset": { "type": "integer" } },
            "required": ["path"],
        })
    );
    let result = execute_tool_fixture(&env.ctx, input("c1", "read", json!({ "path": "/a" }))).await;
    assert_eq!(result.content(), &[text("read /a")]);
}

#[tokio::test]
async fn raw_registered_tools_are_not_validated_by_define_tool() {
    let env = setup().await;
    let mut raw = echo_tool();
    raw.name = "raw".into();
    raw.parameters = serde_json::from_value(json!({
        "type": "object",
        "properties": { "n": { "type": "integer" } },
        "required": ["n"],
    }))
    .unwrap();
    raw.execute = Arc::new(|args, _| Box::pin(async move { Ok(json!(format!("{args}"))) }));
    register(&env, raw);
    let result = execute_tool_fixture(
        &env.ctx,
        input("c1", "raw", json!({ "n": "not-an-integer" })),
    )
    .await;
    assert!(!result.is_error());
    assert_eq!(result.value(), Some(&json!(r#"{"n":"not-an-integer"}"#)));
}

// ------------------------------------------------------------------- validateArgs

fn violations(spec: Value, args: Value) -> Vec<String> {
    validate_args(&spec, &args).unwrap()
}

#[test]
fn validate_args_returns_empty_for_valid_args_and_is_total_over_malformed_input() {
    let spec = json!({ "path": { "type": "string", "required": true } });
    assert!(violations(spec.clone(), json!({ "path": "/a" })).is_empty());
    for malformed in [Value::Null, json!("x"), json!(1), json!([1])] {
        assert!(!violations(spec.clone(), malformed).is_empty());
    }
}

#[test]
fn validate_args_flags_a_missing_required_key_and_allows_extra_keys_and_omitted_optionals() {
    let spec = json!({
        "path": { "type": "string", "required": true },
        "offset": { "type": "integer" },
    });
    assert_eq!(
        violations(spec.clone(), json!({})),
        vec!["missing required property \"path\""]
    );
    assert!(violations(spec.clone(), json!({ "path": "a", "extra": true })).is_empty());
    assert!(violations(spec, json!({ "path": "a" })).is_empty());
}

#[test]
fn validate_args_type_checks_primitives_and_enum_membership() {
    let spec = json!({
        "s": { "type": "string" }, "n": { "type": "number" }, "i": { "type": "integer" },
        "b": { "type": "boolean" }, "mode": { "type": "string", "enum": ["a", "b"] },
    });
    assert_eq!(
        violations(spec.clone(), json!({ "s": 1 })),
        vec!["\"s\" must be a string"]
    );
    assert_eq!(
        violations(spec.clone(), json!({ "n": "1" })),
        vec!["\"n\" must be a number"]
    );
    assert_eq!(
        violations(spec.clone(), json!({ "i": 1.5 })),
        vec!["\"i\" must be an integer"]
    );
    assert_eq!(
        violations(spec.clone(), json!({ "b": "x" })),
        vec!["\"b\" must be a boolean"]
    );
    assert_eq!(
        violations(spec, json!({ "mode": "c" })),
        vec!["\"mode\" must be one of [\"a\",\"b\"]"]
    );
}

#[test]
fn validate_args_recurses_into_nested_objects_and_array_items() {
    let spec = json!({
        "cfg": {
            "type": "object", "additionalProperties": true,
            "properties": { "depth": { "type": "integer", "required": true } },
        },
        "list": { "type": "array", "items": { "type": "string" } },
        "rows": {
            "type": "array",
            "items": {
                "type": "object", "additionalProperties": true,
                "properties": { "id": { "type": "integer", "required": true } },
            },
        },
    });
    assert_eq!(
        violations(spec.clone(), json!({ "cfg": {} })),
        vec!["missing required property \"cfg.depth\""]
    );
    assert_eq!(
        violations(spec.clone(), json!({ "list": ["a", 1] })),
        vec!["\"list[1]\" must be a string"]
    );
    assert_eq!(
        violations(spec, json!({ "rows": [{ "id": 1 }, {}] })),
        vec!["missing required property \"rows[1].id\""]
    );
}

#[test]
fn validate_args_rejects_an_unknown_schema_type_at_the_author_boundary() {
    let error =
        validate_args(&json!({ "x": { "type": "tuple" } }), &json!({})).expect_err("rejected");
    assert!(error.is_named("JsonSchemaError"));
}

// ------------------------------------------------------ defineTool validation

fn guarded_tool() -> hoh_tools::ToolDefinition {
    define_tool(DefineToolOptions::new(
        "guarded",
        "guarded",
        json!({ "path": { "type": "string", "required": true } }),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, value| vec![text(value.as_str().unwrap())]),
            presentation_meta: None,
        },
        |args, _| Box::pin(async move { Ok(args["path"].clone()) }),
    ))
    .unwrap()
}

#[tokio::test]
async fn define_tool_returns_an_is_error_result_with_the_violations_for_bad_args() {
    let env = setup().await;
    register(&env, guarded_tool());
    let result = execute_tool_fixture(&env.ctx, input("c1", "guarded", json!({}))).await;
    assert!(result.is_error());
    assert_eq!(
        result.error().unwrap().message,
        "invalid arguments: missing required property \"path\""
    );
    assert_eq!(info(&result), named("ToolArgsError", "INVALID_ARGS"));
    let ok = execute_tool_fixture(&env.ctx, input("c2", "guarded", json!({ "path": "/x" }))).await;
    assert_eq!(ok.value(), Some(&json!("/x")));
}

#[test]
fn tool_args_error_carries_a_stable_code_and_the_violation_list() {
    let error = ToolArgsError::new(vec!["a".into(), "b".into()]);
    assert_eq!(error.name(), "ToolArgsError");
    assert_eq!(error.code(), "INVALID_ARGS");
    assert_eq!(error.message(), "invalid arguments: a; b");
    assert_eq!(
        ToolArgsError::violations(&error),
        Some(&["a".to_owned(), "b".to_owned()][..])
    );
    let output = ToolOutputError::new("t", vec!["x".into()]);
    assert_eq!(output.message(), "tool \"t\" returned invalid output: x");
    assert_eq!(
        ToolOutputError::violations(&output),
        Some(&["x".to_owned()][..])
    );
}

#[tokio::test]
async fn a_tool_throwing_a_harness_error_surfaces_its_name_and_code_and_a_plain_throw_keeps_its_message()
 {
    let env = setup().await;
    register(
        &env,
        string_tool("harness", || {
            Err(Box::new(HarnessError::new(
                "harness failure",
                "CUSTOM_CODE",
            )))
        }),
    );
    register(&env, string_tool("plain", || Err("plain failure".into())));
    let harness = execute_tool_fixture(&env.ctx, input("c1", "harness", json!({}))).await;
    assert_eq!(info(&harness), named("HarnessError", "CUSTOM_CODE"));
    let plain = execute_tool_fixture(&env.ctx, input("c2", "plain", json!({}))).await;
    assert_eq!(plain.error().unwrap().message, "plain failure");
    assert!(plain.error().unwrap().info.is_none());
}

#[test]
fn attaches_a_positive_finite_timeout_ms_and_rejects_the_rest() {
    let make = |timeout: Option<f64>| {
        let mut options = DefineToolOptions::new(
            "t",
            "",
            json!({}),
            DefineToolOutput {
                schema: json!({ "type": "string" }),
                render: Arc::new(|_, _| vec![]),
                presentation_meta: None,
            },
            |_, _| Box::pin(async { Ok(json!("")) }),
        );
        options.timeout_ms = timeout;
        define_tool(options)
    };
    assert_eq!(make(Some(1_500.0)).unwrap().timeout_ms, Some(1_500.0));
    assert_eq!(make(None).unwrap().timeout_ms, None);
    for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
        let error = make(Some(bad)).err().expect("rejected");
        assert!(
            error
                .to_string()
                .contains("timeoutMs must be a positive finite number")
        );
    }
}

#[test]
fn presenters_validate_softly_and_never_fail() {
    let mut options = DefineToolOptions::new(
        "present",
        "",
        json!({ "x": { "type": "string", "required": true } }),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, _| vec![]),
            presentation_meta: None,
        },
        |_, _| Box::pin(async { Ok(json!("")) }),
    );
    options.present_call = Some(Arc::new(|args| {
        Some(hoh_tools::ToolCallView::Generic(
            hoh_tools::GenericCallView {
                title: format!("run {}", args["x"].as_str().unwrap()),
                kind: Some(hoh_tools::ToolCallKind::Execute),
                raw_input: None,
                content: None,
                locations: None,
            },
        ))
    }));
    options.is_concurrency_safe = Some(Arc::new(|_| true));
    let tool = define_tool(options).unwrap();
    let present = tool.present_call.as_ref().unwrap();
    assert!(present(&json!({ "x": "a" })).is_some());
    assert!(present(&json!({})).is_none());
    assert!(present(&json!("garbage")).is_none());
    assert!(tool.present_result.is_none());
    let classify = tool.is_concurrency_safe.as_ref().unwrap();
    assert!(classify(&json!({ "x": "a" })));
    assert!(!classify(&json!({})));
}

// ---------------------------------------------------------------- raw native batches

#[tokio::test]
async fn executes_raw_native_calls_without_an_agent_or_session_and_keeps_input_order() {
    let env = setup().await;
    register(&env, echo_tool());
    let batch = env
        .tools
        .execute(hoh_tools::ToolBatchInput::new(
            vec![
                model_call("raw", "echo", r#"{ "text": "raw value" }"#),
                model_call("malformed", "echo", "{invalid"),
                model_call("empty", "echo", ""),
            ],
            live_signal(),
        ))
        .await
        .unwrap();
    let ids: Vec<_> = batch
        .results
        .iter()
        .map(|outcome| outcome.call_id.to_string())
        .collect();
    assert_eq!(ids, vec!["raw", "malformed", "empty"]);
    assert!(!batch.results[0].result.is_error());
    assert_eq!(batch.results[0].result.value(), Some(&json!("raw value")));
    assert!(batch.results[1].result.is_error());
    assert!(!batch.results[2].result.is_error());
    assert_eq!(batch.results[2].result.value(), Some(&json!("")));
    assert!(batch.additional_contexts.is_empty());
    assert!(!batch.concluded);
    assert!(env.ctx.get::<hoh_session::Sessions>().is_none());
}

#[tokio::test]
async fn settles_every_unowned_call_when_cancellation_precedes_the_batch() {
    let env = setup().await;
    register(&env, echo_tool());
    let controller = Arc::new(AbortController::new());
    controller.abort();
    let batch = env
        .tools
        .execute(hoh_tools::ToolBatchInput::new(
            vec![model_call("a", "echo", "{}"), model_call("b", "echo", "{}")],
            controller.signal(),
        ))
        .await
        .unwrap();
    assert_eq!(batch.results.len(), 2);
    for outcome in &batch.results {
        assert_eq!(
            info(&outcome.result),
            named("AbortError", "ABORTED_BEFORE_DISPATCH")
        );
    }
}

#[tokio::test]
async fn rejects_a_zero_parallel_override_with_a_range_error() {
    let env = setup().await;
    let mut input = hoh_tools::ToolBatchInput::new(vec![], live_signal());
    input.max_parallel_tool_calls = Some(0);
    let error = env.tools.execute(input).await.expect_err("rejected");
    assert_eq!(
        error.to_string(),
        "maxParallelToolCalls must be a positive integer"
    );
}

#[tokio::test]
async fn logs_root_call_and_result_events_to_the_agent_session_in_model_order() {
    let env = setup().await;
    register(&env, echo_tool());
    let agent = TestAgent::new(&env.ctx);
    let mut input = hoh_tools::ToolBatchInput::new(
        vec![
            model_call("a", "echo", r#"{"text":"one"}"#),
            model_call("b", "missing", "{}"),
        ],
        live_signal(),
    );
    input.agent = Some(agent.clone());
    input.log = Some(hoh_tools::ToolLogStep { turn: 1, step: 2 });
    let batch = env.tools.execute(input).await.unwrap();
    assert_eq!(batch.results.len(), 2);
    let events: Vec<_> = agent
        .session
        .own_events()
        .iter()
        .filter(|event| event.event_type().starts_with("tool/"))
        .map(|event| event.event_type().to_owned())
        .collect();
    assert_eq!(
        events,
        vec!["tool/call", "tool/result", "tool/call", "tool/result"]
    );
}
