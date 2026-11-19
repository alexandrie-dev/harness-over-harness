//! Todo-list tools for model-visible planning and progress tracking.
//!
//! Each successful call appends a complete `todo/write` snapshot to the calling agent's Session.
//! The `todos` projection is last-write-wins and clears when the next turn starts.

use std::{collections::HashSet, sync::Arc};

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::ContentBlock;
use hoh_session::SessionLogOffset;
use hoh_session_projection::{
    ProjectionDefinition, ProjectionWire, SessionProjectionRegistry, SessionProjections,
};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, GenericCallView, ToolArgsError, ToolCallKind,
    ToolCallView, ToolContextUnavailableError, ToolRegistrationOptions, Tools, ToolsService,
    define_tool,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Session event carrying one complete list replacement.
pub const TODO_WRITE_EVENT: &str = "todo/write";
/// Client-visible key of the standing-plan projection.
pub const TODOS_PROJECTION: &str = "todos";

/// Lifecycle state of one todo entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// One entry in an agent's todo list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TodoItem {
    /// Short imperative task description shown in the UI.
    pub content: String,
    /// Current lifecycle state.
    pub status: TodoStatus,
}

/// Required deployment policy for simultaneously active work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// Whether several todos may be `in_progress` at once.
    pub allow_parallel_in_progress: bool,
}

/// Services used by `inject = ['tools', 'sessionProjections']`.
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    projections: Service<SessionProjections>,
}

/// Function-plugin counterpart of `@deepseek-ai/dsh-tool-todo`.
pub struct ToolTodo;

impl Plugin for ToolTodo {
    const NAME: &'static str = "tool-todo";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        apply(
            ctx,
            (*deps.tools).clone(),
            (*deps.projections).clone(),
            (*config.get()).clone(),
        )
        .await
    }
}

const DESCRIPTION_HEAD: &str = "Record and update a task list to plan multi-step work and show progress; skip it for trivial single-step tasks. Add one todo per concrete step before you start. ";
const DESCRIPTION_PARALLEL: &str = "While work remains, keep the todos being worked on `in_progress`, several only when work runs in parallel. ";
const DESCRIPTION_SINGLE: &str = "While work remains, keep exactly one todo `in_progress`. ";
const DESCRIPTION_TAIL: &str = "Mark each todo `completed` as soon as it is done.";

fn describe(allow_parallel: bool) -> String {
    [
        DESCRIPTION_HEAD,
        if allow_parallel {
            DESCRIPTION_PARALLEL
        } else {
            DESCRIPTION_SINGLE
        },
        DESCRIPTION_TAIL,
    ]
    .concat()
}

fn parse_projection_state(value: Option<&Value>) -> Result<Arc<Option<Vec<TodoItem>>>, BoxError> {
    let value = value
        .ok_or_else(|| -> BoxError { "todos projection state must be an array or null".into() })?;
    let state = serde_json::from_value(value.clone())?;
    Ok(Arc::new(state))
}

/// Projection definition for the current standing todo plan.
pub fn todo_projection_definition() -> ProjectionDefinition<Option<Vec<TodoItem>>> {
    ProjectionDefinition {
        key: TODOS_PROJECTION.into(),
        state_schema: Arc::new(parse_projection_state),
        init: Arc::new(|_, _: SessionLogOffset| Ok(Arc::new(None))),
        apply: Arc::new(|state, event| match event.event_type() {
            TODO_WRITE_EVENT => {
                let todos = event
                    .data()
                    .get("todos")
                    .ok_or_else(|| -> BoxError { "todo/write event is missing todos".into() })?;
                Ok(Arc::new(Some(serde_json::from_value(todos.clone())?)))
            }
            "turn/start" => Ok(Arc::new(None)),
            _ => Ok(state.clone()),
        }),
        wire: Some(ProjectionWire {
            view_schema: Arc::new(|value| {
                let parsed = parse_projection_state(value)?;
                Ok(Arc::new(serde_json::to_value(parsed.as_ref())?))
            }),
            view: Arc::new(|state| Ok(Some(Arc::new(serde_json::to_value(state.as_ref())?)))),
            summary: false,
        }),
        state_version: 2,
    }
}

fn parameters() -> Value {
    json!({
        "todos": {
            "type": "array",
            "required": true,
            "description": "The COMPLETE task list, replacing any previous list.",
            "items": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "content": { "type": "string", "required": true, "description": "What the task is — a short imperative line." },
                    "status": {
                        "type": "string",
                        "required": true,
                        "enum": ["pending", "in_progress", "completed"],
                        "description": "pending (not started) | in_progress (now) | completed (done)."
                    }
                }
            }
        }
    })
}

