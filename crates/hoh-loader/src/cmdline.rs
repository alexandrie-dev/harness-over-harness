//! The immutable launcher argument snapshot and app-owned commander parsing.
use hoh_commander::{Command, Output, ParseError, ParseOptions};
use hoh_error::BoxError;
use hoh_kernel::{Context, Undo};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub type Listener = Arc<dyn Fn() + Send + Sync>;
/// A pending listener is removed on cancellation or drop.
pub struct Subscription(Option<Box<dyn FnOnce() + Send>>);
impl Subscription {
    pub fn new(cancel: impl FnOnce() + Send + 'static) -> Self {
        Self(Some(Box::new(cancel)))
    }
    pub fn cancel(mut self) {
        if let Some(cancel) = self.0.take() {
            cancel();
        }
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(cancel) = self.0.take() {
            cancel();
        }
    }
}
#[derive(Clone)]
pub struct CmdlineArgs(Arc<[String]>);
impl CmdlineArgs {
    pub fn get(&self) -> &[String] {
        &self.0
    }
}
#[derive(Clone)]
pub struct AppExit(Arc<dyn Fn(i32) + Send + Sync>);
impl AppExit {
    pub fn new(exit: impl Fn(i32) + Send + Sync + 'static) -> Self {
        Self(Arc::new(exit))
    }
    pub fn request(&self, code: i32) {
        (self.0)(code);
    }
}
#[derive(Clone)]
pub struct AppReady(Arc<dyn Fn(Listener) -> Subscription + Send + Sync>);
impl AppReady {
    pub fn new(subscribe: impl Fn(Listener) -> Subscription + Send + Sync + 'static) -> Self {
        Self(Arc::new(subscribe))
    }
    pub fn on_ready(&self, listener: Listener) -> Subscription {
        (self.0)(listener)
    }
}
hoh_kernel::service!(pub CmdlineArgsKey: CmdlineArgs = "cmdlineArgs");
hoh_kernel::service!(pub AppExitKey: AppExit = "appExit");
hoh_kernel::service!(pub AppReadyKey: AppReady = "appReady");
pub struct CmdlineHost {
    pub args: Vec<String>,
    pub exit: AppExit,
    pub ready: Option<AppReady>,
}
pub async fn provide_cmdline(
    ctx: &Context,
    host: CmdlineHost,
) -> Result<(), hoh_kernel::KernelError> {
    ctx.provide::<CmdlineArgsKey>(Arc::new(CmdlineArgs(host.args.into())))
        .await?;
    ctx.provide::<AppExitKey>(Arc::new(host.exit)).await?;
    if let Some(ready) = host.ready {
        ctx.provide::<AppReadyKey>(Arc::new(ready)).await?;
    }
    Ok(())
}
fn configure(command: &mut Command, output: &Output) {
    command.exit_override().configure_output(output.clone());
    for child in &mut command.commands {
        configure(child, output);
    }
}
/// Output is per host, replacing the source's mutable process-stream adapter.
pub fn parse_cmdline(
    ctx: &Context,
    program: &mut Command,
    output: &Output,
) -> Result<(), BoxError> {
    let (Some(args), Some(exit)) = (ctx.get::<CmdlineArgsKey>(), ctx.get::<AppExitKey>()) else {
        return Err(format!(
            "{}: the launcher must provide ctx.cmdlineArgs and ctx.appExit before the tree mounts",
            program.get_name()
        )
        .into());
    };
    if !program.has_action_recursive() {
        return Err(format!("{}: no command in the program declares an action; parseCmdline runs the invoked command's action on a successful parse, and app code there publishes its service",program.get_name()).into());
    }
    configure(program, output);
    match program.parse(args.get().iter().cloned(), ParseOptions::USER) {
        Ok(_) => Ok(()),
        Err(ParseError::Commander(error)) if error.code.starts_with("commander.") => {
            exit.request(error.exit_code);
            Ok(())
        }
        Err(ParseError::Commander(error)) => Err(Box::new(error)),
        Err(ParseError::Other(error)) => Err(error),
    }
}
/// EOF notification only: this interface never reads or resumes the transport's bytes.
pub trait AppStdin: Send + Sync {
    fn readable_ended(&self) -> bool;
    fn once_end(&self, listener: Listener) -> Subscription;
}
struct EndState {
    active: AtomicBool,
    ended: AtomicBool,
    ready: Mutex<Option<Subscription>>,
    stdin: Mutex<Option<Subscription>>,
}
pub fn exit_on_stdin_end(
    ctx: &Context,
    label: impl Into<Arc<str>>,
    stdin: Arc<dyn AppStdin>,
) -> Result<(), BoxError> {
    exit_on_stdin_end_with_queue(ctx, label, stdin, &|on_end| {
        ctx.spawn("already ended stdin", move |_| async move {
            tokio::task::yield_now().await;
            on_end();
            Ok(())
        })?;
        Ok(())
    })
}
/// Injectable microtask queue for hosts/tests, without replacing any process-global scheduler.
pub fn exit_on_stdin_end_with_queue(
    ctx: &Context,
    label: impl Into<Arc<str>>,
    stdin: Arc<dyn AppStdin>,
    queue: &dyn Fn(Listener) -> Result<(), BoxError>,
) -> Result<(), BoxError> {
    let (Some(exit), Some(ready)) = (ctx.get::<AppExitKey>(), ctx.get::<AppReadyKey>()) else {
        return Err("stdio app: the launcher must provide ctx.appExit and ctx.appReady before the tree mounts".into());
    };
    let state = Arc::new(EndState {
        active: AtomicBool::new(true),
        ended: AtomicBool::new(false),
        ready: Mutex::new(None),
        stdin: Mutex::new(None),
    });
    let on_end: Listener = {
        let state = state.clone();
        Arc::new(move || {
            if !state.active.load(Ordering::Acquire) || state.ended.swap(true, Ordering::AcqRel) {
                return;
            }
            let active = state.clone();
            let exit = exit.clone();
            let subscription = ready.on_ready(Arc::new(move || {
                if active.active.load(Ordering::Acquire) {
                    exit.request(0);
                }
            }));
            let mut pending = crate::runtime::lock(&state.ready);
            if state.active.load(Ordering::Acquire) {
                *pending = Some(subscription);
            }
        })
    };
    let dispose = state.clone();
    ctx.effect(label, move || {
        Undo::sync(move || {
            dispose.active.store(false, Ordering::Release);
            let pending = crate::runtime::lock(&dispose.ready).take();
            let subscription = crate::runtime::lock(&dispose.stdin).take();
            drop(pending);
            drop(subscription);
        })
    })?;
    let subscription = stdin.once_end(on_end.clone());
    {
        let mut pending = crate::runtime::lock(&state.stdin);
        if state.active.load(Ordering::Acquire) {
            *pending = Some(subscription);
        }
    }
    if stdin.readable_ended() {
        queue(on_end)?;
    }
    Ok(())
}
