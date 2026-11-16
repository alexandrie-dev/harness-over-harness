#![allow(dead_code)]
pub use hoh_error::SharedError;
use hoh_native_command::{PathOpenerInternals, host_environment::Environment, runner::*};
pub use hoh_timeout::{AbortController, AbortSignal};
use std::sync::{Arc, Mutex};
pub fn signal() -> AbortSignal {
    AbortController::new().signal()
}
pub fn detail(error: &SharedError) -> &NativeCommandError {
    error
        .downcast_ref::<NativeCommandError>()
        .expect("native command error")
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub command: String,
    pub args: Vec<String>,
    pub window: Window,
}
type Handler = dyn Fn(&str, &[String], &AbortSignal, Window) -> Result<CommandOutput, SharedError>
    + Send
    + Sync;
pub struct Mock {
    pub calls: Mutex<Vec<Call>>,
    pub signals: Mutex<Vec<usize>>,
    pub handler: Arc<Handler>,
}
impl Mock {
    pub fn new(
        handler: impl Fn(&str, &[String], &AbortSignal, Window) -> Result<CommandOutput, SharedError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            signals: Mutex::new(Vec::new()),
            handler: Arc::new(handler),
        }
    }
    pub fn output(stdout: &str) -> Self {
        let stdout = stdout.to_owned();
        Self::new(move |_, _, _, _| Ok(output(&stdout)))
    }
    pub fn assert_signals(&self, original: &AbortSignal) {
        let expected = std::ptr::from_ref(original) as usize;
        let signals = self.signals.lock().unwrap();
        assert!(!signals.is_empty());
        assert!(signals.iter().all(|actual| *actual == expected));
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
        signal: &'a AbortSignal,
        window: Window,
    ) -> CommandFuture<'a, Result<CommandOutput, SharedError>> {
        Box::pin(async move {
            self.signals
                .lock()
                .unwrap()
                .push(std::ptr::from_ref(signal) as usize);
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
pub fn exit(code: i32) -> SharedError {
    NativeCommandError {
        code: Some(CommandCode::Exit(code)),
        ..NativeCommandError::message("launch failed")
    }
    .into()
}
