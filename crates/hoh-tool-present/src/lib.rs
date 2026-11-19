//! Explicit workspace-file delivery declarations.
//!
//! Port of `@deepseek-ai/dsh-tool-present` (`packages/deliverables/tool-present`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use hoh_agent::Agents;
use hoh_error::BoxError;
use hoh_fs::{Fs, FsError, FsErrorCode, FsResolveOptions, FsType};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{ContentBlock, ToolCallId};
use hoh_session::{MAX_SAFE_INTEGER, Session};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolArgsError, ToolContextUnavailableError,
    ToolExecutionToken, ToolRegistrationOptions, Tools, ToolsResult, define_tool,
};
use hoh_util_values::js_trim;
use hoh_working_directory::{WorkingDirectoryKey, live_agent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Stable Loader identity.
pub const NAME: &str = "tool-present";

const DESCRIPTION: &str = "Declare existing files as final deliverables for the user. Hosts may show presented files only after your turn ends, so call it once the files are final. If the user needs to review files earlier, such as before a tool call that waits for their answer, link them in your message instead. Use it when the user needs a separate file, especially Office documents, spreadsheets, and slide decks; prefer your final response when that suffices. The user opens the current files; their contents are not copied.";

/// Per-call delivery limit.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Maximum number of files in one call.
    #[serde(default = "default_max_files")]
    pub max_files: f64,
}

const fn default_max_files() -> f64 {
    8.0
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_files: default_max_files(),
        }
    }
}

/// A declared filesystem file whose current contents remain at its source path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentedFile {
    /// Filesystem-provider path of the existing regular file.
    pub path: String,
    /// Optional description supplied by the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Data of the durable `deliverables/presented` Session event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentedEvent {
    pub turn: u64,
    pub call_id: ToolCallId,
    pub files: Vec<PresentedFile>,
}

/// Services used by the scoped delivery tool.
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    fs: Service<Fs>,
    agents: Service<Agents>,
    working_directory: Service<WorkingDirectoryKey>,
}

#[derive(Clone)]
struct PendingDelivery {
    session: Session,
    turn: u64,
    files: Vec<PresentedFile>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn validated_max_files(config: &Config) -> Result<usize, BoxError> {
    let max = config.max_files;
    if !max.is_finite() || max.fract() != 0.0 || !(1.0..=MAX_SAFE_INTEGER as f64).contains(&max) {
        return Err("present requires a positive integer maxFiles".into());
    }
    Ok(max as usize)
}

/// Register the `present` tool for this composition.
pub struct ToolPresent;

impl Plugin for ToolPresent {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let max_files = validated_max_files(&config.get())?;
        let pending = Arc::new(Mutex::new(
            HashMap::<ToolExecutionToken, PendingDelivery>::new(),
        ));

