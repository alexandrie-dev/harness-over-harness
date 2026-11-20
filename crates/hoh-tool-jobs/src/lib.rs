//! Model-facing `job_output`, `job_list`, and `job_kill` tools over `ctx.jobs`. Loading the plugin
//! attaches the controller required by producers. It also delivers completions the model has not
//! already collected to the owning agent: injected into a busy owner's next step, or opening a
//! turn on an idle one under the default `wakeup` delivery, unbounded unless
//! `maxConsecutiveWakes` caps it per owner. A notice carries the job's output and result when it
//! fits `inlineOutputBytes`, and `job_output` withdraws a notice still pending.
//!
//! Port of `@deepseek-ai/dsh-tool-jobs` (`packages/jobs/tool-jobs`).

mod notice;
pub mod render;

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
};

use hoh_agent::{Agent, AgentInboxClaimed, AgentInboxInserted, AgentStatus, Agents};
use hoh_error::BoxError;
use hoh_jobs::{
    JobEvent, JobEventFilter, JobEventOwners, JobId, JobRead, JobSettleCause, JobStatus, JobView,
    Jobs, KillOutcome,
};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{ContentBlock, MessageId, MessageSource, NewUserMessage, create_user_message};
use hoh_session::{SessionEventEmitted, SessionId};
use hoh_system_prompt::{PromptSection, PromptSectionOrderName, SystemPrompt, SystemPromptKey};
use hoh_timeout::{AbortController, AbortError, AbortSignal};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, GenericCallView, PreToolDecision, ToolArgsError,
    ToolCallKind, ToolCallView, ToolExecution, ToolExecutionResult, ToolExecutionToken, Tools,
    ToolsPreExecute, define_tool,
};
use hoh_util_values::{js_json_stringify, js_number_string};
use notice::{
    bound_single_text, completion_summary, fit_completion_notice, fit_with_suffix,
    inline_completion_notice, raw_single_text,
};
use render::{PublicJobSnapshot, public_job, render_model_delta, status_line};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Cordis plugin name.
pub const NAME: &str = "tool-jobs";
/// Required service names, in the source declaration order.
pub const INJECT: [&str; 3] = ["tools", "jobs", "systemPrompt"];

const PROMPT_TEXT: &str = "Track every background job id you start. You are notified in-session when a job finishes — do not busy-poll or sleep on one; keep working on independent steps and do not duplicate a running job's work. Before giving a final answer, collect every still-relevant job with job_output (set wait: true only when you are genuinely blocked on it), and job_kill jobs that stopped mattering.";

/// How an uncollected completion reaches an owner that is already idle: `wakeup` opens a turn for
/// it, `quiet` leaves it pending until something else wakes the owner. A busy owner is injected
/// either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CompletionDelivery {
    Quiet,
    Wakeup,
}

const fn default_wait_timeout_ms() -> f64 {
    30_000.0
}

const fn default_max_wait_timeout_ms() -> f64 {
    600_000.0
}

const fn default_delivery() -> CompletionDelivery {
    CompletionDelivery::Wakeup
}

const fn default_inline_output_bytes() -> u64 {
    2048
}

/// Configures bounded `job_output` waits and completion-notice delivery.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Wait duration applied when `job_output` sets `wait` without `timeout_ms` (default 30s).
    #[serde(default = "default_wait_timeout_ms")]
    #[schemars(range(min = 1))]
    pub wait_timeout_ms: f64,
    /// Hard cap on any single wait; a larger model-supplied `timeout_ms` is clamped down to it
    /// (default 10min).
    #[serde(default = "default_max_wait_timeout_ms")]
    #[schemars(range(min = 1))]
    pub max_wait_timeout_ms: f64,
    /// Whether a completion opens a turn on an idle owner (default `wakeup`).
    #[serde(default = "default_delivery")]
    pub completion_delivery: CompletionDelivery,
    /// Turns one owner may have opened by completion wakes before the next notice degrades to
    /// injection, reset by any user-authored input. Absent by default: every idle completion
    /// wakes its owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub max_consecutive_wakes: Option<f64>,
    /// Largest complete completion notice, in UTF-8 bytes, that carries the job's output and
    /// final result inline (default 2048); a larger one, or one over the producer's
    /// `outputLimitBytes`, leaves them for `job_output`. 0 never inlines.
    #[serde(default = "default_inline_output_bytes")]
    pub inline_output_bytes: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            wait_timeout_ms: default_wait_timeout_ms(),
            max_wait_timeout_ms: default_max_wait_timeout_ms(),
            completion_delivery: default_delivery(),
            max_consecutive_wakes: None,
            inline_output_bytes: default_inline_output_bytes(),
        }
    }
}

