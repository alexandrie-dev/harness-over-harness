//! The tool body: argument validation, sandbox escalation, request building, and the foreground,
//! promoted and background paths.

use std::sync::{Arc, Mutex, MutexGuard};

use hoh_agent::Agent;
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::Service;
use hoh_sandbox::{
    EscalationApproval, EscalationApprover, EscalationAsk, EscalationOutcome, EscalationRequest,
    SandboxExecutionPolicy, SandboxFuture, SandboxMode, approve_escalation,
    validate_escalation_args,
};
use hoh_session::SessionId;
use hoh_shell::{
    ShellExecRequest, ShellExecSpec, ShellExecution, ShellExpiryPolicy, ShellRunResult,
};
use hoh_subprocess::{SubprocessFailure, subprocess_spawn_failure};
use hoh_timeout::AbortSignal;
use hoh_tools::{TOOL_ABORTED, ToolArgsError, ToolContextUnavailableError, ToolExecution};
use hoh_user_approval::{Approval, ApprovalOutcome, ApprovalRequest, DisplayReason};
use hoh_util_values::{js_json_stringify, js_trim};
use hoh_util_workspace_path::node_path::is_absolute;
use serde_json::{Map, Value, json};

use crate::Shared;
use crate::background::{
    ProcessFailure, process_job, process_outcome, process_sources, ring_delta,
};
use crate::render::{RenderedRun, render_job_read, render_promoted, render_result};
use hoh_jobs::{JobHooks, JobId, JobSpec, JobToolCall, JobView, Jobs};
use hoh_sandbox_policy::SandboxPolicyRequest;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Which optional argument groups the registered schema advertises in this composition.
struct Advertised {
    /// `sandbox_permissions` and `justification`; advertised only with a sandboxing executor.
    escalation: bool,
    /// `run_in_background`; advertised unless `enableRunInBackground: false`.
    background: bool,
}

/// Parsed tool args; execute validates value constraints absent from the parameter schema.
struct BashToolArgs {
    command: String,
    description: String,
    timeout_ms: Option<f64>,
    workdir: Option<String>,
    run_in_background: bool,
    /// Present when the model supplied the key at all (any type).
    sandbox_permissions: Option<Value>,
    justification: Option<Value>,
}

impl BashToolArgs {
    fn parse(args: &Value) -> Self {
        let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
        Self {
            command: text("command").unwrap_or_default(),
            description: text("description").unwrap_or_default(),
            timeout_ms: args.get("timeoutMs").and_then(Value::as_f64),
            workdir: text("workdir"),
            run_in_background: args.get("run_in_background") == Some(&Value::Bool(true)),
            sandbox_permissions: args.get("sandbox_permissions").cloned(),
            justification: args.get("justification").cloned(),
        }
    }

    fn permissions(&self) -> Option<&str> {
        self.sandbox_permissions.as_ref().and_then(Value::as_str)
    }

    fn justification_text(&self) -> Option<&str> {
        self.justification.as_ref().and_then(Value::as_str)
    }
}

fn args_error(message: impl Into<String>) -> BoxError {
    Box::new(ToolArgsError::new(vec![message.into()]))
}

/// Reject arguments the schema does not advertise. Schema validation checks advertised keys only,
/// so these still reach `execute`; the model fixes the call by dropping them, which makes them
/// argument errors. Runs before escalation pairing checks, approval, and working-directory setup.
fn reject_unadvertised_args(args: &BashToolArgs, advertised: &Advertised) -> Result<(), BoxError> {
    // `(args.justification ?? '').trim() !== ''`.
    let justification_present = match &args.justification {
        None | Some(Value::Null) => false,
        Some(Value::String(text)) => !js_trim(text).is_empty(),
        Some(_) => true,
    };
    if !advertised.escalation && (args.sandbox_permissions.is_some() || justification_present) {
        return Err(args_error(
            "sandbox_permissions and justification are not available in this composition (no sandboxing executor to escalate)",
        ));
    }
    if !advertised.background && args.run_in_background {
        return Err(args_error(
            "run_in_background is disabled for this deployment (enableRunInBackground: false)",
        ));
    }
    Ok(())
}

