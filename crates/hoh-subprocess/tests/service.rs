use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Kernel, NoConfig, Plugin};
use hoh_subprocess::*;
use hoh_timeout::{AbortController, AbortSignal};
use indexmap::IndexMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct EmptyReader;
impl SubprocessOutputReader for EmptyReader {
    fn read_from(&self, _: u64) -> SubprocessOutputRead {
        SubprocessOutputRead {
            text: String::new(),
            next_offset: 0,
            lossy: false,
            spill_path: None,
        }
    }
}

#[derive(Default)]
struct ProcessLifecycle {
    terminations: AtomicUsize,
}
impl SubprocessLifecycle for ProcessLifecycle {
    fn terminate(&self) {
        self.terminations.fetch_add(1, Ordering::SeqCst);
    }
    fn wait_for_exit(&self, signal: Option<AbortSignal>) -> SubprocessFuture<'_, bool> {
        Box::pin(async move { Ok(!signal.is_some_and(|signal| signal.aborted())) })
    }
}

#[derive(Default)]
struct TerminalLifecycle {
    calls: Mutex<Vec<String>>,
}
impl SubprocessTerminalControl for TerminalLifecycle {
    fn write<'a>(&'a self, data: &'a str) -> SubprocessFuture<'a, ()> {
        self.calls.lock().unwrap().push(format!("write:{data}"));
        Box::pin(async { Ok(()) })
    }
    fn resize(&self, cols: u64, rows: u64) -> SubprocessFuture<'_, ()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("resize:{cols}:{rows}"));
        Box::pin(async { Ok(()) })
    }
    fn inspect_foreground(&self) -> SubprocessFuture<'_, Option<SubprocessTerminalForeground>> {
        Box::pin(async {
            Ok(Some(SubprocessTerminalForeground {
                process_group_id: 1,
                input_waiting: true,
            }))
        })
    }
    fn inspect_activity(&self) -> SubprocessFuture<'_, SubprocessTerminalActivity> {
        Box::pin(async {
            Ok(SubprocessTerminalActivity {
                state: SubprocessTerminalActivityState::Unknown,
                revision: 0,
            })
        })
    }
    fn signal_foreground(&self, signal: SubprocessTerminalSignal) -> SubprocessFuture<'_, u64> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("signal:{signal:?}"));
        Box::pin(async { Ok(1) })
    }
    fn terminate(&self) -> SubprocessFuture<'_, ()> {
        self.calls.lock().unwrap().push("terminate".into());
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct StubSubprocessRuntime {
    process: Arc<ProcessLifecycle>,
    terminal: Arc<TerminalLifecycle>,
}
impl SubprocessRuntime for StubSubprocessRuntime {
    fn resolve_executable<'a>(
        &'a self,
        command: &'a str,
        _: Option<&'a IndexMap<String, String>>,
        signal: Option<AbortSignal>,
    ) -> SubprocessFuture<'a, String> {
        Box::pin(async move {
            if let Some(signal) = signal {
                signal.throw_if_aborted()?;
            }
            Ok(format!("/bin/{command}"))
        })
    }
    fn terminal_environment(
        &self,
        signal: Option<AbortSignal>,
    ) -> SubprocessFuture<'_, SubprocessTerminalEnvironment> {
        Box::pin(async move {
            if let Some(signal) = signal {
                signal.throw_if_aborted()?;
            }
            Ok(SubprocessTerminalEnvironment {
                platform: SubprocessPlatform::Posix,
                default_shell: None,
            })
        })
    }
    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<SubprocessHandle, BoxError> {
        if let Some(signal) = spec.signal {
            signal.throw_if_aborted()?;
        }
        let stdout = matches!(spec.stdio.stdout, SubprocessOutputMode::Collect(_))
            .then(|| Arc::new(EmptyReader) as Arc<dyn SubprocessOutputReader>);
        Ok(SubprocessHandle {
            stdin: None,
            stdout: None,
            stderr: None,
            control: None,
            collected: SubprocessCollectedOutputs {
                stdout,
                stderr: None,
            },
            done: subprocess_done(async {
                Ok(SubprocessOutcome {
                    exit_code: Some(0),
                    signal: None,
                })
            }),
            lifecycle: self.process.clone(),
        })
    }
    fn spawn_terminal(
        &self,
        spec: SubprocessTerminalSpawnSpec,
    ) -> SubprocessFuture<'_, SubprocessTerminalHandle> {
        Box::pin(async move {
            if let Some(signal) = spec.signal {
                signal.throw_if_aborted()?;
            }
            Ok(SubprocessTerminalHandle {
                pid: spec.argv.len() as u64,
                output: Box::new(tokio::io::empty()),
                done: subprocess_done(async {
                    Ok(SubprocessOutcome {
                        exit_code: Some(0),
                        signal: None,
                    })
                }),
                lifecycle: self.terminal.clone(),
            })
        })
    }
}

struct StubProvider;
impl Plugin for StubProvider {
    const NAME: &'static str = "StubSubprocessRuntime";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
        provide_subprocess(&ctx, Arc::new(StubSubprocessRuntime::default())).await?;
        Ok(())
    }
}
struct SecondProvider;
impl Plugin for SecondProvider {
    const NAME: &'static str = "SecondService";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
        provide_subprocess(&ctx, Arc::new(StubSubprocessRuntime::default())).await?;
        Ok(())
    }
}

fn spec() -> SubprocessSpawnSpec {
    SubprocessSpawnSpec {
        argv: vec!["true".into()],
        cwd: "/stub".into(),
        stdio: SubprocessStdio {
            stdin: SubprocessStdinMode::Ignore,
            stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                max_bytes: 1,
                spill: None,
            }),
            stderr: SubprocessOutputMode::Inherit,
            control: None,
        },
        grace_ms: 1.0,
        signal: None,
        env: None,
    }
}

