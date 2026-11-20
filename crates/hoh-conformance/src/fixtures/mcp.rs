//! `snapshots/session/multimodal-spill-ends/fixture.mjs`: a deterministic MCP screenshot result for
//! real-profile retention replay.
use super::SNAPSHOTS;
use hoh_attachment::Attachments;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_loader::PluginRegistry;
use hoh_mcp_client::tools::{McpToolDefinitionOptions, create_mcp_tool_definition};
use hoh_tools::{ToolRegistrationOptions, Tools};
use serde_json::{Value, json};
use std::sync::Arc;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAACXBIWXMAAAPoAAAD6AG1e1JrAAAADElEQVQImWNgZGIGAAAOAAeCcsnOAAAAAElFTkSuQmCC";

#[derive(Inject)]
pub struct McpFixtureDeps {
    tools: Service<Tools>,
    #[allow(dead_code)]
    attachments: Service<Attachments>,
}

/// The fixed text/image sequence the snapshot model asks for (`layout`: `ends` or `middle`).
fn content(layout: &str) -> Vec<Value> {
    let text = |text: String| json!({"type": "text", "text": text});
    let image = || json!({"type": "image", "mimeType": "image/png", "data": PNG});
    if layout == "ends" {
        vec![
            text("A".into()),
            image(),
            text("B".repeat(20000)),
            image(),
            text("C".into()),
        ]
    } else {
        vec![text("A".repeat(8000)), image(), text("C".repeat(8000))]
    }
}

pub struct MultimodalSpillFixture;
impl Plugin for MultimodalSpillFixture {
    const NAME: &'static str = "multimodal-spill-fixture";
    type Config = ();
    type Deps = McpFixtureDeps;
    async fn apply(ctx: Context, deps: McpFixtureDeps, _: Cfg<()>) -> Result<(), BoxError> {
        let input_schema = json!({
            "type": "object",
            "properties": {"layout": {"type": "string", "enum": ["ends", "middle"]}},
            "required": ["layout"],
        });
        let definition = create_mcp_tool_definition(
            &ctx,
            McpToolDefinitionOptions {
                name: "mcp__fixture__inspect".into(),
                raw_name: "inspect".into(),
                description: "Read a fixed window snapshot.".into(),
                input_schema: input_schema.as_object().cloned().unwrap_or_default(),
                output_schema: None,
                task_required: false,
                redact: None,
                call: Arc::new(|arguments, _| {
                    let layout = arguments
                        .get("layout")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    Box::pin(async move { Ok(json!({"content": content(&layout)})) })
                }),
            },
        )?;
        deps.tools
            .register(&ctx, definition, ToolRegistrationOptions::default())?;
        Ok(())
    }
}

pub fn add(registry: &mut PluginRegistry) {
    registry.add::<MultimodalSpillFixture>(&format!(
        "{SNAPSHOTS}session/multimodal-spill-ends/fixture.mjs"
    ));
}