fn validate_bash_args(
    args: &BashToolArgs,
    effective_mode: Option<SandboxMode>,
    advertised: &Advertised,
) -> Result<(), BoxError> {
    if js_trim(&args.command).is_empty() {
        return Err(args_error("invalid command: expected a non-empty string"));
    }
    if js_trim(&args.description).is_empty() {
        return Err(args_error(
            "invalid description: expected a non-empty string",
        ));
    }
    if let Some(timeout_ms) = args.timeout_ms
        && (!timeout_ms.is_finite() || timeout_ms <= 0.0)
    {
        return Err(args_error(format!(
            "invalid timeoutMs: expected a positive number, got {}",
            js_json_stringify(&json!(timeout_ms))
        )));
    }
    reject_unadvertised_args(args, advertised)?;
    if let (Some(permissions), Some(mode)) = (args.permissions(), effective_mode)
        && permissions == mode.as_str()
    {
        return Ok(());
    }
    // `sandbox_permissions === undefined && justification?.trim() === ''` drops a blank justification.
    let justification = if args.sandbox_permissions.is_none()
        && args
            .justification_text()
            .is_some_and(|text| js_trim(text).is_empty())
    {
        None
    } else {
        args.justification_text()
    };
    validate_escalation_args(args.permissions(), justification)
        .map_err(|error| Box::new(error) as BoxError)
}

/// Resolve a relative per-call override against the Session's current directory.
fn resolve_workdir(model_workdir: Option<&str>, session_cwd: Option<&str>) -> Option<String> {
    let Some(model_workdir) = model_workdir else {
        return session_cwd.map(str::to_owned);
    };
    if let Some(session_cwd) = session_cwd
        && !is_absolute(model_workdir)
    {
        return Some(format!(
            "{session_cwd}{}{model_workdir}",
            std::path::MAIN_SEPARATOR
        ));
    }
    Some(model_workdir.to_owned())
}

/// A canonical number: an integral value is written as an integer, as JavaScript writes it.
fn number(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        Value::from(value as i64)
    } else {
        json!(value)
    }
}

fn output_value(output: &hoh_shell::CollectedOutput) -> Value {
    let mut map = Map::new();
    map.insert("text".into(), json!(output.text));
    map.insert("truncated".into(), json!(output.truncated));
    if let Some(spill_path) = &output.spill_path {
        map.insert("spillPath".into(), json!(spill_path));
    }
    Value::Object(map)
}

/// Detach the executor DTO from readonly Service Definition types into plain JSON data.
fn canonical_bash_result(result: &ShellRunResult, cwd: &str) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("cwd".into(), json!(cwd));
    map.insert(
        "exitCode".into(),
        result.exit_code.map_or(Value::Null, Value::from),
    );
    map.insert("signal".into(), json!(result.signal));
    map.insert("timedOut".into(), json!(result.timed_out));
    map.insert("aborted".into(), json!(result.aborted));
    map.insert("timeoutMs".into(), number(result.timeout_ms));
    map.insert("stdout".into(), output_value(&result.stdout));
    map.insert("stderr".into(), output_value(&result.stderr));
    if let Some(sandbox) = &result.sandbox {
        let mut facts = Map::new();
        facts.insert("mode".into(), json!(sandbox.mode.as_str()));
        facts.insert("denied".into(), json!(sandbox.denied));
        if let Some(enforcement) = sandbox.enforcement {
            facts.insert(
                "enforcement".into(),
                serde_json::to_value(enforcement).unwrap_or(Value::Null),
            );
        }
        if let Some(runner_failed) = sandbox.runner_failed {
            facts.insert("runnerFailed".into(), json!(runner_failed));
        }
        map.insert("sandbox".into(), Value::Object(facts));
    }
    map
}

fn foreground_value(result: &ShellRunResult, cwd: &str, stopped: Option<&str>) -> Value {
    let mut map = Map::new();
    map.insert("kind".into(), json!("foreground"));
    map.extend(canonical_bash_result(result, cwd));
    if let Some(stopped) = stopped {
        map.insert("stopped".into(), json!(stopped));
    }
    Value::Object(map)
}

/// The structured abort the foreground paths throw when the caller cancels the call.
fn tool_aborted() -> BoxError {
    Box::new(HarnessError::named(
        "AbortError",
        "tool call aborted",
        TOOL_ABORTED,
    ))
}

