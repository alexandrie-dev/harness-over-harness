//! Native CLI dispatch. Environment and streams are supplied by main or an embedding test.
use super::{
    args::{DshInvocation, parse_dsh_args},
    process_shutdown::Exit,
    profile_boot::{ProfileHost, RunProfileOptions, ShippedBundles, prepare_profile, run_profile},
};
use futures_util::FutureExt;
use hoh_error::BoxError;
use hoh_loader::{
    EvalEnv,
    cmdline::Subscription,
    fail_loud::{FailLoudEvent, FailLoudProcess, FailLoudReason, FailureListener},
};
use indexmap::IndexMap;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
pub type Writer = Arc<dyn Fn(&str) -> Result<(), BoxError> + Send + Sync>;
#[derive(Clone)]
pub struct CliIo {
    pub stdout: Writer,
    pub stderr: Writer,
}
pub struct CliHost {
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub exe: PathBuf,
    pub platform: &'static str,
    pub environment: IndexMap<String, String>,
    pub io: CliIo,
    pub force_exit: Exit,
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn evaluation(host: &CliHost) -> EvalEnv {
    let variables = Arc::new(host.environment.clone());
    EvalEnv {
        home: host.home.clone(),
        cwd: host.cwd.clone(),
        exe: host.exe.clone(),
        platform: host.platform,
        var: Arc::new(move |name| variables.get(name).cloned()),
    }
}
struct NativeProcess {
    handlers: Arc<Mutex<Vec<(FailLoudEvent, FailureListener)>>>,
    io: CliIo,
    exit: Exit,
}
impl NativeProcess {
    fn emit(&self, event: FailLoudEvent, reason: FailLoudReason) {
        let handlers = lock(&self.handlers).clone();
        for (e, listener) in handlers {
            if e == event {
                listener(reason.clone());
            }
        }
    }
}
impl FailLoudProcess for NativeProcess {
    fn on(&self, event: FailLoudEvent, listener: FailureListener) -> Subscription {
        lock(&self.handlers).push((event, listener.clone()));
        let handlers = self.handlers.clone();
        Subscription::new(move || {
            lock(&handlers).retain(|(e, l)| *e != event || !Arc::ptr_eq(l, &listener));
        })
    }
    fn write_stderr(&self, text: &str) {
        let _ = (self.io.stderr)(text);
    }
    fn exit(&self, code: i32) {
        (self.exit)(code);
    }
}
/// Native signals are installed before profile boot begins and stay until the process outcome commits.
fn install_signals(host: ProfileHost) -> Result<Subscription, BoxError> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    signal=term.recv()=>if signal.is_some(){host.interrupt(0);}else{break;},
                    signal=interrupt.recv()=>if signal.is_some(){host.interrupt(130);}else{break;},
                }
            }
        });
        Ok(Subscription::new(move || task.abort()))
    }
    #[cfg(not(unix))]
    {
        let task = tokio::spawn(async move {
            while tokio::signal::ctrl_c().await.is_ok() {
                host.interrupt(130);
            }
        });
        Ok(Subscription::new(move || task.abort()))
    }
}
pub async fn run_cli(argv: &[String], version: &str, host: CliHost) -> Result<i32, BoxError> {
    let io = host.io.clone();
    run_cli_with_host(argv, version, io, move || Ok(host)).await
}

