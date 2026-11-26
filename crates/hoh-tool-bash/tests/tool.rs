//! Ports of the foreground cases of tools.spec.ts: the tool body, its schema, the prompt section,
//! session-cwd routing, the request the tool builds and launch failures.
//!
//! A recording executor stands in for the shell wherever the subject is the tool's rendering and its
//! request construction; the `real_process_*` cases run `hoh-subprocess-local` under
//! `hoh-bash-local`. There is no real job registry yet (`hoh-jobs-local`): `FakeJobs` scripts the
//! `hoh-jobs` contract. Sandbox escalation runs on the real `hoh-sandbox-policy` and
//! `hoh-user-approval`.

#[path = "../../hoh-working-directory/tests/support/mod.rs"]
mod wd;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use hoh_error::BoxError;
use hoh_jobs::{
    JobEventFilter, JobEventListener, JobEvents, JobFuture, JobHandle, JobHooks, JobId,
    JobOutputRead, JobRead, JobRegistryApi, JobSpec, JobStatus, JobView, KillOutcome,
};
use hoh_kernel::{Context, Disposer, Fiber, KernelError};
use hoh_session::SessionId;
use hoh_shell::{
    CollectedOutput, ShellExecRequest, ShellExecSpec, ShellExecution, ShellExecutor,
    ShellExpiryPolicy, ShellProcess, ShellProcessRead, ShellProcessStatus, ShellRunResult,
    ShellSandboxInfo,
    types::{ShellDone, ShellFuture, ShellObservedStreams, shell_done},
};
use hoh_subprocess::{SubprocessOutputRead, SubprocessOutputReader};
use hoh_system_prompt::{
    AssembleContext, PromptSection, PromptSectionOrderName, SystemPrompt, SystemPromptKey,
};
use hoh_timeout::AbortController;
use hoh_timeout::AbortSignal;
use hoh_tool_bash::ToolBash;
use hoh_tools::{
    Agent, ScheduledToolDispatch, ScheduledToolPreparation, TOOL_ABORTED, ToolExecutionInput,
    ToolExecutionPipeline, ToolExecutionResult, ToolRuntime, Tools,
};
use serde_json::{Value, json};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---------------------------------------------------------------------------------------------
// A recording executor: `RecordingBashExecutor` / `RecordingSandboxExecutor` of the source.
// ---------------------------------------------------------------------------------------------

type ResultFn = Arc<dyn Fn(&ShellExecSpec) -> ShellRunResult + Send + Sync>;
type FailFn = Arc<dyn Fn(&ShellExecSpec) -> BoxError + Send + Sync>;

#[derive(Clone)]
enum Mode {
    Result(ResultFn),
    /// The foreground result settles when the call's signal aborts.
    WaitAbort,
    /// `execute` rejects.
    Fail(FailFn),
    /// `execute` stays pending until the spec's signal aborts, then rejects (confinement that only
    /// ends with the job's own cancellation).
    HangUntilAbort,
}

struct Recorder {
    requests: Mutex<Vec<ShellExecRequest>>,
    specs: Mutex<Vec<ShellExecSpec>>,
    mode: Mutex<Mode>,
    workdir_override: Mutex<Option<String>>,
    /// `RecordingSandboxExecutor.sandboxMode`.
    sandbox: Option<hoh_sandbox::SandboxMode>,
    /// `RecordingSandboxExecutor.modes`: the policy mode of every executed spec.
    modes: Mutex<Vec<Option<hoh_sandbox::SandboxMode>>>,
}

fn collected(text: &str) -> CollectedOutput {
    CollectedOutput {
        text: text.to_owned(),
        truncated: false,
        spill_path: None,
    }
}

fn run_result(
    spec: &ShellExecSpec,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i64>,
) -> ShellRunResult {
    ShellRunResult {
        exit_code,
        signal: None,
        timed_out: false,
        aborted: false,
        timeout_ms: spec.timeout_ms,
        stdout: collected(stdout),
        stderr: collected(stderr),
        sandbox: None::<ShellSandboxInfo>,
    }
}

impl Recorder {
    fn new() -> Arc<Self> {
        Self::with_sandbox(None)
    }

    /// The source's `RecordingSandboxExecutor`: confines in `read-only` mode by default.
    fn sandboxed() -> Arc<Self> {
        Self::with_sandbox(Some(hoh_sandbox::SandboxMode::ReadOnly))
    }

    fn with_sandbox(sandbox: Option<hoh_sandbox::SandboxMode>) -> Arc<Self> {
        Arc::new(Self {
            sandbox,
            modes: Mutex::default(),
            requests: Mutex::default(),
            specs: Mutex::default(),
            mode: Mutex::new(Mode::Result(Arc::new(|spec| {
                run_result(spec, "ok", "", Some(0))
            }))),
            workdir_override: Mutex::default(),
        })
    }

    fn set(&self, mode: Mode) {
        *lock(&self.mode) = mode;
    }

    fn respond(&self, result: impl Fn(&ShellExecSpec) -> ShellRunResult + Send + Sync + 'static) {
        self.set(Mode::Result(Arc::new(result)));
    }

    fn request(&self, index: usize) -> ShellExecRequest {
        lock(&self.requests)[index].clone()
    }
}

struct SilentReader;

impl SubprocessOutputReader for SilentReader {
    fn read_from(&self, from_byte: u64) -> SubprocessOutputRead {
        SubprocessOutputRead {
            text: String::new(),
            next_offset: from_byte,
            lossy: false,
            spill_path: None,
        }
    }
}

/// One settled handle: the foreground call projects `result`.
struct Settled {
    done: ShellDone,
    result: ResultFn,
    spec: ShellExecSpec,
    wait_abort: bool,
}

impl ShellProcess for Settled {
    fn status(&self) -> ShellProcessStatus {
        ShellProcessStatus::Completed
    }
    fn exit_code(&self) -> Option<i64> {
        Some(0)
    }
    fn signal(&self) -> Option<String> {
        None
    }
    fn done(&self) -> ShellDone {
        self.done.clone()
    }
    fn sandbox(&self) -> Option<ShellSandboxInfo> {
        None
    }
    fn read_output(&self) -> ShellProcessRead {
        ShellProcessRead {
            delta: String::new(),
            lossy: false,
            stdout_spill_path: None,
            stderr_spill_path: None,
        }
    }
    fn observed(&self) -> ShellObservedStreams {
        ShellObservedStreams {
            stdout: Arc::new(SilentReader),
            stderr: Arc::new(SilentReader),
        }
    }
    fn kill(&self) -> bool {
        false
    }
}

impl ShellExecution for Settled {
    fn result(&self) -> ShellFuture<'_, ShellRunResult> {
        let result = self.result.clone();
        let spec = self.spec.clone();
        let wait_abort = self.wait_abort;
        Box::pin(async move {
            if wait_abort && let Some(signal) = &spec.signal {
                signal.cancelled().await;
                return Ok(ShellRunResult {
                    aborted: true,
                    signal: Some("SIGTERM".to_owned()),
                    exit_code: None,
                    ..result(&spec)
                });
            }
            Ok(result(&spec))
        })
    }
}

impl ShellExecutor for Recorder {
    fn sandbox_mode(&self) -> Option<hoh_sandbox::SandboxMode> {
        self.sandbox
    }

    fn resolve(&self, request: &ShellExecRequest) -> ShellExecSpec {
        lock(&self.requests).push(request.clone());
        let workdir = lock(&self.workdir_override).clone().unwrap_or_else(|| {
            request.workdir.clone().unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
        });
        ShellExecSpec {
            command: request.command.clone(),
            workdir,
            timeout_ms: request.timeout_ms.unwrap_or(0.0),
            on_expiry: request.on_expiry.unwrap_or(ShellExpiryPolicy::Kill),
            stdout_max_bytes: request.stdout_max_bytes.unwrap_or(64_000),
            signal: request.signal.clone(),
            stdin: request.stdin.clone(),
            env: request.env.clone(),
            dsh_env: request.dsh_env.clone(),
            sandbox_policy: request.sandbox_policy.clone(),
        }
    }

