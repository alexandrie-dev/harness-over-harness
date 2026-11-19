use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentPreStep, AgentRegistryPlugin, AgentStatus, CancelOptions, Inbox,
    InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection, PreStepDecision,
    agent_events,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Fiber, Kernel, Level, NoConfig, Plugin};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, UserMessage, create_user_message};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId, SessionPlugin, SurfaceIntent};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_shell::types::{ShellDone, ShellFuture, ShellObservedStreams, shell_done};
use hoh_shell::{
    CollectedOutput, ShellExecRequest, ShellExecSpec, ShellExecution, ShellExecutor,
    ShellExpiryPolicy, ShellProcess, ShellProcessRead, ShellProcessStatus, ShellRunResult,
    provide_shell,
};
use hoh_timeout::AbortController;
use hoh_tmux_context::TmuxContext;
use serde_json::{Map, Value, json};
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::sync::Notify;

const LAYOUT: &str =
    "d517,270x71,0,0{135x71,0,0,87,134x71,136,0[134x35,136,0,90,134x35,136,36,93]}";

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn tmux_line(window_name: &str, pane_id: &str) -> String {
    ["0", "1", window_name, "2", pane_id, "1", "0", LAYOUT].join("\\t")
}

fn run_result(stdout: impl Into<String>, exit_code: i64) -> ShellRunResult {
    ShellRunResult {
        exit_code: Some(exit_code),
        signal: None,
        timed_out: false,
        aborted: false,
        timeout_ms: 60_000.0,
        stdout: CollectedOutput {
            text: stdout.into(),
            truncated: false,
            spill_path: None,
        },
        stderr: CollectedOutput {
            text: String::new(),
            truncated: false,
            spill_path: None,
        },
        sandbox: None,
    }
}

#[derive(Clone)]
enum FailureMode {
    Error(String),
    SyncPanic(String),
    AsyncPanic(String),
}

struct FakeExecution {
    result: Result<ShellRunResult, String>,
    failure: Option<FailureMode>,
    done: ShellDone,
}

impl ShellProcess for FakeExecution {
    fn status(&self) -> ShellProcessStatus {
        ShellProcessStatus::Completed
    }
    fn exit_code(&self) -> Option<i64> {
        self.result
            .as_ref()
            .ok()
            .and_then(|result| result.exit_code)
    }
    fn signal(&self) -> Option<String> {
        None
    }
    fn done(&self) -> ShellDone {
        self.done.clone()
    }
    fn read_output(&self) -> ShellProcessRead {
        ShellProcessRead::default()
    }
    fn observed(&self) -> ShellObservedStreams {
        panic!("tmux fixture does not stream output")
    }
    fn kill(&self) -> bool {
        false
    }
}

impl ShellExecution for FakeExecution {
    fn result(&self) -> ShellFuture<'_, ShellRunResult> {
        match self.failure.clone() {
            Some(FailureMode::Error(message)) => {
                Box::pin(async move { Err(BoxError::from(message)) })
            }
            Some(FailureMode::SyncPanic(message)) => std::panic::panic_any(message),
            Some(FailureMode::AsyncPanic(message)) => Box::pin(async move {
                std::panic::panic_any(message);
            }),
            None => Box::pin(async move { self.result.clone().map_err(BoxError::from) }),
        }
    }
}

struct FakeShell {
    commands: Mutex<Vec<String>>,
    result: Mutex<Result<ShellRunResult, String>>,
    resolve_panic: Mutex<Option<String>>,
    execute_failure: Mutex<Option<FailureMode>>,
    result_failure: Mutex<Option<FailureMode>>,
}

impl FakeShell {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            commands: Mutex::new(Vec::new()),
            result: Mutex::new(Ok(run_result(format!("{}\n", tmux_line("node", "%90")), 0))),
            resolve_panic: Mutex::new(None),
            execute_failure: Mutex::new(None),
            result_failure: Mutex::new(None),
        })
    }

    fn command_count(&self) -> usize {
        lock(&self.commands).len()
    }
}

