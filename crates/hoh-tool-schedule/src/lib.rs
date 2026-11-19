//! Model-facing tools for creating, inspecting, and editing Host schedules.

use std::{future::Future, sync::Arc};

use hoh_agent_preset_registry::AgentPresets;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Cfg, Context, Inject, NoConfig, Plugin, Service};
use hoh_llm::ContentBlock;
use hoh_schedule::{
    DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT, MAX_TITLE_LENGTH, MIN_EVERY_INTERVAL_SECONDS, Schedule,
    ScheduleCatalogEntry, ScheduleCreateRequest, ScheduleEditRequest, ScheduleEditResult,
    ScheduleId, ScheduleInputError, ScheduleInputErrorCode, ScheduleListRequest,
    ScheduleListResult, SchedulePatch, ScheduleSessionCreateSpec, ScheduleTargetRequest,
    ScheduleTiming, ScheduleViewRequest, ScheduleViewResult, format_schedule_local_time,
};
use hoh_system_prompt::{
    PromptSection, PromptSectionOrderName, PromptText, SystemPrompt, SystemPromptKey,
};
use hoh_tools::{
    Agent, DefineToolOptions, DefineToolOutput, GenericCallView, ToolCallKind, ToolCallView,
    ToolExecution, ToolRegistrationOptions, Tools, define_tool,
};
use hoh_util_values::js_json_stringify;
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Loader-visible plugin name.
pub const NAME: &str = "tool-schedule";

pub const MANAGEMENT_AUTHORIZATION: &str = "Only create or modify scheduled tasks at the user's explicit request; prior explicit user authorization for the running scheduled task remains valid.";
pub const INTERNAL_ERROR_MESSAGE: &str = "The schedule operation failed.";
pub const SUBAGENT_ERROR_MESSAGE: &str = "A delegated subagent cannot use scheduled tasks.";

const CREATE_DESCRIPTION: &str = "Create a scheduled task. At each occurrence of rrule, prompt is delivered to the target session as a scheduled message from the user. The result includes the task id and the next occurrence as scheduledAt in UTC and scheduledAtLocal in the zone of rrule. After downtime, only the latest missed occurrence is delivered. Delivery can repeat after a crash.";
const LIST_DESCRIPTION: &str = "List scheduled tasks, including tasks created from other sessions. Active tasks come first, ordered by scheduledAt.";
const VIEW_DESCRIPTION: &str = "View one scheduled task by id.";
const EDIT_DESCRIPTION: &str = "Update, pause, resume, or delete one scheduled task by id. Deleting does not retract queued messages.";
const RRULE_HELP: &str = "Supported RFC 5545 subset: one DTSTART line, a newline, and one RRULE line. DTSTART ends in Z for UTC or carries TZID with an IANA time zone; a fixed offset such as UTC+5 is Etc/GMT-5, with the sign reversed. DTSTART must name an existing local time. A TZID rule keeps local clock time across daylight-saving changes; generated nonexistent local times are skipped, and repeated ones occur once, at the earlier instant. DTSTART may be in the past; delivery starts with the first occurrence after creation. A one-time delivery is RRULE:FREQ=DAILY;COUNT=1. No SECONDLY; BYSECOND takes one value. COUNT is at most 1000; use UNTIL for longer rules. MINUTELY and HOURLY accept no BY parts, so select clock times with DAILY or WEEKLY plus BYHOUR and BYMINUTE. BYMONTHDAY and ordinal BYDAY require MONTHLY or YEARLY; BYYEARDAY and BYWEEKNO require YEARLY. Ordinal BYDAY cannot combine with BYWEEKNO. Each FREQ period allows at most 10080 candidates before BYSETPOS, which accepts at most 31 values. UNTIL is a UTC instant ending in Z. Example: \"DTSTART;TZID=Asia/Singapore:20260901T080000\\nRRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1\"";