/// Report a launch or preparation rejection as this tool's failure. This call's cancellation reason
/// stays as thrown so the registry records it as aborted; a platform spawn error is classified as a
/// subprocess spawn failure with the original as `cause`; any other value, including a coded error,
/// keeps its own identity.
fn launch_failure(error: BoxError, signal: &AbortSignal) -> BoxError {
    if let Some(reason) = signal.reason()
        && SharedError::of(error.as_ref()).is_some_and(|thrown| thrown.ptr_eq(&reason))
    {
        return error;
    }
    match subprocess_spawn_failure(SubprocessFailure::from(error), "bash") {
        SubprocessFailure::Error(error) => Box::new(error),
        other => other.to_string().into(),
    }
}

/// `String(error)` of a thrown error: `Name: message`.
fn error_text(error: &BoxError) -> String {
    let name = HarnessError::of(error.as_ref()).map_or("Error", HarnessError::name);
    let message = error.to_string();
    if message.is_empty() {
        name.to_owned()
    } else {
        format!("{name}: {message}")
    }
}

/// `ctx.shell.resolve`, which throws in the source on an unusable executor budget.
///
/// SHIM(hoh-shell): `ShellExecutor::resolve` returns no `Result`; a throwing executor surfaces as a
/// panic carrying the source message, converted back to the error a throw would be. Replace by the
/// fallible `resolve` when hoh-shell provides it.
fn resolve_spec(
    shell: &Service<hoh_shell::Shell>,
    request: &ShellExecRequest,
) -> Result<ShellExecSpec, BoxError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| shell.resolve(request))).map_err(
        |payload| -> BoxError {
            let text = payload.downcast_ref::<String>().cloned().or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
            });
            text.unwrap_or_else(|| "shell resolve failed".to_owned())
                .into()
        },
    )
}

/// Render the canonical output value into the model-facing text.
pub(crate) fn render_value(value: &Value, escalation_modes: &[SandboxMode]) -> String {
    let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or_default();
    match value.get("kind").and_then(Value::as_str) {
        Some("background") => format!("started background job {}", text("jobId")),
        Some("promoted") => render_promoted(
            text("jobId"),
            value
                .get("timeoutMs")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
            text("output"),
        ),
        _ => match serde_json::from_value::<ShellRunResult>(value.clone()) {
            Ok(result) => render_result(
                &RenderedRun {
                    result: &result,
                    stopped: value.get("stopped").and_then(Value::as_str),
                },
                escalation_modes,
            ),
            Err(_) => js_json_stringify(value),
        },
    }
}

/// Adapts the approval owner service to the shared sandbox escalation sequence.
struct ApprovalAdapter(Service<Approval>);

impl EscalationApprover<Arc<dyn Agent>, hoh_llm::ToolCallId> for ApprovalAdapter {
    fn request(
        &self,
        request: EscalationAsk<Arc<dyn Agent>, hoh_llm::ToolCallId>,
    ) -> SandboxFuture<'_, EscalationOutcome> {
        let mut texts = request.display_reason.into_iter();
        let display_reason = texts.next().map(|(locale, text)| {
            let mut reason = DisplayReason::new(text.clone());
            reason.insert(locale, text);
            for (locale, text) in texts {
                reason.insert(locale, text);
            }
            reason
        });
        let mut asked = ApprovalRequest::new(request.agent, request.tool_name);
        asked.call_id = Some(request.call_id);
        asked.reason = Some(request.reason);
        asked.display_reason = display_reason;
        asked.signal = request.signal;
        let future = self.0.request(asked);
        Box::pin(async move {
            Ok(match future.await? {
                ApprovalOutcome::AllowedOnce => EscalationOutcome::AllowedOnce,
                ApprovalOutcome::Rejected => EscalationOutcome::Rejected,
                ApprovalOutcome::Cancelled => EscalationOutcome::Cancelled,
                ApprovalOutcome::Unavailable => EscalationOutcome::Unavailable,
            })
        })
    }
}

/// A registered command: its id, the process once the starter spawned it, and the reason an outside
/// kill gave.
struct StartedJob {
    id: JobId,
    process: Arc<Mutex<Option<Arc<dyn ShellExecution>>>>,
    stopped: Arc<Mutex<Option<String>>>,
    /// The failure of a registration whose preparation failed before the process published.
    failure: Arc<Mutex<Option<ProcessFailure>>>,
}