impl ShellExecutor for FakeShell {
    fn resolve(&self, request: &ShellExecRequest) -> ShellExecSpec {
        if let Some(message) = lock(&self.resolve_panic).take() {
            std::panic::panic_any(message);
        }
        ShellExecSpec {
            command: request.command.clone(),
            workdir: request.workdir.clone().unwrap_or_else(|| "/work".into()),
            timeout_ms: request.timeout_ms.unwrap_or(60_000.0),
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
        assert_ne!(spec.on_expiry, ShellExpiryPolicy::None);
        lock(&self.commands).push(spec.command);
        match lock(&self.execute_failure).take() {
            Some(FailureMode::Error(message)) => {
                return Box::pin(async move { Err(BoxError::from(message)) });
            }
            Some(FailureMode::SyncPanic(message)) => std::panic::panic_any(message),
            Some(FailureMode::AsyncPanic(message)) => {
                return Box::pin(async move {
                    std::panic::panic_any(message);
                });
            }
            None => {}
        }
        let result = lock(&self.result).clone();
        let failure = lock(&self.result_failure).take();
        Box::pin(async move {
            Ok(Arc::new(FakeExecution {
                result,
                failure,
                done: shell_done(async {}),
            }) as Arc<dyn ShellExecution>)
        })
    }
}

struct RegistrationGate {
    entered: Notify,
    release: Notify,
    outcome: Mutex<Option<Result<(), String>>>,
}

hoh_kernel::service!(RegistrationGateService: RegistrationGate = "tmuxContextTestGate");

struct GatedTmuxContext;

impl Plugin for GatedTmuxContext {
    const NAME: &'static str = "tmux-context-test-gated";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
        let gate = ctx
            .get::<RegistrationGateService>()
            .ok_or_else(|| BoxError::from("tmux context test gate is unavailable"))?;
        TmuxContext::apply_with_registration_gate(ctx, None, async move {
            gate.entered.notify_one();
            gate.release.notified().await;
            lock(&gate.outcome)
                .take()
                .expect("registration gate has one outcome")
                .map_err(BoxError::from)
        })
        .await
    }
}

struct TestAgent {
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
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
        panic!("tmux fixture has no model")
    }
    fn inbox(&self) -> &dyn Inbox {
        panic!("tmux fixture has no inbox")
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Running
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        Ok(())
    }
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        panic!("tmux fixture is never driven")
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        panic!("tmux fixture is never driven")
    }
    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("tmux-context must append directly to the open step")
    }
}

struct Env {
    kernel: Kernel,
    ctx: Context,
    shell: Option<Arc<FakeShell>>,
}

struct GatedEnv {
    kernel: Kernel,
    ctx: Context,
    shell: Arc<FakeShell>,
    plugin: Fiber,
    gate: Arc<RegistrationGate>,
}

async fn base() -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    (kernel, ctx)
}

async fn mount(config: Value, with_shell: bool) -> Env {
    let (kernel, ctx) = base().await;
    let shell = if with_shell {
        let shell = FakeShell::new();
        provide_shell(&ctx, shell.clone()).await.unwrap();
        Some(shell)
    } else {
        None
    };
    let plugin = ctx.plugin::<TmuxContext>(config).unwrap();
    plugin.settled().await.unwrap();
    Env { kernel, ctx, shell }
}

async fn mount_gated(outcome: Result<(), String>) -> GatedEnv {
    let (kernel, ctx) = base().await;
    let shell = FakeShell::new();
    provide_shell(&ctx, shell.clone()).await.unwrap();
    let gate = Arc::new(RegistrationGate {
        entered: Notify::new(),
        release: Notify::new(),
        outcome: Mutex::new(Some(outcome)),
    });
    let _provider = ctx
        .provide::<RegistrationGateService>(gate.clone())
        .await
        .unwrap();
    let entered = gate.entered.notified();
    let plugin = ctx.plugin::<GatedTmuxContext>(json!({})).unwrap();
    entered.await;
    GatedEnv {
        kernel,
        ctx,
        shell,
        plugin,
        gate,
    }
}

fn session(id: &str) -> Session {
    Session::create(id.into(), None, None, None, None).unwrap()
}

fn agent(ctx: &Context, session: Session) -> Arc<dyn Agent> {
    Arc::new(TestAgent {
        session,
        ctx: ctx.clone(),
        scope: ScopeKey::new(),
        options: AgentOptions::default(),
    })
}