    fn execute(&self, spec: ShellExecSpec) -> ShellFuture<'_, Arc<dyn ShellExecution>> {
        lock(&self.specs).push(spec.clone());
        lock(&self.modes).push(spec.sandbox_policy.as_ref().map(|policy| policy.mode));
        let mode = lock(&self.mode).clone();
        let sandboxed = self.sandbox.is_some();
        Box::pin(async move {
            let confined = sandboxed;
            let (result, wait_abort): (ResultFn, bool) = match mode {
                Mode::HangUntilAbort => {
                    let signal = spec.signal.clone().expect("a cancellable start");
                    signal.cancelled().await;
                    return Err("fixture preparation aborted".into());
                }
                Mode::Fail(error) => return Err(error(&spec)),
                Mode::Result(result) => (result, false),
                Mode::WaitAbort => (Arc::new(|spec| run_result(spec, "", "", Some(0))), true),
            };
            // `RecordingSandboxExecutor`: the canonical result carries the facts of the mode the
            // spec ran under; one command is answered without the optional facts.
            let result: ResultFn = if confined {
                Arc::new(move |spec| ShellRunResult {
                    sandbox: Some(ShellSandboxInfo {
                        mode: spec
                            .sandbox_policy
                            .as_ref()
                            .map_or(hoh_sandbox::SandboxMode::ReadOnly, |policy| policy.mode),
                        denied: false,
                        enforcement: (spec.command != "without optional sandbox facts")
                            .then_some(hoh_sandbox::SandboxEnforcement::Full),
                        runner_failed: (spec.command != "without optional sandbox facts")
                            .then_some(false),
                    }),
                    ..result(spec)
                })
            } else {
                result
            };
            Ok(Arc::new(Settled {
                done: shell_done(async {}),
                result,
                spec,
                wait_abort,
            }) as Arc<dyn ShellExecution>)
        })
    }
}

// ---------------------------------------------------------------------------------------------
// The harness: the real working directory, tools runtime, shell environment and bash tool.
// ---------------------------------------------------------------------------------------------

struct Harness {
    fx: wd::Fixture,
    shell: Arc<Recorder>,
    tool: Fiber,
    pipeline: ToolExecutionPipeline,
    dsh_home: String,
    /// The registry mounted as `Jobs`, kept to observe what the call left in it.
    jobs: Option<Arc<FakeJobs>>,
}

impl Harness {
    fn ctx(&self) -> &Context {
        &self.fx.ctx
    }

    async fn agent(&self, id: &str, cwd: Option<&str>) -> Arc<dyn Agent> {
        let agent = self.fx.create(id, cwd).await;
        agent.agent
    }

    async fn call(
        &self,
        name: &str,
        args: Value,
        agent: Option<Arc<dyn Agent>>,
    ) -> ToolExecutionResult {
        self.call_with(name, args, agent, Arc::new(AbortController::new()))
            .await
    }

    async fn call_with(
        &self,
        name: &str,
        args: Value,
        agent: Option<Arc<dyn Agent>>,
        controller: Arc<AbortController>,
    ) -> ToolExecutionResult {
        run_pipeline(&self.pipeline, name, args, agent, controller).await
    }
}

async fn run_pipeline(
    pipeline: &ToolExecutionPipeline,
    name: &str,
    args: Value,
    agent: Option<Arc<dyn Agent>>,
    controller: Arc<AbortController>,
) -> ToolExecutionResult {
    let mut input = ToolExecutionInput::new(
        format!("call-{name}").into(),
        name,
        args,
        controller.signal(),
    );
    input.agent = agent;
    let prepared = pipeline
        .prepare(input)
        .await
        .expect("the tool lookup does not reject");
    let exec = prepared.exec().clone();
    let outcome = match prepared {
        ScheduledToolPreparation::Dispatch(exec) => pipeline.dispatch(exec).await,
        ScheduledToolPreparation::PostResult { result, .. } => {
            ScheduledToolDispatch::PostResult(result)
        }
        ScheduledToolPreparation::FinalResult { result, .. } => {
            ScheduledToolDispatch::FinalResult(result)
        }
    };
    pipeline.complete(&exec, outcome).await
}

/// Which services the harness composes around the bash tool.
#[derive(Default)]
struct Setup {
    /// The job registry (`setupWithJobs`).
    jobs: Option<Arc<FakeJobs>>,
    /// `RecordingSandboxExecutor` instead of the plain recorder.
    sandboxed: bool,
    /// The shared sandbox-policy service (`SandboxPolicyService`).
    policy: bool,
    /// The real approval service (`ApprovalService`).
    approval: bool,
    /// The bash plugin's own config.
    tool_config: Option<Value>,
}

/// Foreground-only harness (`setup()` of the source): no job runtime.
async fn setup() -> Harness {
    setup_with(None).await
}

async fn setup_with(jobs: Option<Arc<FakeJobs>>) -> Harness {
    build(Setup {
        jobs,
        ..Setup::default()
    })
    .await
    .unwrap()
}

/// `setupSandboxed(withApproval, toolConfig)` of the source.
async fn setup_sandboxed(approval: bool, tool_config: Value) -> Harness {
    build(Setup {
        sandboxed: true,
        policy: true,
        approval,
        tool_config: Some(tool_config),
        jobs: Some(FakeJobs::new(false)),
    })
    .await
    .unwrap()
}

/// Mount everything; the bash plugin's own load error is returned, not unwrapped.
async fn build(setup: Setup) -> Result<Harness, BoxError> {
    let fx = wd::Fixture::new().await;
    let ctx = fx.ctx.clone();
    let jobs = setup.jobs.clone();
    if let Some(jobs) = setup.jobs {
        ctx.provide::<hoh_jobs::Jobs>(jobs).await?;
    }
    ctx.plugin::<ToolRuntime>(json!({}))?.settled().await?;
    let dsh_home = fx.root_text.clone() + "/dsh-home";
    ctx.plugin::<hoh_tool_executor_native::NativeToolExecutorPlugin>(json!({}))?
        .settled()
        .await?;
    ctx.plugin::<hoh_shell_env::ShellEnvPlugin>(json!({ "dshHome": dsh_home }))?
        .settled()
        .await?;
    if setup.policy {
        ctx.plugin::<hoh_sandbox_policy::SandboxPolicyService>(json!({}))?
            .settled()
            .await?;
    }
    let shell = if setup.sandboxed {
        Recorder::sandboxed()
    } else {
        Recorder::new()
    };
    hoh_shell::provide_shell(&ctx, shell.clone()).await?;
    if setup.approval {
        ctx.plugin::<hoh_user_approval::ApprovalService>(json!({}))?
            .settled()
            .await?;
    }
    let tool = ctx.plugin::<ToolBash>(setup.tool_config.unwrap_or_else(|| json!({})))?;
    tool.settled().await?;
    let pipeline = ToolExecutionPipeline::new(ctx.clone())?;
    Ok(Harness {
        fx,
        shell,
        tool,
        pipeline,
        dsh_home,
        jobs,
    })
}