impl StartedJob {
    fn process(&self) -> Option<Arc<dyn ShellExecution>> {
        lock(&self.process).clone()
    }
}

fn owner_of(exec: &ToolExecution) -> Option<SessionId> {
    exec.agent.as_ref().map(|agent| agent.id().clone())
}

/// Register the command as a job; the process spawns inside the starter, after admission.
fn start_job(
    shared: &Arc<Shared>,
    registry: &Service<Jobs>,
    command: &str,
    exec: &ToolExecution,
    spec: &ShellExecSpec,
) -> Result<StartedJob, BoxError> {
    let process: Arc<Mutex<Option<Arc<dyn ShellExecution>>>> = Arc::default();
    let stopped: Arc<Mutex<Option<String>>> = Arc::default();
    let failure: Arc<Mutex<Option<ProcessFailure>>> = Arc::default();
    let escalation_modes = shared.escalation_modes.clone();
    let id = registry.start(JobSpec {
        kind: "bash".to_owned(),
        label: command.to_owned(),
        owner: owner_of(exec),
        tool_call: Some(JobToolCall {
            call_id: exec.call_id.clone(),
            root_call_id: exec.root_call_id.clone(),
        }),
        output_limit_bytes: None,
        output: Some(process_sources({
            let process = process.clone();
            move || lock(&process).clone()
        })),
        run: Box::new({
            let process = process.clone();
            let stopped = stopped.clone();
            let failure = failure.clone();
            let shell = shared.shell.clone();
            let spec = spec.clone();
            move |_handle| {
                let job = process_job(
                    Box::new(move |signal| {
                        Box::pin(async move {
                            let mut spec = spec;
                            spec.signal = Some(signal);
                            let started = shell.execute(spec).await?;
                            *lock(&process) = Some(started.clone());
                            Ok(started)
                        })
                    }),
                    move |started| process_outcome(started, &escalation_modes),
                );
                *lock(&failure) = Some(job.failure);
                let cancel = job.hooks.cancel;
                Ok(JobHooks {
                    done: job.hooks.done,
                    // A kill from outside this call (the human's, a parallel job_kill) reaches the
                    // process here; its reason is what the model reads.
                    cancel: Box::new(move |reason| {
                        *lock(&stopped) = reason.map(str::to_owned);
                        cancel(reason)
                    }),
                })
            }
        }),
    })?;
    Ok(StartedJob {
        id,
        process,
        stopped,
        failure,
    })
}

fn is_live(view: &JobView) -> bool {
    view.status.is_live()
}