const CALLER_ERRORS: &[ScheduleInputErrorCode] = &[ScheduleInputErrorCode::SubagentSession];
const CREATE_ERRORS: &[ScheduleInputErrorCode] = &[
    ScheduleInputErrorCode::InvalidPrompt,
    ScheduleInputErrorCode::InvalidRule,
    ScheduleInputErrorCode::InvalidTimeZone,
    ScheduleInputErrorCode::NotFuture,
    ScheduleInputErrorCode::FrequencyTooHigh,
    ScheduleInputErrorCode::SubagentSession,
];
const LIST_ERRORS: &[ScheduleInputErrorCode] = &[
    ScheduleInputErrorCode::InvalidRule,
    ScheduleInputErrorCode::SubagentSession,
];
const EDIT_ERRORS: &[ScheduleInputErrorCode] = &[
    ScheduleInputErrorCode::InvalidSelector,
    ScheduleInputErrorCode::InvalidPrompt,
    ScheduleInputErrorCode::InvalidRule,
    ScheduleInputErrorCode::InvalidTimeZone,
    ScheduleInputErrorCode::NotFuture,
    ScheduleInputErrorCode::FrequencyTooHigh,
    ScheduleInputErrorCode::SubagentSession,
];

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TargetChoice {
    ThisSession,
    NewOnce,
    New,
}

#[derive(Deserialize)]
struct CreateArgs {
    title: String,
    prompt: String,
    rrule: String,
    target: TargetChoice,
}

#[derive(Deserialize)]
struct ListArgs {
    active: Option<bool>,
    limit: Option<i128>,
    offset: Option<i128>,
}

#[derive(Deserialize)]
struct ViewArgs {
    id: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EditAction {
    Update,
    Delete,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditArgs {
    id: String,
    action: EditAction,
    title: Option<String>,
    prompt: Option<String>,
    rrule: Option<String>,
    target: Option<TargetChoice>,
    active: Option<bool>,
    expected_updated_at: Option<String>,
}

/// Static plugin dependencies. Schedule itself is watched dynamically so a preset can stay inert
/// on Hosts that deliberately omit scheduling.
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    system_prompt: Service<SystemPromptKey>,
}

#[derive(Inject)]
struct ScheduleDeps {
    schedule: Service<Schedule>,
}

pub struct ToolSchedule;

impl Plugin for ToolSchedule {
    const NAME: &'static str = NAME;
    type Config = NoConfig;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        apply(ctx, deps.tools, deps.system_prompt).await
    }
}

/// Register the dynamic Schedule integration owned by `ctx`.
pub async fn apply(
    ctx: Context,
    tools: Service<Tools>,
    system_prompt: Service<SystemPromptKey>,
) -> Result<(), BoxError> {
    let registration = ctx.inject::<ScheduleDeps, _>(move |scope, dynamic| {
        let tools = tools.clone();
        let prompt = system_prompt.clone();
        async move { register_schedule_tools(scope, tools, prompt, dynamic.schedule) }
    })?;
    if ctx.has("schedule") {
        registration.settled().await?;
    }
    Ok(())
}

fn title_help() -> String {
    format!(
        "Task name of at most {MAX_TITLE_LENGTH} characters in the user's language: action plus object. Omit timing, status, ids, and bracketed suffixes."
    )
}

pub fn target_help(new_session_interval_seconds: u64) -> String {
    let spacing = if new_session_interval_seconds <= MIN_EVERY_INTERVAL_SECONDS {
        String::new()
    } else if new_session_interval_seconds.is_multiple_of(60) {
        format!(
            ", at most once every {} minutes",
            new_session_interval_seconds / 60
        )
    } else {
        format!(", at most once every {new_session_interval_seconds} seconds")
    };
    format!(
        "this_session delivers to this session; new_once delivers to the same new session every time, opened at the first delivery; new opens a different new session for every delivery{spacing}. New sessions use this session's working directory and preset. With this_session or new_once, a delivery waits until the previous one finishes, and only the latest occurrence that passed meanwhile is delivered. Use this_session for reminders and for follow-ups to this conversation; use new_once or new for independent work, such as a recurring report, and whenever the user asks for a new conversation."
    )
}

fn object_schema(properties: Map<String, Value>) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
    })
}

