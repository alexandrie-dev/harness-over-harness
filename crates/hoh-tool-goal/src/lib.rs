//! Model-facing `get_goal`, `create_goal`, and `update_goal` controls over the
//! persisted same-session goal domain.

pub mod authority;
pub mod wrapup;

use std::{error::Error, fmt, sync::Arc};

use authority::{
    GoalToolAuthority, completion_authority, goal_tool_execution, require_direct_human,
};
use hoh_error::{BoxError, HarnessError};
use hoh_goal::{
    CreateGoalRequest, EditGoalRequest, GoalActivation, GoalBlockReason, GoalPhase, GoalRef,
    GoalService, GoalView, Goals,
};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{
    ContentBlock, MessageSource, NewUserMessage, bound_context_summary, create_user_message,
};
use hoh_system_prompt::{PromptSection, PromptSectionOrderName, SystemPrompt, SystemPromptKey};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, GenericCallView, ToolCallKind, ToolCallView,
    ToolRegistrationOptions, Tools, ToolsService, define_tool,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use wrapup::render_wrapup_context;

/// Cordis plugin name.
pub const NAME: &str = "tool-goal";
/// Required service names, in the source declaration order.
pub const INJECT: &[&str] = &["agents", "goals", "tools", "systemPrompt"];

const CREATE_DESCRIPTION: &str = "Create a persisted goal that keeps this session working across automatic continuation rounds. Use it when the direct human request is a long-running objective, even if the user did not say \"goal\"; not for single-turn work.";
const GET_DESCRIPTION: &str =
    "Read the current session goal, including the id and revision that update_goal requires.";
const UPDATE_DESCRIPTION: &str = "Update the current goal.";

/// Model policy and hard lower bounds for goal-state updates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// Minimum admitted goal rounds before the model may self-report `blocked`.
    #[schemars(
        default = "default_blocked_after",
        range(min = 1, max = 9007199254740991u64)
    )]
    pub blocked_after_consecutive_rounds: u64,
}

const fn default_blocked_after() -> u64 {
    3
}

impl Default for Config {
    fn default() -> Self {
        Self {
            blocked_after_consecutive_rounds: default_blocked_after(),
        }
    }
}

#[derive(Debug)]
struct ConfigError;

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("blockedAfterConsecutiveRounds must be a positive safe integer")
    }
}

impl Error for ConfigError {}

/// Services used by `inject = ['agents', 'goals', 'tools', 'systemPrompt']`.
#[derive(Inject)]
pub struct Deps {
    agents: Service<hoh_agent::Agents>,
    goals: Service<Goals>,
    tools: Service<Tools>,
    system_prompt: Service<SystemPromptKey>,
}

/// Function-plugin counterpart of `@deepseek-ai/dsh-tool-goal`.
pub struct ToolGoal;

impl Plugin for ToolGoal {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        apply(
            ctx,
            (*deps.agents).clone(),
            (*deps.goals).clone(),
            (*deps.tools).clone(),
            &deps.system_prompt,
            (*config.get()).clone(),
        )
    }
}

fn guidance(blocked_after: u64) -> String {
    format!(
        "create_goal may infer goal intent from a direct human request in any language. After session resume or fork, an active goal is disarmed: when a human asks to continue or resume in any wording or language, use update_goal action resume to rearm it. Mark complete only when the objective is actually achieved. Mark blocked only after the same blocking condition persists for at least {blocked_after} consecutive goal rounds, and report that concrete condition in blocked_reason; difficulty, uncertainty, or useful remaining work is not blocked."
    )
}

fn resolve_config(config: Config) -> Result<u64, BoxError> {
    if config.blocked_after_consecutive_rounds == 0
        || config.blocked_after_consecutive_rounds > hoh_session::MAX_SAFE_INTEGER
    {
        return Err(Box::new(ConfigError));
    }
    Ok(config.blocked_after_consecutive_rounds)
}