/// Services used by `inject = ['tools', 'jobs', 'systemPrompt']`.
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    jobs: Service<Jobs>,
    system_prompt: Service<SystemPromptKey>,
}

/// The `tool-jobs` plugin.
pub struct ToolJobs;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A completion notice not yet logged; see `State::pending`.
struct PendingNotice {
    owner: SessionId,
    message_id: MessageId,
    /// Whether the notice carries `held`; otherwise `job_output` returns it.
    inline: bool,
    /// The consuming read taken at settlement, absent when the read stayed in the registry.
    held: Option<String>,
}

/// A settlement event, the one kind this plugin delivers.
#[derive(Clone)]
struct SettledEvent {
    job: JobView,
    cause: JobSettleCause,
}

#[derive(Default)]
struct State {
    /// Turns this plugin opened on each owner since that owner last consumed human input. Keyed
    /// by the exact agent, so a same-session replacement starts with a full budget.
    spent_wakes: HashMap<usize, (Weak<dyn Agent>, u64)>,
    /// Each completion notice until the model receives its output.
    pending: HashMap<JobId, PendingNotice>,
    /// Live jobs whose kill the model itself requested through `job_kill`.
    killed_by_model: HashSet<JobId>,
    /// Waits this plugin ended on next-step input whose consuming read has not committed yet; one
    /// pending claim per released read.
    released_reads: HashMap<JobId, u64>,
    /// Jobs whose terminal state a `job_output` read already returned; their settlement event is
    /// a repeat. The registry commits a settlement before it announces it, so across threads a
    /// read can land between the two; a single-threaded host orders them for free.
    collected: HashSet<JobId>,
    /// A settlement a released read is expected to carry.
    suppressed: HashMap<JobId, SettledEvent>,
    /// The producer's cap for the job a call names, taken before the call runs.
    output_limits: HashMap<ToolExecutionToken, (Weak<ToolExecution>, u64)>,
}

struct Shared {
    ctx: Context,
    jobs: Service<Jobs>,
    delivery: CompletionDelivery,
    wake_budget: Option<u64>,
    inline_bytes: u64,
    wait_default: f64,
    wait_cap: f64,
    state: Mutex<State>,
}

fn agent_key(agent: &Arc<dyn Agent>) -> usize {
    Arc::as_ptr(agent).cast::<()>() as usize
}

/// The consuming read as the model sees it: the delta, then the result once, then the status line.
fn read_body(read: &JobRead) -> (String, PublicJobSnapshot) {
    let spill: &[String] = read.job.output.spill_paths.as_deref().unwrap_or(&[]);
    let delta = render_model_delta(&read.chunks, read.lossy, spill);
    let text = match &read.result {
        None => delta,
        Some(result) => {
            let separator = if !delta.is_empty() && !delta.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            format!("{delta}{separator}{result}")
        }
    };
    (text, public_job(&read.job))
}

/// The producer's cap for the job a `job_output` or `job_kill` call names, when it is visible to
/// the caller.
fn visible_output_limit(shared: &Shared, exec: &ToolExecution) -> Option<u64> {
    if exec.name != "job_output" && exec.name != "job_kill" {
        return None;
    }
    let job_id = exec.arguments.get("job_id").and_then(Value::as_str)?;
    if job_id.is_empty() {
        return None;
    }
    let caller = exec.agent.as_ref().map(|agent| agent.id());
    shared
        .jobs
        .list(caller)
        .into_iter()
        .find(|job| job.id == job_id)
        .and_then(|job| job.output_limit_bytes)
}

/// Validate the non-empty constraint that the parameter schema cannot express.
fn validate_job_id(value: &str) -> Result<JobId, BoxError> {
    if value.is_empty() {
        return Err(Box::new(ToolArgsError::new(vec![format!(
            "invalid job_id: expected a non-empty string, got {}",
            js_json_stringify(&Value::String(value.to_owned()))
        )])));
    }
    Ok(JobId::from(value))
}