fn target_schema() -> Value {
    let create = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "cwd": { "type": "string" },
            "agentPreset": { "type": "string" },
        },
    });
    json!({
        "oneOf": [
            {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "kind": { "type": "string", "required": true, "const": "session" },
                    "sessionId": { "type": "string", "required": true },
                },
            },
            {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "kind": { "type": "string", "required": true, "const": "new_once" },
                    "sessionId": { "type": "string", "required": true },
                    "create": { "type": "object", "required": true, "additionalProperties": false,
                        "properties": create["properties"].clone() },
                },
            },
            {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "kind": { "type": "string", "required": true, "const": "new" },
                    "create": { "type": "object", "required": true, "additionalProperties": false,
                        "properties": create["properties"].clone() },
                },
            },
        ],
    })
}

fn task_schema() -> Value {
    let mut common = Map::from_iter([
        ("id".into(), json!({ "type": "string", "required": true })),
        (
            "title".into(),
            json!({ "type": "string", "required": true }),
        ),
        (
            "prompt".into(),
            json!({ "type": "string", "required": true }),
        ),
        (
            "scheduledAt".into(),
            json!({ "type": "string", "required": true }),
        ),
        (
            "scheduledAtLocal".into(),
            json!({ "type": "string", "required": true }),
        ),
        (
            "target".into(),
            json!({ "required": true, "oneOf": target_schema()["oneOf"].clone() }),
        ),
        (
            "status".into(),
            json!({ "type": "string", "required": true, "enum": ["active", "paused", "ended"] }),
        ),
        ("updatedAt".into(), json!({ "type": "string" })),
        (
            "lastDelivery".into(),
            json!({
                "type": "object", "additionalProperties": false,
                "properties": {
                    "scheduledAt": { "type": "string", "required": true },
                    "deliveredAt": { "type": "string", "required": true },
                    "targetSessionId": { "type": "string", "required": true },
                },
            }),
        ),
    ]);
    let variants = [
        (
            "rrule",
            vec![("rrule", json!({ "type": "string", "required": true }))],
        ),
        (
            "cron",
            vec![
                ("expression", json!({ "type": "string", "required": true })),
                ("timeZone", json!({ "type": "string", "required": true })),
            ],
        ),
        (
            "every",
            vec![(
                "everySeconds",
                json!({ "type": "integer", "required": true }),
            )],
        ),
        (
            "after",
            vec![(
                "afterSeconds",
                json!({ "type": "integer", "required": true }),
            )],
        ),
        ("at", Vec::new()),
        (
            "daily",
            vec![
                ("time", json!({ "type": "string", "required": true })),
                ("timeZone", json!({ "type": "string", "required": true })),
            ],
        ),
        (
            "weekly",
            vec![
                ("time", json!({ "type": "string", "required": true })),
                ("timeZone", json!({ "type": "string", "required": true })),
                (
                    "weekdays",
                    json!({ "type": "array", "required": true, "items": { "type": "integer" } }),
                ),
            ],
        ),
    ]
    .into_iter()
    .map(|(kind, fields)| {
        let mut properties = common.clone();
        properties.insert(
            "kind".into(),
            json!({ "type": "string", "required": true, "const": kind }),
        );
        properties.extend(fields.into_iter().map(|(name, value)| (name.into(), value)));
        object_schema(properties)
    })
    .collect::<Vec<_>>();
    common.clear();
    json!({ "oneOf": variants })
}

fn error_schema(codes: &[&str]) -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "code": { "type": "string", "required": true, "enum": codes },
            "message": { "type": "string", "required": true },
        },
    })
}

fn caller_error_codes() -> &'static [&'static str] {
    &["subagent_session", "internal_error"]
}

fn create_output_schema() -> Value {
    json!({ "oneOf": [task_schema(), error_schema(&[
        "invalid_prompt", "invalid_rule", "invalid_time_zone", "not_future",
        "frequency_too_high", "subagent_session", "internal_error",
    ])] })
}

