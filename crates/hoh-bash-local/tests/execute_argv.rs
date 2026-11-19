mod common;

use common::*;
use futures_util::FutureExt;
use hoh_bash_local::{ArgvSource, Config, ExecuteHooks, LocalBashExecutor};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_shell::ShellSandboxInfo;
use hoh_subprocess::{
    Subprocess, SubprocessCollectedOutputs, SubprocessFuture, SubprocessHandle,
    SubprocessLifecycle, SubprocessOutcome, SubprocessOutputMode, SubprocessOutputRead,
    SubprocessOutputReader, SubprocessRuntime, SubprocessSpawnSpec, SubprocessTerminalEnvironment,
    SubprocessTerminalHandle, SubprocessTerminalSpawnSpec, provide_subprocess, subprocess_done,
};
use hoh_timeout::AbortSignal;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// A composing plugin: builds the executor the way a sandboxing subclass would and hands it out.
static EXECUTOR: Mutex<Option<Arc<LocalBashExecutor>>> = Mutex::new(None);
static MOUNTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[derive(Inject)]
struct ProbeDeps {
    subprocess: Service<Subprocess>,
}
struct Probe;
impl Plugin for Probe {
    const NAME: &'static str = "ExecuteArgvProbe";
    type Config = Config;
    type Deps = ProbeDeps;

    async fn apply(_ctx: Context, deps: ProbeDeps, config: Cfg<Config>) -> Result<(), BoxError> {
        let executor = LocalBashExecutor::new(config, deps.subprocess);
        *EXECUTOR.lock().unwrap() = Some(Arc::new(executor));
        Ok(())
    }
}

fn sandbox(denied: bool) -> ShellSandboxInfo {
    serde_json::from_value(json!({"mode": "read-only", "denied": denied})).unwrap()
}

/// Mount the probe plugin on `ctx` and take the executor it composed.
async fn executor_over(ctx: &Context, config: serde_json::Value) -> Arc<LocalBashExecutor> {
    let _mounting = MOUNTING.lock().await;
    ctx.plugin::<Probe>(config)
        .unwrap()
        .settled()
        .await
        .unwrap();
    EXECUTOR.lock().unwrap().take().unwrap()
}

struct EmptyReader;
impl SubprocessOutputReader for EmptyReader {
    fn read_from(&self, _from_byte: u64) -> SubprocessOutputRead {
        SubprocessOutputRead {
            text: String::new(),
            next_offset: 0,
            lossy: false,
            spill_path: None,
        }
    }
}

struct ReadyLifecycle;
impl SubprocessLifecycle for ReadyLifecycle {
    fn terminate(&self) {}
    fn wait_for_exit(&self, _signal: Option<AbortSignal>) -> SubprocessFuture<'_, bool> {
        async { Ok(true) }.boxed()
    }
}

/// A provider whose handle is already settled the moment `spawn` returns, recording each spec.
#[derive(Default)]
struct ReadyRuntime {
    specs: Mutex<Vec<SubprocessSpawnSpec>>,
}
impl SubprocessRuntime for ReadyRuntime {
    fn resolve_executable<'a>(
        &'a self,
        command: &'a str,
        _env: Option<&'a indexmap::IndexMap<String, String>>,
        _signal: Option<AbortSignal>,
    ) -> SubprocessFuture<'a, String> {
        async move { Ok(command.to_owned()) }.boxed()
    }
    fn terminal_environment(
        &self,
        _signal: Option<AbortSignal>,
    ) -> SubprocessFuture<'_, SubprocessTerminalEnvironment> {
        async { Err(Box::new(FixtureError) as BoxError) }.boxed()
    }
    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<SubprocessHandle, BoxError> {
        self.specs.lock().unwrap().push(spec);
        let reader: Arc<dyn SubprocessOutputReader> = Arc::new(EmptyReader);
        Ok(SubprocessHandle {
            stdin: None,
            stdout: None,
            stderr: None,
            control: None,
            collected: SubprocessCollectedOutputs {
                stdout: Some(reader.clone()),
                stderr: Some(reader),
            },
            done: subprocess_done(async {
                Ok(SubprocessOutcome {
                    exit_code: Some(0),
                    signal: None,
                })
            }),
            lifecycle: Arc::new(ReadyLifecycle),
        })
    }
    fn spawn_terminal(
        &self,
        _spec: SubprocessTerminalSpawnSpec,
    ) -> SubprocessFuture<'_, SubprocessTerminalHandle> {
        async { Err(Box::new(FixtureError) as BoxError) }.boxed()
    }
}

