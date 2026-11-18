use std::sync::{Arc, Mutex};

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Kernel, NoConfig, Plugin};
use hoh_shell::types::{ShellDone, ShellFuture, ShellObservedStreams, shell_done};
use hoh_shell::{
    CollectedOutput, Shell, ShellExecRequest, ShellExecSpec, ShellExecution, ShellExecutor,
    ShellExpiryPolicy, ShellProcess, ShellProcessRead, ShellProcessStatus, ShellRunResult,
    provide_shell,
};
use hoh_subprocess::{SubprocessOutputRead, SubprocessOutputReader};

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

struct StubExecution {
    status: Mutex<ShellProcessStatus>,
    done: ShellDone,
    observed: ShellObservedStreams,
    timeout_ms: f64,
}

impl StubExecution {
    fn new(timeout_ms: f64) -> Self {
        Self {
            status: Mutex::new(ShellProcessStatus::Running),
            done: shell_done(async {}),
            observed: ShellObservedStreams {
                stdout: Arc::new(SilentReader),
                stderr: Arc::new(SilentReader),
            },
            timeout_ms,
        }
    }
}

impl ShellProcess for StubExecution {
    fn status(&self) -> ShellProcessStatus {
        *self.status.lock().unwrap()
    }

    fn exit_code(&self) -> Option<i64> {
        None
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
        self.observed.clone()
    }

    fn kill(&self) -> bool {
        let mut status = self.status.lock().unwrap();
        if *status != ShellProcessStatus::Running {
            return false;
        }
        *status = ShellProcessStatus::Killed;
        true
    }
}

impl ShellExecution for StubExecution {
    fn result(&self) -> ShellFuture<'_, ShellRunResult> {
        Box::pin(async move {
            Ok(ShellRunResult {
                exit_code: Some(0),
                signal: None,
                timed_out: false,
                aborted: false,
                timeout_ms: self.timeout_ms,
                stdout: CollectedOutput {
                    text: "ok".into(),
                    truncated: false,
                    spill_path: None,
                },
                stderr: CollectedOutput {
                    text: String::new(),
                    truncated: false,
                    spill_path: None,
                },
                sandbox: None,
            })
        })
    }
}

struct StubExecutor;
impl ShellExecutor for StubExecutor {
    fn resolve(&self, request: &ShellExecRequest) -> ShellExecSpec {
        ShellExecSpec {
            command: request.command.clone(),
            workdir: request.workdir.clone().unwrap_or_else(|| "/stub".into()),
            timeout_ms: request.timeout_ms.unwrap_or(1000.0),
            on_expiry: request.on_expiry.unwrap_or(ShellExpiryPolicy::Kill),
            stdout_max_bytes: request.stdout_max_bytes.unwrap_or(64_000),
            signal: request.signal.clone(),
            stdin: None,
            env: None,
            dsh_env: None,
            sandbox_policy: request.sandbox_policy.clone(),
        }
    }

    fn execute(&self, spec: ShellExecSpec) -> ShellFuture<'_, Arc<dyn ShellExecution>> {
        Box::pin(async move {
            Ok(Arc::new(StubExecution::new(spec.timeout_ms)) as Arc<dyn ShellExecution>)
        })
    }
}

struct StubProvider;
impl Plugin for StubProvider {
    const NAME: &'static str = "StubExecutor";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
        provide_shell(&ctx, Arc::new(StubExecutor)).await?;
        Ok(())
    }
}

struct SecondProvider;
impl Plugin for SecondProvider {
    const NAME: &'static str = "SecondExecutor";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
        provide_shell(&ctx, Arc::new(StubExecutor)).await?;
        Ok(())
    }
}

fn request() -> ShellExecRequest {
    ShellExecRequest {
        command: "echo hi".into(),
        workdir: None,
        timeout_ms: None,
        on_expiry: None,
        stdout_max_bytes: None,
        signal: None,
        stdin: None,
        env: None,
        dsh_env: None,
        sandbox_policy: None,
    }
}

#[tokio::test]
async fn a_concrete_subclass_registers_as_ctx_shell_and_serves_the_abstract_api() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx.plugin::<StubProvider>(serde_json::json!({})).unwrap();
    fiber.settled().await.unwrap();
    let shell = ctx.get::<Shell>().unwrap();
    let spec = shell.resolve(&request());
    assert_eq!(spec.command, "echo hi");
    assert_eq!(spec.workdir, "/stub");
    assert_eq!(spec.timeout_ms, 1000.0);
    assert_eq!(spec.on_expiry, ShellExpiryPolicy::Kill);
    assert_eq!(spec.stdout_max_bytes, 64_000);
    assert!(spec.signal.is_none());
    assert!(spec.stdin.is_none());
    assert!(spec.env.is_none());
    assert!(spec.dsh_env.is_none());
    assert!(spec.sandbox_policy.is_none());

    let execution = shell.execute(spec.clone()).await.unwrap();
    let result = execution.result().await.unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout.text, "ok");

    let process = shell
        .execute(ShellExecSpec {
            on_expiry: ShellExpiryPolicy::None,
            ..spec
        })
        .await
        .unwrap();
    assert_eq!(process.status(), ShellProcessStatus::Running);
    assert_eq!(process.read_output(), ShellProcessRead::default());
    assert!(process.kill());
    assert!(!process.kill());
    process.done().await;

    drop(process);
    drop(execution);
    drop(shell);
    kernel.shutdown().await;
}

#[tokio::test]
async fn reports_no_default_sandbox_mode_from_the_task_free_base_seam() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx.plugin::<StubProvider>(serde_json::json!({})).unwrap();
    fiber.settled().await.unwrap();
    let shell = ctx.get::<Shell>().unwrap();
    assert_eq!(shell.sandbox_mode(), None);
    drop(shell);
    kernel.shutdown().await;
}

#[tokio::test]
async fn loading_a_second_implementation_throws_one_shell_service_per_context() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let first = ctx.plugin::<StubProvider>(serde_json::json!({})).unwrap();
    first.settled().await.unwrap();
    let second = ctx.plugin::<SecondProvider>(serde_json::json!({})).unwrap();
    let error = second.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("service \"shell\" has been registered")
    );
    kernel.shutdown().await;
}