fn string_arg<'a>(args: &'a Value, name: &str) -> &'a str {
    args.get(name).and_then(Value::as_str).unwrap_or_default()
}

/// Pending presentation shared by the three generic job controls.
fn present_job_call(title: String, kind: ToolCallKind, raw_input: Option<&str>) -> ToolCallView {
    ToolCallView::Generic(GenericCallView {
        title,
        kind: Some(kind),
        raw_input: raw_input.map(|input| Value::String(input.to_owned())),
        content: None,
        locations: None,
    })
}

/// Shared schema for job-control outputs.
fn public_job_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "id": { "type": "string", "required": true },
            "kind": { "type": "string", "required": true },
            "label": { "type": "string", "required": true },
            "status": {
                "type": "string",
                "required": true,
                "enum": ["running", "stopping", "completed", "killed", "failed"]
            },
            "detail": { "type": "string" },
            "startedAt": { "type": "integer", "required": true },
            "finishedAt": { "type": "integer" }
        }
    })
}

/// `{ ...PUBLIC_JOB_SCHEMA, required: true }`
fn required_public_job_schema() -> Value {
    let mut schema = public_job_schema();
    if let Value::Object(map) = &mut schema {
        map.insert("required".into(), Value::Bool(true));
    }
    schema
}

fn value_status_line(job: &Value) -> String {
    status_line(
        job.get("status")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        job.get("detail").and_then(Value::as_str),
    )
}

impl Shared {
    /// Begin a released-read claim; the returned guard value ends it.
    fn begin_released_read(&self, id: &JobId) {
        *lock(&self.state)
            .released_reads
            .entry(id.clone())
            .or_insert(0) += 1;
    }

    /// End one released-read claim; the last one delivers a settlement it withheld.
    fn end_released_read(&self, id: &JobId) -> Result<(), BoxError> {
        let pending = {
            let mut state = lock(&self.state);
            let Some(claim) = state.released_reads.get_mut(id) else {
                return Ok(());
            };
            *claim -= 1;
            if *claim != 0 {
                return Ok(());
            }
            state.released_reads.remove(id);
            // Every released read committed: the result carried the settlement.
            state.suppressed.remove(id)
        };
        match pending {
            Some(event) => self.deliver_settlement(&event),
            None => Ok(()),
        }
    }

    /// Deliver one settlement that no caller already collected.
    fn deliver_settlement(&self, event: &SettledEvent) -> Result<(), BoxError> {
        if event.cause == JobSettleCause::Teardown {
            return Ok(());
        }
        let Some(owner_id) = event.job.owner.as_ref() else {
            return Ok(());
        };
        // The destination is the agent registered for the owner session now. An owned job needed
        // the agent registry to start, so the registry is only absent here when it left before
        // settlement — and then no inbox is left.
        let Some(owner) = self
            .ctx
            .get::<Agents>()
            .and_then(|agents| agents.get(owner_id))
        else {
            return Ok(());
        };
        // A stream longer than the bound cannot fit, so the read stays in the registry. A read
        // whose result makes the notice too long is held for `job_output`, whose result the tool
        // pipeline bounds and spills.
        let mut consumed = false;
        let held = if self.inline_bytes > 0 && event.job.output.total <= self.inline_bytes {
            let read = self.jobs.read(&event.job.id, Some(owner_id))?;
            consumed = !read.chunks.is_empty() || read.result.is_some();
            Some(read_body(&read).0)
        } else {
            None
        };
        let inline = held
            .as_deref()
            .and_then(|held| inline_completion_notice(&event.job, held, self.inline_bytes));
        let text = inline
            .clone()
            .unwrap_or_else(|| fit_completion_notice(&event.job));
        let mut source = MessageSource::new("tool-jobs");
        source
            .data
            .insert("form".into(), Value::String("notice".into()));
        source.data.insert(
            "summary".into(),
            Value::String(completion_summary(&event.job)),
        );
        let message = create_user_message(NewUserMessage {
            content: vec![ContentBlock::text(text)],
            source,
        });
        {
            let mut state = lock(&self.state);
            // A read that returned the terminal state while this notice was being built is the
            // model's collection of it.
            // Unless this notice's own read took the output first: the model's read then found
            // nothing, and the notice is the only place the result survives.
            if !consumed && state.collected.contains(&event.job.id) {
                return Ok(());
            }
            state.pending.insert(
                event.job.id.clone(),
                PendingNotice {
                    owner: owner_id.clone(),
                    message_id: message.id.clone(),
                    inline: inline.is_some(),
                    held,
                },
            );
        }
        if self.delivery == CompletionDelivery::Wakeup && owner.status() == AgentStatus::Idle {
            let Some(budget) = self.wake_budget else {
                return owner.followup(message);
            };
            let key = agent_key(&owner);
            let wake = {
                let mut state = lock(&self.state);
                let spent = state
                    .spent_wakes
                    .get(&key)
                    .filter(|(agent, _)| agent.strong_count() > 0)
                    .map_or(0, |(_, spent)| *spent);
                if spent < budget {
                    state
                        .spent_wakes
                        .insert(key, (Arc::downgrade(&owner), spent + 1));
                    true
                } else {
                    false
                }
            };
            if wake {
                return owner.followup(message);
            }
        }
        owner.inject(message)
    }