fn list_output_schema() -> Value {
    let page = json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "items": { "type": "array", "required": true, "items": task_schema() },
            "shown": { "type": "integer", "required": true },
            "total": { "type": "integer", "required": true },
            "limit": { "type": "integer", "required": true },
            "offset": { "type": "integer", "required": true },
        },
    });
    json!({ "oneOf": [page, error_schema(&[
        "invalid_rule", "subagent_session", "internal_error",
    ])] })
}

fn not_found_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "id": { "type": "string", "required": true },
            "code": { "type": "string", "required": true, "const": "schedule_not_found" },
        },
    })
}

fn view_output_schema() -> Value {
    json!({ "oneOf": [task_schema(), not_found_schema(), error_schema(caller_error_codes())] })
}

fn edit_output_schema() -> Value {
    let deleted = json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "id": { "type": "string", "required": true },
            "deleted": { "type": "boolean", "required": true, "const": true },
        },
    });
    let conflict = json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "id": { "type": "string", "required": true },
            "code": { "type": "string", "required": true, "const": "schedule_conflict" },
        },
    });
    json!({ "oneOf": [task_schema(), deleted, not_found_schema(), conflict, error_schema(&[
        "invalid_selector", "invalid_prompt", "invalid_rule", "invalid_time_zone", "not_future",
        "frequency_too_high", "subagent_session", "internal_error",
    ])] })
}

fn output(schema: Value) -> DefineToolOutput {
    DefineToolOutput {
        schema,
        render: Arc::new(|_, value| vec![ContentBlock::text(js_json_stringify(value))]),
        presentation_meta: None,
    }
}

fn internal_error() -> Value {
    json!({ "code": "internal_error", "message": INTERNAL_ERROR_MESSAGE })
}

fn list_request(args: ListArgs) -> Result<ScheduleListRequest, ScheduleInputError> {
    const MAX_SAFE_INTEGER: i128 = 9_007_199_254_740_991;
    let valid_limit = args
        .limit
        .is_none_or(|limit| (1..=i128::from(MAX_LIST_LIMIT)).contains(&limit));
    let valid_offset = args
        .offset
        .is_none_or(|offset| (0..=MAX_SAFE_INTEGER).contains(&offset));
    if !valid_limit || !valid_offset {
        return Err(ScheduleInputError::new(
            ScheduleInputErrorCode::InvalidRule,
            format!("limit must be 1-{MAX_LIST_LIMIT} and offset must be a non-negative integer."),
        ));
    }
    Ok(ScheduleListRequest {
        active: args.active,
        limit: args.limit.map(|limit| limit as u64),
        offset: args.offset.map(|offset| offset as u64),
    })
}

async fn run_for_caller<F, Fut>(
    exec: Arc<ToolExecution>,
    allowed: &'static [ScheduleInputErrorCode],
    operation: F,
) -> Result<Value, BoxError>
where
    F: FnOnce(Arc<dyn Agent>) -> Fut,
    Fut: Future<Output = Result<Value, BoxError>>,
{
    let Some(agent) = exec.agent.clone() else {
        return Ok(internal_error());
    };
    let header = agent.session().header();
    let depth = delegation_depth(&header)?;
    if depth > 0 {
        return Ok(json!({
            "code": "subagent_session",
            "message": SUBAGENT_ERROR_MESSAGE,
        }));
    }
    match operation(agent).await {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Some(input) = error.downcast_ref::<ScheduleInputError>()
                && allowed.contains(&input.code())
            {
                return Ok(json!({
                    "code": input.code().as_str(),
                    "message": input.to_string(),
                }));
            }
            Ok(internal_error())
        }
    }
}

fn delegation_depth(header: &Value) -> Result<u64, BoxError> {
    let depth = match header.get("delegationDepth") {
        None => 0,
        Some(value) => value.as_u64().ok_or_else(|| -> BoxError {
            Box::new(HarnessError::named(
                "SubagentError",
                "agent subagentDepth must be a non-negative safe integer",
                "INVALID_DEPTH",
            ))
        })?,
    };
    Ok(depth)
}

fn resolved_agent_preset(
    composed: Option<Option<String>>,
    persisted: Option<String>,
) -> Option<String> {
    match composed {
        Some(value) => value,
        None => persisted,
    }
}