fn goal_value(goal: Option<&GoalView>) -> Value {
    let Some(goal) = goal else {
        return json!({ "goal": null });
    };
    let mut value = Map::from_iter([
        ("id".into(), Value::String(goal.goal.id.as_str().into())),
        ("revision".into(), Value::from(goal.goal.revision)),
        (
            "objective".into(),
            Value::String(goal.goal.objective.clone()),
        ),
        ("phase".into(), Value::String(goal.goal.phase.to_string())),
        ("roundsStarted".into(), Value::from(goal.rounds_started)),
        (
            "maxGoalRounds".into(),
            Value::from(goal.goal.max_goal_rounds),
        ),
    ]);
    if let Some(reason) = &goal.goal.blocked_reason {
        value.insert(
            "blockedReason".into(),
            json!({ "code": reason.code, "message": reason.message }),
        );
    }
    json!({
        "goal": Value::Object(value),
        "activation": match goal.activation {
            GoalActivation::Armed => "armed",
            GoalActivation::Disarmed => "disarmed",
        },
    })
}

fn goal_value_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "properties": { "goal": { "type": "null", "required": true } }
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "goal": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": true,
                        "properties": {
                            "id": { "type": "string", "required": true },
                            "revision": { "type": "integer", "required": true },
                            "objective": { "type": "string", "required": true },
                            "phase": { "type": "string", "required": true, "enum": ["active", "paused", "blocked", "complete"] },
                            "roundsStarted": { "type": "integer", "required": true },
                            "maxGoalRounds": { "type": "integer", "required": true },
                            "blockedReason": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "code": { "type": "string", "required": true },
                                    "message": { "type": "string", "required": true }
                                }
                            }
                        }
                    },
                    "activation": { "type": "string", "required": true, "enum": ["armed", "disarmed"] }
                }
            }
        ]
    })
}

fn output() -> DefineToolOutput {
    DefineToolOutput {
        schema: goal_value_schema(),
        render: Arc::new(|_, value| {
            vec![ContentBlock::text(hoh_util_values::js_json_stringify(
                value,
            ))]
        }),
        presentation_meta: None,
    }
}

fn present(title: impl Into<String>, kind: ToolCallKind, raw_input: Option<Value>) -> ToolCallView {
    ToolCallView::Generic(GenericCallView {
        title: title.into(),
        kind: Some(kind),
        raw_input,
        content: None,
        locations: None,
    })
}

fn positive_safe_integer(value: &Value) -> Option<u64> {
    if let Some(number) = value.as_u64() {
        return (number > 0 && number <= hoh_session::MAX_SAFE_INTEGER).then_some(number);
    }
    let number = value.as_f64()?;
    (number.is_finite()
        && number.fract() == 0.0
        && number >= 1.0
        && number < (hoh_session::MAX_SAFE_INTEGER as f64 + 1.0))
        .then_some(number as u64)
}

fn nonnegative_safe_integer(value: &Value) -> Option<u64> {
    if let Some(number) = value.as_u64() {
        return (number <= hoh_session::MAX_SAFE_INTEGER).then_some(number);
    }
    let number = value.as_f64()?;
    (number.is_finite()
        && number.fract() == 0.0
        && number >= 0.0
        && number < (hoh_session::MAX_SAFE_INTEGER as f64 + 1.0))
        .then_some(number as u64)
}

fn max_rounds_for_domain(args: &Value, key: &str, zero_is_absent: bool) -> Option<u64> {
    let value = args.get(key)?;
    if zero_is_absent && nonnegative_safe_integer(value) == Some(0) {
        return None;
    }
    // The tool boundary accepts JSON numbers that the typed domain request cannot represent.
    // Map each invalid value to the domain-invalid sentinel zero so `GoalService` rejects it
    // under its mutation gate, after the same objective/CAS checks as the TypeScript source.
    Some(positive_safe_integer(value).unwrap_or(0))
}

