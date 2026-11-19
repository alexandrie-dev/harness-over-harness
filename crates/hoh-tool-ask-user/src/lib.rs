//! Model-facing `ask_user_question` tool over the human-question capability.
//!
//! The legacy definition blocks until a provider submits a batch. The opt-in timed definition can
//! continue with a durable pending result while the same question remains answerable in the UI.

mod shim;

use std::{collections::HashSet, sync::Arc};

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::ContentBlock;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolArgsError, ToolContextUnavailableError,
    ToolRegistrationOptions, Tools, ToolsService, define_tool,
};
use hoh_user_questions::{
    AskUserQuestionAnswer, AskUserQuestionItem, AskUserQuestionOption, AskUserQuestionRequest,
    TIMED_WAIT_PARAMETER, TimedUserQuestionResult, UserQuestionError, UserQuestionService,
    UserQuestionWait, UserQuestions,
};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value, json};

/// Cordis row selecting the tool schema and its default foreground wait.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The blocking definition used by the bundled presets.
    #[default]
    Legacy,
    /// The opt-in definition that may return a pending result.
    Timed,
}

/// Plugin configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Tool definition selected by this row. Defaults to [`Mode::Legacy`].
    pub mode: Mode,
    /// Foreground wait in seconds. `-1` waits indefinitely; the default is 120.
    #[serde(deserialize_with = "deserialize_timeout")]
    pub timeout: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Legacy,
            timeout: 120,
        }
    }
}

impl JsonSchema for Config {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Config".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::try_from(json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "mode": { "enum": ["legacy", "timed"], "default": "legacy" },
                "timeout": {
                    "oneOf": [
                        { "const": -1 },
                        { "type": "integer", "minimum": 1, "maximum": 2_147_483 }
                    ],
                    "default": 120
                }
            }
        }))
        .unwrap_or_else(|_| Schema::from(true))
    }
}

/// Services used by `inject = ['tools', 'userQuestions']`.
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    user_questions: Service<UserQuestions>,
}

/// Function-plugin counterpart of `@deepseek-ai/dsh-tool-ask-user`.
pub struct ToolAskUser;

impl Plugin for ToolAskUser {
    const NAME: &'static str = "tool-ask-user";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        apply(
            ctx,
            (*deps.tools).clone(),
            (*deps.user_questions).clone(),
            (*config.get()).clone(),
        )
    }
}

/// Model-facing meaning of an explicitly skipped answer item.
pub const SKIPPED_ANSWER_GUIDANCE: &str = "An answer item with empty selected and no custom means the user skipped that question: continue without it, do not ask it again, and do not treat the skip as permission.";

const LEGACY_DESCRIPTION: &str = "Ask the user a concise question when you need confirmation, a choice, or missing information before proceeding. An answer item with empty selected and no custom means the user skipped that question: continue without it, do not ask it again, and do not treat the skip as permission.";

const TIMED_DESCRIPTION: &str = "Ask brief, direct, self-contained questions about missing information, preferences, or decisions. Use user-facing terms; assume no knowledge of background work or internal names. Use distinct stable question IDs. An answer item with empty selected and no custom means the user skipped that question: continue without it, do not ask it again, and do not treat the skip as permission. A pending result instead means no answer batch arrived before the timeout and the user can still answer.";

const PENDING_NOTICE: &str = "No answer batch arrived before the timeout. This is pending, not a skipped answer. Continue useful independent work. The user can still answer; their reply will be a user message identified as answer_to_pending_question with this callId and the original questions. Do not treat this as permission.";

#[derive(Clone, Debug, Deserialize)]
struct ToolQuestionOption {
    label: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ToolQuestion {
    id: String,
    question: String,
    #[serde(default)]
    header: Option<String>,
    #[serde(default)]
    options: Option<Vec<ToolQuestionOption>>,
    #[serde(default)]
    multi_select: Option<bool>,
}

#[derive(Default)]
enum TimeoutArgument {
    #[default]
    Missing,
    Present(Value),
}

impl<'de> Deserialize<'de> for TimeoutArgument {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Value::deserialize(deserializer).map(Self::Present)
    }
}

#[derive(Deserialize)]
struct ToolArguments {
    questions: Vec<ToolQuestion>,
    #[serde(default)]
    timeout: TimeoutArgument,
}

/// Register the definition selected by `config`.
pub fn apply(
    ctx: Context,
    tools: ToolsService,
    user_questions: UserQuestionService,
    config: Config,
) -> Result<(), BoxError> {
    validate_timeout(config.timeout)?;
    match config.mode {
        Mode::Legacy => register_legacy(ctx, tools, user_questions),
        Mode::Timed => register_timed_ask_user(ctx, tools, user_questions, config.timeout),
    }
}