/// Wait on a registered foreground command until it settles or the timeout passes.
async fn wait_on_job(
    shared: &Arc<Shared>,
    registry: &Service<Jobs>,
    attached: &StartedJob,
    exec: &Arc<ToolExecution>,
    spec: &ShellExecSpec,
) -> Result<Value, BoxError> {
    let owner = owner_of(exec);
    let timeout_ms = spec.timeout_ms;
    // Stop the job on this call's own account and stay on it until it settles, so the settlement is
    // `awaited` and no completion notice follows a result this call already carries; the record then
    // leaves with the call, as the model never saw the id.
    let stop = |reason: &'static str| {
        let registry = registry.clone();
        let id = attached.id.clone();
        let owner = owner.clone();
        async move {
            registry.kill(&id, owner.as_ref(), Some(reason))?;
            let settled = registry.wait(&id, timeout_ms, owner.as_ref(), None).await?;
            if !is_live(&settled) {
                registry.remove(&id, owner.as_ref())?;
            }
            Ok::<JobView, BoxError>(settled)
        }
    };
    let view = match registry
        .wait(
            &attached.id,
            timeout_ms,
            owner.as_ref(),
            Some(exec.signal()),
        )
        .await
    {
        Ok(view) => view,
        Err(_) => {
            // A wait on the caller's own live job rejects only for the caller's abort: the model's
            // call is over, so the command goes with it, as under the deadline kill.
            stop("tool call aborted").await?;
            return Err(tool_aborted());
        }
    };
    if is_live(&view) && attached.process().is_none() {
        // The wait expired while preparation still ran: nothing is running to keep, so this is the
        // deadline's preparation timeout, with the settled empty result the executor's own deadline
        // would have given.
        stop("timed out during preparation").await?;
        let mut map = Map::new();
        map.insert("kind".into(), json!("foreground"));
        map.insert("cwd".into(), json!(spec.workdir));
        map.insert("exitCode".into(), Value::Null);
        map.insert("signal".into(), Value::Null);
        map.insert("timedOut".into(), json!(true));
        map.insert("aborted".into(), json!(false));
        map.insert("timeoutMs".into(), number(timeout_ms));
        map.insert("stdout".into(), json!({ "text": "", "truncated": false }));
        map.insert("stderr".into(), json!({ "text": "", "truncated": false }));
        if let Some(policy) = &spec.sandbox_policy {
            map.insert(
                "sandbox".into(),
                json!({ "mode": policy.mode.as_str(), "denied": false }),
            );
        }
        return Ok(Value::Object(map));
    }
    if is_live(&view) {
        // The wait timed out: the job keeps running. One consuming read seeds the result with the
        // output so far, so `job_output` continues exactly where this result stops.
        let read = registry.read(&attached.id, owner.as_ref())?;
        let sandbox = attached.process().and_then(|process| process.sandbox());
        let output = render_job_read(
            &ring_delta(&read.chunks),
            read.lossy,
            read.job.output.spill_paths.as_deref().unwrap_or_default(),
            sandbox.as_ref(),
            &shared.escalation_modes,
        );
        let mut map = Map::new();
        map.insert("kind".into(), json!("promoted"));
        map.insert("cwd".into(), json!(spec.workdir));
        map.insert("jobId".into(), json!(attached.id.as_str()));
        map.insert("timeoutMs".into(), number(timeout_ms));
        map.insert("output".into(), json!(output));
        return Ok(Value::Object(map));
    }
    // Settled while the call waited: the model never saw the id, so the record leaves the registry
    // with this result. The process handle keeps the split streams and classification the
    // foreground result needs.
    registry.remove(&attached.id, owner.as_ref())?;
    let Some(process) = attached.process() else {
        // No process was published: the starter failed before the spawn. The original error keeps
        // its class and code, exactly as the foreground projection would have rejected; a kill
        // before the spawn has only its detail.
        let failure = lock(&attached.failure)
            .clone()
            .and_then(|failure| failure.get());
        return Err(match failure {
            None => view.detail.unwrap_or_default().into(),
            Some(error) => launch_failure(Box::new(error), &exec.signal()),
        });
    };
    let result = process
        .result()
        .await
        .map_err(|error| launch_failure(error, &exec.signal()))?;
    let stopped = lock(&attached.stopped).clone();
    Ok(foreground_value(&result, &spec.workdir, stopped.as_deref()))
}

/// `approveBashEscalation`: resolve a sandbox-escalation request through `approval` BEFORE anything
/// executes, delegating the shared fail-closed sequence to `approve_escalation`.
async fn approve_bash_escalation(
    shared: &Shared,
    mode: &str,
    justification: &str,
    exec: &ToolExecution,
    standing_policy: Option<&SandboxExecutionPolicy>,
) -> Result<SandboxMode, BoxError> {
    let Some(standing_policy) = standing_policy else {
        return Err("tool-bash: sandbox escalation requires a standing sandbox policy".into());
    };
    let approver: Option<Arc<dyn EscalationApprover<Arc<dyn Agent>, hoh_llm::ToolCallId>>> = shared
        .ctx
        .get::<Approval>()
        .map(|approval| Arc::new(ApprovalAdapter(approval)) as Arc<_>);
    approve_escalation(
        EscalationRequest {
            requested_mode: mode.to_owned(),
            justification: justification.to_owned(),
            effective_mode: standing_policy.mode,
            subject: "command".to_owned(),
        },
        EscalationApproval {
            approver,
            agent: exec.agent.clone(),
            call_id: exec.call_id.clone(),
            tool_name: "bash".to_owned(),
            signal: Some(exec.signal()),
        },
    )
    .await
}

/// Resolve the complete standing policy for this call when a confining executor is mounted.
fn resolve_sandbox_policy(
    shared: &Shared,
    exec: &ToolExecution,
) -> Result<Option<SandboxExecutionPolicy>, BoxError> {
    let Some(policy) = &shared.sandbox_policy else {
        return Ok(None);
    };
    let session = exec.agent.as_ref().map(|agent| agent.session());
    policy
        .resolve(SandboxPolicyRequest {
            session,
            mode: None,
        })
        .map(Some)
}