fn text(result: &ToolExecutionResult) -> String {
    result
        .content()
        .iter()
        .filter(|block| block.r#type == "text")
        .filter_map(|block| block.text_value())
        .collect()
}

fn failure_info(result: &ToolExecutionResult) -> (String, String) {
    let ToolExecutionResult::Failure(failure) = result else {
        panic!("expected a failed call, got {result:?}");
    };
    let info = failure.error().info.as_ref().expect("error info");
    (info.name.clone(), info.code.clone())
}

fn success_value(result: &ToolExecutionResult) -> Value {
    let ToolExecutionResult::Success(success) = result else {
        panic!("expected a successful call, got {result:?}");
    };
    success.value().clone()
}

// ---------------------------------------------------------------------------------------------
// describe('bash tool')
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn returns_stdout_for_a_successful_command() {
    let h = setup().await;
    h.shell
        .respond(|spec| run_result(spec, "hello\n", "", Some(0)));
    let result = h
        .call(
            "bash",
            json!({"command": "echo hello", "description": "test command"}),
            None,
        )
        .await;
    assert!(!result.is_error());
    let value = success_value(&result);
    assert_eq!(value["kind"], "foreground");
    assert_eq!(value["exitCode"], 0);
    assert_eq!(value["signal"], Value::Null);
    assert_eq!(value["timedOut"], false);
    assert_eq!(value["aborted"], false);
    assert_eq!(
        value["stdout"],
        json!({"text": "hello\n", "truncated": false})
    );
    assert_eq!(value["stderr"], json!({"text": "", "truncated": false}));
    assert_eq!(text(&result), "hello\n");
}

#[tokio::test]
async fn reports_no_output_for_silent_commands() {
    let h = setup().await;
    h.shell.respond(|spec| run_result(spec, "", "", Some(0)));
    let result = h
        .call(
            "bash",
            json!({"command": "true", "description": "test command"}),
            None,
        )
        .await;
    assert_eq!(text(&result), "(no output)");
}

#[tokio::test]
async fn marks_stderr_sections() {
    let h = setup().await;
    h.shell
        .respond(|spec| run_result(spec, "out\n", "err\n", Some(0)));
    let result = h
        .call(
            "bash",
            json!({"command": "echo out; echo err >&2", "description": "test command"}),
            None,
        )
        .await;
    assert_eq!(text(&result), "out\n[stderr]\nerr\n");
    assert!(!result.is_error());
}

#[tokio::test]
async fn reports_non_zero_exits_without_is_error() {
    let h = setup().await;
    h.shell
        .respond(|spec| run_result(spec, "failing\n", "", Some(3)));
    let result = h
        .call(
            "bash",
            json!({"command": "echo failing; exit 3", "description": "test command"}),
            None,
        )
        .await;
    assert!(!result.is_error());
    assert_eq!(text(&result), "failing\n[exit code: 3]");
}

#[tokio::test]
async fn reports_timeout_kills_with_both_markers_timeout_first() {
    let h = setup().await;
    h.shell.respond(|spec| ShellRunResult {
        exit_code: None,
        signal: Some("SIGTERM".to_owned()),
        timed_out: true,
        ..run_result(spec, "", "", None)
    });
    let result = h
        .call(
            "bash",
            json!({"command": "sleep 60", "description": "test command", "timeoutMs": 100}),
            None,
        )
        .await;
    assert!(!result.is_error());
    assert_eq!(
        text(&result),
        "(no output)\n[timed out after 100ms]\n[killed by signal: SIGTERM]"
    );
    assert_eq!(h.shell.request(0).timeout_ms, Some(100.0));
}

#[tokio::test]
async fn reports_a_timeout_even_when_the_command_traps_the_signal_and_exits_zero() {
    let h = setup().await;
    h.shell.respond(|spec| ShellRunResult {
        timed_out: true,
        ..run_result(spec, "", "", Some(0))
    });
    let result = h
        .call(
            "bash",
            json!({"command": "trap \"exit 0\" TERM; sleep 60", "description": "test command", "timeoutMs": 100}),
            None,
        )
        .await;
    assert!(!result.is_error());
    assert!(text(&result).contains("[timed out after 100ms]"));
    assert!(!text(&result).contains("[exit code:"));
}

#[tokio::test]
async fn reports_truncation_with_the_spill_path() {
    let h = setup().await;
    h.shell.respond(|spec| ShellRunResult {
        stdout: CollectedOutput {
            text: "line-0100\n".to_owned(),
            truncated: true,
            spill_path: Some("/spill/out.log".to_owned()),
        },
        ..run_result(spec, "", "", Some(0))
    });
    let result = h
        .call(
            "bash",
            json!({"command": "seq", "description": "test command"}),
            None,
        )
        .await;
    assert!(text(&result).contains("[output truncated; full output: "));
    assert!(text(&result).contains("line-0100"));
}

#[tokio::test]
async fn honors_workdir() {
    let h = setup().await;
    let result = h
        .call(
            "bash",
            json!({"command": "pwd", "description": "test command", "workdir": "/tmp"}),
            None,
        )
        .await;
    assert!(!result.is_error());
    assert_eq!(lock(&h.shell.specs)[0].workdir, "/tmp");
}

#[tokio::test]
async fn surfaces_spawn_failures_as_is_error() {
    let h = setup().await;
    h.shell.set(Mode::Fail(Arc::new(|_| {
        Box::new(std::io::Error::from_raw_os_error(2))
    })));
    let result = h
        .call(
            "bash",
            json!({"command": "true", "description": "test command", "workdir": "/nonexistent-dsh"}),
            None,
        )
        .await;
    assert!(result.is_error());
    assert!(text(&result).contains("ENOENT"), "{}", text(&result));
}

#[tokio::test]
async fn surfaces_foreground_aborts_as_the_structured_tool_aborted_error() {
    let h = setup().await;
    h.shell.set(Mode::WaitAbort);
    let controller = Arc::new(AbortController::new());
    let stopper = controller.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        stopper.abort();
    });
    let result = h
        .call_with(
            "bash",
            json!({"command": "sleep 60", "description": "test command"}),
            None,
            controller,
        )
        .await;
    assert!(result.is_error());
    let ToolExecutionResult::Failure(failure) = &result else {
        unreachable!()
    };
    assert_eq!(failure.error().message, "tool call aborted");
    assert_eq!(
        failure_info(&result),
        ("AbortError".to_owned(), TOOL_ABORTED.to_owned())
    );
}

#[tokio::test]
async fn rejects_schema_invalid_args() {
    let h = setup().await;
    let cases = [
        (json!({}), "missing required property \"command\""),
        (
            json!({"command": 42, "description": "d"}),
            "\"command\" must be a string",
        ),
        (
            json!({"command": "x"}),
            "missing required property \"description\"",
        ),
        (
            json!({"command": "x", "description": 7}),
            "\"description\" must be a string",
        ),
        (
            json!({"command": "x", "description": "d", "timeoutMs": "soon"}),
            "\"timeoutMs\" must be a number",
        ),
        (
            json!({"command": "x", "description": "d", "workdir": 7}),
            "\"workdir\" must be a string",
        ),
    ];
    for (args, expected) in cases {
        let result = h.call("bash", args.clone(), None).await;
        assert!(result.is_error(), "{args}");
        assert!(
            text(&result).contains(expected),
            "{args}: {}",
            text(&result)
        );
    }
}

#[tokio::test]
async fn rejects_value_invalid_args() {
    let h = setup().await;
    let cases = [
        (
            json!({"command": "  ", "description": "d"}),
            "invalid command",
        ),
        (
            json!({"command": "x", "description": "   "}),
            "invalid description",
        ),
        (
            json!({"command": "x", "description": "d", "timeoutMs": -1}),
            "invalid timeoutMs",
        ),
    ];
    for (args, expected) in cases {
        let result = h.call("bash", args.clone(), None).await;
        assert_eq!(
            failure_info(&result),
            ("ToolArgsError".to_owned(), "INVALID_ARGS".to_owned()),
            "{args}"
        );
        assert!(
            text(&result).contains(expected),
            "{args}: {}",
            text(&result)
        );
    }
}