/// Parse terminal-only invocations before resolving filesystem-dependent host facts.
pub async fn run_cli_with_host<F>(
    argv: &[String],
    version: &str,
    io: CliIo,
    make_host: F,
) -> Result<i32, BoxError>
where
    F: FnOnce() -> Result<CliHost, BoxError>,
{
    let write_failure = Arc::new(Mutex::new(None));
    let out = io.stdout.clone();
    let err = io.stderr.clone();
    let out_failure = write_failure.clone();
    let err_failure = write_failure.clone();
    let output = hoh_commander::Output::new(
        move |text| {
            if let Err(error) = out(text) {
                *lock(&out_failure) = Some(error);
            }
        },
        move |text| {
            if let Err(error) = err(text) {
                *lock(&err_failure) = Some(error);
            }
        },
    );
    let invocation = parse_dsh_args(argv, version, false, output);
    if let Some(error) = lock(&write_failure).take() {
        return Err(error);
    }
    let invocation = match invocation {
        Ok(invocation) => invocation,
        Err(hoh_commander::ParseError::Commander(error)) => return Ok(error.exit_code),
        Err(hoh_commander::ParseError::Other(error)) => return Err(error),
    };
    let mut host = make_host()?;
    let bundles = Arc::new(ShippedBundles);
    let io = host.io.clone();
    let mut warning_error = None;
    let mut warn = |text: String| {
        if let Err(error) = (io.stderr)(&text) {
            warning_error = Some(error);
        }
    };
    match invocation {
        DshInvocation::PluginNew { id } => {
            let path = super::plugin_new::create(&host.cwd, &id)?;
            (host.io.stdout)(&format!("Created {}\n", path.display()))?;
            Ok(0)
        }
        DshInvocation::Plugin { .. } => Err(
            "hoh: plugin package commands are unavailable in this build (package-directory installer redesign pending)"
                .into(),
        ),
        DshInvocation::DumpConfig {
            profile,
            from_default_profile,
            default_only,
            patches,
        } => {
            let env = evaluation(&host);
            let profile = prepare_profile(
                &profile,
                !default_only,
                from_default_profile.as_deref(),
                &host.home,
                bundles.as_ref(),
                &env,
                &mut warn,
            )?;
            let text = super::dump_config::run_dump_config(
                &profile,
                default_only,
                &patches.into_iter().map(PathBuf::from).collect::<Vec<_>>(),
                &host.home,
                &host.cwd,
                &mut warn,
            )?;
            if let Some(error) = warning_error {
                return Err(error);
            }
            (host.io.stdout)(&text)?;
            Ok(0)
        }
        DshInvocation::DumpConfigSchema {
            profile,
            from_default_profile,
            patches,
        } => {
            let env = evaluation(&host);
            let profile = prepare_profile(
                &profile,
                true,
                from_default_profile.as_deref(),
                &host.home,
                bundles.as_ref(),
                &env,
                &mut warn,
            )?;
            if let Some(error) = warning_error {
                return Err(error);
            }
            let dump = super::dump_config_schema::run_dump_config_schema(
                &profile,
                &patches.into_iter().map(PathBuf::from).collect::<Vec<_>>(),
                &host.home,
                &host.cwd,
                &super::registry::registry(),
            )
            .await?;
            (host.io.stdout)(&dump.stdout)?;
            (host.io.stderr)(&dump.stderr)?;
            Ok(dump.exit_code)
        }
        DshInvocation::Profile {
            profile,
            from_default_profile,
            patches,
            args,
        } => {
            let environment = hoh_loader::load_layered_env(
                "dsh",
                &host.cwd,
                &host.home,
                &mut host.environment,
                &mut warn,
            )?;
            let env = evaluation(&host);
            let profile = prepare_profile(
                &profile,
                false,
                from_default_profile.as_deref(),
                &host.home,
                bundles.as_ref(),
                &env,
                &mut warn,
            )?;
            if let Some(error) = warning_error {
                return Err(error);
            }
            let name = profile.name.clone();
            let (finished, mut receiver) = tokio::sync::watch::channel(None);
            let complete = finished.clone();
            let force = finished.clone();
            let force_exit = host.force_exit.clone();
            let exit: Exit = Arc::new(move |code| {
                force.send_replace(Some(code));
                force_exit(code);
            });
            let application = ProfileHost::new(
                exit.clone(),
                Arc::new(move |code| {
                    complete.send_replace(Some(code));
                }),
            ).with_reporter(host.io.stderr.clone());
            let _signals = install_signals(application.clone())?;
            let process = Arc::new(NativeProcess {
                handlers: Arc::default(),
                io: host.io.clone(),
                exit,
            });
            let cleanup = application.kernel.clone();
            let _fatal = hoh_loader::install_fail_loud(
                "dsh",
                process.clone(),
                Some(Arc::new(move || {
                    let kernel = cleanup.clone();
                    Box::pin(async move {
                        kernel.shutdown().await;
                        Ok(())
                    })
                })),
            );
            let launch = std::panic::AssertUnwindSafe(run_profile(
                RunProfileOptions {
                    profile,
                    patch_files: patches.into_iter().map(PathBuf::from).collect(),
                    home: host.home.clone(),
                    bundles,
                    environment,
                    env,
                    args,
                    registry: Arc::new(super::registry::registry()),
                    install_root: Some(super::profile_boot::install_root_of(&host.exe)),
                },
                application.clone(),
            ))
            .catch_unwind()
            .await;
            let _running = match launch {
                Ok(Ok(running)) => Some(running),
                Ok(Err(error)) => {
                    if let Some(hoh_loader::LoadError::Startup(startup)) =
                        error.downcast_ref::<hoh_loader::LoadError>()
                    {
                        let write = host.io.stderr.clone();
                        super::startup_diagnostics::report_startup_failure(
                            startup,
                            &super::startup_diagnostics::StartupDiagnosticContext {
                                home: host.home.clone(),
                                version: version.into(),
                                profile: name,
                            },
                            move |text| {
                                let write = write.clone();
                                async move { write(&text) }
                            },
                        )
                        .await?;
                        return Ok(1);
                    }
                    return Err(error);
                }
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).into()))
                        .unwrap_or_else(|| "launcher panicked".into());
                    process.emit(
                        FailLoudEvent::UncaughtException,
                        FailLoudReason::new(hoh_error::SharedError::new(
                            hoh_error::BoxError::from(message),
                        )),
                    );
                    None
                }
            };
            // Keep signal/fatal guards alive while graceful disposal is pending.
            loop {
                if let Some(code) = *receiver.borrow_and_update() {
                    return Ok(code);
                }
                receiver
                    .changed()
                    .await
                    .map_err(|_| "process shutdown controller closed")?;
            }
        }
    }
}