fn goal_ref(args: &Value) -> Result<GoalRef, BoxError> {
    let goal_id = args
        .get("goal_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let revision = args.get("revision").and_then(positive_safe_integer);
    if goal_id.is_empty() || hoh_util_values::js_trim(goal_id) != goal_id || revision.is_none() {
        return Err(Box::new(HarnessError::new(
            "goal_id must be non-empty and revision must be a positive safe integer",
            "GOAL_TOOL_INVALID_UPDATE",
        )));
    }
    Ok(GoalRef {
        id: goal_id.into(),
        revision: revision.unwrap_or_default(),
    })
}

fn tool_goal_source(action: &str, objective: &str) -> MessageSource {
    MessageSource {
        kind: "tool-goal".into(),
        data: Map::from_iter([
            ("form".into(), Value::String("notice".into())),
            (
                "summary".into(),
                Value::String(bound_context_summary(&format!("{action}: {objective}"))),
            ),
        ]),
    }
}

fn create_parameters() -> Value {
    json!({
        "objective": {
            "type": "string",
            "required": true,
            "description": "The concrete completion objective inferred from the direct human request."
        },
        "max_goal_rounds": {
            "type": "number",
            "description": "Optional positive safe-integer limit on automatic continuation rounds."
        }
    })
}

fn update_parameters() -> Value {
    json!({
        "goal_id": { "type": "string", "required": true, "description": "Exact id returned by get_goal." },
        "revision": { "type": "number", "required": true, "description": "Exact positive revision returned by get_goal." },
        "action": {
            "type": "string",
            "required": true,
            "enum": ["edit", "pause", "resume", "complete", "blocked"],
            "description": "edit, pause, and resume require a direct top-level human request. complete and blocked are also allowed during an automatic continuation of this goal; blocked is rejected before the configured minimum round count."
        },
        "objective": { "type": "string", "description": "Replacement objective; valid only with action edit." },
        "max_goal_rounds": { "type": "number", "description": "Replacement cap; valid only with action edit." },
        "blocked_reason": {
            "type": "string",
            "description": "Required only with action blocked: the concrete condition that persisted across rounds and blocks progress."
        }
    })
}

/// Register the three Codex-shaped goal tools and their shared policy section.
pub fn apply(
    ctx: Context,
    agents: hoh_agent::AgentRegistry,
    goals: GoalService,
    tools: ToolsService,
    system_prompt: &SystemPrompt,
    config: Config,
) -> Result<(), BoxError> {
    let blocked_after = resolve_config(config)?;
    system_prompt.section(
        &ctx,
        PromptSection {
            name: "tool:goal".into(),
            order: SystemPrompt::get_section_order(PromptSectionOrderName::ToolGoal),
            text: guidance(blocked_after).into(),
            ..PromptSection::default()
        },
    )?;

    let get_agents = agents.clone();
    let get_goals = goals.clone();
    let get_definition = define_tool(DefineToolOptions::new(
        "get_goal",
        GET_DESCRIPTION,
        json!({}),
        output(),
        move |_, exec| {
            let agents = get_agents.clone();
            let goals = get_goals.clone();
            Box::pin(async move {
                let execution = goal_tool_execution(&agents, &exec)?;
                let goal = goals.get(&execution.agent)?;
                Ok(goal_value(goal.as_ref()))
            })
        },
    ))?;
    let mut get_definition = get_definition;
    get_definition.present_call = Some(Arc::new(|_| {
        Some(present("Read current goal", ToolCallKind::Read, None))
    }));
    tools.register(&ctx, get_definition, ToolRegistrationOptions::default())?;

    let create_agents = agents.clone();
    let create_goals = goals.clone();
    let mut create_definition = DefineToolOptions::new(
        "create_goal",
        CREATE_DESCRIPTION,
        create_parameters(),
        output(),
        move |args, exec| {
            let agents = create_agents.clone();
            let goals = create_goals.clone();
            Box::pin(async move {
                let execution = goal_tool_execution(&agents, &exec)?;
                require_direct_human(&agents, &execution)?;
                let objective = args["objective"].as_str().unwrap_or_default().to_owned();
                let max_goal_rounds = max_rounds_for_domain(&args, "max_goal_rounds", false);
                let goal = goals.create(
                    &execution.agent,
                    CreateGoalRequest {
                        objective,
                        max_goal_rounds,
                    },
                )?;
                Ok(goal_value(Some(&goal)))
            })
        },
    );
    create_definition.present_call = Some(Arc::new(|args| {
        Some(present(
            "Create goal",
            ToolCallKind::Other,
            args.get("objective").cloned(),
        ))
    }));
    tools.register(
        &ctx,
        define_tool(create_definition)?,
        ToolRegistrationOptions::default(),
    )?;

    let update_agents = agents;
    let update_goals = goals;
    let mut update_definition = DefineToolOptions::new(
        "update_goal",
        UPDATE_DESCRIPTION,
        update_parameters(),
        output(),
        move |args, exec| {
            let agents = update_agents.clone();
            let goals = update_goals.clone();
            Box::pin(async move {
                let execution = goal_tool_execution(&agents, &exec)?;
                let reference = goal_ref(&args)?;
                let action = args["action"].as_str().unwrap_or_default();
                let goal = match action {
                    "edit" => {
                        require_direct_human(&agents, &execution)?;
                        let objective = args
                            .get("objective")
                            .and_then(Value::as_str)
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned);
                        let max_goal_rounds = max_rounds_for_domain(&args, "max_goal_rounds", true);
                        goals.edit(
                            &execution.agent,
                            &reference,
                            EditGoalRequest {
                                objective,
                                max_goal_rounds,
                            },
                        )?
                    }
                    "pause" | "resume" => {
                        require_direct_human(&agents, &execution)?;
                        if action == "resume"
                            && goals.get(&execution.agent)?.is_some_and(|goal| {
                                goal.goal.id == reference.id
                                    && goal.goal.revision == reference.revision
                                    && goal.goal.phase == GoalPhase::Paused
                            })
                        {
                            return Err(Box::new(HarnessError::new(
                                "the model cannot resume a paused goal; the user must resume it",
                                "GOAL_TOOL_RESUME_PAUSED",
                            )) as BoxError);
                        }
                        if action == "pause" {
                            goals.pause(&execution.agent, &reference)?
                        } else {
                            goals.resume(&execution.agent, &reference)?
                        }
                    }
                    "complete" | "blocked" => {
                        let authority = completion_authority(&agents, &goals, &execution)?;
                        let reason = args.get("blocked_reason").and_then(Value::as_str);
                        if action == "blocked"
                            && reason
                                .is_none_or(|reason| hoh_util_values::js_trim(reason).is_empty())
                        {
                            return Err(Box::new(HarnessError::new(
                                "blocked_reason is required with action blocked",
                                "GOAL_TOOL_INVALID_UPDATE",
                            )) as BoxError);
                        }
                        if action == "blocked"
                            && matches!(&authority, GoalToolAuthority::GoalRound { goal } if goal.rounds_started < blocked_after)
                        {
                            let GoalToolAuthority::GoalRound { goal } = &authority else {
                                unreachable!("matched goal-round authority")
                            };
                            return Err(Box::new(HarnessError::new(
                                format!(
                                    "blocked requires at least {blocked_after} consecutive goal rounds; current round is {}",
                                    goal.rounds_started
                                ),
                                "GOAL_TOOL_BLOCK_THRESHOLD",
                            )) as BoxError);
                        }
                        let goal = if action == "complete" {
                            goals.complete(&execution.agent, &reference)?
                        } else {
                            goals.block(
                                &execution.agent,
                                &reference,
                                GoalBlockReason {
                                    code: "model-reported".into(),
                                    message: reason.unwrap_or_default().into(),
                                },
                            )?
                        };
                        if matches!(authority, GoalToolAuthority::GoalRound { .. }) {
                            exec.defer_context(create_user_message(NewUserMessage {
                                content: render_wrapup_context(
                                    &goal.goal.objective,
                                    (action == "blocked").then_some(reason.unwrap_or_default()),
                                ),
                                source: tool_goal_source(action, &goal.goal.objective),
                            }));
                        }
                        goal
                    }
                    _ => unreachable!("update_goal action was schema-validated"),
                };
                Ok(goal_value(Some(&goal)))
            })
        },
    );
    update_definition.present_call = Some(Arc::new(|args| {
        let action = args.get("action")?.as_str()?;
        let title = if action == "blocked" {
            "Mark goal".to_owned()
        } else {
            let mut chars = action.chars();
            format!("{}{} goal", chars.next()?.to_uppercase(), chars.as_str())
        };
        let raw_input = if action == "blocked" {
            args.get("blocked_reason")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.into()))
        } else if action == "edit" {
            args.get("objective")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.into()))
                .or_else(|| {
                    args.get("max_goal_rounds")
                        .filter(|value| nonnegative_safe_integer(value) != Some(0))
                        .cloned()
                })
        } else {
            None
        }
        .or_else(|| args.get("goal_id").cloned());
        Some(present(title, ToolCallKind::Other, raw_input))
    }));
    tools.register(
        &ctx,
        define_tool(update_definition)?,
        ToolRegistrationOptions::default(),
    )?;
    Ok(())
}
