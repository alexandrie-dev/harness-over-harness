use hoh_kernel::{Cfg, Context, Error, Inject, NoConfig, Plugin, Service};
use hoh_llm::ContentBlock;
use hoh_tools::{DefineToolOptions, DefineToolOutput, ToolRegistrationOptions, Tools, define_tool};
use serde_json::json;
use std::sync::Arc;

pub const TOOL: &str = "@TOOL@";

#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
}

pub struct AuthorPlugin;

impl Plugin for AuthorPlugin {
    const NAME: &'static str = "hoh:plugin-@ID@";
    type Config = NoConfig;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, _: Cfg<NoConfig>) -> Result<(), Error> {
        let tool = define_tool(DefineToolOptions::new(
            TOOL,
            "Echo a string.",
            json!({"text": {"type": "string", "required": true}}),
            DefineToolOutput {
                schema: json!({"type": "string"}),
                render: Arc::new(|_, value| {
                    vec![ContentBlock::text(value.as_str().unwrap_or_default())]
                }),
                presentation_meta: None,
            },
            |args, _| Box::pin(async move { Ok(args["text"].clone()) }),
        ))?;
        deps.tools.register(&ctx, tool, ToolRegistrationOptions::default())?;
        Ok(())
    }
}
