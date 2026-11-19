//! `describe` of the native executor: the model-facing projection of visible definitions.

use std::sync::Arc;

use hoh_kernel::Kernel;
use hoh_tool_executor_native::NativeToolExecutor;
use hoh_tools::{ToolDefinition, ToolExecutor, ToolOutputDefinition};
use serde_json::{Map, Value, json};

fn definition(name: &str, defer_loading: Option<bool>) -> Arc<ToolDefinition> {
    let mut definition = ToolDefinition::new(
        name,
        "A tool.",
        Map::new(),
        ToolOutputDefinition {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, _| Ok(Vec::new())),
            presentation_meta: None,
        },
        Arc::new(|_, _| Box::pin(async { Ok(json!("done")) })),
    );
    definition.defer_loading = defer_loading;
    Arc::new(definition)
}

/// An executor needs no registry to describe: `describe([])` is `{ schemas: [] }` on an empty context.
#[tokio::test]
async fn describes_nothing_for_an_empty_context_without_a_tools_service() {
    let kernel = Kernel::start();
    let executor = NativeToolExecutor::new(kernel.root()).expect("constructs without a registry");
    let described = executor.describe(&[]).expect("describes");
    assert!(described.schemas.is_empty());
    assert_eq!(described.known_names, None);
    kernel.shutdown().await;
}

/// `projectToolSchema` carries `deferLoading` only when it is exactly `true`.
#[tokio::test]
async fn describes_defer_loading_only_when_it_is_true() {
    let kernel = Kernel::start();
    let executor = NativeToolExecutor::new(kernel.root()).expect("constructs");
    let described = executor
        .describe(&[
            definition("absent", None),
            definition("disabled", Some(false)),
            definition("deferred", Some(true)),
        ])
        .expect("describes");
    let schemas: Vec<Value> = described
        .schemas
        .iter()
        .map(|schema| serde_json::to_value(schema).expect("serializes"))
        .collect();
    assert_eq!(schemas[0].get("deferLoading"), None);
    assert_eq!(schemas[1].get("deferLoading"), None);
    assert_eq!(schemas[2].get("deferLoading"), Some(&json!(true)));
    assert_eq!(
        schemas[2],
        json!({
            "name": "deferred",
            "description": "A tool.",
            "parameters": {},
            "deferLoading": true,
        })
    );
    kernel.shutdown().await;
}