fn target_for(agent: &dyn Agent, ctx: &Context, target: TargetChoice) -> ScheduleTargetRequest {
    let session = agent.session();
    if matches!(target, TargetChoice::ThisSession) {
        return ScheduleTargetRequest::Session {
            session_id: session.id().clone(),
        };
    }
    let header = session.header();
    let composed_preset = ctx
        .get::<AgentPresets>()
        .map(|presets| presets.composed_preset(ctx));
    let create = ScheduleSessionCreateSpec {
        cwd: header.get("cwd").and_then(Value::as_str).map(str::to_owned),
        agent_preset: resolved_agent_preset(
            composed_preset,
            header
                .get("agentPreset")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
    };
    match target {
        TargetChoice::ThisSession => unreachable!("returned above"),
        TargetChoice::NewOnce => ScheduleTargetRequest::NewOnce {
            create: Some(create),
        },
        TargetChoice::New => ScheduleTargetRequest::New {
            create: Some(create),
        },
    }
}

pub fn model_task(task: &ScheduleCatalogEntry) -> Result<Value, BoxError> {
    let local = format_schedule_local_time(&task.record, task.record.scheduled_at())?;
    let mut value = serde_json::to_value(task)?;
    let object = value
        .as_object_mut()
        .ok_or("schedule catalog entry did not serialize as an object")?;
    for field in [
        "involvedSessionIds",
        "createdAt",
        "executionActive",
        "deliveryFloorEndsAt",
    ] {
        object.remove(field);
    }
    object.insert("scheduledAtLocal".into(), Value::String(local));
    if let Some(last_delivery) = object
        .get_mut("lastDelivery")
        .and_then(Value::as_object_mut)
    {
        last_delivery.remove("messageId");
    }
    Ok(value)
}

fn model_page(page: ScheduleListResult) -> Result<Value, BoxError> {
    let items = page
        .items
        .iter()
        .map(model_task)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({
        "items": items,
        "shown": page.shown,
        "total": page.total,
        "limit": page.limit,
        "offset": page.offset,
    }))
}

fn model_view(result: ScheduleViewResult) -> Result<Value, BoxError> {
    match result {
        ScheduleViewResult::Entry(task) => model_task(&task),
        ScheduleViewResult::NotFound(missing) => Ok(serde_json::to_value(missing)?),
    }
}

fn model_edit(result: ScheduleEditResult) -> Result<Value, BoxError> {
    match result {
        ScheduleEditResult::Entry(task) => model_task(&task),
        other => Ok(serde_json::to_value(other)?),
    }
}

fn present(title: &str, kind: ToolCallKind, raw_input: Option<Value>) -> ToolCallView {
    ToolCallView::Generic(GenericCallView {
        title: title.into(),
        kind: Some(kind),
        raw_input,
        content: None,
        locations: None,
    })
}