    fn handle_job_event(&self, event: &JobEvent) -> Result<(), BoxError> {
        let (job, cause, awaited) = match event {
            JobEvent::Removed { job } => {
                let mut state = lock(&self.state);
                state.killed_by_model.remove(&job.id);
                state.pending.remove(&job.id);
                state.collected.remove(&job.id);
                return Ok(());
            }
            JobEvent::Settled {
                job,
                cause,
                awaited,
            } => (job, *cause, *awaited),
            _ => return Ok(()),
        };
        let settled = SettledEvent {
            job: job.clone(),
            cause,
        };
        {
            let mut state = lock(&self.state);
            // The model's own kill result and a live waiter's collection both deliver this
            // settlement; only a settlement no caller collected is held for a released read
            // that has not committed yet. Holding an already-delivered settlement would repeat
            // it when that read never commits.
            if state.killed_by_model.remove(&job.id) || awaited || state.collected.contains(&job.id)
            {
                return Ok(());
            }
            if state.released_reads.contains_key(&job.id) {
                state.suppressed.insert(job.id.clone(), settled);
                return Ok(());
            }
        }
        self.deliver_settlement(&settled)
    }

    /// `finalizeContent` shared by `job_output` and `job_kill`.
    fn finalize_job_content(
        &self,
        exec: &ToolExecution,
        result: &ToolExecutionResult,
    ) -> Option<Vec<ContentBlock>> {
        let recorded = lock(&self.state)
            .output_limits
            .remove(&exec.token)
            .map(|(_, max)| max);
        let max_bytes = recorded.or_else(|| visible_output_limit(self, exec))?;
        if exec.name == "job_output"
            && !result.is_error()
            && let Some(value) = result.value()
        {
            // This definition owns and schema-validates the canonical value. Preserve its
            // output/status split only while policy left the default rendering intact.
            let text = value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let body = if text.is_empty() {
                "(no new output)"
            } else {
                text
            };
            let content = body.strip_suffix('\n').unwrap_or(body);
            let suffix = format!(
                "\n{}",
                value_status_line(value.get("job").unwrap_or(&Value::Null))
            );
            if raw_single_text(result.content()).as_deref() == Some(&format!("{content}{suffix}")) {
                return Some(vec![ContentBlock::text(fit_with_suffix(
                    content,
                    &suffix,
                    Some(max_bytes),
                    "\n[output truncated]",
                ))]);
            }
        }
        bound_single_text(result.content(), max_bytes)
    }
}