fn question_schema(description: &str) -> Value {
    json!({
        "type": "array",
        "required": true,
        "description": description,
        "items": {
            "type": "object",
            "additionalProperties": true,
            "properties": {
                "id": { "type": "string", "required": true, "description": "Stable id for this question; echoed in the answer." },
                "question": { "type": "string", "required": true, "description": "The specific question to ask the user." },
                "header": {
                    "type": "string",
                    "description": "Optional short heading for the question, such as \"Confirm\" or \"Choose Mode\"."
                },
                "options": {
                    "type": "array",
                    "description": "Optional choices to show the user. If you recommend one, put it first and append \"(Recommended)\" to that label.",
                    "items": {
                        "type": "object",
                        "additionalProperties": true,
                        "properties": {
                            "label": { "type": "string", "required": true, "description": "Short user-facing option label." },
                            "description": { "type": "string", "description": "One sentence explaining the tradeoff or impact." }
                        }
                    }
                },
                "multi_select": {
                    "type": "boolean",
                    "description": "Whether the user may select more than one option. Defaults to false."
                }
            }
        }
    })
}

fn answer_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "answers": {
                "type": "array",
                "required": true,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "id": { "type": "string", "required": true },
                        "selected": { "type": "array", "required": true, "items": { "type": "string" } },
                        "custom": { "type": "string" }
                    }
                }
            }
        }
    })
}

fn timed_answer_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "pending": {
                        "type": "boolean", "enum": [true], "required": true,
                        "description": "True when the foreground wait expired before the user submitted an answer batch. The questions remain answerable; this is not a skipped answer."
                    },
                    "callId": { "type": "string", "required": true, "description": "Tool call whose unanswered questions remain pending." },
                    "message": { "type": "string", "required": true, "description": "How to continue while the questions stay answerable." }
                }
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "answers": {
                        "type": "array", "required": true,
                        "description": "Submitted answer batch with one item per question. A skipped question has empty selected and no custom; unlike pending, the user has completed the batch.",
                        "items": {
                            "type": "object", "additionalProperties": false,
                            "properties": {
                                "id": { "type": "string", "required": true, "description": "Stable question id echoed from the request." },
                                "selected": {
                                    "type": "array", "required": true, "items": { "type": "string" },
                                    "description": "Selected option labels. Empty with no custom means the user explicitly skipped this question."
                                },
                                "custom": { "type": "string", "description": "Optional free-form answer; omitted for a skipped question." }
                            }
                        }
                    }
                }
            }
        ]
    })
}

fn output(schema: Value) -> DefineToolOutput {
    DefineToolOutput {
        schema,
        render: Arc::new(|_, value| {
            vec![ContentBlock::text(hoh_util_values::js_json_stringify(
                value,
            ))]
        }),
        presentation_meta: None,
    }
}

fn parse_arguments(args: Value) -> Result<ToolArguments, BoxError> {
    serde_json::from_value(args)
        .map_err(|error| Box::new(ToolArgsError::new(vec![error.to_string()])) as BoxError)
}

fn questions_for_service(questions: Vec<ToolQuestion>) -> Vec<AskUserQuestionItem> {
    questions
        .into_iter()
        .map(|question| AskUserQuestionItem {
            id: question.id,
            question: question.question,
            detail: None,
            header: question.header,
            options: question.options.map(|options| {
                options
                    .into_iter()
                    .map(|option| AskUserQuestionOption {
                        label: option.label,
                        description: option.description,
                    })
                    .collect()
            }),
            multi_select: question.multi_select,
            intent: None,
        })
        .collect()
}

fn answer_result(result: AskUserQuestionAnswer) -> Value {
    json!({ "answers": result.answers })
}

fn validate_timeout(timeout: i64) -> Result<i64, BoxError> {
    if timeout != -1 && !(1..=2_147_483).contains(&timeout) {
        return Err(ToolArgsError::new(vec![
            "timeout must be -1 or a positive integer up to 2147483 seconds".into(),
        ])
        .into());
    }
    Ok(timeout)
}

fn validate_timeout_value(timeout: &Value) -> Result<i64, BoxError> {
    let valid = timeout
        .as_f64()
        .filter(|number| number.is_finite() && number.fract() == 0.0)
        .filter(|number| *number == -1.0 || (1.0..=2_147_483.0).contains(number));
    valid.map(|number| number as i64).ok_or_else(|| {
        ToolArgsError::new(vec![
            "timeout must be -1 or a positive integer up to 2147483 seconds".into(),
        ])
        .into()
    })
}

fn deserialize_timeout<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    validate_timeout_value(&value).map_err(de::Error::custom)
}

fn validate_question_ids(questions: &[ToolQuestion]) -> Result<(), BoxError> {
    let mut ids = HashSet::new();
    for question in questions {
        if !ids.insert(&question.id) {
            let id = serde_json::to_string(&question.id)?;
            return Err(ToolArgsError::new(vec![format!(
                "question id {id} must be unique within this call"
            )])
            .into());
        }
    }
    Ok(())
}

