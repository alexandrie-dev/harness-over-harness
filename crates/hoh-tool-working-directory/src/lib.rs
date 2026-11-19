//! Port of @deepseek-ai/dsh-tool-working-directory (packages/session/tool-working-directory).
//!
//! Read or change the calling Session's working directory through one model tool.

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::ContentBlock;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolArgsError, ToolContextUnavailableError,
    ToolRegistrationOptions, Tools, define_tool,
};
use hoh_working_directory::{WorkingDirectoryKey, live_agent};
use serde_json::{Map, Value, json};

/// Services used by the tool (`inject = ['tools', 'workingDirectory']`).
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    working_directory: Service<WorkingDirectoryKey>,
}

/// Register the `working_directory` tool for this composition.
pub struct ToolWorkingDirectory;

impl Plugin for ToolWorkingDirectory {
    const NAME: &'static str = "tool-working-directory";
    type Config = ();
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, _config: Cfg<()>) -> Result<(), BoxError> {
        let mut parameters = Map::new();
        parameters.insert(
            "cd".into(),
            json!({
                "type": "string",
                "description": "Existing directory to enter. Omit to read the current directory.",
            }),
        );
        let output = DefineToolOutput {
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "cwd": { "type": "string", "required": true, "description": "Current absolute working directory." },
                },
            }),
            render: Arc::new(|_args, value| {
                vec![ContentBlock::text(
                    value["cwd"].as_str().unwrap_or_default(),
                )]
            }),
            presentation_meta: None,
        };
        let directory = deps.working_directory;
        let owner = ctx.clone();
        let options = DefineToolOptions::new(
            "working_directory",
            "Read the current working directory, or change it with cd. Relative paths use the current directory. Existing shells and running processes keep their own directories.",
            Value::Object(parameters),
            output,
            move |args, exec| {
                let directory = directory.clone();
                let owner = owner.clone();
                Box::pin(async move {
                    let Some(calling) = &exec.agent else {
                        return Err(ToolContextUnavailableError::new(
                            "working_directory requires an agent Session",
                        )
                        .into());
                    };
                    // SHIM(hoh-tools): `exec.agent` is the session-level seam; the whole Agent
                    // (context, inbox for the transition notice) is the registered one.
                    let Some(agent) = live_agent(&owner, calling.session()) else {
                        return Err(ToolContextUnavailableError::new(
                            "working_directory requires an agent registered in the agents registry",
                        )
                        .into());
                    };
                    let signal = exec.signal();
                    let cwd = match args.get("cd").and_then(Value::as_str) {
                        Some("") => {
                            return Err(ToolArgsError::new(vec![
                                "cd must not be empty; omit it to read the current directory"
                                    .into(),
                            ])
                            .into());
                        }
                        None => directory.ensure(&agent, Some(&signal)).await?,
                        Some(path) => directory.set(&agent, path, Some(&signal)).await?,
                    };
                    Ok(json!({ "cwd": cwd }))
                })
            },
        );
        deps.tools.register(
            &ctx,
            define_tool(options)?,
            ToolRegistrationOptions::default(),
        )?;
        Ok(())
    }
}