fn validate_config(config: &Config) -> Result<(Option<u64>, f64, f64), BoxError> {
    let wait_default = config.wait_timeout_ms;
    let wait_cap = config.max_wait_timeout_ms;
    if wait_default > wait_cap {
        return Err(format!(
            "tool-jobs: waitTimeoutMs ({}) exceeds maxWaitTimeoutMs ({})",
            js_number_string(wait_default),
            js_number_string(wait_cap)
        )
        .into());
    }
    // A budget is a count of turns: a fraction never names a turn, and `Infinity` would spell an
    // "unbounded" that omitting the field already means.
    let budget = match config.max_consecutive_wakes {
        None => None,
        Some(wakes) => {
            if !(wakes.is_finite()
                && wakes.fract() == 0.0
                && wakes.abs() <= 9_007_199_254_740_991.0)
            {
                return Err(format!(
                    "tool-jobs: maxConsecutiveWakes ({}) must be a whole number of turns",
                    js_number_string(wakes)
                )
                .into());
            }
            Some(wakes as u64)
        }
    };
    Ok((budget, wait_default, wait_cap))
}

impl Plugin for ToolJobs {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        apply(&ctx, &deps, &config.get())
    }
}

/// Register the three job tools, the completion-notice delivery, and the guidance section.
pub fn apply(ctx: &Context, deps: &Deps, config: &Config) -> Result<(), BoxError> {
    let (wake_budget, wait_default, wait_cap) = validate_config(config)?;
    let shared = Arc::new(Shared {
        ctx: ctx.clone(),
        jobs: deps.jobs.clone(),
        delivery: config.completion_delivery,
        wake_budget,
        inline_bytes: config.inline_output_bytes,
        wait_default,
        wait_cap,
        state: Mutex::new(State::default()),
    });

    if shared.delivery == CompletionDelivery::Wakeup && shared.wake_budget.is_some() {
        let claimed = shared.clone();
        ctx.on_contained::<AgentInboxClaimed>(move |event| {
            // Claiming is the point the human's input actually enters a step; a notice this
            // plugin itself queued must not refill the budget it just spent.
            if event.message.source.kind == "user" {
                lock(&claimed.state)
                    .spent_wakes
                    .remove(&agent_key(&event.agent));
            }
            Ok(())
        })?;
    }
    let logged = shared.clone();
    ctx.on_contained::<SessionEventEmitted>(move |event| {
        if event.event.event_type() != "user/message" {
            return Ok(());
        }
        let data = event.event.data();
        if data["source"]["kind"].as_str() != Some("tool-jobs") {
            return Ok(());
        }
        let Some(message_id) = data["id"].as_str() else {
            return Ok(());
        };
        lock(&logged.state).pending.retain(|_, notice| {
            if notice.message_id != message_id || &notice.owner != event.session.id() {
                return true;
            }
            // A logged notice keeps only a read it did not carry, for `job_output`.
            !(notice.inline || notice.held.is_none())
        });
        Ok(())
    })?;

    let pre = shared.clone();
    ctx.listen()
        .prepend()
        .on_waterfall::<ToolsPreExecute, PreToolDecision, _>(move |event, next| {
            let exec = event.exec.clone();
            if let Some(max_bytes) = visible_output_limit(&pre, &exec) {
                let mut state = lock(&pre.state);
                state
                    .output_limits
                    .retain(|_, (execution, _)| execution.strong_count() > 0);
                state
                    .output_limits
                    .insert(exec.token, (Arc::downgrade(&exec), max_bytes));
            }
            next.run()
        })?;

    // Producers may start work only while a controller is attached.
    deps.jobs.attach_controller(ctx, NAME)?;

    // Cross-call guidance follows the filesystem sections and precedes product sections.
    deps.system_prompt.section(
        ctx,
        PromptSection {
            name: "tool:jobs".into(),
            order: SystemPrompt::get_section_order(PromptSectionOrderName::ToolJobs),
            text: PROMPT_TEXT.into(),
            ..PromptSection::default()
        },
    )?;

    let listener = shared.clone();
    deps.jobs.events().subscribe(
        ctx,
        JobEventFilter::Owners {
            owners: JobEventOwners::Scope,
        },
        Arc::new(move |event| listener.handle_job_event(event)),
    )?;

    register_job_output(ctx, deps, &shared)?;
    register_job_list(ctx, deps, &shared)?;
    register_job_kill(ctx, deps, &shared)?;
    Ok(())
}

fn finalizer(shared: &Arc<Shared>) -> hoh_tools::ContentHook {
    let shared = shared.clone();
    Arc::new(move |exec, result| shared.finalize_job_content(exec, result))
}