pub(crate) async fn execute(
    shared: Arc<Shared>,
    jobs: Option<Service<Jobs>>,
    promote: bool,
    args: Value,
    exec: Arc<ToolExecution>,
) -> Result<Value, BoxError> {
    let args = BashToolArgs::parse(&args);
    // Description is display metadata; workdir defaults to the caller's session.
    let standing_policy = resolve_sandbox_policy(&shared, &exec)?;
    validate_bash_args(
        &args,
        standing_policy.as_ref().map(|policy| policy.mode),
        &Advertised {
            escalation: !shared.escalation_modes.is_empty(),
            background: shared.background_enabled,
        },
    )?;
    let approved_mode = match (args.permissions(), args.justification_text()) {
        (Some(mode), Some(justification)) => Some(
            approve_bash_escalation(
                &shared,
                mode,
                justification,
                &exec,
                standing_policy.as_ref(),
            )
            .await?,
        ),
        _ => None,
    };
    let policy = match approved_mode {
        None => standing_policy,
        Some(mode) => standing_policy.map(|policy| SandboxExecutionPolicy { mode, ..policy }),
    };
    let cwd = match &exec.agent {
        None => None,
        Some(agent) => Some(
            shared
                .working_directory
                .ensure(agent, Some(&exec.signal()))
                .await?,
        ),
    };
    let workdir = resolve_workdir(args.workdir.as_deref(), cwd.as_deref());
    let dsh_env = shared.shell_env.collect(&exec)?;
    let request = ShellExecRequest {
        command: args.command.clone(),
        workdir,
        timeout_ms: args.timeout_ms,
        on_expiry: None,
        stdout_max_bytes: None,
        signal: None,
        stdin: None,
        env: None,
        dsh_env: Some(dsh_env),
        sandbox_policy: policy,
    };
    let none_policy = |request: &ShellExecRequest| ShellExecRequest {
        on_expiry: Some(ShellExpiryPolicy::None),
        ..request.clone()
    };

    if args.run_in_background {
        let Some(registry) = &jobs else {
            return Err(Box::new(ToolContextUnavailableError::new(
                "background jobs unavailable: load @deepseek-ai/dsh-jobs and @deepseek-ai/dsh-tool-jobs",
            )));
        };
        // The caller owns cancellation until `jobs` commits detached ownership.
        if exec.signal().aborted() {
            return Err(tool_aborted());
        }
        let spec = resolve_spec(&shared.shell, &none_policy(&request))?;
        let started = start_job(&shared, registry, &args.command, &exec, &spec)?;
        let mut map = Map::new();
        map.insert("kind".into(), json!("background"));
        map.insert("jobId".into(), json!(started.id.as_str()));
        map.insert("cwd".into(), json!(spec.workdir));
        return Ok(Value::Object(map));
    }

    // A foreground call is a job the tool waits on, so the command is visible and killable from the
    // moment it starts and outlives the wait when the timeout passes. Admission refused at the start
    // (the owner's job limit, no controller) runs the command under the deadline kill instead,
    // exactly as a composition without a registry does.
    if let Some(registry) = &jobs
        && promote
    {
        let spec = resolve_spec(&shared.shell, &none_policy(&request))?;
        match start_job(&shared, registry, &args.command, &exec, &spec) {
            Ok(attached) => return wait_on_job(&shared, registry, &attached, &exec, &spec).await,
            Err(error) => shared.ctx.logger().warn(format!(
                "bash: job registration refused, running in the foreground with the timeout kill instead: {}",
                error_text(&error)
            )),
        }
    }
    let signal = exec.signal();
    let spec = resolve_spec(
        &shared.shell,
        &ShellExecRequest {
            signal: Some(signal.clone()),
            ..request
        },
    )?;
    let result = match shared.shell.execute(spec.clone()).await {
        Ok(execution) => execution.result().await,
        Err(error) => Err(error),
    }
    .map_err(|error| launch_failure(error, &signal))?;
    if result.aborted {
        return Err(tool_aborted());
    }
    Ok(foreground_value(&result, &spec.workdir, None))
}