        let parameters = json!({
            "files": {
                "type": "array",
                "required": true,
                "description": "Usually the 1-2 most important deliverables; at most 4 per call.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "path": {
                            "type": "string",
                            "required": true,
                            "description": "Path of an existing regular file. Relative paths use the Session working directory."
                        },
                        "description": {
                            "type": "string",
                            "description": "Brief description for the user."
                        }
                    }
                }
            }
        });
        let output = DefineToolOutput {
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "turn": { "type": "integer", "required": true },
                    "files": {
                        "type": "array",
                        "required": true,
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "path": { "type": "string", "required": true },
                                "description": { "type": "string" }
                            }
                        }
                    }
                }
            }),
            render: Arc::new(|_args, value| {
                let text = value["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|file| file["path"].as_str())
                    .map(|path| format!("Presented {path}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                vec![ContentBlock::text(text)]
            }),
            presentation_meta: None,
        };

        let owner = ctx.clone();
        let fs = deps.fs.clone();
        let agents = deps.agents.clone();
        let working_directory = deps.working_directory.clone();
        let execute_pending = pending.clone();
        let options = DefineToolOptions::new(
            "present",
            DESCRIPTION,
            parameters,
            output,
            move |args, exec| {
                let owner = owner.clone();
                let fs = fs.clone();
                let agents = agents.clone();
                let working_directory = working_directory.clone();
                let pending = execute_pending.clone();
                Box::pin(async move {
                    let Some(calling) = &exec.agent else {
                        return Err(ToolContextUnavailableError::new(
                            "present requires an agent Session",
                        )
                        .into());
                    };
                    let session = calling.session();
                    let boundary = agents.turn_boundary_of(&session)?;
                    if boundary.open_turn_start_seq.is_none() {
                        return Err(ToolContextUnavailableError::new(
                            "present requires an open turn",
                        )
                        .into());
                    }
                    let raw_files = args["files"]
                        .as_array()
                        .expect("define_tool validates the files array");
                    if raw_files.is_empty() || raw_files.len() > max_files {
                        return Err(ToolArgsError::new(vec![format!(
                            "present accepts 1 to {max_files} files"
                        )])
                        .into());
                    }

                    // SHIM(hoh-tools): ToolExecution currently carries the session-level Agent
                    // seam, while WorkingDirectoryService needs the live hoh-agent Agent.
                    let Some(agent) = live_agent(&owner, &session) else {
                        return Err(ToolContextUnavailableError::new(
                            "present requires an agent registered in the agents registry",
                        )
                        .into());
                    };
                    let signal = exec.signal();
                    let cwd = working_directory.ensure(&agent, Some(&signal)).await?;
                    let mut files = Vec::with_capacity(raw_files.len());
                    for raw in raw_files {
                        let path = raw["path"]
                            .as_str()
                            .expect("define_tool validates each file path");
                        if js_trim(path).is_empty() {
                            return Err(ToolArgsError::new(vec![
                                "present requires a non-empty file path".into(),
                            ])
                            .into());
                        }
                        if fs
                            .lstat(path, Some(&cwd), Some(signal.clone()))
                            .await?
                            .is_some_and(|entry| entry.r#type != hoh_fs::FsPathType::File)
                        {
                            return Err(FsError::new(
                                format!("Cannot present {path}: not a regular file"),
                                FsErrorCode::NotRegularFile,
                            )
                            .into());
                        }
                        let target = fs
                            .resolve(
                                path,
                                FsResolveOptions {
                                    cwd: Some(cwd.clone()),
                                    signal: Some(signal.clone()),
                                },
                            )
                            .await?;
                        let Some(info) = fs.stat(&target, Some(signal.clone())).await? else {
                            return Err(FsError::new(
                                format!(
                                    "Cannot present {path}: file not found. Check the path, create the file if needed, and retry."
                                ),
                                FsErrorCode::NotFound,
                            )
                            .into());
                        };
                        if info.r#type != FsType::File {
                            return Err(FsError::new(
                                format!("Cannot present {path}: not a regular file"),
                                FsErrorCode::NotRegularFile,
                            )
                            .into());
                        }
                        files.push(PresentedFile {
                            path: fs.process_path(&target),
                            description: raw
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        });
                    }
                    signal.throw_if_aborted()?;
                    lock(&pending).insert(
                        exec.token,
                        PendingDelivery {
                            session,
                            turn: boundary.last_turn,
                            files: files.clone(),
                        },
                    );
                    Ok(json!({ "turn": boundary.last_turn, "files": files }))
                })
            },
        );
        deps.tools.register(
            &ctx,
            define_tool(options)?,
            ToolRegistrationOptions::default(),
        )?;

        ctx.on_contained::<ToolsResult>(move |event| {
            let delivery = lock(&pending).remove(&event.exec.token);
            let Some(delivery) = delivery else {
                return Ok(());
            };
            if event.result.is_error() {
                return Ok(());
            }
            let data = PresentedEvent {
                turn: delivery.turn,
                call_id: event.exec.call_id.clone(),
                files: delivery.files,
            };
            delivery
                .session
                .append("deliverables/presented", serde_json::to_value(data)?, None)?;
            Ok(())
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_deployment_limits_before_registration() {
        for max_files in [0.0, 1.5, f64::INFINITY] {
            let error = validated_max_files(&Config { max_files }).unwrap_err();
            assert_eq!(
                error.to_string(),
                "present requires a positive integer maxFiles"
            );
        }
        assert_eq!(validated_max_files(&Config::default()).unwrap(), 8);
        assert_eq!(
            validated_max_files(&Config {
                max_files: MAX_SAFE_INTEGER as f64,
            })
            .unwrap(),
            MAX_SAFE_INTEGER as usize
        );
    }
}