fn register_job_output(ctx: &Context, deps: &Deps, shared: &Arc<Shared>) -> Result<(), BoxError> {
    let runner = shared.clone();
    let mut options = DefineToolOptions::new(
        "job_output",
        "Read a background job: output since the previous read for stream jobs, or the result of a finished final-output job.",
        // A timed-out wait returns job state rather than a TOOL_TIMEOUT error, so this tool owns
        // its deadline instead of using ToolDefinition.timeoutMs.
        json!({
            "job_id": { "type": "string", "required": true, "description": "Job id returned by the tool that started the background work." },
            "wait": { "type": "boolean", "description": "Block until the job finishes or the timeout expires; a timed-out wait leaves the job running. Defaults to false." },
            "timeout_ms": { "type": "number", "description": "Max wait in milliseconds with wait: true. Defaults to and is capped by configuration." }
        }),
        DefineToolOutput {
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "text": { "type": "string", "required": true },
                    "job": required_public_job_schema()
                }
            }),
            render: Arc::new(|_, value| {
                let text = value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let body = if text.is_empty() {
                    "(no new output)"
                } else {
                    text
                };
                let separator = if body.ends_with('\n') { "" } else { "\n" };
                vec![ContentBlock::text(format!(
                    "{body}{separator}{}",
                    value_status_line(value.get("job").unwrap_or(&Value::Null))
                ))]
            }),
            presentation_meta: None,
        },
        move |args, exec| {
            let shared = runner.clone();
            Box::pin(async move { job_output(shared, args, exec).await })
        },
    );
    options.finalize_content = Some(finalizer(shared));
    options.present_call = Some(Arc::new(|args| {
        let job_id = string_arg(args, "job_id");
        Some(present_job_call(
            format!("Read output from background job {job_id}"),
            ToolCallKind::Read,
            Some(job_id),
        ))
    }));
    deps.tools
        .register(ctx, define_tool(options)?, Default::default())?;
    Ok(())
}

