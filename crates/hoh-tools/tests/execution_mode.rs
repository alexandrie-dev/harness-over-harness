//! Port of tests/execution-mode.spec.ts. The "truthy non-boolean classifier" case and the
//! `expectTypeOf` case have no counterpart: the classifier returns `bool` and the mode is an enum.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use hoh_tools::{ToolDefinition, ToolExecutionMode, ToolExecutionPipeline, ToolOutputDefinition};
use serde_json::{Value, json};

fn mode(env: &Env, name: &str, args: Value) -> ToolExecutionMode {
    ToolExecutionPipeline::new(env.ctx.clone())
        .expect("pipeline")
        .execution_mode(&input("c1", name, args))
}

fn empty(name: &str, parameters: Value, safe: Option<hoh_tools::ConcurrencyFn>) -> ToolDefinition {
    content_tool_with(
        name,
        parameters,
        |_, _| async { Ok(vec![]) },
        move |options| {
            options.is_concurrency_safe = safe;
        },
    )
}

fn raw(name: &str, safe: hoh_tools::ConcurrencyFn) -> ToolDefinition {
    let mut definition = ToolDefinition::new(
        name,
        "raw",
        serde_json::from_value(json!({ "type": "object", "properties": {} })).unwrap(),
        ToolOutputDefinition {
            schema: json!({ "type": "null" }),
            render: Arc::new(|_, _| Ok(vec![])),
            presentation_meta: None,
        },
        Arc::new(|_, _| Box::pin(async { Ok(Value::Null) })),
    );
    definition.is_concurrency_safe = Some(safe);
    definition
}

#[tokio::test]
async fn returns_parallel_only_for_an_explicit_true_classifier() {
    let env = setup().await;
    register(&env, empty("safe", json!({}), Some(Arc::new(|_| true))));
    assert_eq!(mode(&env, "safe", json!({})), ToolExecutionMode::Parallel);
}

#[tokio::test]
async fn defaults_to_exclusive_for_a_tool_with_no_declaration() {
    let env = setup().await;
    register(&env, empty("plain", json!({}), None));
    assert_eq!(mode(&env, "plain", json!({})), ToolExecutionMode::Exclusive);
}

#[tokio::test]
async fn returns_exclusive_for_an_unknown_tool() {
    let env = setup().await;
    assert_eq!(
        mode(&env, "nonexistent", json!({})),
        ToolExecutionMode::Exclusive
    );
}

#[tokio::test]
async fn returns_exclusive_when_the_classifier_returns_false_for_these_args() {
    let env = setup().await;
    register(
        &env,
        empty(
            "rw",
            json!({ "mode": { "type": "string", "required": true } }),
            Some(Arc::new(|args| args["mode"] == "read")),
        ),
    );
    assert_eq!(
        mode(&env, "rw", json!({ "mode": "read" })),
        ToolExecutionMode::Parallel
    );
    assert_eq!(
        mode(&env, "rw", json!({ "mode": "write" })),
        ToolExecutionMode::Exclusive
    );
}

#[tokio::test]
async fn classifies_invalid_define_tool_arguments_as_exclusive_without_throwing() {
    let env = setup().await;
    register(
        &env,
        empty(
            "needs-mode",
            json!({ "mode": { "type": "string", "required": true } }),
            Some(Arc::new(|_| true)),
        ),
    );
    assert_eq!(
        mode(&env, "needs-mode", json!({})),
        ToolExecutionMode::Exclusive
    );
}

#[tokio::test]
async fn treats_a_throwing_raw_classifier_as_exclusive() {
    let env = setup().await;
    register(&env, raw("thrower", Arc::new(|_| panic!("boom"))));
    assert_eq!(
        mode(&env, "thrower", json!({})),
        ToolExecutionMode::Exclusive
    );
}

#[tokio::test]
async fn passes_parsed_arguments_directly_to_a_raw_definition() {
    let env = setup().await;
    let seen = Arc::new(Mutex::new(None));
    {
        let seen = seen.clone();
        register(
            &env,
            raw(
                "raw-safe",
                Arc::new(move |args| {
                    *lock(&seen) = Some(args.clone());
                    true
                }),
            ),
        );
    }
    assert_eq!(
        mode(&env, "raw-safe", json!({ "anything": 1 })),
        ToolExecutionMode::Parallel
    );
    assert_eq!(*lock(&seen), Some(json!({ "anything": 1 })));
}

#[tokio::test]
async fn is_concurrency_safe_never_reaches_the_model_facing_schemas_projection() {
    let env = setup().await;
    register(
        &env,
        empty(
            "safe",
            json!({ "x": { "type": "string", "required": true } }),
            Some(Arc::new(|_| true)),
        ),
    );
    let schema = serde_json::to_value(&env.tools.schemas(None, None).unwrap()[0]).unwrap();
    let mut keys: Vec<_> = schema.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, vec!["description", "name", "parameters"]);
    assert!(schema.get("isConcurrencySafe").is_none());
}
