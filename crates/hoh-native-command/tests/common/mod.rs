#![allow(dead_code)]
use hoh_native_command::{PathOpenerInternals, host_environment::Environment, runner::*};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub struct Signal {
    state: watch::Sender<Option<String>>,
}
impl Default for Signal {
    fn default() -> Self {
        Self {
            state: watch::channel(None).0,
        }
    }
}
impl Signal {
    pub fn abort(&self, reason: &str) {
        self.state.send_replace(Some(reason.into()));
    }
}
impl CommandSignal for Signal {
    fn throw_if_aborted(&self) -> Result<(), NativeCommandError> {
        match self.state.borrow().clone() {
            Some(reason) => Err(NativeCommandError::message(reason)),
            None => Ok(()),
        }
    }
    fn cancelled(&self) -> CommandFuture<'_, NativeCommandError> {
        let mut rx = self.state.subscribe();
        Box::pin(async move {
            loop {
                if let Some(reason) = rx.borrow_and_update().clone() {
                    return NativeCommandError::message(reason);
                }
                rx.changed().await.unwrap();
            }
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub command: String,
    pub args: Vec<String>,
    pub window: Window,
}
type Handler = dyn Fn(&str, &[String], &dyn CommandSignal, Window) -> Result<CommandOutput, NativeCommandError>
    + Send
    + Sync;
pub struct Mock {
    pub calls: Mutex<Vec<Call>>,
    pub handler: Arc<Handler>,
}
impl Mock {
    pub fn new(
        handler: impl Fn(
            &str,
            &[String],
            &dyn CommandSignal,
            Window,
        ) -> Result<CommandOutput, NativeCommandError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            handler: Arc::new(handler),
        }
    }
    pub fn output(stdout: &str) -> Self {
        let stdout = stdout.to_owned();
        Self::new(move |_, _, _, _| Ok(output(&stdout)))
    }
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}
impl NativeCommandRunner for Mock {
    fn run<'a>(
        &'a self,
        command: &'a str,
        args: &'a [String],
        signal: &'a dyn CommandSignal,
        window: Window,
    ) -> CommandFuture<'a, Result<CommandOutput, NativeCommandError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(call(
                command,
                args.iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .as_slice(),
                window,
            ));
            (self.handler)(command, args, signal, window)
        })
    }
}
pub fn output(stdout: &str) -> CommandOutput {
    CommandOutput {
        stdout: stdout.into(),
        stderr: String::new(),
    }
}
pub fn env(pairs: &[(&str, &str)]) -> Environment {
    pairs
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect()
}
pub fn facts<'a>(platform: &str, run: &'a dyn NativeCommandRunner) -> PathOpenerInternals<'a> {
    PathOpenerInternals {
        platform: Some(platform.into()),
        os_release: Some("generic".into()),
        env: Some(Environment::new()),
        run: Some(run),
    }
}
pub fn call(command: &str, args: &[&str], window: Window) -> Call {
    Call {
        command: command.into(),
        args: args.iter().map(|s| (*s).into()).collect(),
        window,
    }
}
pub fn exit(code: i32) -> NativeCommandError {
    NativeCommandError {
        code: Some(CommandCode::Exit(code)),
        ..NativeCommandError::message("launch failed")
    }
}