fn live_agent(
    ctx: &Context,
    agent: &Option<Arc<dyn hoh_tools::Agent>>,
) -> Result<Option<Arc<dyn hoh_agent::Agent>>, BoxError> {
    agent
        .as_ref()
        .map(|agent| {
            shim::live_agent(ctx, agent).ok_or_else(|| {
                UserQuestionError::new(
                    "human interaction requires the exact live calling agent when an agent is supplied",
                    "CALLER_NOT_LIVE",
                )
                .into()
            })
        })
        .transpose()
}

fn register_legacy(
    ctx: Context,
    tools: ToolsService,
    user_questions: UserQuestionService,
) -> Result<(), BoxError> {
    let parameters = json!({
        "questions": question_schema("Questions to ask the user before continuing.")
    });
    let owner = ctx.clone();
    let options = DefineToolOptions::new(
        "ask_user_question",
        LEGACY_DESCRIPTION,
        parameters,
        output(answer_schema()),
        move |args, exec| {
            let owner = owner.clone();
            let user_questions = user_questions.clone();
            Box::pin(async move {
                let args = parse_arguments(args)?;
                let result = user_questions
                    .ask(AskUserQuestionRequest {
                        questions: questions_for_service(args.questions),
                        agent: live_agent(&owner, &exec.agent)?,
                        signal: Some(exec.signal()),
                        wait: None,
                    })
                    .await?;
                Ok(answer_result(result))
            })
        },
    );
    tools.register(
        &ctx,
        define_tool(options)?,
        ToolRegistrationOptions::default(),
    )?;
    Ok(())
}

/// Register the opt-in timed definition with `default_timeout` seconds of foreground wait.
pub fn register_timed_ask_user(
    ctx: Context,
    tools: ToolsService,
    user_questions: UserQuestionService,
    default_timeout: i64,
) -> Result<(), BoxError> {
    let default_timeout = validate_timeout(default_timeout)?;
    let mut parameters = Map::new();
    parameters.insert(
        "questions".into(),
        question_schema("Questions to ask the user."),
    );
    parameters.insert(
        TIMED_WAIT_PARAMETER.into(),
        json!({
            // The model-facing schema is restored to `integer` below. Runtime validation stays in
            // this crate so JSON numbers such as `3.0` follow JavaScript `Number.isInteger`, and
            // every invalid numeric representation receives the source's timeout-specific error.
            "type": "json",
            "description": format!(
                "Wait seconds for the entire batch (default {default_timeout}); omit unless the user specifies a duration. Use -1 only when an answer is required before proceeding."
            )
        }),
    );
    let owner = ctx.clone();
    let options = DefineToolOptions::new(
        "ask_user_question",
        TIMED_DESCRIPTION,
        Value::Object(parameters),
        output(timed_answer_schema()),
        move |args, exec| {
            let owner = owner.clone();
            let user_questions = user_questions.clone();
            Box::pin(async move {
                let args = parse_arguments(args)?;
                let timeout = match &args.timeout {
                    TimeoutArgument::Present(timeout) => validate_timeout_value(timeout)?,
                    TimeoutArgument::Missing => default_timeout,
                };
                validate_question_ids(&args.questions)?;
                let agent = live_agent(&owner, &exec.agent)?;
                let request = AskUserQuestionRequest {
                    questions: questions_for_service(args.questions),
                    agent: agent.clone(),
                    signal: Some(exec.signal()),
                    wait: None,
                };
                if timeout != -1 {
                    let Some(agent) = agent else {
                        return Err(ToolContextUnavailableError::new(
                            "timed questions require a live agent",
                        )
                        .into());
                    };
                    let result = user_questions
                        .ask_timed(
                            AskUserQuestionRequest {
                                agent: Some(agent),
                                ..request
                            },
                            exec.call_id.clone(),
                            u64::try_from(timeout)? * 1_000,
                        )
                        .await?;
                    return Ok(match result {
                        TimedUserQuestionResult::Answer(answer) => answer_result(answer),
                        TimedUserQuestionResult::Pending { pending, call_id } => json!({
                            "pending": pending,
                            "callId": call_id,
                            "message": PENDING_NOTICE
                        }),
                    });
                }
                let result = user_questions
                    .ask(AskUserQuestionRequest {
                        wait: Some(UserQuestionWait {
                            call_id: exec.call_id.clone(),
                            timed: None,
                        }),
                        ..request
                    })
                    .await?;
                Ok(answer_result(result))
            })
        },
    );
    let mut definition = define_tool(options)?;
    definition.parameters["properties"][TIMED_WAIT_PARAMETER]["type"] = json!("integer");
    tools.register(&ctx, definition, ToolRegistrationOptions::default())?;
    Ok(())
}
