use hoh_error::{SharedError, node_error_code, node_fs_error_message, node_spawn_error_message};
use hoh_timeout::AbortSignal;
use std::{future::Future, pin::Pin, process::Stdio};
use tokio::io::AsyncReadExt;

pub type CommandFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Hidden,
    Visible,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandCode {
    Exit(i32),
    Name(String),
}

/// An ordinary Error with the fields supplied by Node's execFile callback.
/// None stdout/stderr represent a synchronous execFile throw, before the callback.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct NativeCommandError {
    pub message: String,
    pub code: Option<CommandCode>,
    pub signal: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    #[source]
    pub cause: Option<SharedError>,
}
impl NativeCommandError {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            signal: None,
            stdout: None,
            stderr: None,
            cause: None,
        }
    }
    pub fn io(error: std::io::Error) -> Self {
        let message = error.to_string();
        Self {
            code: node_error_code(&error).map(|code| CommandCode::Name(code.into())),
            cause: Some(SharedError::new(error)),
            ..Self::message(message)
        }
    }
    pub(crate) fn fs(error: std::io::Error, syscall: &str, path: Option<&str>) -> Self {
        let message = node_fs_error_message(&error, syscall, path, None);
        Self {
            message,
            ..Self::io(error)
        }
    }
}
impl From<NativeCommandError> for SharedError {
    fn from(error: NativeCommandError) -> Self {
        Self::new(error)
    }
}

pub trait NativeCommandRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        command: &'a str,
        args: &'a [String],
        signal: &'a AbortSignal,
        window: Window,
    ) -> CommandFuture<'a, Result<CommandOutput, SharedError>>;
}
pub struct SystemCommandRunner;
impl NativeCommandRunner for SystemCommandRunner {
    fn run<'a>(
        &'a self,
        command: &'a str,
        args: &'a [String],
        signal: &'a AbortSignal,
        window: Window,
    ) -> CommandFuture<'a, Result<CommandOutput, SharedError>> {
        Box::pin(run_native_command(command, args, signal, window))
    }
}

pub async fn run_native_command(
    command: &str,
    args: &[String],
    signal: &AbortSignal,
    window: Window,
) -> Result<CommandOutput, SharedError> {
    signal.throw_if_aborted()?;
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    if window == Window::Hidden {
        cmd.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    let _ = window;
    let mut child = cmd.spawn().map_err(|error| {
        let message = node_spawn_error_message(&error, command);
        let code = node_error_code(&error).map(|code| CommandCode::Name(code.into()));
        let callback = matches!(
            node_error_code(&error),
            Some("EACCES" | "EAGAIN" | "EMFILE" | "ENFILE" | "ENOENT")
        );
        NativeCommandError {
            message,
            code,
            stdout: callback.then(String::new),
            stderr: callback.then(String::new),
            cause: callback.then(|| SharedError::new(error)),
            ..NativeCommandError::message("")
        }
    })?;
    // Both streams are present because they were configured as piped above.
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let capture = async {
        let (status, _, _) = tokio::try_join!(
            async {
                child
                    .wait()
                    .await
                    .map_err(|e| SharedError::new(NativeCommandError::io(e)))
            },
            read_limited(&mut stdout, &mut out, "stdout"),
            read_limited(&mut stderr, &mut err, "stderr")
        )?;
        Ok::<_, SharedError>(status)
    };
    let status = tokio::select! {
        biased;
        reason = signal.cancelled() => Err(reason),
        result = capture => result,
    };
    if status.is_err() {
        #[cfg(unix)]
        if let Some(pid) = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        #[cfg(windows)]
        child.start_kill().ok();
        let _ = tokio::join!(
            child.wait(),
            read_limited(&mut stdout, &mut out, "stdout"),
            read_limited(&mut stderr, &mut err, "stderr")
        );
    }
    let output = CommandOutput {
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    };
    match status {
        Ok(status) if status.success() => Ok(output),
        Ok(status) => {
            let message = format!(
                "Command failed: {command}{}\n{}",
                args.iter().map(|s| format!(" {s}")).collect::<String>(),
                output.stderr
            );
            #[cfg(unix)]
            let termination = {
                use std::os::unix::process::ExitStatusExt;
                status.signal().map(signal_name)
            };
            #[cfg(not(unix))]
            let termination = None;
            Err(NativeCommandError {
                code: status.code().map(CommandCode::Exit),
                signal: termination,
                stdout: Some(output.stdout),
                stderr: Some(output.stderr),
                cause: Some(SharedError::new(std::io::Error::other(message.clone()))),
                ..NativeCommandError::message(message)
            }
            .into())
        }
        Err(error) => {
            // Cancellation forwards the exact caller reason, without a command-error wrapper.
            if signal.reason().is_some_and(|reason| reason.ptr_eq(&error)) {
                return Err(error);
            }
            if let Some(detail) = error.downcast_ref::<NativeCommandError>() {
                return Err(NativeCommandError {
                    stdout: Some(output.stdout),
                    stderr: Some(output.stderr),
                    ..detail.clone()
                }
                .into());
            }
            Err(error)
        }
    }
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    use rustix::process::Signal;
    for (value, name) in [
        (Signal::HUP, "SIGHUP"),
        (Signal::INT, "SIGINT"),
        (Signal::QUIT, "SIGQUIT"),
        (Signal::ILL, "SIGILL"),
        (Signal::ABORT, "SIGABRT"),
        (Signal::FPE, "SIGFPE"),
        (Signal::KILL, "SIGKILL"),
        (Signal::SEGV, "SIGSEGV"),
        (Signal::PIPE, "SIGPIPE"),
        (Signal::ALARM, "SIGALRM"),
        (Signal::TERM, "SIGTERM"),
        (Signal::USR1, "SIGUSR1"),
        (Signal::USR2, "SIGUSR2"),
        (Signal::CHILD, "SIGCHLD"),
        (Signal::CONT, "SIGCONT"),
        (Signal::STOP, "SIGSTOP"),
        (Signal::TSTP, "SIGTSTP"),
        (Signal::TTIN, "SIGTTIN"),
        (Signal::TTOU, "SIGTTOU"),
    ] {
        if value.as_raw() == signal {
            return name.into();
        }
    }
    format!("SIG{signal}")
}

async fn read_limited(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    output: &mut Vec<u8>,
    stream: &str,
) -> Result<(), SharedError> {
    const MAX_BUFFER: usize = 1024 * 1024;
    let mut buffer = [0; 8192];
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|e| SharedError::new(NativeCommandError::io(e)))?;
        if count == 0 {
            return Ok(());
        }
        let remaining = MAX_BUFFER.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
        if count > remaining {
            return Err(NativeCommandError {
                code: Some(CommandCode::Name(
                    "ERR_CHILD_PROCESS_STDIO_MAXBUFFER".into(),
                )),
                ..NativeCommandError::message(format!("{stream} maxBuffer length exceeded"))
            }
            .into());
        }
    }
}
