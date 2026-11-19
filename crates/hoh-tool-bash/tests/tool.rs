//! Ports of the foreground cases of tools.spec.ts: the tool body, its schema, the prompt section,
//! session-cwd routing, the request the tool builds and launch failures.
//!
//! The source runs these against the real local executor and subprocess runtime. Neither
//! `hoh-subprocess-local` nor a job registry exists yet, so a recording executor stands in for the
//! shell: cases whose subject is the tool's rendering and request construction run against canned
//! results, and cases whose subject is a real process are listed in PORT_NOTES.

#[path = "../../hoh-working-directory/tests/support/mod.rs"]
mod wd;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber};
use hoh_scope::ScopeKey;
use hoh_session::Session;
use hoh_shell::{
    CollectedOutput, ShellExecRequest, ShellExecSpec, ShellExecution, ShellExecutor,
    ShellExpiryPolicy, ShellProcess, ShellProcessRead, ShellProcessStatus, ShellRunResult,
    ShellSandboxInfo,
    types::{ShellDone, ShellFuture, ShellObservedStreams, shell_done},
};
use hoh_subprocess::{SubprocessOutputRead, SubprocessOutputReader};
use hoh_system_prompt::{AssembleContext, SystemPromptKey};
use hoh_timeout::AbortController;
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
}

struct Recorder {
    requests: Mutex<Vec<ShellExecRequest>>,
    specs: Mutex<Vec<ShellExecSpec>>,
    mode: Mutex<Mode>,
    workdir_override: Mutex<Option<String>>,
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
        Arc::new(Self {
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
        None
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
        let mode = lock(&self.mode).clone();
        Box::pin(async move {
            let (result, wait_abort): (ResultFn, bool) = match mode {
                Mode::Fail(error) => return Err(error(&spec)),
                Mode::Result(result) => (result, false),
                Mode::WaitAbort => (Arc::new(|spec| run_result(spec, "", "", Some(0))), true),
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

struct ToolAgent {
    key: ScopeKey,
    session: Session,
}

impl Agent for ToolAgent {
    fn scope_key(&self) -> ScopeKey {
        self.key.clone()
    }
    fn session(&self) -> Session {
        self.session.clone()
    }
}

struct Harness {
    fx: wd::Fixture,
    shell: Arc<Recorder>,
    tool: Fiber,
    pipeline: ToolExecutionPipeline,
    dsh_home: String,
}

impl Harness {
    fn ctx(&self) -> &Context {
        &self.fx.ctx
    }

    async fn agent(&self, id: &str, cwd: Option<&str>) -> Arc<dyn Agent> {
        let agent = self.fx.create(id, cwd).await;
        Arc::new(ToolAgent {
            key: ScopeKey::new(),
            session: agent.session(),
        })
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
        let mut input = ToolExecutionInput::new(
            format!("call-{name}").into(),
            name,
            args,
            controller.signal(),
        );
        input.agent = agent;
        let prepared = self.pipeline.prepare(input).await;
        let exec = prepared.exec().clone();
        let outcome = match prepared {
            ScheduledToolPreparation::Dispatch(exec) => self.pipeline.dispatch(exec).await,
            ScheduledToolPreparation::PostResult { result, .. } => {
                ScheduledToolDispatch::PostResult(result)
            }
            ScheduledToolPreparation::FinalResult { result, .. } => {
                ScheduledToolDispatch::FinalResult(result)
            }
        };
        self.pipeline.complete(&exec, outcome).await
    }
}

/// Foreground-only harness (`setup()` of the source): no job runtime.
async fn setup() -> Harness {
    let fx = wd::Fixture::new().await;
    let ctx = fx.ctx.clone();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let dsh_home = fx.root_text.clone() + "/dsh-home";
    ctx.plugin::<hoh_tool_executor_native::NativeToolExecutorPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<hoh_shell_env::ShellEnvPlugin>(json!({ "dshHome": dsh_home }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let shell = Recorder::new();
    hoh_shell::provide_shell(&ctx, shell.clone()).await.unwrap();
    let tool = ctx.plugin::<ToolBash>(json!({})).unwrap();
    tool.settled().await.unwrap();
    let pipeline = ToolExecutionPipeline::new(ctx.clone()).unwrap();
    Harness {
        fx,
        shell,
        tool,
        pipeline,
        dsh_home,
    }
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

#[tokio::test]
async fn contributes_the_exit_code_habit_as_its_prompt_section() {
    let h = setup().await;
    let prompt = h.ctx().get::<SystemPromptKey>().unwrap();
    let assembly = prompt.assemble(AssembleContext::new(None)).await.unwrap();
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
    let names = |assembly: &hoh_system_prompt::PromptAssembly| {
        assembly
            .sections
            .iter()
            .map(|section| section.name.clone())
            .collect::<Vec<_>>()
    };
    assert!(
        names(&prompt.assemble(AssembleContext::new(None)).await.unwrap())
            .contains(&"tool:bash".to_owned())
    );
    h.tool.dispose().await;
    assert_eq!(tools.schemas(None, None).unwrap().len(), 0);
    assert!(
        !names(&prompt.assemble(AssembleContext::new(None)).await.unwrap())
            .contains(&"tool:bash".to_owned())
    );
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

// tool-bash/index.ts:533 -> working-directory/index.ts:223: the agent is told when its selected
// directory is gone and the command runs in the original project.
#[tokio::test]
async fn tells_the_agent_when_the_selected_working_directory_is_unavailable() {
    let h = setup().await;
    let project = h.fx.root_text.clone();
    let owner = h.fx.create("cwd-recovery", Some(&project)).await;
    let child = h.fx.path(&["tmp"]);
    std::fs::create_dir(&child).unwrap();
    h.fx.service.set(&owner.agent, &child, None).await.unwrap();
    owner.claim();
    std::fs::remove_dir_all(&child).unwrap();
    let agent: Arc<dyn Agent> = Arc::new(ToolAgent {
        key: ScopeKey::new(),
        session: owner.session(),
    });
    assert_eq!(pwd_in(&h, None, Some(agent)).await, project);
    let delivered = owner.claim();
    assert_eq!(
        wd::texts(&delivered),
        [format!(
            "The working directory {} is unavailable. The working directory is now {}.",
            serde_json::to_string(&child).unwrap(),
            serde_json::to_string(&project).unwrap()
        )]
    );
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