fn output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "todos": {
                "type": "array",
                "required": true,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "content": { "type": "string", "required": true },
                        "status": {
                            "type": "string",
                            "required": true,
                            "enum": ["pending", "in_progress", "completed"]
                        }
                    }
                }
            },
            "counts": {
                "type": "object",
                "additionalProperties": false,
                "required": true,
                "properties": {
                    "pending": { "type": "integer", "required": true },
                    "inProgress": { "type": "integer", "required": true },
                    "completed": { "type": "integer", "required": true }
                }
            }
        }
    })
}

fn is_ecmascript_trim_character(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            ..='\u{000D}'
                | '\u{0020}'
                | '\u{00A0}'
                | '\u{1680}'
                | '\u{2000}'..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

fn trim_ecmascript(value: &str) -> &str {
    value.trim_matches(is_ecmascript_trim_character)
}

fn to_todo_list(raw: &Value, allow_parallel: bool) -> Result<Vec<TodoItem>, BoxError> {
    let raw: Vec<TodoItem> = serde_json::from_value(raw.clone())?;
    let mut todos = Vec::with_capacity(raw.len());
    let mut seen = HashSet::new();
    let mut active = 0;
    for (index, mut item) in raw.into_iter().enumerate() {
        item.content = trim_ecmascript(&item.content).to_owned();
        if item.content.is_empty() {
            return Err(ToolArgsError::new(vec![format!(
                "todos[{index}].content must be a non-empty string"
            )])
            .into());
        }
        if !seen.insert(item.content.clone()) {
            let content = serde_json::to_string(&item.content)?;
            return Err(ToolArgsError::new(vec![format!(
                "todos[{index}].content duplicates {content}"
            )])
            .into());
        }
        if item.status == TodoStatus::InProgress {
            active += 1;
        }
        todos.push(item);
    }
    if !allow_parallel && active > 1 {
        return Err(ToolArgsError::new(vec![format!(
            "at most one task may be in_progress (got {active})"
        )])
        .into());
    }
    Ok(todos)
}

fn count(todos: &[TodoItem], status: TodoStatus) -> usize {
    todos.iter().filter(|todo| todo.status == status).count()
}

fn tool_definition(allow_parallel: bool) -> Result<hoh_tools::ToolDefinition, BoxError> {
    let output = DefineToolOutput {
        schema: output_schema(),
        render: Arc::new(|_, value| {
            let counts = &value["counts"];
            vec![ContentBlock::text(format!(
                "Updated todo list: {} pending, {} in progress, {} completed.",
                counts["pending"].as_u64().unwrap_or_default(),
                counts["inProgress"].as_u64().unwrap_or_default(),
                counts["completed"].as_u64().unwrap_or_default(),
            ))]
        }),
        presentation_meta: None,
    };
    let mut options = DefineToolOptions::new(
        "todo_write",
        describe(allow_parallel),
        parameters(),
        output,
        move |args, exec| {
            Box::pin(async move {
                let todos = to_todo_list(&args["todos"], allow_parallel)?;
                let agent = exec.agent.as_ref().ok_or_else(|| -> BoxError {
                    ToolContextUnavailableError::new("todo_write requires an owning agent session")
                        .into()
                })?;
                agent
                    .session()
                    .append(TODO_WRITE_EVENT, json!({ "todos": todos }), None)?;
                Ok(json!({
                    "todos": todos,
                    "counts": {
                        "pending": count(&todos, TodoStatus::Pending),
                        "inProgress": count(&todos, TodoStatus::InProgress),
                        "completed": count(&todos, TodoStatus::Completed)
                    }
                }))
            })
        },
    );
    options.present_call = Some(Arc::new(|args| {
        Some(ToolCallView::Generic(GenericCallView {
            title: "Update todo list".into(),
            kind: Some(ToolCallKind::Other),
            raw_input: Some(args.get("todos").cloned().unwrap_or(Value::Null)),
            content: None,
            locations: None,
        }))
    }));
    define_tool(options)
}

/// Register the projection and `todo_write` definition for one plugin activation.
pub async fn apply(
    ctx: Context,
    tools: ToolsService,
    projections: SessionProjectionRegistry,
    config: Config,
) -> Result<(), BoxError> {
    projections
        .register(&ctx, todo_projection_definition())
        .await?;
    tools.register(
        &ctx,
        tool_definition(config.allow_parallel_in_progress)?,
        ToolRegistrationOptions::default(),
    )?;
    Ok(())
}