fn register_schedule_tools(
    ctx: Context,
    tools: Service<Tools>,
    system_prompt: Service<SystemPromptKey>,
    schedule: Service<Schedule>,
) -> Result<(), BoxError> {
    let prompt_tools = tools.clone();
    system_prompt.section(
        &ctx,
        PromptSection {
            name: "tool:schedule".into(),
            order: SystemPrompt::get_section_order(PromptSectionOrderName::ToolSchedule),
            text: PromptText::provider(move |assemble| {
                let scope = assemble.scope.as_ref();
                Ok(
                    if prompt_tools.get("schedule_create", scope, None).is_some()
                        || prompt_tools.get("schedule_edit", scope, None).is_some()
                    {
                        MANAGEMENT_AUTHORIZATION.into()
                    } else {
                        String::new()
                    },
                )
            }),
            ..PromptSection::default()
        },
    )?;

    let mut create = DefineToolOptions::new(
        "schedule_create",
        CREATE_DESCRIPTION,
        json!({
            "title": { "type": "string", "required": true, "description": title_help() },
            "prompt": { "type": "string", "required": true, "description": "Instruction delivered at each occurrence." },
            "rrule": { "type": "string", "required": true, "description": RRULE_HELP },
            "target": { "type": "string", "required": true, "enum": ["this_session", "new_once", "new"],
                "description": target_help(schedule.delivery_limits()) },
        }),
        output(create_output_schema()),
        {
            let schedule = schedule.clone();
            let call_ctx = ctx.clone();
            move |args, exec| {
                let schedule = schedule.clone();
                let call_ctx = call_ctx.clone();
                Box::pin(async move {
                    let args: CreateArgs = serde_json::from_value(args)?;
                    let signal = exec.signal();
                    run_for_caller(exec, CREATE_ERRORS, move |agent| async move {
                        let result = schedule
                            .create(
                                ScheduleCreateRequest {
                                    title: args.title,
                                    prompt: args.prompt,
                                    timing: ScheduleTiming::Rrule { rrule: args.rrule },
                                    target: Some(target_for(
                                        agent.as_ref(),
                                        &call_ctx,
                                        args.target,
                                    )),
                                },
                                Some(agent.session().id().clone()),
                                Some(signal),
                            )
                            .await?;
                        model_task(&result)
                    })
                    .await
                })
            }
        },
    );
    create.present_call = Some(Arc::new(|args| {
        Some(present(
            "Create scheduled task",
            ToolCallKind::Other,
            args.get("title").cloned(),
        ))
    }));
    tools.register(
        &ctx,
        define_tool(create)?,
        ToolRegistrationOptions::default(),
    )?;

    let mut list = DefineToolOptions::new(
        "schedule_list",
        LIST_DESCRIPTION,
        json!({
            "active": { "type": "boolean", "description": "true for active tasks only; false for paused and ended tasks only; omitted for all tasks." },
            "limit": { "type": "integer", "description": format!("Page size from 1 to {MAX_LIST_LIMIT}; defaults to {DEFAULT_LIST_LIMIT}.") },
            "offset": { "type": "integer", "description": "Number of tasks to skip; defaults to 0." },
        }),
        output(list_output_schema()),
        {
            let schedule = schedule.clone();
            move |args, exec| {
                let schedule = schedule.clone();
                Box::pin(async move {
                    let args: ListArgs = serde_json::from_value(args)?;
                    run_for_caller(exec, LIST_ERRORS, move |_| async move {
                        let request =
                            list_request(args).map_err(|error| Box::new(error) as BoxError)?;
                        let page = schedule.list(request).await?;
                        model_page(page)
                    })
                    .await
                })
            }
        },
    );
    list.is_concurrency_safe = Some(Arc::new(|_| true));
    list.present_call = Some(Arc::new(|_| {
        Some(present("List scheduled tasks", ToolCallKind::Read, None))
    }));
    tools.register(&ctx, define_tool(list)?, ToolRegistrationOptions::default())?;

    let mut view = DefineToolOptions::new(
        "schedule_view",
        VIEW_DESCRIPTION,
        json!({ "id": { "type": "string", "required": true, "description": "Task id." } }),
        output(view_output_schema()),
        {
            let schedule = schedule.clone();
            move |args, exec| {
                let schedule = schedule.clone();
                Box::pin(async move {
                    let args: ViewArgs = serde_json::from_value(args)?;
                    run_for_caller(exec, CALLER_ERRORS, move |_| async move {
                        model_view(
                            schedule
                                .view(ScheduleViewRequest {
                                    id: ScheduleId::from(args.id),
                                })
                                .await?,
                        )
                    })
                    .await
                })
            }
        },
    );
    view.is_concurrency_safe = Some(Arc::new(|_| true));
    view.present_call = Some(Arc::new(|args| {
        Some(present(
            "View scheduled task",
            ToolCallKind::Read,
            args.get("id").cloned(),
        ))
    }));
    tools.register(&ctx, define_tool(view)?, ToolRegistrationOptions::default())?;

    let edit_ctx = ctx.clone();
    let mut edit = DefineToolOptions::new(
        "schedule_edit",
        EDIT_DESCRIPTION,
        json!({
            "id": { "type": "string", "required": true, "description": "Task id." },
            "action": { "type": "string", "required": true, "enum": ["update", "delete"],
                "description": "update applies the provided fields, at least one; delete removes the task." },
            "title": { "type": "string", "description": title_help() },
            "prompt": { "type": "string", "description": "New instruction delivered at each occurrence." },
            "rrule": { "type": "string", "description": "New recurrence in the schedule_create rrule format." },
            "target": { "type": "string", "enum": ["this_session", "new_once", "new"],
                "description": "Omit to retain the destination and its creation settings. Providing target replaces them; new_once reserves a different Session. New targets capture this Session's working directory and preset. Choices are the same as schedule_create." },
            "active": { "type": "boolean", "description": "false pauses the task; true resumes only if its timing has a future occurrence. Otherwise provide a new rrule." },
            "expectedUpdatedAt": { "type": "string", "description": "updatedAt from the latest result that returned this task; if the task changed after that result, the result is schedule_conflict and nothing changes." },
        }),
        output(edit_output_schema()),
        move |args, exec| {
            let schedule = schedule.clone();
            let call_ctx = edit_ctx.clone();
            Box::pin(async move {
                let args: EditArgs = serde_json::from_value(args)?;
                let signal = exec.signal();
                run_for_caller(exec, EDIT_ERRORS, move |agent| async move {
                    let id = ScheduleId::from(args.id);
                    let expected_updated_at = args.expected_updated_at.map(Some);
                    let request = match args.action {
                        EditAction::Delete => {
                            if args.title.is_some()
                                || args.prompt.is_some()
                                || args.rrule.is_some()
                                || args.target.is_some()
                                || args.active.is_some()
                            {
                                return Err(Box::new(ScheduleInputError::new(
                                    ScheduleInputErrorCode::InvalidSelector,
                                    "delete accepts only id, action, and expectedUpdatedAt.",
                                )) as BoxError);
                            }
                            ScheduleEditRequest::Delete {
                                id,
                                expected_updated_at,
                            }
                        }
                        EditAction::Update => {
                            if args.title.is_none()
                                && args.prompt.is_none()
                                && args.rrule.is_none()
                                && args.target.is_none()
                                && args.active.is_none()
                            {
                                return Err(Box::new(ScheduleInputError::new(
                                    ScheduleInputErrorCode::InvalidSelector,
                                    "update requires at least one of title, prompt, rrule, target, or active.",
                                )) as BoxError);
                            }
                            ScheduleEditRequest::Update {
                                id,
                                patch: SchedulePatch {
                                    title: args.title,
                                    prompt: args.prompt,
                                    timing: args
                                        .rrule
                                        .map(|rrule| ScheduleTiming::Rrule { rrule }),
                                    target: args
                                        .target
                                        .map(|target| target_for(agent.as_ref(), &call_ctx, target)),
                                    active: args.active,
                                },
                                expected_updated_at,
                            }
                        }
                    };
                    model_edit(
                        schedule
                            .edit(request, Some(agent.session().id().clone()), Some(signal))
                            .await?,
                    )
                })
                .await
            })
        },
    );
    edit.present_call = Some(Arc::new(|args| {
        let title = if args.get("action").and_then(Value::as_str) == Some("delete") {
            "Delete scheduled task"
        } else {
            "Edit scheduled task"
        };
        Some(present(title, ToolCallKind::Other, args.get("id").cloned()))
    }));
    tools.register(&ctx, define_tool(edit)?, ToolRegistrationOptions::default())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{TargetChoice, delegation_depth, resolved_agent_preset, target_for};
    use hoh_agent::{
        AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
        MaintenanceTask, Model,
    };
    use hoh_error::{BoxError, HarnessError};
    use hoh_kernel::{Context, Kernel};
    use hoh_llm::UserMessage;
    use hoh_schedule::{ScheduleSessionCreateSpec, ScheduleTargetRequest};
    use hoh_scope::ScopeKey;
    use hoh_session::{AgentCancelCause, Session, SessionId};
    use hoh_tools::Agent;
    use hoh_tools::shim::BoxFuture;
    use serde_json::json;

    struct TestAgent {
        scope: ScopeKey,
        session: Session,
        ctx: Context,
        options: AgentOptions,
    }

    impl TestAgent {
        fn new(ctx: &Context, session: Session) -> Self {
            Self {
                scope: ScopeKey::new(),
                session,
                ctx: ctx.clone(),
                options: AgentOptions::default(),
            }
        }
    }

    impl Agent for TestAgent {
        fn id(&self) -> &SessionId {
            self.session.id()
        }

        fn scope_key(&self) -> ScopeKey {
            self.scope.clone()
        }

        fn options(&self) -> &AgentOptions {
            &self.options
        }

        fn session(&self) -> &Session {
            &self.session
        }

        fn model(&self) -> &dyn Model {
            panic!("the schedule tool does not read the model")
        }

        fn inbox(&self) -> &dyn Inbox {
            panic!("the schedule tool does not read the inbox")
        }

        fn status(&self) -> AgentStatus {
            AgentStatus::Idle
        }

        fn ctx(&self) -> &Context {
            &self.ctx
        }

        fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
            panic!("the schedule tool does not cancel the agent")
        }

        fn when_idle(&self) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }

        fn run_maintenance_erased(
            &self,
            _: MaintenanceTask,
        ) -> Result<MaintenanceFuture, BoxError> {
            panic!("the schedule tool does not run maintenance")
        }

        fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
            panic!("the schedule tool does not send input")
        }

        fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
            panic!("the schedule tool does not send input")
        }

        fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
            panic!("the schedule tool does not send input")
        }

        fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
            panic!("the schedule tool does not send input")
        }
    }

    #[test]
    fn present_preset_service_does_not_fall_back_to_the_persisted_header() {
        assert_eq!(
            resolved_agent_preset(Some(None), Some("persisted".into())),
            None
        );
        assert_eq!(
            resolved_agent_preset(Some(Some("composed".into())), Some("persisted".into())),
            Some("composed".into())
        );
    }

    #[test]
    fn rosterless_mount_uses_the_persisted_preset_fallback() {
        assert_eq!(
            resolved_agent_preset(None, Some("persisted".into())),
            Some("persisted".into())
        );
    }

    #[test]
    fn invalid_delegation_depth_is_the_source_coded_failure() {
        for depth in [json!(-1), json!(1.5)] {
            let error = delegation_depth(&json!({ "delegationDepth": depth })).unwrap_err();
            let error = HarnessError::of(error.as_ref()).expect("coded SubagentError");
            assert_eq!(error.name(), "SubagentError");
            assert_eq!(error.code(), "INVALID_DEPTH");
            assert_eq!(
                error.message(),
                "agent subagentDepth must be a non-negative safe integer"
            );
        }
    }

    #[tokio::test]
    async fn target_choices_copy_only_creation_values_available_to_the_caller() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let with_context = TestAgent::new(
            &ctx,
            Session::create(
                SessionId::from("caller"),
                None,
                Some(json!({
                    "version": 4, "id": "caller", "createdAt": 1, "isSeeded": false,
                    "cwd": "/work", "agentPreset": "persisted",
                })),
                None,
                None,
            )
            .unwrap(),
        );
        assert_eq!(
            target_for(&with_context, &ctx, TargetChoice::New),
            ScheduleTargetRequest::New {
                create: Some(ScheduleSessionCreateSpec {
                    cwd: Some("/work".into()),
                    agent_preset: Some("persisted".into()),
                }),
            }
        );
        assert!(matches!(
            target_for(&with_context, &ctx, TargetChoice::NewOnce),
            ScheduleTargetRequest::NewOnce {
                create: Some(_),
                ..
            }
        ));

        let bare = TestAgent::new(
            &ctx,
            Session::create(
                SessionId::from("bare"),
                None,
                Some(json!({
                    "version": 4, "id": "bare", "createdAt": 1, "isSeeded": false,
                })),
                None,
                None,
            )
            .unwrap(),
        );
        assert_eq!(
            target_for(&bare, &ctx, TargetChoice::New),
            ScheduleTargetRequest::New {
                create: Some(ScheduleSessionCreateSpec::default()),
            }
        );
        kernel.shutdown().await;
    }
}