#[tokio::test]
async fn requests_description_before_command_in_the_foreground_only_schema() {
    let h = setup().await;
    let tools = h.ctx().get::<Tools>().unwrap();
    let schemas = tools.schemas(None, None).unwrap();
    assert_eq!(schemas.len(), 1);
    let schema = &schemas[0];
    assert_eq!(schema.name, "bash");
    let keys: Vec<_> = schema.parameters["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, ["description", "command", "timeoutMs", "workdir"]);
    assert_eq!(
        schema.parameters["required"],
        json!(["description", "command"])
    );
    assert!(
        schema
            .description
            .contains("Provide `description` before `command` in the arguments.")
    );
    let rendered = serde_json::to_string(&schema.parameters).unwrap();
    assert!(!rendered.contains("job_output"));
    assert!(rendered.contains("kills the command on expiry"));
}

const BUILT_IN_BEFORE: [&str; 2] = ["harness:identity", "deployment:persona-prefix"];
const BUILT_IN_AFTER: &str = "deployment:persona-suffix";

fn section_names(assembly: &hoh_system_prompt::PromptAssembly) -> Vec<String> {
    assembly
        .sections
        .iter()
        .map(|section| section.name.clone())
        .collect()
}

#[tokio::test]
async fn contributes_the_exit_code_habit_as_its_prompt_section() {
    let h = setup().await;
    let prompt = h.ctx().get::<SystemPromptKey>().unwrap();
    // Neighbours at `TOOL_BASH ± 10` pin the section's order, not only its presence.
    let order = SystemPrompt::get_section_order(PromptSectionOrderName::ToolBash);
    for (name, order) in [
        ("test:before-bash", order - 10.0),
        ("test:after-bash", order + 10.0),
    ] {
        prompt
            .section(
                h.ctx(),
                PromptSection {
                    name: name.to_owned(),
                    order,
                    text: name.into(),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let assembly = prompt.assemble(AssembleContext::new(None)).await.unwrap();
    assert_eq!(
        section_names(&assembly),
        [
            BUILT_IN_BEFORE[0],
            BUILT_IN_BEFORE[1],
            "test:before-bash",
            "tool:bash",
            "test:after-bash",
            BUILT_IN_AFTER
        ]
    );
    let section = assembly
        .sections
        .iter()
        .find(|section| section.name == "tool:bash")
        .expect("the tool:bash section");
    assert!(section.text.contains("[exit code: N]"));
    assert_eq!(
        section.text,
        "Check the [exit code: N] marker on every bash result; investigate failures before moving on."
    );
}

#[tokio::test]
async fn unregisters_everything_when_the_plugin_fiber_is_disposed() {
    let h = setup().await;
    let tools = h.ctx().get::<Tools>().unwrap();
    assert_eq!(tools.schemas(None, None).unwrap().len(), 1);
    let prompt = h.ctx().get::<SystemPromptKey>().unwrap();
    assert_eq!(
        section_names(&prompt.assemble(AssembleContext::new(None)).await.unwrap()),
        [
            BUILT_IN_BEFORE[0],
            BUILT_IN_BEFORE[1],
            "tool:bash",
            BUILT_IN_AFTER
        ]
    );
    h.tool.dispose().await;
    assert_eq!(tools.schemas(None, None).unwrap().len(), 0);
    // Only the system-prompt plugin's own built-in sections remain.
    assert_eq!(
        section_names(&prompt.assemble(AssembleContext::new(None)).await.unwrap()),
        [BUILT_IN_BEFORE[0], BUILT_IN_BEFORE[1], BUILT_IN_AFTER]
    );
}

#[tokio::test]
async fn present_call_validates_softly_and_never_throws_on_malformed_args() {
    let h = setup().await;
    let tools = h.ctx().get::<Tools>().unwrap();
    let definition = tools.get("bash", None, None).expect("the bash tool");
    let present = definition.present_call.as_ref().unwrap();
    // `defineTool` soft-validates replayed logged args: a missing required `description` gives the
    // generic presentation (`None`) instead of a failure.
    assert!(present(&json!({"command": "ls"})).is_none());
    assert!(present(&json!("not an object")).is_none());
    // A well-formed call is presented, so the `None` above is the validation and not a dead hook.
    assert!(present(&json!({"command": "ls", "description": "list"})).is_some());
}

// ---------------------------------------------------------------------------------------------
// describe('session-cwd routing (per-session workdir)')
// ---------------------------------------------------------------------------------------------

async fn pwd_in(h: &Harness, workdir: Option<&str>, agent: Option<Arc<dyn Agent>>) -> String {
    h.shell
        .respond(|spec| run_result(spec, &format!("{}\n", spec.workdir), "", Some(0)));
    let mut args = json!({"command": "pwd", "description": "pwd"});
    if let Some(workdir) = workdir {
        args["workdir"] = json!(workdir);
    }
    text(&h.call("bash", args, agent).await).trim().to_owned()
}

// A missing conversation folder requires a new explicit selection before running the command.
#[tokio::test]
async fn refuses_to_run_when_the_selected_working_directory_is_unavailable() {
    let h = setup().await;
    let project = h.fx.root_text.clone();
    let owner = h.fx.create("cwd-recovery", Some(&project)).await;
    let child = h.fx.path(&["tmp"]);
    std::fs::create_dir(&child).unwrap();
    h.fx.service.set(&owner.agent, &child, None).await.unwrap();
    std::fs::remove_dir_all(&child).unwrap();
    let reply = pwd_in(&h, None, Some(owner.agent.clone())).await;
    assert!(reply.contains(child.as_str()), "{reply}");
    assert!(reply.contains("is unavailable"), "{reply}");
    assert!(lock(&h.shell.requests).is_empty());
}

#[tokio::test]
async fn defaults_bash_to_the_agents_session_cwd() {
    let h = setup().await;
    let agent = h.agent("cwd-default", Some("/tmp")).await;
    assert_eq!(pwd_in(&h, None, Some(agent)).await, "/tmp");
}

#[tokio::test]
async fn an_explicit_absolute_workdir_overrides_the_session_cwd() {
    let h = setup().await;
    let agent = h.agent("cwd-absolute", Some("/")).await;
    assert_eq!(pwd_in(&h, Some("/tmp"), Some(agent)).await, "/tmp");
}

#[tokio::test]
async fn a_relative_workdir_is_resolved_against_the_session_cwd() {
    let h = setup().await;
    let agent = h.agent("cwd-relative", Some("/usr")).await;
    assert_eq!(pwd_in(&h, Some("bin"), Some(agent)).await, "/usr/bin");
}

#[tokio::test]
async fn two_sessions_with_different_cwds_each_run_bash_in_their_own_dir() {
    let h = setup().await;
    let usr = h.agent("cwd-usr", Some("/usr")).await;
    let tmp = h.agent("cwd-tmp", Some("/tmp")).await;
    assert_eq!(pwd_in(&h, None, Some(usr)).await, "/usr");
    assert_eq!(pwd_in(&h, None, Some(tmp)).await, "/tmp");
}

#[tokio::test]
async fn falls_back_to_the_executor_default_when_there_is_no_agent() {
    let h = setup().await;
    let output = pwd_in(&h, None, None).await;
    assert!(!output.is_empty());
    assert_eq!(h.shell.request(0).workdir, None);
}

// ---------------------------------------------------------------------------------------------
// describe('the model-facing bash tool builds its request from named args only')
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn describes_the_managed_harness_environment_namespace_to_the_model() {
    let h = setup().await;
    let tools = h.ctx().get::<Tools>().unwrap();
    let definition = tools.get("bash", None, None).expect("the bash tool");
    assert!(definition.description.contains("$DSH_*"));
}

#[tokio::test]
async fn injects_built_ins_and_the_stable_session_id_into_a_foreground_request() {
    let h = setup().await;
    let agent = h.agent("request-fg", None).await;
    h.call(
        "bash",
        json!({"command": "true", "description": "run command"}),
        Some(agent),
    )
    .await;
    let env = serde_json::to_value(h.shell.request(0).dsh_env.unwrap()).unwrap();
    assert_eq!(env["DSH_HOME"], h.dsh_home.as_str());
    assert_eq!(env["DSH_SESSION_ID"], "request-fg");
    assert_eq!(env["DSH_SHELL"], "1");
    // SHIM: the Rust tool is one executable, so the Node executable is never advertised.
    assert!(env.get("DSH_NODE_EXECUTABLE").is_none());
}

#[tokio::test]
async fn keeps_parent_and_child_agent_session_environments_isolated() {
    let h = setup().await;
    let parent = h.agent("request-parent", None).await;
    let child = h.agent("request-child", None).await;
    for agent in [parent, child] {
        h.call(
            "bash",
            json!({"command": "true", "description": "run command"}),
            Some(agent),
        )
        .await;
    }
    let ids: Vec<_> = (0..2)
        .map(|index| {
            serde_json::to_value(h.shell.request(index).dsh_env.unwrap()).unwrap()["DSH_SESSION_ID"]
                .clone()
        })
        .collect();
    assert_eq!(ids, [json!("request-parent"), json!("request-child")]);
}

#[tokio::test]
async fn does_not_forward_trusted_only_fields_even_when_the_model_adds_extra_arguments() {
    let h = setup().await;
    h.call(
        "bash",
        json!({
            "command": "echo hi", "description": "echo",
            "env": {"SNEAKY_API_KEY": "leak"}, "stdin": "malicious payload", "stdoutMaxBytes": 999_999
        }),
        None,
    )
    .await;
    assert_eq!(lock(&h.shell.requests).len(), 1);
    let request = h.shell.request(0);
    assert_eq!(request.command, "echo hi");
    assert_eq!(request.env, None);
    assert_eq!(request.stdin, None);
    assert_eq!(request.stdout_max_bytes, None);
}

// ---------------------------------------------------------------------------------------------
// describe('resolved launch directory metadata') — the foreground rows
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn records_the_executor_directory_for_a_foreground_call() {
    for workdir in [None, Some("relative")] {
        let h = setup().await;
        let directory = h.fx.root_text.clone() + "/executor-dir";
        *lock(&h.shell.workdir_override) = Some(directory.clone());
        let owner = h.agent("directory-metadata", Some(&h.fx.root_text)).await;
        let mut args =
            json!({"command": "printf metadata-ok", "description": "Read launch metadata"});
        if let Some(workdir) = workdir {
            args["workdir"] = json!(workdir);
        }
        h.shell
            .respond(|spec| run_result(spec, "metadata-ok", "", Some(0)));
        let result = h.call("bash", args, Some(owner)).await;
        assert!(!result.is_error());
        assert_eq!(success_value(&result)["cwd"], directory.as_str());
        assert_eq!(result.meta(), Some(&json!({"cwd": directory})));
        assert_eq!(lock(&h.shell.requests).len(), 1);
        let expected = match workdir {
            None => h.fx.root_text.clone(),
            Some(relative) => format!("{}/{relative}", h.fx.root_text),
        };
        assert_eq!(
            h.shell.request(0).workdir.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(lock(&h.shell.specs)[0].workdir, directory);
        assert_eq!(text(&result), "metadata-ok");
    }
}

// The `background=true` rows of the two blocks above, on the scripted job registry.

#[tokio::test]
async fn injects_the_same_trusted_variables_into_a_background_request_without_forwarding_model_env()
{
    let h = setup_with(Some(FakeJobs::new(false))).await;
    let agent = h.agent("request-bg", None).await;
    h.call(
        "bash",
        json!({
            "command": "sleep 1", "description": "run command", "run_in_background": true,
            "env": {"DSH_SESSION_ID": "spoofed"}
        }),
        Some(agent),
    )
    .await;
    let request = h.shell.request(0);
    assert_eq!(request.env, None);
    let env = serde_json::to_value(request.dsh_env.unwrap()).unwrap();
    assert_eq!(env["DSH_HOME"], h.dsh_home.as_str());
    assert_eq!(env["DSH_SESSION_ID"], "request-bg");
    assert_eq!(env["DSH_SHELL"], "1");
    assert!(env.get("DSH_NODE_EXECUTABLE").is_none());
}

#[tokio::test]
async fn a_background_bash_call_likewise_carries_no_trusted_only_fields() {
    let h = setup_with(Some(FakeJobs::new(false))).await;
    let result = h
        .call(
            "bash",
            json!({
                "command": "sleep 1", "description": "sleep", "run_in_background": true,
                "env": {"TOKEN": "leak"}, "stdin": "x", "stdoutMaxBytes": 999_999
            }),
            None,
        )
        .await;
    // The call really went down the background path, so the absent fields are a real negative.
    assert_eq!(text(&result), "started background job bash-1");
    assert_eq!(lock(&h.shell.requests).len(), 1);
    let request = h.shell.request(0);
    assert_eq!(request.command, "sleep 1");
    assert_eq!(request.env, None);
    assert_eq!(request.stdin, None);
    assert_eq!(request.stdout_max_bytes, None);
}

#[tokio::test]
async fn records_the_executor_directory_for_a_background_call() {
    for workdir in [None, Some("relative")] {
        let h = setup_with(Some(FakeJobs::new(false))).await;
        let directory = h.fx.root_text.clone() + "/executor-dir";
        *lock(&h.shell.workdir_override) = Some(directory.clone());
        let owner = h.agent("directory-metadata", Some(&h.fx.root_text)).await;
        let mut args = json!({
            "command": "printf metadata-ok", "description": "Read launch metadata",
            "run_in_background": true
        });
        if let Some(workdir) = workdir {
            args["workdir"] = json!(workdir);
        }
        let result = h.call("bash", args, Some(owner)).await;
        assert!(!result.is_error(), "{}", text(&result));
        assert_eq!(success_value(&result)["cwd"], directory.as_str());
        assert_eq!(result.meta(), Some(&json!({"cwd": directory})));
        assert_eq!(lock(&h.shell.requests).len(), 1);
        let expected = match workdir {
            None => h.fx.root_text.clone(),
            Some(relative) => format!("{}/{relative}", h.fx.root_text),
        };
        assert_eq!(
            h.shell.request(0).workdir.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(lock(&h.shell.specs)[0].workdir, directory);
        assert_eq!(text(&result), "started background job bash-1");
    }
}

// ---------------------------------------------------------------------------------------------
// describe('launch failures without a job registry')
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn classifies_a_platform_spawn_error() {
    let h = setup().await;
    h.shell.set(Mode::Fail(Arc::new(|_| {
        Box::new(std::io::Error::from_raw_os_error(2))
    })));
    let result = h
        .call("bash", json!({"command": "true", "description": "d"}), None)
        .await;
    assert_eq!(
        failure_info(&result),
        (
            "SubprocessError".to_owned(),
            "SUBPROCESS_SPAWN_NOT_FOUND".to_owned()
        )
    );
}

#[tokio::test]
async fn keeps_this_calls_cancellation_when_the_launch_rejects_with_the_abort_reason() {
    let h = setup().await;
    let controller = Arc::new(AbortController::new());
    let aborting = controller.clone();
    h.shell.set(Mode::Fail(Arc::new(move |_| {
        aborting.abort_with("caller stopped");
        Box::new(aborting.signal().reason().expect("the reason"))
    })));
    let result = h
        .call_with(
            "bash",
            json!({"command": "true", "description": "d"}),
            None,
            controller,
        )
        .await;
    assert_eq!(
        failure_info(&result),
        ("AbortError".to_owned(), TOOL_ABORTED.to_owned())
    );
}

#[tokio::test]
async fn classifies_a_spawn_error_that_settles_after_this_call_was_cancelled() {
    let h = setup().await;
    let controller = Arc::new(AbortController::new());
    let aborting = controller.clone();
    h.shell.set(Mode::Fail(Arc::new(move |_| {
        aborting.abort_with("caller stopped");
        Box::new(std::io::Error::from_raw_os_error(2))
    })));
    let result = h
        .call_with(
            "bash",
            json!({"command": "true", "description": "d"}),
            None,
            controller,
        )
        .await;
    assert_eq!(
        failure_info(&result),
        (
            "SubprocessError".to_owned(),
            "SUBPROCESS_SPAWN_NOT_FOUND".to_owned()
        )
    );
}

// ---------------------------------------------------------------------------------------------
// A scripted `hoh-jobs` registry: the tool's wiring (JobSpec, JobHandle, wait, read, remove).
// ---------------------------------------------------------------------------------------------

struct NoEvents;

impl JobEvents for NoEvents {
    fn subscribe(
        &self,
        _ctx: &Context,
        _filter: JobEventFilter,
        _listener: JobEventListener,
    ) -> Result<Disposer, KernelError> {
        unimplemented!("bash does not subscribe")
    }
}

/// What `start` saw of one `JobSpec`.
#[derive(Clone, Debug, PartialEq)]
struct Started {
    kind: String,
    label: String,
    owner: Option<SessionId>,
    call_id: Option<String>,
    output_limit_bytes: Option<u64>,
    channels: Option<Vec<Option<hoh_jobs::JobChannel>>>,
}

struct FakeJobs {
    /// `wait` reports the job still running instead of settled.
    keep_running: bool,
    started: Mutex<Vec<Started>>,
    hooks: Mutex<Option<JobHooks>>,
    reads: Mutex<usize>,
    removed: Mutex<Vec<JobId>>,
    killed: Mutex<Vec<(JobId, Option<String>)>>,
    /// The records `list` shows: added by `start`, dropped by `remove`.
    live: Mutex<Vec<JobId>>,
    /// `wait` settles as killed once the job was killed (a registry whose kill takes effect).
    settles_on_kill: bool,
    events: NoEvents,
}

impl FakeJobs {
    fn new(keep_running: bool) -> Arc<Self> {
        Arc::new(Self {
            keep_running,
            started: Mutex::default(),
            hooks: Mutex::default(),
            reads: Mutex::default(),
            removed: Mutex::default(),
            killed: Mutex::default(),
            live: Mutex::default(),
            settles_on_kill: false,
            events: NoEvents,
        })
    }

    /// A registry that keeps the job running until it is killed, then settles it as killed.
    fn settling_on_kill() -> Arc<Self> {
        Arc::new(Self {
            keep_running: true,
            settles_on_kill: true,
            started: Mutex::default(),
            hooks: Mutex::default(),
            reads: Mutex::default(),
            removed: Mutex::default(),
            killed: Mutex::default(),
            live: Mutex::default(),
            events: NoEvents,
        })
    }

    fn listed(&self) -> Vec<JobId> {
        lock(&self.live).clone()
    }

    fn view(id: &JobId, status: JobStatus) -> JobView {
        JobView {
            id: id.clone(),
            kind: "bash".into(),
            label: "label".into(),
            owner: None,
            output_limit_bytes: None,
            status,
            progress: None,
            detail: None,
            started_at: 0,
            finished_at: None,
            output: Default::default(),
        }
    }
}

impl JobRegistryApi for FakeJobs {
    fn events(&self) -> &dyn JobEvents {
        &self.events
    }

    fn start(&self, spec: JobSpec) -> Result<JobId, BoxError> {
        let id = JobId::from("bash-1");
        lock(&self.started).push(Started {
            kind: spec.kind.clone(),
            label: spec.label.clone(),
            owner: spec.owner.clone(),
            call_id: spec.tool_call.as_ref().map(|call| call.call_id.to_string()),
            output_limit_bytes: spec.output_limit_bytes,
            channels: spec
                .output
                .as_ref()
                .map(|sources| sources.iter().map(|source| source.channel).collect()),
        });
        let handle = JobHandle::new(id.clone(), |_, _| {}, |_| {});
        *lock(&self.hooks) = Some((spec.run)(handle)?);
        lock(&self.live).push(id.clone());
        Ok(id)
    }

    fn list(&self, _caller: Option<&SessionId>) -> Vec<JobView> {
        self.listed()
            .iter()
            .map(|id| Self::view(id, JobStatus::Running))
            .collect()
    }

    fn get(&self, id: &JobId, _caller: Option<&SessionId>) -> Result<JobView, BoxError> {
        Ok(Self::view(id, JobStatus::Running))
    }

    fn read(&self, id: &JobId, _caller: Option<&SessionId>) -> Result<JobRead, BoxError> {
        *lock(&self.reads) += 1;
        Ok(JobRead {
            chunks: vec![hoh_jobs::JobChunk {
                at: 0,
                text: "so far\n".into(),
                channel: Some(hoh_jobs::JobChannel::Stdout),
                gap_before: false,
            }],
            lossy: false,
            result: None,
            job: Self::view(id, JobStatus::Running),
        })
    }

    fn read_at(
        &self,
        _id: &JobId,
        _from: f64,
        _caller: Option<&SessionId>,
    ) -> Result<JobOutputRead, BoxError> {
        unimplemented!("bash does not read at an offset")
    }

    fn kill(
        &self,
        id: &JobId,
        _caller: Option<&SessionId>,
        reason: Option<&str>,
    ) -> Result<KillOutcome, BoxError> {
        lock(&self.killed).push((id.clone(), reason.map(str::to_owned)));
        if let Some(hooks) = lock(&self.hooks).as_ref() {
            (hooks.cancel)(reason)?;
        }
        Ok(KillOutcome::Requested)
    }

    fn wait(
        &self,
        id: &JobId,
        _timeout_ms: f64,
        _caller: Option<&SessionId>,
        _signal: Option<AbortSignal>,
    ) -> JobFuture<'static, Result<JobView, BoxError>> {
        let id = id.clone();
        if self.settles_on_kill && !lock(&self.killed).is_empty() {
            return Box::pin(async move { Ok(Self::view(&id, JobStatus::Killed)) });
        }
        if self.keep_running {
            // The registry's wait lasts the call's timeout: the process is published by then.
            return Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(Self::view(&id, JobStatus::Running))
            });
        }
        let done = lock(&self.hooks).take().map(|hooks| hooks.done);
        Box::pin(async move {
            let mut view = Self::view(&id, JobStatus::Completed);
            if let Some(done) = done {
                view.detail = done.await?.detail;
            }
            Ok(view)
        })
    }

    fn remove(&self, id: &JobId, _caller: Option<&SessionId>) -> Result<(), BoxError> {
        lock(&self.removed).push(id.clone());
        lock(&self.live).retain(|live| live != id);
        Ok(())
    }

    fn attach_controller(&self, _ctx: &Context, _name: &str) -> Result<Disposer, KernelError> {
        unimplemented!("bash attaches no controller")
    }
}

#[tokio::test]
async fn background_call_registers_a_bash_job_with_both_stream_sources_and_returns_its_id() {
    let jobs = FakeJobs::new(true);
    let h = setup_with(Some(jobs.clone())).await;
    let agent = h.agent("owner", None).await;
    let owner = agent.session().id().clone();
    let result = h
        .call(
            "bash",
            json!({"command":"sleep 5","description":"d","run_in_background":true}),
            Some(agent),
        )
        .await;
    let value = success_value(&result);
    assert_eq!(value["kind"], "background");
    assert_eq!(value["jobId"], "bash-1");
    assert_eq!(
        lock(&jobs.started).as_slice(),
        [Started {
            kind: "bash".into(),
            label: "sleep 5".into(),
            owner: Some(owner),
            call_id: Some("call-bash".into()),
            output_limit_bytes: None,
            channels: Some(vec![
                Some(hoh_jobs::JobChannel::Stdout),
                Some(hoh_jobs::JobChannel::Stderr)
            ]),
        }]
    );
    // As in the source, the executor is called inside `registry.start`: the recorded request exists
    // by the time the call returns, with no yield in between.
    assert_eq!(lock(&h.shell.specs).len(), 1);
    let hooks = lock(&jobs.hooks).take().expect("run produced hooks");
    (hooks.cancel)(Some("stop")).unwrap();
    let outcome = hooks.done.await.unwrap();
    assert_eq!(outcome.status, hoh_jobs::JobOutcomeStatus::Completed);
    assert_eq!(outcome.detail.as_deref(), Some("exit code: 0"));
}

#[tokio::test]
async fn foreground_call_settling_during_the_wait_leaves_the_registry_with_its_result() {
    let jobs = FakeJobs::new(false);
    let h = setup_with(Some(jobs.clone())).await;
    h.shell
        .respond(|spec| run_result(spec, "hello\n", "", Some(0)));
    let result = h
        .call(
            "bash",
            json!({"command":"echo hello","description":"d"}),
            None,
        )
        .await;
    assert_eq!(text(&result), "hello\n");
    assert_eq!(success_value(&result)["kind"], "foreground");
    assert_eq!(lock(&jobs.removed).as_slice(), [JobId::from("bash-1")]);
    assert_eq!(*lock(&jobs.reads), 0);
}

#[tokio::test]
async fn foreground_call_still_running_after_the_wait_is_promoted_with_one_consuming_read() {
    let jobs = FakeJobs::new(true);
    let h = setup_with(Some(jobs.clone())).await;
    let result = h
        .call(
            "bash",
            json!({"command":"sleep 60","description":"d","timeoutMs":50}),
            None,
        )
        .await;
    let value = success_value(&result);
    assert_eq!(value["kind"], "promoted");
    assert_eq!(value["jobId"], "bash-1");
    assert_eq!(*lock(&jobs.reads), 1);
    assert!(lock(&jobs.removed).is_empty());
    assert!(lock(&jobs.killed).is_empty());
}

// ---------------------------------------------------------------------------------------------
// Cases that need no job registry: the missing runtime, the opt-out, and the sandbox escalation
// path (tools.spec.ts: 'fails loud when the job runtime is not loaded', 'enableRunInBackground:
// false …', 'sandbox escalation through the generic task producer').
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn fails_loud_when_the_job_runtime_is_not_loaded() {
    let h = setup().await; // no job registry
    let result = h
        .call(
            "bash",
            json!({"command": "sleep 60", "description": "test command", "run_in_background": true}),
            None,
        )
        .await;
    assert!(result.is_error());
    assert_eq!(
        failure_info(&result),
        (
            "ToolContextUnavailableError".to_owned(),
            "TOOL_CONTEXT_UNAVAILABLE".to_owned()
        )
    );
    assert!(text(&result).contains(
        "background jobs unavailable: load @deepseek-ai/dsh-jobs and @deepseek-ai/dsh-tool-jobs"
    ));
    assert!(lock(&h.shell.specs).is_empty());
}

#[tokio::test]
async fn enable_run_in_background_false_removes_the_parameter_and_rejects_the_call() {
    let h = build(Setup {
        tool_config: Some(json!({"enableRunInBackground": false})),
        ..Setup::default()
    })
    .await
    .unwrap();
    let tools = h.ctx().get::<Tools>().unwrap();
    let schemas = tools.schemas(None, None).unwrap();
    let schema = schemas.iter().find(|s| s.name == "bash").unwrap();
    let keys: Vec<_> = schema.parameters["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, ["description", "command", "timeoutMs", "workdir"]);
    assert!(!schema.description.contains("run_in_background"));
    // The registry-held definition agrees: schema and capability never disagree.
    let held = tools.get("bash", None, None).unwrap();
    assert!(
        held.parameters["properties"]
            .get("run_in_background")
            .is_none()
    );

    // Schema omission is advertising; execution must also enforce the opt-out.
    let forced = h
        .call(
            "bash",
            json!({"command": "echo hi", "description": "test command", "run_in_background": true}),
            None,
        )
        .await;
    assert_eq!(
        failure_info(&forced),
        ("ToolArgsError".to_owned(), "INVALID_ARGS".to_owned())
    );
    assert!(text(&forced).contains(
        "invalid arguments: run_in_background is disabled for this deployment (enableRunInBackground: false)"
    ));
    let foreground = h
        .call(
            "bash",
            json!({"command": "echo hi", "description": "test command"}),
            None,
        )
        .await;
    assert!(!foreground.is_error());
}

#[tokio::test]
async fn fails_load_when_a_confining_executor_has_no_shared_sandbox_policy_resolver() {
    let error = build(Setup {
        sandboxed: true,
        ..Setup::default()
    })
    .await
    .err()
    .expect("the load fails");
    assert!(
        error.to_string().contains(
            "tool-bash: the mounted bash executor confines but ctx.sandboxPolicy is missing"
        ),
        "{error}"
    );
}

const ESCALATE: &str = r#"{"command":"true","description":"test escalation","sandbox_permissions":"workspace-write","justification":"the command needs workspace writes"}"#;

fn escalate() -> Value {
    serde_json::from_str(ESCALATE).unwrap()
}

fn without(mut args: Value, key: &str) -> Value {
    args.as_object_mut().unwrap().remove(key);
    args
}

use hoh_sandbox::SandboxMode;
use hoh_user_approval::{ApprovalOutcome, ApprovalRequestEvent};
use std::sync::atomic::{AtomicUsize, Ordering};

/// `sandboxAgent(mode)`: a real agent with an open turn and, optionally, a session sandbox mode.
async fn sandbox_agent(h: &Harness, mode: Option<SandboxMode>) -> Arc<dyn Agent> {
    let agent = h.agent("sandbox-session", None).await;
    agent
        .session()
        .append("turn/start", json!({"turn": 1}), None)
        .unwrap();
    if let Some(mode) = mode {
        hoh_sandbox_policy::set_sandbox_mode(agent.session(), mode).unwrap();
    }
    agent
}

/// `ctx.on('approval/request', …)`; the returned counter is the source's `prompted` spy.
fn answer_with(h: &Harness, outcome: ApprovalOutcome) -> Arc<AtomicUsize> {
    let prompted = Arc::new(AtomicUsize::new(0));
    let seen = prompted.clone();
    h.ctx()
        .on_waterfall::<ApprovalRequestEvent, Value, _>(move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            async move { Ok(outcome.into()) }
        })
        .unwrap();
    prompted
}

fn modes(h: &Harness) -> Vec<Option<SandboxMode>> {
    lock(&h.shell.modes).clone()
}

#[tokio::test]
async fn advertises_the_sandbox_fields_and_validates_their_pairing() {
    let h = setup_sandboxed(false, json!({})).await;
    let tools = h.ctx().get::<Tools>().unwrap();
    let schemas = tools.schemas(None, None).unwrap();
    let schema = schemas.iter().find(|s| s.name == "bash").unwrap();
    let permissions = &schema.parameters["properties"]["sandbox_permissions"];
    assert_eq!(
        permissions["enum"],
        json!(["workspace-write", "danger-full-access"])
    );
    assert!(
        permissions["description"]
            .as_str()
            .unwrap()
            .contains("asks the user for approval")
    );
    for args in [
        json!({"command": "true", "description": "d", "sandbox_permissions": "workspace-write"}),
        json!({"command": "true", "description": "d", "justification": "why"}),
        json!({"command": "true", "description": "d", "sandbox_permissions": "workspace-write", "justification": " "}),
    ] {
        let result = h.call("bash", args, None).await;
        assert_eq!(
            failure_info(&result),
            (
                "SandboxEscalationError".to_owned(),
                "SANDBOX_ESCALATION_INVALID_ARGUMENT".to_owned()
            ),
            "{}",
            text(&result)
        );
    }
}

#[tokio::test]
async fn rejects_injected_escalation_without_a_sandbox_and_narrower_escalation_without_prompting() {
    let plain = setup().await;
    let both = escalate();
    for args in [
        both.clone(),
        without(both.clone(), "justification"),
        without(both.clone(), "sandbox_permissions"),
    ] {
        let unavailable = plain.call("bash", args, None).await;
        assert!(text(&unavailable).contains(
            "invalid arguments: sandbox_permissions and justification are not available in this composition"
        ));
        assert_eq!(
            failure_info(&unavailable),
            ("ToolArgsError".to_owned(), "INVALID_ARGS".to_owned())
        );
    }
    assert!(lock(&plain.shell.specs).is_empty());

    let h = setup_sandboxed(true, json!({})).await;
    let prompted = answer_with(&h, ApprovalOutcome::AllowedOnce);
    let agent = sandbox_agent(&h, Some(SandboxMode::DangerFullAccess)).await;
    let result = h
        .call(
            "bash",
            json!({
                "command": "true", "description": "test escalation",
                "sandbox_permissions": "workspace-write", "justification": "narrower"
            }),
            Some(agent),
        )
        .await;
    assert!(
        text(&result).contains("not strictly wider"),
        "{}",
        text(&result)
    );
    assert_eq!(prompted.load(Ordering::SeqCst), 0);
    assert!(modes(&h).is_empty());
}

#[tokio::test]
async fn runs_without_escalation_for_a_missing_or_blank_justification() {
    for justification in [None, Some(""), Some(" \t\n")] {
        let h = setup_sandboxed(true, json!({})).await;
        let prompted = answer_with(&h, ApprovalOutcome::AllowedOnce);
        let mut args = json!({"command": "true", "description": "ordinary"});
        if let Some(justification) = justification {
            args["justification"] = json!(justification);
        }
        let agent = sandbox_agent(&h, Some(SandboxMode::WorkspaceWrite)).await;
        let result = h.call("bash", args, Some(agent)).await;
        assert!(!result.is_error(), "{}", text(&result));
        assert_eq!(text(&result), "ok");
        assert_eq!(modes(&h), [Some(SandboxMode::WorkspaceWrite)]);
        assert_eq!(prompted.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn rejects_an_explicit_mode_with_a_blank_justification_before_execution() {
    for justification in ["", " \t\n"] {
        let h = setup_sandboxed(true, json!({})).await;
        let prompted = answer_with(&h, ApprovalOutcome::AllowedOnce);
        let mut args = escalate();
        args["justification"] = json!(justification);
        let agent = sandbox_agent(&h, None).await;
        let result = h.call("bash", args, Some(agent)).await;
        assert!(result.is_error());
        assert!(text(&result).contains("invalid justification: expected a non-empty sentence"));
        assert!(modes(&h).is_empty());
        assert_eq!(prompted.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn rejects_a_forced_background_call_before_asking_for_an_escalation() {
    let h = setup_sandboxed(true, json!({"enableRunInBackground": false})).await;
    let prompted = answer_with(&h, ApprovalOutcome::AllowedOnce);
    let mut args = escalate();
    args["run_in_background"] = json!(true);
    let agent = sandbox_agent(&h, None).await;
    let result = h.call("bash", args, Some(agent)).await;
    assert!(text(&result).contains(
        "invalid arguments: run_in_background is disabled for this deployment (enableRunInBackground: false)"
    ));
    assert_eq!(
        failure_info(&result),
        ("ToolArgsError".to_owned(), "INVALID_ARGS".to_owned())
    );
    assert_eq!(prompted.load(Ordering::SeqCst), 0);
    assert!(modes(&h).is_empty());
}

#[tokio::test]
async fn runs_a_repeated_mode_request_without_a_reason_or_approval() {
    for mode in [SandboxMode::WorkspaceWrite, SandboxMode::DangerFullAccess] {
        // Without an approval service at all: the request names the mode the call already has.
        let bare = setup_sandboxed(false, json!({})).await;
        let agent = sandbox_agent(&bare, Some(mode)).await;
        let mut args = escalate();
        args["sandbox_permissions"] = json!(mode.as_str());
        let result = bare.call("bash", args, Some(agent)).await;
        assert!(!result.is_error(), "{}", text(&result));
        assert_eq!(modes(&bare), [Some(mode)]);

        let h = setup_sandboxed(true, json!({})).await;
        let prompted = answer_with(&h, ApprovalOutcome::AllowedOnce);
        let agent = sandbox_agent(&h, Some(mode)).await;
        for justification in [None, Some(""), Some(" \t\n")] {
            let mut args = json!({
                "command": "true", "description": "repeat current mode",
                "sandbox_permissions": mode.as_str()
            });
            if let Some(justification) = justification {
                args["justification"] = json!(justification);
            }
            let result = h.call("bash", args, Some(agent.clone())).await;
            assert!(!result.is_error(), "{}", text(&result));
        }
        assert_eq!(modes(&h), [Some(mode), Some(mode), Some(mode)]);
        assert_eq!(prompted.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn fails_closed_when_approval_cannot_be_routed() {
    let without_service = setup_sandboxed(false, json!({})).await;
    let agent = sandbox_agent(&without_service, None).await;
    let result = without_service.call("bash", escalate(), Some(agent)).await;
    assert!(text(&result).contains("no approval service"));

    let with_service = setup_sandboxed(true, json!({})).await;
    let no_agent = with_service.call("bash", escalate(), None).await;
    assert!(text(&no_agent).contains("no agent to route"));
    let agent = sandbox_agent(&with_service, None).await;
    let no_channel = with_service.call("bash", escalate(), Some(agent)).await;
    assert!(text(&no_channel).contains("no approval channel"));
    assert!(modes(&with_service).is_empty());
}

#[tokio::test]
async fn maps_an_approval_rejected_or_cancelled_to_its_distinct_failure() {
    for (outcome, message, code) in [
        (
            ApprovalOutcome::Rejected,
            "user rejected",
            "SANDBOX_ESCALATION_REJECTED",
        ),
        (
            ApprovalOutcome::Cancelled,
            "was cancelled",
            "SANDBOX_ESCALATION_CANCELLED",
        ),
    ] {
        let h = setup_sandboxed(true, json!({})).await;
        answer_with(&h, outcome);
        let agent = sandbox_agent(&h, None).await;
        let result = h.call("bash", escalate(), Some(agent)).await;
        assert!(text(&result).contains(message), "{}", text(&result));
        assert_eq!(failure_info(&result).1, code);
        assert!(modes(&h).is_empty());
    }
}

#[tokio::test]
async fn runs_a_granted_foreground_or_background_call_under_the_approved_mode() {
    let h = setup_sandboxed(true, json!({})).await;
    let prompted = answer_with(&h, ApprovalOutcome::AllowedOnce);
    let agent = sandbox_agent(&h, None).await;
    let foreground = h.call("bash", escalate(), Some(agent.clone())).await;
    assert!(!foreground.is_error(), "{}", text(&foreground));
    // The foreground call was job bash-1 for the time it ran; its record left with the result.
    let jobs = h
        .jobs
        .clone()
        .expect("the sandboxed harness mounts a registry");
    assert!(jobs.listed().is_empty());
    assert_eq!(lock(&jobs.removed).as_slice(), [JobId::from("bash-1")]);
    let mut args = escalate();
    args["run_in_background"] = json!(true);
    let background = h.call("bash", args, Some(agent)).await;
    assert_eq!(success_value(&background)["kind"], "background");
    assert_eq!(text(&background), "started background job bash-1");
    assert_eq!(jobs.listed(), [JobId::from("bash-1")]);
    assert_eq!(
        modes(&h),
        [
            Some(SandboxMode::WorkspaceWrite),
            Some(SandboxMode::WorkspaceWrite)
        ]
    );
    assert_eq!(prompted.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn uses_the_session_override_for_ordinary_calls_and_evaluates_widening_against_it() {
    let h = setup_sandboxed(true, json!({})).await;
    let agent = sandbox_agent(&h, Some(SandboxMode::WorkspaceWrite)).await;
    h.call(
        "bash",
        json!({"command": "true", "description": "ordinary"}),
        Some(agent.clone()),
    )
    .await;
    answer_with(&h, ApprovalOutcome::AllowedOnce);
    let widened = h
        .call(
            "bash",
            json!({
                "command": "true", "description": "widen",
                "sandbox_permissions": "danger-full-access", "justification": "needs the network"
            }),
            Some(agent),
        )
        .await;
    assert!(!widened.is_error(), "{}", text(&widened));
    assert_eq!(
        modes(&h),
        [
            Some(SandboxMode::WorkspaceWrite),
            Some(SandboxMode::DangerFullAccess)
        ]
    );
}

#[tokio::test]
async fn stamps_the_resolved_sandbox_mode_on_a_preparation_timeout_of_a_confined_foreground_job() {
    // Confinement that only ends with the job's own cancellation: the wait expires with no process,
    // so the call ends as the deadline's preparation timeout carrying the mode it resolved.
    let jobs = FakeJobs::settling_on_kill();
    let h = build(Setup {
        sandboxed: true,
        policy: true,
        jobs: Some(jobs.clone()),
        ..Setup::default()
    })
    .await
    .unwrap();
    h.shell.set(Mode::HangUntilAbort);
    let result = h
        .call(
            "bash",
            json!({"command": "true", "description": "confined preparation", "timeoutMs": 100}),
            None,
        )
        .await;
    let value = success_value(&result);
    assert_eq!(value["kind"], "foreground");
    assert_eq!(value["timedOut"], true);
    assert_eq!(value["exitCode"], Value::Null);
    assert_eq!(
        value["sandbox"],
        json!({"mode": "read-only", "denied": false})
    );
    assert_eq!(
        text(&result),
        "(no output)\n[timed out after 100ms]\n[exit code: null]"
    );
    // The call stopped its own job and the record left with the result.
    assert_eq!(
        lock(&jobs.killed).as_slice(),
        [(
            JobId::from("bash-1"),
            Some("timed out during preparation".to_owned())
        )]
    );
    assert_eq!(lock(&jobs.removed).as_slice(), [JobId::from("bash-1")]);
    assert!(jobs.listed().is_empty());
}

#[tokio::test]
async fn omits_sandbox_facts_the_executor_did_not_acquire_from_the_canonical_result() {
    let h = setup_sandboxed(false, json!({})).await;
    let result = h
        .call(
            "bash",
            json!({
                "command": "without optional sandbox facts",
                "description": "exercise optional sandbox facts"
            }),
            None,
        )
        .await;
    assert!(!result.is_error(), "{}", text(&result));
    let value = success_value(&result);
    assert_eq!(value["kind"], "foreground");
    assert_eq!(
        value["sandbox"],
        json!({"mode": "read-only", "denied": false})
    );
    let keys: Vec<_> = value["sandbox"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["mode", "denied"]);

    // The facts the executor did acquire ride in the canonical order.
    let ordinary = h
        .call(
            "bash",
            json!({"command": "true", "description": "ordinary"}),
            None,
        )
        .await;
    let sandbox = &success_value(&ordinary)["sandbox"];
    assert_eq!(
        sandbox,
        &json!({"mode": "read-only", "denied": false, "enforcement": "full", "runnerFailed": false})
    );
    let keys: Vec<_> = sandbox.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["mode", "denied", "enforcement", "runnerFailed"]);
}

#[tokio::test]
async fn a_non_string_justification_without_a_sandbox_is_a_tool_args_error() {
    // Source: `rejectUnadvertisedArgs` throws a `TypeError` from the `justification.trim()` of a
    // non-string value; here the typed argument parse rejects it as `ToolArgsError` first.
    let h = setup().await;
    let result = h
        .call(
            "bash",
            json!({"command": "true", "description": "d", "justification": 7}),
            None,
        )
        .await;
    assert!(result.is_error());
    assert_eq!(
        failure_info(&result),
        ("ToolArgsError".to_owned(), "INVALID_ARGS".to_owned())
    );
    assert!(lock(&h.shell.specs).is_empty());
}

// ---------------------------------------------------------------------------------------------
// Real processes: `hoh-subprocess-local` under `hoh-bash-local` (the source's `setup()`).
// ---------------------------------------------------------------------------------------------

struct RealHarness {
    _fx: wd::Fixture,
    _spill: tempfile::TempDir,
    pipeline: ToolExecutionPipeline,
}

impl RealHarness {
    async fn call(&self, args: Value) -> ToolExecutionResult {
        run_pipeline(
            &self.pipeline,
            "bash",
            args,
            None,
            Arc::new(AbortController::new()),
        )
        .await
    }
}

/// `LocalSubprocessRuntime` (spilling into a private directory) + `LocalBashExecutor` + the tool.
async fn setup_real(executor: Value) -> RealHarness {
    let fx = wd::Fixture::new().await;
    let ctx = fx.ctx.clone();
    let spill = tempfile::tempdir().unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<hoh_tool_executor_native::NativeToolExecutorPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let runtime = hoh_subprocess_local::LocalSubprocessRuntime::new(ctx.clone()).unwrap();
    runtime.internals.lock().unwrap().spill_dir = Some(spill.path().to_path_buf());
    hoh_subprocess::provide_subprocess(&ctx, Arc::new(runtime))
        .await
        .unwrap();
    ctx.plugin::<hoh_shell_env::ShellEnvPlugin>(
        json!({ "dshHome": fx.root_text.clone() + "/dsh-home" }),
    )
    .unwrap()
    .settled()
    .await
    .unwrap();
    ctx.plugin::<hoh_bash_local::LocalBashExecutor>(executor)
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolBash>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let pipeline = ToolExecutionPipeline::new(ctx).unwrap();
    RealHarness {
        _fx: fx,
        _spill: spill,
        pipeline,
    }
}

#[tokio::test]
async fn real_process_returns_stdout_and_marks_stderr_and_exit_codes() {
    let h = setup_real(json!({"timeoutMs": 10_000, "graceMs": 200})).await;
    let result = h
        .call(json!({"command": "echo hello", "description": "test command"}))
        .await;
    assert!(!result.is_error(), "{}", text(&result));
    let value = success_value(&result);
    assert_eq!(value["kind"], "foreground");
    assert_eq!(value["exitCode"], 0);
    assert_eq!(
        value["stdout"],
        json!({"text": "hello\n", "truncated": false})
    );
    assert_eq!(text(&result), "hello\n");
    let mixed = h
        .call(json!({"command": "echo out; echo err >&2; exit 3", "description": "test command"}))
        .await;
    assert_eq!(text(&mixed), "out\n[stderr]\nerr\n[exit code: 3]");
}

#[tokio::test]
async fn real_process_reports_truncation_with_the_spill_path() {
    let h = setup_real(json!({"maxOutputBytes": 100, "graceMs": 200})).await;
    let result = h
        .call(json!({
            "command": "for i in $(seq 1 100); do printf \"line-%04d\\n\" $i; done",
            "description": "test command"
        }))
        .await;
    assert!(
        text(&result).contains("[output truncated; full output: "),
        "{}",
        text(&result)
    );
    assert!(text(&result).contains("line-0100"));
}

#[tokio::test]
async fn real_process_reports_a_timeout_even_when_the_command_traps_the_signal_and_exits_zero() {
    // The signal-independent timeout marker: a trapped SIGTERM that exits 0 after the timer fired
    // must not look like a clean success.
    let h = setup_real(json!({"timeoutMs": 10_000, "graceMs": 200})).await;
    let result = h
        .call(json!({
            "command": "trap \"exit 0\" TERM; sleep 60",
            "description": "test command",
            "timeoutMs": 100
        }))
        .await;
    assert!(!result.is_error(), "{}", text(&result));
    assert!(
        text(&result).contains("[timed out after 100ms]"),
        "{}",
        text(&result)
    );
    assert!(!text(&result).contains("[exit code:"));
}