// index.ts:387 — `onStarted` is synchronous, the settlement is a later microtask: the started hook
// always precedes settlement, so it cannot overwrite the facts the settlement hook computed.
//
// Explicit synchronization: the provider's `done` is already settled at spawn, so a settlement
// that is not held back is runnable at once on another worker. The started hook is held until
// either the settlement hook signals its entry (the old behaviour: the signal ends the hold
// immediately) or a generous bound passes with no entry (the gate: the settlement hook cannot be
// entered before the started hook returns). The test asserts the entry signal never arrived while
// the started hook was held, and then that both hooks ran in the order `started → done`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn on_started_runs_before_settlement_and_cannot_overwrite_final_sandbox_facts() {
    let ctx = hoh_kernel::Kernel::start().root();
    provide_subprocess(&ctx, Arc::new(ReadyRuntime::default()))
        .await
        .unwrap();
    let executor = executor_over(&ctx, json!({})).await;

    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let (settlement_entered, settlement_seen) = std::sync::mpsc::channel::<()>();
    let entered_while_held = Arc::new(Mutex::new(None::<bool>));
    let held_result = entered_while_held.clone();
    let started_order = order.clone();
    let done_order = order.clone();
    let hooks = ExecuteHooks {
        on_started: Some(Box::new(move |execution| {
            let entered = settlement_seen
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_ok();
            *held_result.lock().unwrap() = Some(entered);
            started_order.lock().unwrap().push("started");
            execution.set_sandbox(Some(sandbox(false)));
        })),
        on_process_done: Some(Arc::new(move |execution, _stderr, _rejected, _error| {
            done_order.lock().unwrap().push("done");
            execution.set_sandbox(Some(sandbox(true)));
            let _ = settlement_entered.send(());
        })),
    };
    let spec = executor.try_resolve(&request("true")).unwrap();
    let execution = executor
        .execute_argv(spec, ArgvSource::Ready(vec!["true".into()]), hooks)
        .await
        .unwrap();
    let result = hoh_shell::ShellExecution::result(&execution).await.unwrap();
    assert_eq!(
        *entered_while_held.lock().unwrap(),
        Some(false),
        "the settlement hook was entered while the started hook was still held"
    );
    assert_eq!(*order.lock().unwrap(), ["started", "done"]);
    assert_eq!(result.sandbox, Some(sandbox(true)));
}

// index.ts:128 — a fractional `maxOutputBytes` is a positive finite number: accepted, no panic.
#[tokio::test]
async fn accepts_a_fractional_output_cap_without_panicking() {
    let setup = setup(json!({"maxOutputBytes": 0.5})).await;
    let spec = setup.shell.resolve(&request("true"));
    let result = run(&setup.shell, spec).await.unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout.text, "");
}

// output.ts:162-180 — the collector's trim removes `trunc(length - cap)` bytes of an over-cap
// chunk, so a cap of 0.5 retains one byte: `ceil(cap)` reaches the seam, for the foreground
// stdout cap of the resolved spec and for both collected streams of the spawn.
#[tokio::test]
async fn a_fractional_output_cap_retains_ceil_of_it_like_the_ts_collector() {
    let ctx = hoh_kernel::Kernel::start().root();
    let runtime = Arc::new(ReadyRuntime::default());
    provide_subprocess(&ctx, runtime.clone()).await.unwrap();
    let executor = executor_over(&ctx, json!({"maxOutputBytes": 0.5})).await;

    let spec = executor.try_resolve(&request("true")).unwrap();
    assert_eq!(spec.stdout_max_bytes, 1);
    let execution = executor.execute_spec(spec).await.unwrap();
    hoh_shell::ShellExecution::result(&execution).await.unwrap();

    let specs = runtime.specs.lock().unwrap();
    let cap = |mode: &SubprocessOutputMode| match mode {
        SubprocessOutputMode::Collect(collect) => collect.max_bytes,
        other => panic!("expected a collected stream, got {other:?}"),
    };
    assert_eq!(cap(&specs[0].stdio.stdout), 1);
    assert_eq!(cap(&specs[0].stdio.stderr), 1);
}