fn register_job_list(ctx: &Context, deps: &Deps, shared: &Arc<Shared>) -> Result<(), BoxError> {
    let runner = shared.clone();
    let mut options = DefineToolOptions::new(
        "job_list",
        "List your background jobs (running and finished) with their ids, kinds, and statuses.",
        json!({}),
        DefineToolOutput {
            schema: json!({ "type": "array", "items": public_job_schema() }),
            render: Arc::new(|_, jobs| {
                let jobs = jobs.as_array().map(Vec::as_slice).unwrap_or_default();
                let text = if jobs.is_empty() {
                    "(no background jobs)".to_owned()
                } else {
                    jobs.iter()
                        .map(|job| {
                            let field = |name: &str| {
                                job.get(name).and_then(Value::as_str).unwrap_or_default()
                            };
                            format!(
                                "{} [{}] {} — {}",
                                field("id"),
                                field("kind"),
                                field("status"),
                                field("label")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                vec![ContentBlock::text(text)]
            }),
            presentation_meta: None,
        },
        move |_args, exec| {
            let shared = runner.clone();
            Box::pin(async move {
                let caller = exec.agent.as_ref().map(|agent| agent.id());
                let jobs: Vec<Value> = shared
                    .jobs
                    .list(caller)
                    .iter()
                    .map(|job| Value::Object(public_job(job).to_map()))
                    .collect();
                Ok(Value::Array(jobs))
            })
        },
    );
    // Listing reads a status snapshot and consumes no output cursor or completion notice.
    options.is_concurrency_safe = Some(Arc::new(|_| true));
    options.present_call = Some(Arc::new(|_| {
        Some(present_job_call(
            "List background jobs".into(),
            ToolCallKind::Read,
            None,
        ))
    }));
    deps.tools
        .register(ctx, define_tool(options)?, Default::default())?;
    Ok(())
}

fn register_job_kill(ctx: &Context, deps: &Deps, shared: &Arc<Shared>) -> Result<(), BoxError> {
    let runner = shared.clone();
    let mut options = DefineToolOptions::new(
        "job_kill",
        "Request cancellation of a running background job.",
        json!({
            "job_id": { "type": "string", "required": true, "description": "Job id returned by the tool that started the background work." },
            "reason": { "type": "string", "description": "Optional short reason, recorded in the log and forwarded to the job." }
        }),
        DefineToolOutput {
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "outcome": {
                        "type": "string",
                        "required": true,
                        "enum": ["cancellation-requested", "already-finished"]
                    },
                    "job": required_public_job_schema()
                }
            }),
            render: Arc::new(|_, value| {
                let job = value.get("job").unwrap_or(&Value::Null);
                let id = job.get("id").and_then(Value::as_str).unwrap_or_default();
                let text =
                    if value.get("outcome").and_then(Value::as_str) == Some("already-finished") {
                        format!("job {id} had already finished {}", value_status_line(job))
                    } else {
                        format!("requested cancellation of job {id}")
                    };
                vec![ContentBlock::text(text)]
            }),
            presentation_meta: None,
        },
        move |args, exec| {
            let shared = runner.clone();
            Box::pin(async move {
                let id = validate_job_id(string_arg(&args, "job_id"))?;
                let caller = exec.agent.as_ref().map(|agent| agent.id());
                let reason = args.get("reason").and_then(Value::as_str);
                // The model's own kill is its delivery: the settlement notice would only repeat
                // what this tool result already said. The claim precedes `kill`, which may run the
                // producer's cancel (and so the settlement) before it returns; on another worker
                // the settlement event could otherwise beat the claim.
                lock(&shared.state).killed_by_model.insert(id.clone());
                let result = match shared.jobs.kill(&id, caller, reason) {
                    Ok(result) => result,
                    Err(error) => {
                        lock(&shared.state).killed_by_model.remove(&id);
                        return Err(error);
                    }
                };
                if result != KillOutcome::Requested {
                    lock(&shared.state).killed_by_model.remove(&id);
                }
                // A projection describes current state without consuming pending output.
                let job = public_job(&shared.jobs.get(&id, caller)?);
                let outcome = if result == KillOutcome::AlreadyFinished {
                    "already-finished"
                } else {
                    "cancellation-requested"
                };
                Ok(json!({ "outcome": outcome, "job": Value::Object(job.to_map()) }))
            })
        },
    );
    options.finalize_content = Some(finalizer(shared));
    options.present_call = Some(Arc::new(|args| {
        let job_id = string_arg(args, "job_id");
        Some(present_job_call(
            format!("Kill background job {job_id}"),
            ToolCallKind::Execute,
            Some(job_id),
        ))
    }));
    deps.tools
        .register(ctx, define_tool(options)?, Default::default())?;
    Ok(())
}

async fn job_output(
    shared: Arc<Shared>,
    args: Value,
    exec: Arc<ToolExecution>,
) -> Result<Value, BoxError> {
    let id = validate_job_id(string_arg(&args, "job_id"))?;
    let timeout_ms = args.get("timeout_ms").and_then(Value::as_f64);
    if let Some(timeout) = timeout_ms
        && (!timeout.is_finite() || timeout <= 0.0)
    {
        return Err(Box::new(ToolArgsError::new(vec![format!(
            "invalid timeout_ms: expected a positive number of milliseconds, got {}",
            js_json_stringify(&args["timeout_ms"])
        )])));
    }
    let agent = exec.agent.clone();
    // A wait this execution ends on next-step input claims the read that follows it: the claim
    // stays open until that read commits, so a settlement landing first is delivered by the
    // result, not a notice. The registry keeps its own abort semantics; this tool's input
    // controller only ends the wait early, and the handling below reads the reason from the
    // caller's signals.
    let claimed = Arc::new(Mutex::new(false));
    let outcome = read_job_output(&shared, &args, &exec, &id, timeout_ms, agent, &claimed).await;
    let released = *lock(&claimed);
    let ended = if released {
        shared.end_released_read(&id)
    } else {
        Ok(())
    };
    ended?;
    outcome
}

async fn read_job_output(
    shared: &Arc<Shared>,
    args: &Value,
    exec: &Arc<ToolExecution>,
    id: &JobId,
    timeout_ms: Option<f64>,
    agent: Option<Arc<dyn Agent>>,
    claimed: &Arc<Mutex<bool>>,
) -> Result<Value, BoxError> {
    if args.get("wait").and_then(Value::as_bool) == Some(true) {
        wait_for_job(shared, exec, id, timeout_ms, agent.as_ref(), claimed).await?;
    }
    let released_by_input = *lock(claimed);
    let caller = agent.as_ref().map(|agent| agent.id());
    let read = shared.jobs.read(id, caller)?;
    let (mut text, snapshot) = read_body(&read);
    // This result carries any settlement the released wait suppressed.
    {
        let mut state = lock(&shared.state);
        if released_by_input {
            state.suppressed.remove(id);
        }
        if read.job.status.is_terminal() {
            state.collected.insert(id.clone());
        }
    }
    let notice = lock(&shared.state).pending.remove(id);
    if let Some(notice) = notice {
        // This result covers everything the pending notice reports.
        if let Some(owner) = shared
            .ctx
            .get::<Agents>()
            .and_then(|agents| agents.get(&notice.owner))
        {
            owner.inbox().remove(&notice.message_id)?;
        }
        text = format!("{}{text}", notice.held.unwrap_or_default());
    }
    let mut job = snapshot.to_map();
    // The result, not the schema, explains an interruption: the model sees why a running job came
    // back early beside its unchanged progress line.
    if released_by_input && matches!(snapshot.status, JobStatus::Running | JobStatus::Stopping) {
        let detail = match &snapshot.detail {
            None => "user steered".to_owned(),
            Some(detail) => format!("{detail}; user steered"),
        };
        job.insert("detail".into(), Value::String(detail));
    }
    let mut body = Map::new();
    body.insert("text".into(), Value::String(text));
    body.insert("job".into(), Value::Object(job));
    Ok(Value::Object(body))
}

/// The bounded wait of `job_output` with `wait: true`.
async fn wait_for_job(
    shared: &Arc<Shared>,
    exec: &Arc<ToolExecution>,
    id: &JobId,
    timeout_ms: Option<f64>,
    agent: Option<&Arc<dyn Agent>>,
    claimed: &Arc<Mutex<bool>>,
) -> Result<(), BoxError> {
    let input = Arc::new(AbortController::new());
    // FIXME (source #5965): this treats an inbox insertion whose source is `user` as a user
    // steer, but the source only says who authored the message.
    let check_input: Arc<dyn Fn() + Send + Sync> = {
        let agent = agent.cloned();
        let shared = shared.clone();
        let id = id.clone();
        let input = input.clone();
        let claimed = claimed.clone();
        Arc::new(move || {
            // Only a user-authored message releases the wait. Other current-step inputs —
            // completion notices and injected context — stay queued for the next step instead of
            // interrupting a wait the user did not.
            let Some(agent) = &agent else {
                return;
            };
            if !agent
                .inbox()
                .next_step()
                .iter()
                .any(|message| message.source.kind == "user")
            {
                return;
            }
            {
                let mut claimed = lock(&claimed);
                if !*claimed {
                    shared.begin_released_read(&id);
                    *claimed = true;
                }
            }
            input.abort();
        })
    };
    // Subscribe before reading pending input; each parallel wait owns its listener. The
    // notification follows the durable insertion commit.
    let unsubscribe = match agent {
        None => None,
        Some(agent) => {
            let subject = agent.clone();
            let check = check_input.clone();
            Some(
                shared
                    .ctx
                    .on_contained::<AgentInboxInserted>(move |event| {
                        if Arc::ptr_eq(&event.agent, &subject) {
                            check();
                        }
                        Ok(())
                    })?,
            )
        }
    };
    check_input();
    let timeout = timeout_ms
        .unwrap_or(shared.wait_default)
        .min(shared.wait_cap);
    let signal = AbortSignal::any([exec.signal(), input.signal()]);
    let waited = shared
        .jobs
        .wait(id, timeout, agent.map(|agent| agent.id()), Some(signal))
        .await;
    let outcome = match waited {
        Ok(_) => Ok(()),
        Err(error) => {
            // The registry reports every abort as a cancellation. A wait this tool released with
            // its own input controller ends the same way, so the abort reason is read from the
            // caller's own signal here.
            if !input.signal().aborted() {
                Err(error)
            } else if exec.signal().aborted() {
                Err(Box::new(AbortError::new("wait aborted")) as BoxError)
            } else {
                // An input released this wait, so the read carries the current state.
                Ok(())
            }
        }
    };
    if let Some(unsubscribe) = unsubscribe {
        unsubscribe.dispose().await;
    }
    outcome
}