fn user_message(text: &str, kind: &str, block: ContentBlock) -> UserMessage {
    let mut source = MessageSource::new(kind);
    if kind == "user" {
        source.data = Map::new();
    }
    let _ = text;
    create_user_message(NewUserMessage {
        content: vec![block],
        source,
    })
}

fn open_turn(session: &Session, turn: u64) {
    session
        .append("turn/start", json!({"turn":turn}), None)
        .unwrap();
    let message = user_message(
        &format!("turn {turn}"),
        "user",
        ContentBlock::text(format!("turn {turn}")),
    );
    session
        .append(
            "user/message",
            serde_json::to_value(message).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
}

fn context_texts(session: &Session) -> Vec<String> {
    session
        .snapshot_events(None, None)
        .iter()
        .filter(|event| {
            event.event_type() == "user/message" && event.data()["source"]["kind"] == "tmux-context"
        })
        .map(|event| {
            event.data()["content"]
                .as_array()
                .and_then(|blocks| blocks.iter().find(|block| block["type"] == "text"))
                .and_then(|block| block["text"].as_str())
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

async fn fire(
    ctx: &Context,
    agent: Arc<dyn Agent>,
    turn: u64,
    step: u64,
    signal: hoh_timeout::AbortSignal,
) -> Result<PreStepDecision, BoxError> {
    let decision = agent_events(ctx, &agent)
        .waterfall::<AgentPreStep, PreStepDecision, _>(
            move |agent| AgentPreStep {
                agent,
                messages: Vec::new(),
                turn,
                step,
                signal,
                model_selection: ModelSelection {
                    provider: "mock".into(),
                    model: "mock".into(),
                    reasoning_effort: None,
                },
            },
            |_| async {
                Ok(PreStepDecision::Enter {
                    messages: Vec::new(),
                    starts_request_series: false,
                })
            },
        )
        .await?;
    if let PreStepDecision::Enter { messages, .. } = &decision {
        for message in messages {
            agent.session().append(
                "user/message",
                serde_json::to_value(message)?,
                Some(SurfaceIntent::append()),
            )?;
        }
    }
    Ok(decision)
}

fn live_signal() -> hoh_timeout::AbortSignal {
    AbortController::new().signal()
}

fn assert_query_warning(ctx: &Context, message: &str) {
    let expected =
        format!("tmux location query failed: {message}; injecting no location this turn");
    assert!(
        ctx.logger()
            .buffer()
            .iter()
            .any(|record| record.level == Level::Warn && record.message == expected)
    );
}

async fn assert_step_is_waiting(
    step: &mut tokio::task::JoinHandle<Result<PreStepDecision, BoxError>>,
) {
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut *step)
            .await
            .is_err(),
        "pre-step unexpectedly completed while registration was pending"
    );
}

#[tokio::test]
async fn handles_a_ready_registration_before_querying_the_first_turn_location() {
    let env = mount_gated(Ok(())).await;
    let session = session("ready");
    open_turn(&session, 1);
    let ctx = env.ctx.clone();
    let step_agent = agent(&ctx, session.clone());
    let mut step = tokio::spawn(async move { fire(&ctx, step_agent, 1, 1, live_signal()).await });
    assert_step_is_waiting(&mut step).await;
    assert_eq!(env.shell.command_count(), 0);
    env.gate.release.notify_one();
    env.plugin.settled().await.unwrap();
    let decision = step.await.unwrap().unwrap();
    let PreStepDecision::Enter { messages, .. } = decision else {
        panic!("first step must enter");
    };
    assert_eq!(messages.len(), 1);
    assert_eq!(env.shell.command_count(), 1);
    assert_eq!(context_texts(&session).len(), 1);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn handles_a_cancelled_registration_before_querying_the_first_turn_location() {
    let env = mount_gated(Ok(())).await;
    let session = session("cancelled");
    open_turn(&session, 1);
    let controller = AbortController::new();
    let ctx = env.ctx.clone();
    let step_agent = agent(&ctx, session.clone());
    let signal = controller.signal();
    let mut step = tokio::spawn(async move { fire(&ctx, step_agent, 1, 1, signal).await });
    assert_step_is_waiting(&mut step).await;
    controller.abort();
    env.gate.release.notify_one();
    env.plugin.settled().await.unwrap();
    step.await.unwrap().unwrap();
    assert_eq!(env.shell.command_count(), 0);
    assert!(context_texts(&session).is_empty());
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn handles_a_disposed_registration_before_querying_the_first_turn_location() {
    let env = mount_gated(Ok(())).await;
    let session = session("disposed");
    open_turn(&session, 1);
    let ctx = env.ctx.clone();
    let step_agent = agent(&ctx, session.clone());
    let mut step = tokio::spawn(async move { fire(&ctx, step_agent, 1, 1, live_signal()).await });
    assert_step_is_waiting(&mut step).await;
    let disposing = tokio::spawn(env.plugin.dispose());
    tokio::task::yield_now().await;
    env.gate.release.notify_one();
    disposing.await.unwrap();
    let error = step.await.unwrap().unwrap_err();
    assert!(
        error.to_string().contains("inactive context"),
        "unexpected disposal error: {error}"
    );
    assert_eq!(env.shell.command_count(), 0);
    assert!(context_texts(&session).is_empty());
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn handles_a_failed_registration_before_querying_the_first_turn_location() {
    let env = mount_gated(Err("projection registration failed".into())).await;
    let session = session("failed");
    open_turn(&session, 1);
    let ctx = env.ctx.clone();
    let step_agent = agent(&ctx, session.clone());
    let mut step = tokio::spawn(async move { fire(&ctx, step_agent, 1, 1, live_signal()).await });
    assert_step_is_waiting(&mut step).await;
    env.gate.release.notify_one();
    let plugin_error = env.plugin.settled().await.unwrap_err();
    assert!(
        plugin_error
            .to_string()
            .contains("projection registration failed")
    );
    let step_error = step.await.unwrap().unwrap_err();
    assert!(
        step_error
            .to_string()
            .contains("projection registration failed")
    );
    assert_eq!(env.shell.command_count(), 0);
    assert!(context_texts(&session).is_empty());
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn injects_the_tmux_location_on_the_first_step_of_a_turn() {
    let env = mount(json!({}), true).await;
    let session = session("first");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert_eq!(
        context_texts(&session),
        [format!(
            "tmux location (turn 1):\nsession 0, window 1 \"node\", pane 2 %90\nwindow active=1, pane active=0, layout {LAYOUT}"
        )]
    );
    let event = session.snapshot_events(None, None).last().unwrap().clone();
    assert_eq!(event.data()["source"]["kind"], "tmux-context");
    assert_eq!(event.data()["source"]["form"], "snapshot");
    assert_eq!(
        event.data()["source"]["sections"][0]["name"],
        "tmux-context"
    );
    assert_eq!(event.surface_op().unwrap(), "append");
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn queries_the_pane_this_process_runs_in_and_matches_its_controlling_tty() {
    let env = mount(json!({}), true).await;
    let session = session("command");
    open_turn(&session, 1);
    fire(&env.ctx, agent(&env.ctx, session), 1, 1, live_signal())
        .await
        .unwrap();
    let command = lock(&env.shell.as_ref().unwrap().commands)[0].clone();
    assert!(command.contains("[ -n \"$TMUX_PANE\" ]"));
    assert!(command.contains("tmux display-message -t \"$TMUX_PANE\" -p"));
    assert!(command.contains(&format!("ps -o tty= -p {}", std::process::id())));
    assert!(command.contains("#{pane_tty}"));
    assert!(command.contains("[ \"$pane_tty\" = \"/dev/$self_tty\" ]"));
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn does_not_run_on_later_steps_of_a_turn() {
    let env = mount(json!({}), true).await;
    let session = session("later");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        2,
        live_signal(),
    )
    .await
    .unwrap();
    assert_eq!(env.shell.as_ref().unwrap().command_count(), 0);
    assert!(context_texts(&session).is_empty());
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn reinjects_a_new_turn_only_when_tmux_state_changed() {
    let env = mount(json!({}), true).await;
    let session = session("change");
    let agent = agent(&env.ctx, session.clone());
    open_turn(&session, 1);
    fire(&env.ctx, agent.clone(), 1, 1, live_signal())
        .await
        .unwrap();
    session
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    open_turn(&session, 2);
    fire(&env.ctx, agent.clone(), 2, 1, live_signal())
        .await
        .unwrap();
    assert_eq!(context_texts(&session).len(), 1);
    *lock(&env.shell.as_ref().unwrap().result) =
        Ok(run_result(format!("{}\n", tmux_line("shell", "%12")), 0));
    session
        .append(
            "turn/end",
            json!({"turn":2,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    open_turn(&session, 3);
    fire(&env.ctx, agent, 3, 1, live_signal()).await.unwrap();
    let texts = context_texts(&session);
    assert_eq!(texts.len(), 2);
    assert!(texts[1].contains("tmux location (turn 3):"));
    assert!(texts[1].contains("window 1 \"shell\", pane 2 %12"));
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn honors_a_positive_refresh_interval_between_injections() {
    {
        let env = mount(json!({"refreshIntervalMs":10000}), true).await;
        let session = session("interval");
        let agent = agent(&env.ctx, session.clone());
        open_turn(&session, 1);
        fire(&env.ctx, agent.clone(), 1, 1, live_signal())
            .await
            .unwrap();
        *lock(&env.shell.as_ref().unwrap().result) =
            Ok(run_result(format!("{}\n", tmux_line("node", "%99")), 0));
        open_turn(&session, 2);
        fire(&env.ctx, agent, 2, 1, live_signal()).await.unwrap();
        assert_eq!(context_texts(&session).len(), 1);
        assert_eq!(env.shell.as_ref().unwrap().command_count(), 1);
        env.kernel.shutdown().await;
    }

    let env = mount(json!({"refreshIntervalMs":1}), true).await;
    let session = session("interval-past");
    let agent = agent(&env.ctx, session.clone());
    open_turn(&session, 1);
    fire(&env.ctx, agent.clone(), 1, 1, live_signal())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(3)).await;
    *lock(&env.shell.as_ref().unwrap().result) =
        Ok(run_result(format!("{}\n", tmux_line("node", "%99")), 0));
    open_turn(&session, 2);
    fire(&env.ctx, agent, 2, 1, live_signal()).await.unwrap();
    assert_eq!(context_texts(&session).len(), 2);
    assert_eq!(env.shell.as_ref().unwrap().command_count(), 2);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn treats_a_prior_non_text_plugin_reading_as_absent_and_injects_afresh() {
    let env = mount(json!({}), true).await;
    let session = session("non-text");
    open_turn(&session, 1);
    let prior = user_message(
        "",
        "tmux-context",
        ContentBlock::reasoning("not a location"),
    );
    session
        .append(
            "user/message",
            serde_json::to_value(prior).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert_eq!(env.shell.as_ref().unwrap().command_count(), 1);
    assert!(
        context_texts(&session)
            .last()
            .unwrap()
            .contains("tmux location")
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn treats_a_prior_single_line_plugin_reading_as_empty_state() {
    let env = mount(json!({}), true).await;
    let session = session("single-line");
    open_turn(&session, 1);
    let prior = user_message(
        "single line, no newline",
        "tmux-context",
        ContentBlock::text("single line, no newline"),
    );
    session
        .append(
            "user/message",
            serde_json::to_value(prior).unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert_eq!(env.shell.as_ref().unwrap().command_count(), 1);
    assert!(
        context_texts(&session)
            .last()
            .unwrap()
            .contains("tmux location")
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn is_a_noop_when_no_bash_executor_is_mounted() {
    let env = mount(json!({}), false).await;
    let session = session("no-shell");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn is_a_noop_when_the_tmux_query_exits_nonzero() {
    let env = mount(json!({}), true).await;
    *lock(&env.shell.as_ref().unwrap().result) = Ok(run_result("", 1));
    let session = session("outside");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn is_a_noop_when_the_reading_has_the_wrong_field_count() {
    let env = mount(json!({}), true).await;
    *lock(&env.shell.as_ref().unwrap().result) = Ok(run_result("0\\t1\\tnode\n", 0));
    let session = session("malformed");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn is_a_noop_when_the_pane_id_is_empty() {
    let env = mount(json!({}), true).await;
    *lock(&env.shell.as_ref().unwrap().result) =
        Ok(run_result(format!("{}\n", tmux_line("node", "")), 0));
    let session = session("empty-pane");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn warns_and_injects_nothing_when_the_executor_rejects_the_run() {
    let env = mount(json!({}), true).await;
    *lock(&env.shell.as_ref().unwrap().result) = Err("bash executor unavailable".into());
    let session = session("run-error");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    assert_query_warning(&env.ctx, "bash executor unavailable");
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn warns_and_injects_nothing_when_the_executor_rejects_the_command_at_resolve() {
    let env = mount(json!({}), true).await;
    *lock(&env.shell.as_ref().unwrap().resolve_panic) = Some("command denied by policy".into());
    let session = session("resolve-error");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    assert_query_warning(&env.ctx, "command denied by policy");
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn reports_a_non_error_rejection_in_the_warning() {
    let env = mount(json!({}), true).await;
    *lock(&env.shell.as_ref().unwrap().result_failure) =
        Some(FailureMode::Error("spawn refused".into()));
    let session = session("non-error");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    assert_query_warning(&env.ctx, "spawn refused");
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn warns_when_execute_returns_an_error() {
    let env = mount(json!({}), true).await;
    *lock(&env.shell.as_ref().unwrap().execute_failure) =
        Some(FailureMode::Error("execute rejected".into()));
    let session = session("execute-error");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    assert_query_warning(&env.ctx, "execute rejected");
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn warns_when_execute_panics_synchronously() {
    let env = mount(json!({}), true).await;
    *lock(&env.shell.as_ref().unwrap().execute_failure) =
        Some(FailureMode::SyncPanic("execute sync panic".into()));
    let session = session("execute-sync-panic");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    assert_query_warning(&env.ctx, "execute sync panic");
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn warns_when_execute_panics_while_awaited() {
    let env = mount(json!({}), true).await;
    *lock(&env.shell.as_ref().unwrap().execute_failure) =
        Some(FailureMode::AsyncPanic("execute async panic".into()));
    let session = session("execute-async-panic");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    assert_query_warning(&env.ctx, "execute async panic");
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn warns_when_result_panics_synchronously() {
    let env = mount(json!({}), true).await;
    *lock(&env.shell.as_ref().unwrap().result_failure) =
        Some(FailureMode::SyncPanic("result sync panic".into()));
    let session = session("result-sync-panic");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    assert_query_warning(&env.ctx, "result sync panic");
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn warns_when_result_panics_while_awaited() {
    let env = mount(json!({}), true).await;
    *lock(&env.shell.as_ref().unwrap().result_failure) =
        Some(FailureMode::AsyncPanic("result async panic".into()));
    let session = session("result-async-panic");
    open_turn(&session, 1);
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    assert_query_warning(&env.ctx, "result async panic");
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn skips_an_already_aborted_prompt_submission() {
    let env = mount(json!({}), true).await;
    let session = session("aborted");
    open_turn(&session, 1);
    let controller = AbortController::new();
    controller.abort();
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        controller.signal(),
    )
    .await
    .unwrap();
    assert!(context_texts(&session).is_empty());
    fire(
        &env.ctx,
        agent(&env.ctx, session.clone()),
        1,
        1,
        live_signal(),
    )
    .await
    .unwrap();
    assert_eq!(context_texts(&session).len(), 1);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_a_negative_refresh_interval_at_plugin_load() {
    let (kernel, ctx) = base().await;
    let plugin = ctx
        .plugin::<TmuxContext>(json!({"refreshIntervalMs":-1}))
        .unwrap();
    let error = plugin.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("refreshIntervalMs must be a non-negative safe integer")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_a_non_integer_refresh_interval_at_plugin_load() {
    let (kernel, ctx) = base().await;
    let plugin = ctx
        .plugin::<TmuxContext>(json!({"refreshIntervalMs":1.5}))
        .unwrap();
    let error = plugin.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("refreshIntervalMs must be a non-negative safe integer")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_a_null_refresh_interval_at_plugin_load() {
    let (kernel, ctx) = base().await;
    let error = match ctx.plugin::<TmuxContext>(json!({"refreshIntervalMs":null})) {
        Ok(plugin) => plugin.settled().await.unwrap_err(),
        Err(error) => error,
    };
    assert!(error.to_string().contains("refreshIntervalMs"));
    kernel.shutdown().await;
}