#[tokio::test]
async fn a_concrete_subclass_registers_as_ctx_subprocess_and_serves_the_abstract_api() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx.plugin::<StubProvider>(serde_json::json!({})).unwrap();
    fiber.settled().await.unwrap();
    let runtime = ctx.get::<Subprocess>().unwrap();
    let handle = runtime.spawn(spec()).unwrap();
    // SubprocessHandle has no pid field: target identity is provider-private in its Rust type.
    assert_eq!(
        handle.collected.stdout.as_ref().unwrap().read_from(0),
        SubprocessOutputRead {
            text: String::new(),
            next_offset: 0,
            lossy: false,
            spill_path: None,
        }
    );
    handle.terminate();
    assert!(handle.wait_for_exit(None).await.unwrap());
    assert_eq!(handle.done.clone().await.unwrap().exit_code, Some(0));
    assert_eq!(handle.done.clone().await.unwrap().exit_code, Some(0));
    // Collected streams remain available after settlement, to independent readers.
    assert_eq!(
        handle
            .collected
            .stdout
            .as_ref()
            .unwrap()
            .read_from(0)
            .next_offset,
        0
    );
    drop(runtime);
    kernel.shutdown().await;
}

#[tokio::test]
async fn loading_a_second_implementation_throws_one_subprocess_service_per_context() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let first = ctx.plugin::<StubProvider>(serde_json::json!({})).unwrap();
    first.settled().await.unwrap();
    let second = ctx.plugin::<SecondProvider>(serde_json::json!({})).unwrap();
    let error = second.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("service \"subprocess\" has been registered")
    );
    kernel.shutdown().await;
}

#[test]
fn scrubbed_parent_env_drops_credential_shaped_and_dsh_names_case_insensitively_but_keeps_path() {
    let entries = [
        ("DSH_SCRUB_PROBE", "stale"),
        ("dsh_scrub_probe_lower", "stale"),
        ("SCRUB_PROBE_TOKEN", "secret"),
        ("SCRUB_PROBE_PASSWORD", "secret"),
        ("SCRUB_PROBE_PLAIN", "visible"),
        ("PATH", "/bin"),
        ("monkey", "substring match"),
        ("my_secret", "secret"),
        ("api_key", "secret"),
        ("dſh_UNICODE", "also reserved after JavaScript toUpperCase"),
        (
            "KEY",
            "JavaScript non-Unicode regex does not fold the Kelvin sign to K",
        ),
    ];
    let env = scrubbed_parent_env_from(
        entries.map(|(key, value)| (key.to_owned(), Some(value.to_owned()))),
        None,
    );
    for key in [
        "DSH_SCRUB_PROBE",
        "dsh_scrub_probe_lower",
        "SCRUB_PROBE_TOKEN",
        "SCRUB_PROBE_PASSWORD",
        "monkey",
        "my_secret",
        "api_key",
        "dſh_UNICODE",
    ] {
        assert!(!env.contains_key(key), "{key}");
    }
    assert_eq!(env["SCRUB_PROBE_PLAIN"], "visible");
    assert_eq!(env["PATH"], "/bin");
    assert!(env.contains_key("KEY"));
    if let Some(path) = std::env::var_os("PATH") {
        assert_eq!(scrubbed_parent_env(None)["PATH"], path.to_string_lossy());
    }
}

#[tokio::test]
async fn forwards_terminal_operations_and_shared_cancellation_reasons() {
    let runtime = StubSubprocessRuntime::default();
    assert_eq!(
        runtime.resolve_executable("sh", None, None).await.unwrap(),
        "/bin/sh"
    );
    assert_eq!(
        runtime.terminal_environment(None).await.unwrap().platform,
        SubprocessPlatform::Posix
    );
    let terminal = runtime
        .spawn_terminal(SubprocessTerminalSpawnSpec {
            argv: vec!["sh".into()],
            cwd: "/stub".into(),
            env: None,
            rows: 24,
            cols: 80,
            terminal_type: "xterm-256color".into(),
            shell_activity: Some(true),
            grace_ms: 1.0,
            signal: None,
        })
        .await
        .unwrap();
    assert_eq!(terminal.pid, 1);
    terminal.write("π\n").await.unwrap();
    terminal.resize(100, 40).await.unwrap();
    assert_eq!(
        terminal.inspect_foreground().await.unwrap(),
        Some(SubprocessTerminalForeground {
            process_group_id: 1,
            input_waiting: true
        })
    );
    assert_eq!(
        terminal.inspect_activity().await.unwrap(),
        SubprocessTerminalActivity {
            state: SubprocessTerminalActivityState::Unknown,
            revision: 0
        }
    );
    assert_eq!(
        terminal
            .signal_foreground(SubprocessTerminalSignal::Sigint)
            .await
            .unwrap(),
        1
    );
    terminal.terminate().await.unwrap();
    assert_eq!(
        runtime.terminal.calls.lock().unwrap().as_slice(),
        ["write:π\n", "resize:100:40", "signal:Sigint", "terminate"]
    );
    let controller = AbortController::new();
    controller.abort_with(std::io::Error::other("cancelled by caller"));
    let signal = controller.signal();
    let reason = signal.reason().unwrap();
    let failure = runtime
        .resolve_executable("sh", None, Some(signal))
        .await
        .unwrap_err();
    assert!(
        failure
            .downcast_ref::<hoh_error::SharedError>()
            .unwrap()
            .ptr_eq(&reason)
    );
    let mut request = spec();
    request.signal = Some(controller.signal());
    let failure = runtime.spawn(request).err().unwrap();
    assert!(
        failure
            .downcast_ref::<hoh_error::SharedError>()
            .unwrap()
            .ptr_eq(&reason)
    );
}
